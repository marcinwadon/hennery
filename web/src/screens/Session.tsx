// One session (frontend spec §6): its header and its transcript, read-only.
//
// - The items come from the item store (`useSessionItems`): the first page,
//   then the stream. The header reads the list's summary when the caller has
//   it, else the session's detail, fetched once. While the list's first page
//   is on its way, the header waits for it rather than fetching the detail.
// - The transcript opens at its end and stays there while the reader is at
//   the end. It renders at most the newest `TAIL` items: "Load earlier" (or
//   scrolling to the top) first shows `TAIL` more of the items held, and
//   only when none is held fetches the turns before. Either way what the
//   reader sees stays where it was.
// - The window's first row is pinned by item id: an upsert, or an item
//   joining a turn above it or at the end, never moves it. A resync (the
//   items replaced) goes back to the tail, and to the end.
// - The stream's state shows as "Reconnecting…", and "Resynced" for a moment
//   after the items were replaced.
// - A deleted session says so, and nothing more is fetched.
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { hatList, hostList, namesOf } from '../api/names'
import { sessionDetail } from '../api/view'
import { useClient } from '../app-client'
import SessionHeader, { type HeaderInfo } from '../components/SessionHeader'
import Transcript from '../components/Transcript'
import type { ItemEnv } from '../components/items/types'
import type { SessionSummary } from '../generated/view'
import type { Item } from '../generated/view'
import { useMediaQuery } from '../hooks/useMediaQuery'
import { agentLabel } from '../lib/agent'
import { Icon } from '../lib/ui'
import { Link } from '../router'
import { useSessionItems, type Timing } from '../store/useSessionItems'

/** Within this many pixels of the end, the reader is at the end. */
const STICK_PX = 80
/** Within this many pixels of the top, older turns are fetched. */
const TOP_PX = 120
/** The most items the transcript renders until the reader asks for more
 *  (the windowing measurement: a first render under load stays within
 *  budget up to about 200 items). */
export const TAIL = 200

interface Props {
  id: string
  /** The list store's summary of the session, when it has one. */
  summary?: SessionSummary
  /** The list's first page is on its way: wait for it before fetching the
   *  session's detail. */
  awaitSummary?: boolean
  /** The transcript's window (tests); `TAIL` by default. */
  tail?: number
  /** The item store's timing (tests). */
  timing?: Partial<Timing>
  /** Seams for later tasks: question actions and "Send again". */
  env?: Pick<ItemEnv, 'questionActions' | 'onSendAgain'>
}

/** The newest plan among the loaded items. */
function latestPlan(items: Item[]): Extract<Item, { kind: 'plan' }> | undefined {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]
    if (item.kind === 'plan') return item
  }
  return undefined
}

/** The session's detail, once, unless the caller has its summary (or is
 *  about to). A summary seen once stays shown when the list starts over
 *  (a new search, a filter) until the detail or the summary comes back. */
function useHeaderInfo(id: string, summary: SessionSummary | undefined, awaitSummary: boolean): HeaderInfo | undefined {
  const client = useClient()
  const [detail, setDetail] = useState<HeaderInfo | undefined>(undefined)
  const last = useRef<SessionSummary | undefined>(undefined)
  if (summary) last.current = summary
  const needed = summary === undefined && !awaitSummary
  useEffect(() => {
    if (!needed) return
    let live = true
    sessionDetail(client, id).then(
      (d) => live && setDetail(d),
      // The item store says when the session is gone; the header stays bare.
      () => {},
    )
    return () => {
      live = false
    }
  }, [client, id, needed])
  return summary ?? detail ?? last.current
}

/** `id → name` from a list fetched once, when `wanted`; on failure, none. */
function useNames(fetch: 'hosts' | 'hats', wanted: boolean): Map<string, string> {
  const client = useClient()
  const [names, setNames] = useState<Map<string, string>>(() => new Map())
  useEffect(() => {
    if (!wanted) return
    let live = true
    const request = fetch === 'hosts' ? hostList(client) : hatList(client)
    request.then(
      (list) => live && setNames(namesOf(list, fetch === 'hosts' ? 'host_id' : 'id')),
      () => {},
    )
    return () => {
      live = false
    }
  }, [client, fetch, wanted])
  return names
}

/** Where the transcript's window starts: the pinned item's index, or the
 *  newest `size` items when nothing is pinned for these `loads`. */
function windowStart(items: readonly Item[], pin: string | null, size: number): number {
  const tail = Math.max(0, items.length - size)
  if (pin === null) return tail
  const at = items.findIndex((item) => item.id === pin)
  return at < 0 ? tail : at
}

/** The transcript's window over `items`: its first row pinned by id, `TAIL`
 *  rows at first, more revealed on demand; back to the tail when `loads`
 *  changes (a resync replaced the items). */
