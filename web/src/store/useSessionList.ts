// The session list, fed by the list stream (plan 4c decision 5).
//
// - Seeded from the first page of `GET /api/view/sessions` for the
//   filters' query; further pages by `next_cursor` (`loadMore`).
// - The list stream resumes from the FIRST page's anchor only: a later
//   page never moves it.
// - `session_upsert`, `session_removed` and `waiting_changed` change the
//   store and nothing else: no event ever refetches (F-4). A repeated
//   `session_removed` (the server may send one twice) is a keyed delete,
//   so harmless.
// - The stream is opened with the query's hat (never an empty one): it
//   scopes `waiting_changed`'s count only. A new hat is a new query, so a
//   new page and a new stream.
// - A resync (`resync_required`, or a message that does not parse): close
//   the stream at once, refetch the first page with the current query,
//   replace the store and reopen from the new anchor. A resync that
//   follows another waits first, longer each time (`ResyncBackoff`).
// - A change of the query (hat, lifecycle filter, Hide closed, search)
//   starts over the same way.
import { useEffect, useMemo, useRef, useSyncExternalStore } from 'react'
import { useClient } from '../app-client'
import type { Client } from '../api/client'
import { ApiFailure, Unauthenticated, messageOf } from '../api/errors'
import { openStream, type Stream, type StreamEvent, type StreamState } from '../api/sse'
import { anchorOf, listStreamPath, summaries, type SummaryQuery } from '../api/view'
import type { SessionSummary } from '../generated/view'
import {
  EMPTY_LIST,
  isFirstPage,
  isSummary,
  isWaitingCount,
  listReducer,
  searching,
  serverQuery,
  shownOf,
  waitingCount,
  type ListAction,
  type ListFilters,
  type ListState,
} from './sessionList'
import { ResyncBackoff, type Timing } from './useSessionItems'

const DEFAULT_TIMING: Timing = {
  retryMs: (attempt) => Math.min(1000 * 2 ** attempt, 30000),
  resyncedMs: 3000,
  settleMs: 30000,
}

export interface SessionListSnapshot {
  store: ListState
  loading: boolean
  loadingMore: boolean
  error: string | null
  stream: StreamState
  resynced: boolean
}

const INITIAL: SessionListSnapshot = {
  store: EMPTY_LIST,
  loading: true,
  loadingMore: false,
  error: null,
  stream: 'connecting',
  resynced: false,
}

function parse(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return undefined
  }
}

export class SessionListController {
  private snapshot: SessionListSnapshot = INITIAL
  private readonly listeners = new Set<() => void>()
  private run = 0
  private generation = 0
  private stream: Stream | null = null
  private attempt = 0
  private retryTimer: ReturnType<typeof setTimeout> | undefined
  private resyncedTimer: ReturnType<typeof setTimeout> | undefined
  private readonly client: Client
  private readonly query: SummaryQuery
  private readonly search: boolean
  private readonly timing: Timing
  private readonly backoff: ResyncBackoff

