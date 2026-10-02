// One session's items, fed by its stream (plan 4c decision 4).
//
// - Open: the first page (8 groups under 768 px, else the server's
//   default) and the catalogue, then the item stream from the page's
//   anchor, `<epoch>:<revision>`.
// - `item` upserts, `item_removed` removes, `catalog_changed` replaces the
//   catalogue.
// - A resync (`resync_required`, or any message that does not parse):
//   close the stream at once, refetch the first page and the catalogue,
//   REPLACE the store (older pages dropped), and open a new stream from the
//   new anchor. "Resynced" shows for a moment.
// - `session_removed`, or a 404 from the page or the stream: the stream
//   closes and the session is marked removed.
// - A page that fails otherwise is fetched again, waiting longer each time.
import { useEffect, useMemo, useRef, useSyncExternalStore } from 'react'
import { useClient } from '../app-client'
import type { Client } from '../api/client'
import { ApiFailure, Unauthenticated, messageOf } from '../api/errors'
import { openStream, type Stream, type StreamEvent, type StreamState } from '../api/sse'
import { anchorOf, catalog as fetchCatalog, itemPage, itemStreamPath } from '../api/view'
import type { SessionCatalog } from '../generated/protocol'
import type { Item } from '../generated/view'
import { useMediaQuery } from '../hooks/useMediaQuery'
import { EMPTY_ITEMS, beforeTurnOf, isItem, isItemPage, itemsReducer, type ItemsAction, type ItemsState } from './items'

/** Groups on a phone's first page (O-7). */
export const NARROW_PAGE_GROUPS = 8
const NARROW = '(max-width: 767px)'

export interface Timing {
  /** The wait before fetching a failed page again, by attempt (0, 1, …). */
  retryMs: (attempt: number) => number
  /** How long "Resynced" shows. */
  resyncedMs: number
}

const DEFAULT_TIMING: Timing = {
  retryMs: (attempt) => Math.min(1000 * 2 ** attempt, 30000),
  resyncedMs: 3000,
}

export interface SessionItemsSnapshot {
  store: ItemsState
  /** The first page has not come yet. */
  loading: boolean
  loadingOlder: boolean
  /** The session was deleted, or is not the operator's: nothing more comes. */
  removed: boolean
  error: string | null
  stream: StreamState
  /** A resync replaced the items a moment ago. */
  resynced: boolean
  catalog: SessionCatalog | null
  /** First pages taken: bumped by the first load and by every resync (the
   *  items were replaced). */
  loads: number
}

const INITIAL: SessionItemsSnapshot = {
  store: EMPTY_ITEMS,
  loading: true,
  loadingOlder: false,
  removed: false,
  error: null,
  stream: 'connecting',
  resynced: false,
  catalog: null,
  loads: 0,
}

/** `JSON.parse`, or `undefined` for text that is not JSON. */
function parse(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return undefined
  }
}

function isCatalog(value: unknown): value is SessionCatalog {
  if (!value || typeof value !== 'object') return false
  const v = value as Record<string, unknown>
  return typeof v.session_id === 'string' && Array.isArray(v.config_options) && Array.isArray(v.commands)
}

export class SessionItemsController {
  private snapshot: SessionItemsSnapshot = INITIAL
  private readonly listeners = new Set<() => void>()
  /** Bumped by `start`, `stop`, a resync and `gone`: a continuation of
   *  another run is dropped. */
  private run = 0
  /** Bumped by each first page: an older page of the one before is dropped. */
  private generation = 0
  /** Bumped by each catalogue from the stream or a config answer: a fetch
   *  begun before it is older, and dropped. */
  private catalogSeq = 0
  private stream: Stream | null = null
  private attempt = 0
  private retryTimer: ReturnType<typeof setTimeout> | undefined
  private resyncedTimer: ReturnType<typeof setTimeout> | undefined
  private readonly client: Client
  private readonly id: string
  private readonly limit: () => number | undefined
  private readonly timing: Timing

