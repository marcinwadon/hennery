import SessionName from '../components/SessionNames'
import { useReadResult } from '../components/ResultReview'
import { useTurnEnds } from '../hooks/useTurnEnds'
import '../styles/session.css'
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
// - Under the composer, a session that is not running has its footer
//   (SessionFooter: Resume, the start's spinner, why it failed); the header
//   carries its menu (SessionMenu: Park, Close, Delete). The composer stays
//   whatever the lifecycle, so a draft put there is never lost.
// - A deleted session (deleted from here, `session_removed`, or a 404) says
//   so, nothing more is fetched, its draft and images are dropped, and it
//   leaves the session list (`onRemoved`). Deleted from here, it also says
//   what the delete left of the agent's transcript on the host.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { hatList, hostList, namesOf } from '../api/names'
import { resume } from '../api/turns'
import { useClient } from '../app-client'
import { Composer, type ComposerHandle } from '../components/Composer'
import { ANSWER_LABEL, answerAsMessage } from '../components/composerWords'
import SessionFooter from '../components/SessionFooter'
import SessionHeader, { type HeaderInfo } from '../components/SessionHeader'
import UnseenObserver from '../components/UnseenObserver'
import FamilyPanel from '../components/FamilyPanel'
import SessionDetails, { type SessionDetailsTab } from '../components/SessionDetails'
import { useSessionScope } from '../components/SessionScope'
import { useRegisterAction, useRegisterContext } from '../app-actions'
import { saveTheme, useAccentStyle } from '../lib/theme'
import { themeOf } from '../lib/hats'
import { takeOver } from '../api/view'
import { QuestionPresentationContext } from '../components/QuestionActions'
import { Text } from '../lib/text'
import { useMcpDelivery } from '../hooks/useMcpDelivery'
import { useSessionConfig } from '../hooks/useSessionConfig'
import SessionMenu from '../components/SessionMenu'
import { deleteNotes } from '../components/sessionWords'
import Transcript from '../components/Transcript'
import type { ItemEnv } from '../components/items/types'
import type { Capabilities, DeleteResult, HatItem, SessionDetail } from '../generated/protocol'
import type { FamilyPage, SessionSummary } from '../generated/view'
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
import { useAnswering, useHostAway } from '../store/useAnswer'
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
  /** The session was found gone (deleted from here or elsewhere, or a
   *  404): the list drops it (useSessionList's `remove`). */
  onRemoved?: (id: string) => void
}

/** The newest plan among the loaded items. */
function latestPlan(items: Item[]): Extract<Item, { kind: 'plan' }> | undefined {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]
    if (item.kind === 'plan') return item
  }
  return undefined
}

/** The summary owns the header while present. Delivery's detail read is
 *  shared with the header; a search dropping a summary is not a new read.
 *  Once both sources have been seen, their arrival order decides which
 *  cached one remains when the summary is absent. */
function useHeaderInfo(id: string, summary: SessionSummary | undefined, awaitSummary: boolean, detail: SessionDetail | null): HeaderInfo | undefined {
  const held = useRef<{ id: string; clock: number; summary?: { info: SessionSummary; at: number }; detail?: { source: SessionDetail; info: HeaderInfo; at: number } }>({ id, clock: 0 })
  if (held.current.id !== id) held.current = { id, clock: 0 }
  const current = held.current
  if (detail && current.detail?.source !== detail) {
    current.detail = { source: detail, info: { ...detail, question_waits: Array.isArray(detail.pending) && detail.pending.length > 0 }, at: ++current.clock }
  }
  if (summary && current.summary?.info !== summary) current.summary = { info: summary, at: ++current.clock }
  if (summary) return summary
  if (awaitSummary) return current.summary?.info
  if (current.detail && (!current.summary || current.detail.at > current.summary.at)) return current.detail.info
  return current.summary?.info ?? current.detail?.info
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
    const query = new URLSearchParams(location.search)
    query.delete('notice'); query.delete('code')
    navigate(pathname + (query.size ? `?${query}` : ''), { replace: true })
  }, [fromLink, id, pathname])
  return fromLink ?? (held && held.id === id ? held.text : null)
}