function useTailWindow(items: Item[], loads: number, size: number) {
  const [pinned, setPinned] = useState<{ id: string; loads: number } | null>(null)
  const pin = pinned && pinned.loads === loads ? pinned.id : null
  const start = windowStart(items, pin, size)
  const visible = useMemo(() => (start === 0 ? items : items.slice(start)), [items, start])
  const firstHeld = useRef<string | undefined>(undefined)

  useLayoutEffect(() => {
    const was = firstHeld.current
    firstHeld.current = items[0]?.id
    if (items.length === 0) return
    // Older turns came in above a window that showed everything held: the
    // reader asked for them, so up to `size` of them are shown.
    const prepended = was !== undefined && was !== items[0].id && pin === was && items.some((i) => i.id === was)
    const next = prepended ? Math.max(0, start - size) : start
    if (pin === null || prepended || items[start]?.id !== pin) setPinned({ id: items[next].id, loads })
  }, [items, loads, pin, start, size])

  /** Show `size` more of the held items; `false` when none is held. */
  const reveal = (): boolean => {
    if (start === 0) return false
    setPinned({ id: items[Math.max(0, start - size)].id, loads })
    return true
  }
  return { visible, held: start, reveal }
}

export default function SessionView({ id, summary, awaitSummary = false, tail = TAIL, timing, env: seams }: Props) {
  const s = useSessionItems(id, timing)
  const info = useHeaderInfo(id, summary, awaitSummary)
  const win = useTailWindow(s.items, s.loads, tail)
  const narrow = useMediaQuery('(max-width: 767px)')
  const hosts = useNames('hosts', true)
  const wantsHats = useMemo(() => s.items.some((i) => i.kind === 'marker' && i.marker === 'hat_reassigned'), [s.items])
  const hats = useNames('hats', wantsHats)
  const plan = useMemo(() => latestPlan(s.items), [s.items])

  const env: ItemEnv = useMemo(
    () => ({
      sessionId: id,
      agent: agentLabel(info?.agent),
      hatName: (hat: string) => hats.get(hat),
      ...seams,
    }),
    [id, info?.agent, hats, seams],
  )

  if (s.removed) {
    return (
      <div className="session">
        <div className="welcome" role="status">
          <h1>This session was deleted</h1>
          <p>
            <bdi>{id}</bdi>
          </p>
          <Link to="/sessions" className="btn btn-ghost">
            Back to sessions
          </Link>
        </div>
      </div>
    )
  }

  return (
    <div className="session">
      <SessionHeader id={id} info={info} hostName={info ? hosts.get(info.host_id) : undefined} plan={plan} narrow={narrow} />
      <div className="stream-state" role="status">
        {s.stream === 'reconnecting' ? 'Reconnecting…' : s.resynced ? 'Resynced' : ''}
      </div>
      {s.error && (
        <p className="form-error session-error">
          <bdi>{s.error}</bdi>
        </p>
      )}
      <Scroller
        items={win.visible}
        loads={s.loads}
        earlier={win.held > 0 || s.older}
        loadingOlder={s.loadingOlder}
        loadEarlier={() => {
          if (!win.reveal() && s.older && !s.loadingOlder) void s.loadOlder()
        }}
      >
        {s.loading ? <p className="transcript-empty">Loading…</p> : <Transcript items={win.visible} env={env} />}
      </Scroller>
    </div>
  )
}

interface ScrollerProps {
  /** The rows rendered. */
  items: Item[]
  /** The item store's first pages taken: a new one (a resync) goes to the end. */
  loads: number
  /** Rows are held above the window, or turns before the first loaded. */
  earlier: boolean
  loadingOlder: boolean
  loadEarlier: () => void
  children: React.ReactNode
}

/** The transcript's scrolling box: opens at the end, follows the end while
 *  the reader is there, and keeps the reader's place when rows go in above
 *  (held rows revealed, or older turns prepended). */
function Scroller({ items, loads, earlier, loadingOlder, loadEarlier, children }: ScrollerProps) {
  const ref = useRef<HTMLDivElement>(null)
  const atEnd = useRef(true)
  const before = useRef<{ first?: string; height: number; loads: number }>({ height: 0, loads })

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const first = items[0]?.id
    const was = before.current
    // A resync replaced the items: the window went back to the tail.
    if (loads !== was.loads) atEnd.current = true
    const prepended =
      loads === was.loads && was.first !== undefined && first !== was.first && items.some((i) => i.id === was.first)
    if (prepended) el.scrollTop += el.scrollHeight - was.height
    else if (atEnd.current) el.scrollTop = el.scrollHeight
    before.current = { first, height: el.scrollHeight, loads }
  }, [items, loads])

  const onScroll = () => {
    const el = ref.current
    if (!el) return
    atEnd.current = el.scrollTop + el.clientHeight >= el.scrollHeight - STICK_PX
    if (el.scrollTop < TOP_PX && earlier && !loadingOlder) loadEarlier()
  }

  return (
    <div className="transcript" ref={ref} onScroll={onScroll}>
      <div className="transcript-inner">
        {earlier && (
          <div className="load-earlier">
            <button type="button" className="btn btn-ghost btn-sm" disabled={loadingOlder} onClick={loadEarlier}>
              {loadingOlder ? (
                'Loading…'
              ) : (
                <>
                  <Icon.ArrowUp size={14} /> Load earlier
                </>
              )}
            </button>
          </div>
        )}
        {children}
      </div>
    </div>
  )
}