  constructor(client: Client, id: string, limit: () => number | undefined, timing: Partial<Timing> = {}) {
    this.client = client
    this.id = id
    this.limit = limit
    this.timing = { ...DEFAULT_TIMING, ...timing }
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  getSnapshot = (): SessionItemsSnapshot => this.snapshot

  start(): void {
    this.stop()
    this.snapshot = INITIAL
    this.attempt = 0
    this.notify()
    void this.load(this.run, false)
  }

  stop(): void {
    this.run++
    this.closeStream()
    clearTimeout(this.retryTimer)
    clearTimeout(this.resyncedTimer)
  }

  /** The groups before the first loaded one, prepended. */
  loadOlder = async (): Promise<void> => {
    const before = beforeTurnOf(this.snapshot.store)
    if (before === undefined || this.snapshot.loadingOlder || this.snapshot.removed) return
    const run = this.run
    const generation = this.generation
    this.update({ loadingOlder: true })
    try {
      const page = await itemPage(this.client, this.id, { beforeTurn: before, limit: this.limit() })
      if (run !== this.run || generation !== this.generation) return
      this.apply({ type: 'prepended', page })
    } catch (err) {
      if (run !== this.run || generation !== this.generation) return
      if (err instanceof ApiFailure && err.status === 404) return this.gone()
      // The turn is no longer this session's first loaded one: start over.
      if (err instanceof ApiFailure && err.status === 400) return this.resync()
      if (!(err instanceof Unauthenticated)) this.update({ error: messageOf(err) })
    } finally {
      if (run === this.run) this.update({ loadingOlder: false })
    }
  }

  /** A catalogue from a config answer (202) replaces the one held. */
  setCatalog = (catalog: SessionCatalog): void => {
    this.catalogSeq++
    this.update({ catalog })
  }

  private notify(): void {
    for (const listener of this.listeners) listener()
  }

  private update(patch: Partial<SessionItemsSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    this.notify()
  }

  private apply(action: ItemsAction): void {
    const store = itemsReducer(this.snapshot.store, action)
    if (store !== this.snapshot.store) this.update({ store })
  }

  private closeStream(): void {
    this.stream?.close()
    this.stream = null
  }

  private async loadCatalog(run: number): Promise<void> {
    const seq = this.catalogSeq
    try {
      const catalog = await fetchCatalog(this.client, this.id)
      if (run === this.run && seq === this.catalogSeq && isCatalog(catalog)) this.update({ catalog })
    } catch {
      // The page's own answer says what is wrong.
    }
  }

  private async load(run: number, resync: boolean): Promise<void> {
    // Every (re)load: a gap may have hidden a `catalog_changed`.
    void this.loadCatalog(run)
    try {
      const page = await itemPage(this.client, this.id, { limit: this.limit() })
      if (run !== this.run) return
      if (!isItemPage(page)) throw new Error('The session’s items could not be read.')
      this.generation++
      this.attempt = 0
      this.update({
        store: itemsReducer(this.snapshot.store, { type: 'loaded', page }),
        loading: false,
        error: null,
        resynced: resync || this.snapshot.resynced,
        loads: this.snapshot.loads + 1,
      })
      if (resync) {
        clearTimeout(this.resyncedTimer)
        this.resyncedTimer = setTimeout(() => this.update({ resynced: false }), this.timing.resyncedMs)
      }
      this.open(anchorOf(page))
    } catch (err) {
      if (run !== this.run) return
      if (err instanceof ApiFailure && err.status === 404) return this.gone()
      if (err instanceof Unauthenticated) return this.update({ stream: 'closed' })
      if (err instanceof ApiFailure && err.status >= 400 && err.status < 500) {
        return this.update({ error: messageOf(err), loading: false, stream: 'closed' })
      }
      this.update({ error: messageOf(err), stream: 'reconnecting' })
      this.retryTimer = setTimeout(() => void this.load(run, resync), this.timing.retryMs(this.attempt++))
    }
  }

  private open(anchor: string): void {
    // One stream at a time, whatever opened the one before.
    this.closeStream()
    this.stream = openStream(this.client, itemStreamPath(this.id), {
      lastEventId: anchor,
      // Nothing comes after `close()`: the helper drops it.
      onState: (stream) => this.update({ stream }),
      onEvent: (event) => this.onEvent(event),
      onResync: () => this.resync(),
      onError: (status) => {
        if (status === 404) return this.gone()
        this.update({ error: `The session’s updates stopped (${status}).`, stream: 'closed' })
      },
    })
  }

  private onEvent(event: StreamEvent): void {
    switch (event.event) {
      case 'item': {
        const item = parse(event.data)
        if (!isItem(item)) return this.resync()
        return this.apply({ type: 'upsert', item: item as Item })
      }
      case 'item_removed': {
        const removed = parse(event.data) as { id?: unknown } | undefined
        if (!removed || typeof removed !== 'object' || typeof removed.id !== 'string') return this.resync()
        return this.apply({ type: 'removed', id: removed.id })
      }
      case 'session_removed':
        return this.gone()
      case 'catalog_changed': {
        const catalog = parse(event.data)
        if (!isCatalog(catalog)) return this.resync()
        return this.setCatalog(catalog)
      }
      default:
        // A message this client does not know: a newer server's, ignored.
        return
    }
  }

  /** Close now (the server ends the stream after `resync_required`), then
   *  refetch, replace and reopen from the new anchor. A new run: whatever
   *  was in flight (an older page, a load, a retry) is dropped, so it can
   *  neither resync again nor open a second stream. */
  private resync(): void {
    this.run++
    this.closeStream()
    clearTimeout(this.retryTimer)
    this.update({ stream: 'reconnecting', loadingOlder: false })
    void this.load(this.run, true)
  }

  /** A new run too: a load in flight cannot reopen a removed session. */
  private gone(): void {
    this.run++
    this.closeStream()
    clearTimeout(this.retryTimer)
    this.update({ removed: true, loading: false, loadingOlder: false, stream: 'closed' })
  }
}

export interface SessionItems extends Omit<SessionItemsSnapshot, 'store'> {
  items: Item[]
  /** Groups exist before the first loaded one. */
  older: boolean
  loadOlder: () => Promise<void>
  setCatalog: (catalog: SessionCatalog) => void
}

export function useSessionItems(id: string, timing?: Partial<Timing>): SessionItems {
  const client = useClient()
  const narrow = useMediaQuery(NARROW)
  // Read when a page is fetched: a resize does not reload the session.
  const narrowRef = useRef(narrow)
  narrowRef.current = narrow
  const timingRef = useRef(timing)
  const controller = useMemo(
    () =>
      new SessionItemsController(client, id, () => (narrowRef.current ? NARROW_PAGE_GROUPS : undefined), timingRef.current),
    [client, id],
  )
  useEffect(() => {
    controller.start()
    return () => controller.stop()
  }, [controller])
  const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot)
  const { store, ...rest } = snapshot
  return { ...rest, items: store.items, older: store.older, loadOlder: controller.loadOlder, setCatalog: controller.setCatalog }
}
