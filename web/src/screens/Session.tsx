// One session (frontend spec §6): its header, its transcript, whose question
// cards answer, and its composer.
//
// - The items come from the item store (`useSessionItems`): the first page,
//   then the stream. The header reads the list's summary when the caller has
//   it, else the session's detail, fetched once. While the list's first page
//   is on its way, the header waits for it rather than fetching the detail.
// - The transcript opens at its end and stays there while the reader is at
//   the end. It opens with at most the newest `TAIL` items, and grows at the
//   end while the reader is there: "Load earlier" (or
//   scrolling to the top) first shows `TAIL` more of the items held, and
//   only when none is held fetches the turns before. Either way what the
//   reader sees stays where it was.
// - The window's first row is pinned by item id: an upsert, or an item
//   joining a turn above it or at the end, never moves it. A resync (the
//   items replaced) goes back to the tail, and to the end.
// - A question the operator can answer is never held above the window: the
//   window starts at it when it is older than the tail, and stays there
//   once it is answered. Being answerable, it is pending in the open turn,
//   so this grows the window only as far as that turn reaches back.
// - The stream's state shows as "Reconnecting…", and "Resynced" for a moment
//   after the items were replaced.
// - The composer sits under the transcript: the session's own (F-17), fed
//   the header's session, the item store's catalogue and the capabilities
//   of the session's host. The hosts are fetched once per view: the header
//   names the host, the composer reads its capabilities, and the question
//   cards whether it is connected. Through it, a turn that was not delivered comes
//   back as a draft ("Send again"), and a question the agent stopped
//   waiting on is answered as a new message; neither sends on its own.
// - A session the New Session screen opened with a notice (its start's
//   delivery unknown, or its first prompt refused: lib/start.ts) shows it
//   above the composer. The notice is read from the link once, then dropped
//   from the address, so a reload does not show it again.
// - A deleted session says so, nothing more is fetched, and its draft and
//   images are dropped.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { hatList, hostList, namesOf } from '../api/names'
import { sessionDetail } from '../api/view'
import { useClient } from '../app-client'
import { Composer, type ComposerHandle } from '../components/Composer'
import { ANSWER_LABEL, answerAsMessage } from '../components/composerWords'
import SessionHeader, { type HeaderInfo } from '../components/SessionHeader'
import Transcript from '../components/Transcript'
import type { ItemEnv } from '../components/items/types'
import type { Capabilities } from '../generated/protocol'
import type { SessionSummary } from '../generated/view'
import type { Item } from '../generated/view'
import { useAnnouncement } from '../hooks/useAnnouncement'
import { isAnswerable } from '../lib/delivery'
import { useMediaQuery } from '../hooks/useMediaQuery'
import { agentLabel } from '../lib/agent'
import { forgetAttachments } from '../lib/attachments'
import { saveDraft } from '../lib/drafts'
import { readStartNotice, startNoticeText } from '../lib/start'
import { Icon } from '../lib/ui'
import { Link, navigate, useLocation } from '../router'
import { useAnswering } from '../store/useAnswer'
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
 *  (a new search, a filter) until the detail or the summary comes back.
 *  With no summary given, the newer of the detail and the last summary
 *  seen is shown: a detail fetched before a summary came never wins over
 *  it once that summary goes. */
function useHeaderInfo(id: string, summary: SessionSummary | undefined, awaitSummary: boolean): HeaderInfo | undefined {
  const client = useClient()
  // When each source came, in one order.
  const clock = useRef(0)
  const [detail, setDetail] = useState<{ info: HeaderInfo; at: number } | undefined>(undefined)
  const last = useRef<{ info: SessionSummary; at: number } | undefined>(undefined)
  if (summary && last.current?.info !== summary) last.current = { info: summary, at: ++clock.current }
  const needed = summary === undefined && !awaitSummary
  useEffect(() => {
    if (!needed) return
    let live = true
    sessionDetail(client, id).then(
      // The detail has no `question_waits`: its open pending requests say
      // it, as the summary's does ("whether or not an answer is queued"),
      // so a question outside a turn reads "Waiting on a question" here
      // too (decision 8: one status rule for the row and the header).
      (d) =>
        live &&
        setDetail({ info: { ...d, question_waits: Array.isArray(d.pending) && d.pending.length > 0 }, at: ++clock.current }),
      // The item store says when the session is gone; the header stays bare.
      () => {},
    )
    return () => {
      live = false
    }
  }, [client, id, needed])
  if (summary) return summary
  const held = last.current
  if (detail && (!held || detail.at > held.at)) return detail.info
  return held?.info ?? detail?.info
}

/** A list fetched once, when `wanted`; on failure, none (`undefined`). */
function useList(fetch: 'hosts' | 'hats', wanted: boolean): unknown {
  const client = useClient()
  const [list, setList] = useState<unknown>(undefined)
  useEffect(() => {
    if (!wanted) return
    let live = true
    const request = fetch === 'hosts' ? hostList(client) : hatList(client)
    request.then(
      (answer) => live && setList(answer),
      () => {},
    )
    return () => {
      live = false
    }
  }, [client, fetch, wanted])
  return list
}

/** The capabilities `GET /api/hosts` reports for host `hostId`; null while
 *  unknown (no list yet, the host not in it, or no list of capabilities). */