  constructor(client: Client, query: SummaryQuery, timing: Partial<Timing> = {}) {
    this.client = client
    this.query = query
    this.search = query.q !== undefined && query.q !== ''
    this.timing = { ...DEFAULT_TIMING, ...timing }
    this.backoff = new ResyncBackoff(this.timing)
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  getSnapshot = (): SessionListSnapshot => this.snapshot

  start(): void {
    this.stop()
    this.snapshot = INITIAL
    this.attempt = 0
    this.backoff.reset()
    this.notify()
    void this.load(this.run, false)
  }

  stop(): void {
    this.run++
    this.closeStream()
    clearTimeout(this.retryTimer)
    clearTimeout(this.resyncedTimer)
  }

  /** The next page, by `next_cursor`. */
  loadMore = async (): Promise<void> => {
    const cursor = this.snapshot.store.nextCursor
    if (cursor === undefined || this.snapshot.loadingMore) return
    const run = this.run
    const generation = this.generation
    this.update({ loadingMore: true })
    try {
      const page = await summaries(this.client, { ...this.query, cursor })
      if (run !== this.run || generation !== this.generation) return
      this.apply({ type: 'more', page })
    } catch (err) {
      if (run !== this.run || generation !== this.generation) return
      if (!(err instanceof Unauthenticated)) this.update({ error: messageOf(err) })
    } finally {
      if (run === this.run) this.update({ loadingMore: false })
    }
  }

  private notify(): void {
    for (const listener of this.listeners) listener()
  }

  private update(patch: Partial<SessionListSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    this.notify()
  }

  private apply(action: ListAction): void {
    const store = listReducer(this.snapshot.store, action)
    if (store !== this.snapshot.store) this.update({ store })
  }

  private closeStream(): void {
    this.stream?.close()
    this.stream = null
  }

  private async load(run: number, resync: boolean): Promise<void> {
    try {
      const page = await summaries(this.client, this.query)
      if (run !== this.run) return
      if (!isFirstPage(page)) throw new Error('The session list could not be read.')
      this.generation++
      this.attempt = 0
      this.update({
        store: listReducer(this.snapshot.store, { type: 'first', page, search: this.search }),
        loading: false,
        error: null,
        resynced: resync || this.snapshot.resynced,
      })
      if (resync) {
        clearTimeout(this.resyncedTimer)
        this.resyncedTimer = setTimeout(() => this.update({ resynced: false }), this.timing.resyncedMs)
      }
      this.open(anchorOf(page))
    } catch (err) {
      if (run !== this.run) return
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
    this.stream = openStream(this.client, listStreamPath(this.query.hat), {
      lastEventId: anchor,
      // Nothing comes after `close()`: the helper drops it.
      onState: (stream) => {
        if (stream === 'open') this.backoff.opened()
        this.update({ stream })
      },
      onEvent: (event) => this.onEvent(event),
      onResync: () => this.resync(),
      onError: (status) => this.update({ error: `The session list’s updates stopped (${status}).`, stream: 'closed' }),
    })
  }

  private onEvent(event: StreamEvent): void {
    switch (event.event) {
      case 'session_upsert': {
        const summary = parse(event.data)
        if (!isSummary(summary)) return this.resync()
        return this.apply({ type: 'upsert', summary })
      }
      case 'session_removed': {
        const removed = parse(event.data) as { session_id?: unknown } | undefined
        if (!removed || typeof removed !== 'object' || typeof removed.session_id !== 'string') return this.resync()
        return this.apply({ type: 'removed', id: removed.session_id })
      }
      case 'waiting_changed': {
        const changed = parse(event.data) as { count?: unknown } | undefined
        if (!changed || typeof changed !== 'object' || !isWaitingCount(changed.count)) return this.resync()
        return this.apply({ type: 'waiting', count: changed.count })
      }
      default:
        return
    }
  }

  /** A new run, as in the items' store: a further page, a refetch or a
   *  wait in flight is dropped, and "load more" is free again. The refetch
   *  waits as `ResyncBackoff` says. */
  private resync(): void {
    this.run++
    this.closeStream()
    clearTimeout(this.retryTimer)
    this.update({ stream: 'reconnecting', loadingMore: false })
    const run = this.run
    const wait = this.backoff.next()
    if (wait === 0) void this.load(run, true)
    // `stop` and the next resync clear it, and bump the run.
    else this.retryTimer = setTimeout(() => void this.load(run, true), wait)
  }
}

export interface SessionList extends Omit<SessionListSnapshot, 'store'> {
  /** Every summary received, by id (the header reads it). */
  all: ReadonlyMap<string, SessionSummary>
  /** What the list shows, in order. */
  shown: SessionSummary[]
  /** `waiting`: the sessions of the hat waiting on a question (blocked, or
   *  a question open): the tab title's and the header badge's number. */
  counts: { waiting: number }
  /** More pages exist. */
  hasMore: boolean
  loadMore: () => Promise<void>
  searching: boolean
}

export function useSessionList(filters: ListFilters, timing?: Partial<Timing>): SessionList {
  const client = useClient()
  const query = serverQuery(filters)
  const key = JSON.stringify(query)
  const timingRef = useRef(timing)
  // A new query starts over; a new object with the same query does not.
  const controller = useMemo(() => new SessionListController(client, query, timingRef.current), [client, key])
  useEffect(() => {
    controller.start()
    return () => controller.stop()
  }, [controller])
  const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot)
  const { store, ...rest } = snapshot
  const { hat, lifecycle, hideClosed, q } = filters
  const shown = useMemo(
    () => shownOf(store, { hat, lifecycle, hideClosed, q }),
      [store, hat, hideClosed, q, lifecycle?.join(',')],
  )
  const waiting = useMemo(() => waitingCount(store, hat), [store, hat])
  const counts = useMemo(() => ({ waiting }), [waiting])
  return {
    ...rest,
    all: store.all,
    shown,
    counts,
    hasMore: store.nextCursor !== undefined,
    loadMore: controller.loadMore,
    searching: searching(filters),
  }
}