/** Where the transcript's window starts: the pinned item's index, or the
 *  newest `size` items when nothing is pinned for these `loads`; earlier
 *  when a question that can be answered is held above it. Decided while
 *  rendering, never after: a card that left the window for one render
 *  would be mounted again. */
function windowStart(items: readonly Item[], pin: string | null, size: number, pending?: string): number {
  const tail = Math.max(0, items.length - size)
  const at = pin === null ? -1 : items.findIndex((item) => item.id === pin)
  let start = at < 0 ? tail : at
  const target = pending ? items.findIndex(item => item.kind === 'question' && item.pending_id === pending) : -1
  if (target >= 0) start = Math.min(start, target)
  const open = items.findIndex((item) => item.kind === 'question' && isAnswerable(item))
  return open >= 0 && open < start ? open : start
}

/** The transcript's window over `items`: its first row pinned by id, `TAIL`
 *  rows at first, more revealed on demand; back to the tail when `loads`
 *  changes (a resync replaced the items). */
function useTailWindow(items: Item[], loads: number, size: number, pending?: string) {
  const [pinned, setPinned] = useState<{ id: string; loads: number } | null>(null)
  const pin = pinned && pinned.loads === loads ? pinned.id : null
  const start = windowStart(items, pin, size, pending)
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

export default function SessionView(props: Props) {
  return <SelectedSession key={props.id} {...props} />
}

function SelectedSession({ id, summary, awaitSummary = false, tail = TAIL, timing, onRemoved }: Props) {
  const client = useClient()
  const scope = useSessionScope()
  const wide = useMediaQuery('(min-width: 1024px)')
  const [detailsOpen, setDetailsOpen] = useState(wide)
  const [detailsTab, setDetailsTab] = useState<SessionDetailsTab>('Family')
  const detailsTrigger = useRef<HTMLButtonElement>(null)
  const detailsReturn = useRef<HTMLButtonElement>(null)
  const [settingsFocus, setSettingsFocus] = useState(0)
  const toggleDetails = useCallback(() => { detailsReturn.current = detailsTrigger.current; setDetailsOpen(open => !open) }, [])
  const openFamily = useCallback(() => { detailsReturn.current = detailsTrigger.current; setDetailsTab('Family'); setDetailsOpen(true) }, [])
  const openSettings = useCallback((trigger: HTMLButtonElement) => {
    detailsReturn.current = trigger; setDetailsTab('Details'); setDetailsOpen(true); setSettingsFocus(n => n + 1)
  }, [])
  const closeDetails = useCallback(() => { setDetailsOpen(false); (detailsReturn.current ?? detailsTrigger.current)?.focus() }, [])
  const [familyRefresh, setFamilyRefresh] = useState(0)
  const [taking, setTaking] = useState(false)
  const [takeError, setTakeError] = useState<string | null>(null)
  const [selectedQuestion, setSelectedQuestion] = useState<string | null>(null)
  const [jumpPending, setJumpPending] = useState<string | null>(null)
  const [answerNotice, setAnswerNotice] = useState<string | null>(null)
  const s = useSessionItems(id, timing)
  const configuration = useSessionConfig(id, s.setCatalog)
  const [familyRead, setFamilyRead] = useState<{ id: string; page: FamilyPage } | null>(null)
  const onFamilyPage = useCallback((page: FamilyPage) => setFamilyRead({ id, page }), [id])
  const family = familyRead?.id === id && (!summary || (summary.hat_id === familyRead.page.summary.hat_id && summary.family?.revision === familyRead.page.summary.family?.revision)) ? familyRead.page : null
  const mcp = useMcpDelivery(id, summary?.lifecycle, Math.max(0, s.loads - 1), s.catalogChanges, (summary ?? family?.summary)?.hat_id)
  const unseen = (summary ?? family?.summary)?.unseen
  const info = useHeaderInfo(id, summary ?? family?.summary, awaitSummary, mcp.detail)
  const { read: acknowledgeResult } = useReadResult(id, info?.review)
  const { search } = useLocation()
  useEffect(() => { if (new URLSearchParams(search).get('family') === '1') openFamily() }, [search, openFamily])
  const requested = new URLSearchParams(search).get('pending')
  const legacy = mcp.detail?.pending?.find(p => p.state === 'open' && !p.answered)?.pending_id
  const targetQuestion = requested || legacy
  const focusTarget = useRef<string | null>(null)
  const questionRegion = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!s.loading && !s.removed && targetQuestion) void s.loadQuestion(targetQuestion)
  }, [id, targetQuestion, s.loading, s.removed, s.loads, s.loadQuestion])
  useEffect(() => {
    if (!targetQuestion || focusTarget.current === `${id}:${targetQuestion}`) return
    const card = [...(questionRegion.current?.querySelectorAll<HTMLElement>('[data-pending-id]') ?? [])].find(el => el.dataset.pendingId === targetQuestion)
    if (!card) return
    focusTarget.current = `${id}:${targetQuestion}`
    if (document.activeElement === document.body || document.activeElement === null) {
      if (!card.hasAttribute('tabindex')) card.tabIndex = -1
      card.scrollIntoView?.({ block: 'nearest' }); card.focus({ preventScroll: true })
    }
  }, [id, targetQuestion, s.items, s.pinned])
  const win = useTailWindow(s.items, s.loads, tail, targetQuestion)
  const turnEnds = useTurnEnds(id, win.visible, info?.activity, s.loads)
  const openPending = legacy ?? selectedQuestion ?? [...s.pinned, ...s.items].filter(item => item.kind === 'question').find(item => item.state === 'open' && !item.answered)?.pending_id
  useLayoutEffect(() => {
    if (!jumpPending) return
    const card = [...(questionRegion.current?.querySelectorAll<HTMLElement>('[data-pending-id]') ?? [])].find(el => el.dataset.pendingId === jumpPending)
    if (!card) return
    card.tabIndex = -1
    card.scrollIntoView?.({ block: 'center' }); card.focus({ preventScroll: true })
    setJumpPending(null)
  }, [jumpPending, win.visible, s.pinned])
  const narrow = useMediaQuery('(max-width: 767px)')
  useLayoutEffect(() => {
    if (!selectedQuestion) return
    // A resync or removal may unmount the selected card without its live →
    // stale effect. Reconcile against both rendered transcript regions, using
    // the card's canonical actionable state (including local answer outcomes).
    const cards = questionRegion.current?.querySelectorAll<HTMLElement>('.ask:not(.stale)') ?? []
    if (![...cards].some(card => card.dataset.pendingId === selectedQuestion)) setSelectedQuestion(null)
  }, [selectedQuestion, win.visible, s.pinned, s.loading])
  useLayoutEffect(() => {
    if (!narrow || !selectedQuestion) return
    // Run after the transcript's tail pin and browser focus scrolling settle.
    // Measure the card itself: its bottom action region sits outside the card.
    const frame = requestAnimationFrame(() => {
      const card = [...(questionRegion.current?.querySelectorAll<HTMLElement>('[data-pending-id]') ?? [])].find(el => el.dataset.pendingId === selectedQuestion)
      const scroll = card?.closest<HTMLElement>('.transcript')
      if (card && scroll) scroll.scrollTop += card.getBoundingClientRect().top - scroll.getBoundingClientRect().top - 16
    })
    return () => cancelAnimationFrame(frame)
  }, [narrow, selectedQuestion, s.loads])
  const hostItems = useList('hosts', true)
  const hosts = useMemo(() => namesOf(hostItems, 'host_id'), [hostItems])
  const hatItems = useList('hats', scope === null)
  const hats = useMemo(() => scope?.hats ? new Map(scope.hats.map(h => [h.id, h.name])) : namesOf(hatItems, 'id'), [scope?.hats, hatItems])
  const actualHat = (scope?.hats ?? (Array.isArray(hatItems) ? hatItems as HatItem[] : [])).find(h => h.id === info?.hat_id)
  const reportedSource = mcp.detail?.mcp_source_policy
  // Revisions are local to a hat. A detail read from the previous hat cannot
  // override the current hat, regardless of their revision numbers. Within one
  // hat, retain the newer source setting when a stale GET/list arrives.
  const sourcePolicy = reportedSource && actualHat && (mcp.detail?.hat_id !== actualHat.id || actualHat.mcp_policy_revision > reportedSource.revision)
    ? { ...reportedSource, policy: actualHat.mcp_source_policy, revision: actualHat.mcp_policy_revision }
    : reportedSource
  const accent = useAccentStyle(actualHat?.colour)
  useEffect(() => {
    if (!actualHat) return
    saveTheme(themeOf(actualHat.colour))
    return () => { if (scope?.hats) saveTheme(themeOf(scope.hats.find(h => h.id === scope.hat)?.colour)) }
  }, [actualHat?.id, actualHat?.colour, scope?.hats, scope?.hat])
  const plan = useMemo(() => latestPlan(s.items), [s.items])
  const startNotice = useStartNotice(id)
  const answers = useAnswering(id, info, s.items, s.loading, s.loads, connectedOf(hostItems, info?.host_id), s.mergeQuestion, s.pinned)
  const announcement = useAnnouncement(answers)
  const hostAway = useHostAway(answers)

  // The composer's handle: the item seams reach the draft through it, and
  // stay the same functions for as long as the view is shown.
  const composer = useRef<ComposerHandle>(null)
  const focusNew = useRef(new URLSearchParams(search).get('focus') === 'prompt')
  useEffect(() => {
    if (!focusNew.current || s.loading || s.removed || !info || !composer.current) return
    focusNew.current = false
    composer.current.focus()
    const query = new URLSearchParams(location.search)
    query.delete('focus')
    navigate(location.pathname + (query.size ? `?${query}` : ''), { replace: true })
  }, [s.loading, s.removed, info])
  const focusComposer = useCallback(() => {
    setSelectedQuestion(null)
    setTimeout(() => composer.current?.focus(), 0)
  }, [])
  const openProject = useCallback(() => {
    if (info) navigate(`/new?host=${encodeURIComponent(info.host_id)}&cwd=${encodeURIComponent(info.cwd)}`)
  }, [info])
  const [renaming, setRenaming] = useState(false)
  const beginRename = useCallback(() => setRenaming(true), [])
  const finishRename = useCallback(() => setRenaming(false), [])
  useRegisterAction('rename-session', { label: 'Rename session', enabled: !s.removed && !!info, run: beginRename })
  useRegisterAction('toggle-details', { label: 'Toggle session details', enabled: !s.removed, run: toggleDetails })
  useRegisterAction('focus-composer', { label: 'Focus composer', enabled: !s.removed, run: focusComposer })
  useRegisterAction('new-session', { label: 'New session in this project', enabled: !s.removed && !!info, run: openProject })
  useRegisterContext({ sessionId: s.removed ? undefined : id, hostId: s.removed ? undefined : info?.host_id, cwd: s.removed ? undefined : info?.cwd, hatId: s.removed ? undefined : info?.hat_id })
  const takeover = async () => {
    if (taking) return
    setTaking(true); setTakeError(null)
    try { await takeOver(client, id); setFamilyRefresh(n => n + 1) }
    catch { setTakeError('This session could not be taken over. Try again.') }
    finally { setTaking(false) }
  }
  const onSendAgain = useCallback((item: Extract<Item, { kind: 'marker' }>) => {
    if (item.about_turn) {
      focusComposer()
      void composer.current?.refill(item.about_turn)
    }
  }, [focusComposer])
  const onAnswerAsMessage = useCallback(
    (question: string) => { focusComposer(); composer.current?.prefill(answerAsMessage(question), ANSWER_LABEL) },
    [focusComposer],
  )
  const composerEmpty = useCallback(() => composer.current?.isEmpty() ?? true, [])
  // The view's one resume: the footer's Resume and the composer's "Resume
  // and send" both go through it.
  const onResume = useCallback(async () => {
    const answer = await resume(client, id)
    mcp.refresh()
    return answer
  }, [client, id, mcp.refresh])

  // A delete made here: what it left on the host, shown with the deleted
  // state. The answer may come after the stream's `session_removed`.
  const [deleted, setDeleted] = useState<{ id: string; result?: DeleteResult } | null>(null)
  const { markRemoved } = s
  const onDeleted = useCallback(
    (result: DeleteResult | undefined) => {
      setDeleted({ id, result })
      markRemoved()
    },
    [id, markRemoved],
  )
  const back = useRef<HTMLAnchorElement>(null)
  const capabilities = capabilitiesOf(hostItems, info?.host_id)
  const nextQuestion = useCallback(() => {
    const cards = [...(questionRegion.current?.querySelectorAll<HTMLElement>('.ask:not(.stale)') ?? [])]
    const at = cards.findIndex(card => card.dataset.pendingId === selectedQuestion)
    const next = cards[at + 1] ?? cards[0]
    next?.scrollIntoView?.({ block: 'center' }); next?.focus({ preventScroll: true })
  }, [selectedQuestion])
  const presentation = useMemo(() => narrow ? { selected: selectedQuestion, select: setSelectedQuestion, acknowledge: setAnswerNotice, next: nextQuestion, writePrompt: focusComposer } : null, [narrow, selectedQuestion, nextQuestion, focusComposer])

  const env: ItemEnv = useMemo(
    () => ({
      sessionId: id,
      agent: agentLabel(info?.agent),
      hatName: (hat: string) => hats.get(hat),
      answers,
      familyQuestions: family?.questions,
      onSendAgain,
      onAnswerAsMessage,
      composerEmpty,
    }),
    [id, info?.agent, hats, answers, family?.questions, onSendAgain, onAnswerAsMessage, composerEmpty],
  )

  // A deleted session's draft and images can never be sent: drop them, and
  // the session leaves the list.
  useEffect(() => {
    if (!s.removed) return
    saveDraft(id, '')
    forgetAttachments(id)
    onRemoved?.(id)
  }, [id, s.removed, onRemoved])

  if (s.removed) {
    const notes = deleted?.id === id ? deleteNotes(deleted.result) : []
    const hostNotes = deleted?.id === id ? (deleted.result?.host_transcript?.notes ?? []) : []
    return (
      <div className="session">
        <div className="welcome" role="status">
          <h1>This session was deleted</h1>
          <p>
            <bdi>{id}</bdi>
          </p>
          {notes.map((note) => (
            <p key={note} className="delete-note">
              {note}
            </p>
          ))}
          {hostNotes.length > 0 && (
            <ul className="delete-note">
              {hostNotes.map((note, i) => (
                <li key={i}>
                  <bdi>{String(note)}</bdi>
                </li>
              ))}
            </ul>
          )}
          <Link to="/sessions" className="btn btn-ghost" ref={back}>
            Back to sessions
          </Link>
        </div>
      </div>
    )
  }

  const sessionFooter = info && <SessionFooter key={info.lifecycle} info={info} hostName={hosts.get(info.host_id)} hostAway={hostAway} queuedCount={s.queue.filter(item => item.state === 'queued').length} onResume={onResume} />

  return (
    <QuestionPresentationContext.Provider value={presentation}>
    <div className={'session hn-accent-scope' + (narrow && selectedQuestion ? ' has-question-actions' : '')} style={accent} ref={questionRegion}>
      <SessionHeader
        renaming={renaming}
        onRename={info && !s.removed ? beginRename : undefined}
        finishRename={finishRename}
        id={id}
        info={info}
        hostName={info ? hosts.get(info.host_id) : undefined}
        plan={plan}
        narrow={narrow}
        hatName={actualHat?.name}
        hostConnected={connectedOf(hostItems, info?.host_id)}
        delivery={mcp.detail?.mcp_delivery}
        detailsOpen={detailsOpen}
        toggleDetails={toggleDetails}
        openFamily={openFamily}
        detailsTrigger={detailsTrigger}
        menu={
          <SessionMenu
            queuedCount={s.queue.filter(item => item.state !== 'delivering').length}
            sidebarPinned={info ? info.sidebar_pinned ?? false : undefined}
            onRename={info && !s.removed ? beginRename : undefined}
            id={id}
            lifecycle={info?.lifecycle}
            family={info?.family}
            canPark={capabilities?.includes('park') ?? false}
            supportsIdleHold={capabilities?.includes('idle_hold')}
            hostConnected={connectedOf(hostItems, info?.host_id)}
            onDeleted={onDeleted}
            focusAfterDelete={() => back.current}
          />
        }
      />
      {info?.family?.parent && family?.routing === 'active' && <div className="parent-banner" role="region" aria-label="Parent-driven session"><p>Driven by <Link to={`/sessions/${encodeURIComponent(info.family.parent.session_id)}`}><SessionName session={info.family.parent} fallback="parent session" /></Link>. Answers go to this child. A prompt from you takes over this session.</p><button type="button" className="btn btn-ghost btn-sm" disabled={taking} onClick={() => void takeover()}>{taking ? 'Taking over…' : 'Take over'}</button>{takeError && <p role="alert">{takeError}</p>}</div>}
      <div className="session-workspace">
      <div className="session-conversation">
      {answerNotice && <p className="answer-acknowledgement" role="status">{answerNotice}<button type="button" aria-label="Dismiss answer notice" onClick={() => setAnswerNotice(null)}>×</button></p>}
      {mcp.error && <p className="form-error"><Text>{mcp.error}</Text></p>}
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
      {s.questionError && <p className="form-error" role="status">{s.questionError}</p>}
      <Scroller
        items={win.visible}
        loads={s.loads}
        earlier={win.held > 0 || s.older}
        loadingOlder={s.loadingOlder}
        loadEarlier={() => {
          if (!win.reveal() && s.older && !s.loadingOlder) void s.loadOlder()
        }}
      >
        {s.loading ? <p className="transcript-empty">Loading…</p> : <>
          {s.pinned.length > 0 && <div className="pinned-questions" aria-label="Earlier questions"><Transcript items={s.pinned} env={env} /></div>}
          <Transcript items={win.visible} env={env} review={info?.review} turnEnds={turnEnds} waiting={info?.lifecycle === 'active' && info.activity === 'idle' && !info.question_waits && !info.family?.child_counts.active} />
          {unseen && <UnseenObserver id={id} eventId={unseen.event_id} revision={s.revision} />}
        </>}
      </Scroller>
      {startNotice && (
        <p className="start-notice" role="status">
          {startNotice}
        </p>
      )}
      {info?.lifecycle === 'parked' && sessionFooter}
      <Composer
        onSent={info?.review ? () => void acknowledgeResult() : undefined}
        handle={composer}
        sessionId={id}
        session={info ? { activity: info.activity, lifecycle: info.lifecycle } : null}
        capabilities={capabilities}
        queue={s.queue}
        onQueueChange={s.refreshQueue}
        catalog={s.catalog}
        configuration={configuration}
        onSettings={openSettings}
        settingsExpanded={detailsOpen && detailsTab === 'Details'}
        onResume={onResume}
        onJumpToQuestion={openPending ? () => {
          setJumpPending(openPending)
          navigate(`/sessions/${encodeURIComponent(id)}?pending=${encodeURIComponent(openPending)}`)
          void s.loadQuestion(openPending)
        } : undefined}
      />
      {info?.lifecycle !== 'parked' && sessionFooter}
      </div>
      <SessionDetails open={detailsOpen} tab={detailsTab} onTab={setDetailsTab} onClose={closeDetails} trigger={detailsReturn} settingsFocus={settingsFocus} configuration={configuration} catalog={s.catalog} info={info} hostName={info ? hosts.get(info.host_id) : undefined} hatName={actualHat?.name} plan={plan} delivery={mcp.detail?.mcp_delivery} sourcePolicy={sourcePolicy}>
        <FamilyPanel key={id} id={id} summary={summary} hosts={hosts} resync={Math.max(0, s.loads - 1) + familyRefresh} onPage={onFamilyPage} />
      </SessionDetails>
      </div>
    </div>
    </QuestionPresentationContext.Provider>
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