function capabilitiesOf(hosts: unknown, hostId: string | undefined): Capabilities | null {
  if (hostId === undefined || !Array.isArray(hosts)) return null
  for (const entry of hosts as unknown[]) {
    if (!entry || typeof entry !== 'object') continue
    const host = entry as Record<string, unknown>
    if (host.host_id === hostId) return Array.isArray(host.capabilities) ? (host.capabilities as Capabilities) : null
  }
  return null
}

/** Whether `connected` is what `GET /api/hosts` reports for host `hostId`;
 *  undefined while unknown. */
function connectedOf(hosts: unknown, hostId: string | undefined): boolean | undefined {
  if (hostId === undefined || !Array.isArray(hosts)) return undefined
  for (const entry of hosts as unknown[]) {
    if (!entry || typeof entry !== 'object') continue
    const host = entry as Record<string, unknown>
    if (host.host_id === hostId) return typeof host.connected === 'boolean' ? host.connected : undefined
  }
  return undefined
}

/** The notice a start left in the link to session `id`, in words: shown
 *  while the view shows that session, the link cleaned at once (replacing
 *  the history entry) so a reload does not show it again. */
function useStartNotice(id: string): string | null {
  const { pathname, search } = useLocation()
  const fromLink = useMemo(() => {
    const notice = readStartNotice(search)
    return notice ? startNoticeText(notice) : null
  }, [search])
  const [held, setHeld] = useState<{ id: string; text: string } | null>(null)
  useEffect(() => {
    if (fromLink === null) return
    setHeld({ id, text: fromLink })
    navigate(pathname, { replace: true })
  }, [fromLink, id, pathname])
  return fromLink ?? (held && held.id === id ? held.text : null)
}

/** Where the transcript's window starts: the pinned item's index, or the
 *  newest `size` items when nothing is pinned for these `loads`; earlier
 *  when a question that can be answered is held above it. Decided while
 *  rendering, never after: a card that left the window for one render
 *  would be mounted again. */
function windowStart(items: readonly Item[], pin: string | null, size: number): number {
  const tail = Math.max(0, items.length - size)
  const at = pin === null ? -1 : items.findIndex((item) => item.id === pin)
  const start = at < 0 ? tail : at
  const open = items.findIndex((item) => item.kind === 'question' && isAnswerable(item))
  return open >= 0 && open < start ? open : start
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

export default function SessionView({ id, summary, awaitSummary = false, tail = TAIL, timing }: Props) {
  const s = useSessionItems(id, timing)
  const info = useHeaderInfo(id, summary, awaitSummary)
  const win = useTailWindow(s.items, s.loads, tail)
  const narrow = useMediaQuery('(max-width: 767px)')
  const hostItems = useList('hosts', true)
  const hosts = useMemo(() => namesOf(hostItems, 'host_id'), [hostItems])
  const wantsHats = useMemo(() => s.items.some((i) => i.kind === 'marker' && i.marker === 'hat_reassigned'), [s.items])
  const hatItems = useList('hats', wantsHats)
  const hats = useMemo(() => namesOf(hatItems, 'id'), [hatItems])
  const plan = useMemo(() => latestPlan(s.items), [s.items])
  const startNotice = useStartNotice(id)
  const answers = useAnswering(id, info, s.items, s.loading, s.loads, connectedOf(hostItems, info?.host_id))
  const announcement = useAnnouncement(answers)

  // The composer's handle: the item seams reach the draft through it, and
  // stay the same functions for as long as the view is shown.
  const composer = useRef<ComposerHandle>(null)
  const onSendAgain = useCallback((item: Extract<Item, { kind: 'marker' }>) => {
    if (item.about_turn) void composer.current?.refill(item.about_turn)
  }, [])
  const onAnswerAsMessage = useCallback(
    (question: string) => composer.current?.prefill(answerAsMessage(question), ANSWER_LABEL),
    [],
  )
  const composerEmpty = useCallback(() => composer.current?.isEmpty() ?? true, [])

  const env: ItemEnv = useMemo(
    () => ({
      sessionId: id,
      agent: agentLabel(info?.agent),
      hatName: (hat: string) => hats.get(hat),
      answers,
      onSendAgain,
      onAnswerAsMessage,
      composerEmpty,
    }),
    [id, info?.agent, hats, answers, onSendAgain, onAnswerAsMessage, composerEmpty],
  )

  // A deleted session's draft and images can never be sent: drop them.
  useEffect(() => {
    if (!s.removed) return
    saveDraft(id, '')
    forgetAttachments(id)
  }, [id, s.removed])

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
      {/* One region for the view: a question that opened without taking the
          focus is said here, politely, never one from the first page. */}
      <p className="sr-only" role="status" aria-live="polite">
        {announcement}
      </p>
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
      {startNotice && (
        <p className="start-notice" role="status">
          {startNotice}
        </p>
      )}
      <Composer
        handle={composer}
        sessionId={id}
        session={info ? { activity: info.activity, lifecycle: info.lifecycle } : null}
        capabilities={capabilitiesOf(hostItems, info?.host_id)}
        catalog={s.catalog}
        onCatalog={s.setCatalog}
      />
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
