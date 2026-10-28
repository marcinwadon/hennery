import SessionName from './SessionNames'
// Shared session actions (frontend spec §6.6): inline in the header,
// or portalled beside a list row. Both use the same guards and requests.
//
// - Park only for an active session on a host that announced `park`; Close
//   for any session not closed yet (a starting one on a reachable host is
//   refused, 409 `starting`, in words). Delete always.
// - A disclosure, not an ARIA menu: a button that opens a list of plain
//   buttons, reached with Tab. Escape closes it, and so does a click
//   outside; either way focus goes back to the button.
// - "Delete session" closes the menu and opens a confirmation (ConfirmDialog:
//   modal, focus held in it, a step-up can open over it). The DELETE goes
//   through the client, which asks for the step-up. A 404 means the session
//   is gone already: as deleted. The view shows the deleted state with what
//   the delete left on the host.
// - Park and Close answer with the lifecycle; the list stream's upsert
//   brings it to the header. A refusal shows under the header, as text.
import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react'
import { createPortal } from 'react-dom'
import { ApiFailure } from '../api/errors'
import { close, deleteSession, park, promptQueue } from '../api/turns'
import { useClient } from '../app-client'
import type { DeleteResult, IdleHoldState } from '../generated/protocol'
import type { FamilySummary } from '../generated/view'
import { Link } from '../router'
import { agentLabel } from '../lib/agent'
import { sessionPath } from './SessionRow'
import { Icon } from './ui'
import ConfirmDialog from './ConfirmDialog'
import PinSessionAction from './PinSessionAction'
import { closeRefusal, deleteRefusal, parkRefusal } from './sessionWords'
import { useNewLikeThis } from '../hooks/useNewLikeThis'

export interface RowDisclosure {
  anchor: HTMLButtonElement
  menuId: string
  open: boolean
  setOpen: (open: boolean) => void
  available: boolean
}

interface Props {
  /** List owner outlives disappearing rows; only presentation differs. */
  row?: RowDisclosure
  loadQueueBeforeDelete?: boolean
  sidebarPinned?: boolean
  onRename?: () => void
  id: string
  /** The session's lifecycle; undefined while unknown (Delete only). */
  lifecycle: string | undefined
  /** Already owner/hat-filtered family metadata from the current view. */
  family?: FamilySummary
  /** Unsent operator messages, including retained failures, removed with this session. */
  queuedCount?: number
  /** The session's host announced the `park` capability. */
  canPark: boolean
  /** Undefined until host metadata is known; the DTO records intent, not acknowledgement. */
  supportsIdleHold?: boolean
  hostConnected?: boolean
  /** The session was deleted (`undefined`: a server that answers 204, or
   *  a session already gone). */
  onDeleted: (result: DeleteResult | undefined) => void
  /** Where focus goes when the dialog closes and this menu is gone (the
   *  deleted session's view). */
  focusAfterDelete?: () => HTMLElement | null
}

export default function SessionMenu({ id, lifecycle, family, queuedCount = 0, canPark, supportsIdleHold, hostConnected, onDeleted, focusAfterDelete, onRename, sidebarPinned, row, loadQueueBeforeDelete = false }: Props) {
  const client = useClient()
  const localMenuId = useId()
  const menuId = row?.menuId ?? localMenuId
  const [localOpen, setLocalOpen] = useState(false)
  const open = row ? row.open && row.available : localOpen
  const setOpen = row?.setOpen ?? setLocalOpen
  const [confirming, setConfirming] = useState(false)
  const [actionBusy, setBusy] = useState(false)
  const like = useNewLikeThis(id, row ? () => row.anchor.isConnected : undefined)
  const busy = actionBusy || like.busy
  const [error, setError] = useState<string | null>(null)
  const [hold, setHold] = useState<IdleHoldState | null>(null)
  const [pinBusy, setPinBusy] = useState(false)
  const pinGeneration = useRef(0)
  const trigger = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const anchor = row?.anchor
  const [place, setPlace] = useState({ top: 0, left: 0 })
  const scrollPositions = useRef(new Map<Element, { top: number; left: number }>())
  const [queueCount, setQueueCount] = useState<number>()
  const [queueError, setQueueError] = useState(false)
  const [queueAttempt, retryQueue] = useState(0)
  const currentQueueCount = loadQueueBeforeDelete ? queueCount : queuedCount
  const lifetime = useRef(0)
  useEffect(() => { lifetime.current++; return () => { lifetime.current++ } }, [client, id])

  useEffect(() => {
    if (!confirming || !loadQueueBeforeDelete) return
    let live = true
    setQueueCount(undefined); setQueueError(false)
    promptQueue(client, id).then(queue => {
      if (!Array.isArray(queue) || !queue.every(item => item && ['queued', 'failed', 'delivering'].includes(item.state))) throw new Error('Invalid queue')
      if (live) setQueueCount(queue.filter(item => item.state !== 'delivering').length)
    }).catch(() => { if (live) setQueueError(true) })
    return () => { live = false }
  }, [client, id, confirming, loadQueueBeforeDelete, queueAttempt])

  // Re-measure when the hold note/error changes height. The full menu must
  // fit a short viewport too, and scrolling inside it must keep it open.
  useLayoutEffect(() => {
    if (!open || !anchor || !menu.current) return
    const position = () => {
      scrollPositions.current.clear()
      for (let parent = anchor.parentElement; parent; parent = parent.parentElement) {
        scrollPositions.current.set(parent, { top: parent.scrollTop, left: parent.scrollLeft })
      }
      const at = anchor.getBoundingClientRect(), size = menu.current!.getBoundingClientRect()
      const top = at.bottom + size.height + 6 < innerHeight ? at.bottom + 6 : at.top - size.height - 6
      setPlace({ left: Math.max(8, Math.min(at.right - size.width, innerWidth - size.width - 8)), top: Math.max(8, Math.min(top, innerHeight - size.height - 8)) })
    }
    position()
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(position)
    observer.observe(menu.current)
    return () => observer.disconnect()
  }, [open, anchor, row])

  const showPark = lifecycle === 'active' && canPark
  const showClose = lifecycle !== undefined && lifecycle !== 'closed'
  const holdNote = !hold ? null : supportsIdleHold === false ? 'Parking hold saved; update the host to enforce it.' : hostConnected === false ? 'Parking hold saved; it applies after the host reconnects.' : supportsIdleHold === undefined || hostConnected === undefined ? 'Host enforcement of parking holds is unconfirmed.' : !hold.held ? null : hold.pinned ? 'You requested a parking hold.' : hold.family_active ? 'Family work requests a parking hold.' : 'Pending updates request a parking hold.'

  useEffect(() => {
    const current = ++pinGeneration.current
    if (!open) return
    setHold(null); setError(null); setPinBusy(true)
    client.request<IdleHoldState>('GET', `/api/sessions/${encodeURIComponent(id)}/idle-hold`).then(value => {
      if (pinGeneration.current === current) setHold(value)
    }, () => { if (pinGeneration.current === current) setError('Automatic parking settings could not be read. Reopen this menu to retry.') })
      .finally(() => { if (pinGeneration.current === current) setPinBusy(false) })
    return () => { pinGeneration.current++ }
  }, [client, id, open])

  const togglePin = async () => {
    if (!hold || pinBusy) return
    const current = pinGeneration.current
    setPinBusy(true); setError(null)
    try {
      const value = await client.request<IdleHoldState>('PATCH', `/api/sessions/${encodeURIComponent(id)}/idle-hold`, { pinned: !hold.pinned })
      if (pinGeneration.current === current) setHold(value)
    } catch {
      if (pinGeneration.current === current) setError('Automatic parking settings could not be saved. Your previous preference is kept.')
    } finally { if (pinGeneration.current === current) setPinBusy(false) }
  }

  useEffect(() => {
    if (!open) return
    const outside = (event: MouseEvent) => {
      if (!(event.target instanceof Node)) return
      if (!menu.current?.contains(event.target) && !(anchor ?? trigger.current)?.contains(event.target)) setOpen(false)
    }
    const move = (event: Event) => {
      // Focusing/clicking an offscreen trigger scrolls it before opening,
      // but that scroll event may arrive afterwards. Its position was
      // already measured; only a new scroll should dismiss the menu.
      if (event.type === 'scroll' && event.target instanceof Element) {
        const before = scrollPositions.current.get(event.target)
        if (before && before.top === event.target.scrollTop && before.left === event.target.scrollLeft) return
      }
      if (!(event.target instanceof Node) || !menu.current?.contains(event.target)) {
        if (menu.current?.contains(document.activeElement)) (anchor ?? trigger.current)?.focus({ preventScroll: true })
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', outside)
    if (anchor) { document.addEventListener('scroll', move, true); window.addEventListener('resize', move) }
    return () => {
      document.removeEventListener('mousedown', outside)
      document.removeEventListener('scroll', move, true); window.removeEventListener('resize', move)
    }
  }, [open, anchor, setOpen])

  useEffect(() => {
    if (open) menu.current?.querySelector<HTMLElement>('button:not([disabled])')?.focus()
  }, [open])

  const dismiss = () => {
    setOpen(false)
    const target = anchor ?? trigger.current
    if (target?.isConnected) target.focus()
    else focusAfterDelete?.()?.focus()
  }

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape' && open) {
      e.preventDefault()
      e.stopPropagation()
      dismiss()
    }
  }

  const act = async (run: () => Promise<unknown>, refusal: (err: unknown) => string) => {
    const current = lifetime.current
    dismiss()
    setBusy(true)
    setError(null)
    try {
      await run()
    } catch (err) {
      if (current === lifetime.current) setError(refusal(err))
    } finally {
      if (current === lifetime.current) setBusy(false)
    }
  }

  // A delete is an action like the others: while it runs, no entry starts
  // another (a second Delete included). A header is removed when its view
  // receives session_removed; its parent still needs the eventual host result.
  const remove = async () => {
    const current = lifetime.current
    let result: DeleteResult | undefined
    setBusy(true)
    try {
      result = await deleteSession(client, id)
    } catch (err) {
      // Gone already: as deleted, with nothing to say about the host.
      if (err instanceof ApiFailure && err.status === 404) { if (!row || current === lifetime.current) onDeleted(undefined); return }
      throw new Error(deleteRefusal(err))
    } finally {
      if (current === lifetime.current) setBusy(false)
    }
    if (!row || current === lifetime.current) onDeleted(result)
  }

  const entries = open && (
    <div ref={menu} className={row ? 'session-menu-list session-row-menu' : 'session-menu-list'} style={row ? place : undefined} id={menuId} onKeyDown={onKeyDown}>
      {onRename && <button type="button" onClick={() => { setOpen(false); onRename() }}>Rename</button>}
      <button type="button" disabled={busy || lifecycle === undefined || hostConnected === false} title={hostConnected === false ? 'The host is offline' : undefined} onClick={() => { dismiss(); setError(null); void like.start() }}>New like this</button>
      {sidebarPinned !== undefined && <PinSessionAction key={id} id={id} pinned={sidebarPinned} disabled={busy} />}
      <button type="button" role="switch" aria-label="Do not park" aria-checked={hold?.pinned ?? false} disabled={busy || pinBusy || hold === null} onClick={() => void togglePin()}><span>Do not park</span><span className="idle-hold-state" aria-hidden="true">{hold ? hold.pinned ? '✓ On' : 'Off' : '…'}</span></button>
      {holdNote && <p className="idle-hold-note" role="status">{holdNote}</p>}
      {showPark && (
        <button type="button" disabled={busy || hostConnected === false} title={hostConnected === false ? 'The host is offline' : undefined} onClick={() => void act(() => park(client, id), parkRefusal)}>
          Park
        </button>
      )}
      {showClose && (
        <button type="button" disabled={busy} onClick={() => void act(() => close(client, id), closeRefusal)}>
          Close
        </button>
      )}
      <hr />
      <button
        type="button"
        className="danger"
        disabled={busy}
        onClick={() => {
          setOpen(false)
          setError(null)
          setQueueCount(undefined); setQueueError(false)
          setConfirming(true)
        }}
      >
        Delete session
      </button>
    </div>
  )

  const confirmation = confirming && (
    <ConfirmDialog
      title="Delete this session?"
      confirm="Delete"
      danger
      disabled={loadQueueBeforeDelete && queueCount === undefined}
      action={remove}
      onClose={() => setConfirming(false)}
      returnFocus={() => { const target = anchor ?? trigger.current; return target?.isConnected ? target : (focusAfterDelete?.() ?? null) }}
    >
      <p>
        Its transcript, its questions and the images only it holds are deleted here for good. A running session is
        closed first.
      </p>
      {loadQueueBeforeDelete && queueCount === undefined && (queueError ? <p className="form-error" role="alert">Queued messages could not be read. <button type="button" onClick={() => retryQueue(n => n + 1)}>Retry</button></p> : <p role="status">Reading queued messages…</p>)}
      {currentQueueCount !== undefined && currentQueueCount > 0 && <p>{currentQueueCount} unsent queued {currentQueueCount === 1 ? 'message and its images will' : 'messages and their images will'} also be deleted.</p>}
      {family && <>
        <p>Copies already delivered to related sessions remain in their transcripts. Those sessions are kept.</p>
        <ul aria-label="Related sessions">
          {[...(family.parent ? [family.parent] : []), ...family.children].map(related => <li key={related.session_id}><Link to={sessionPath(related.session_id)}><SessionName session={related} fallback={agentLabel(related.agent)} /></Link></li>)}
        </ul>
      </>}
      <p>The agent’s own transcript on the host is removed too, where the host can.</p>
    </ConfirmDialog>
  )

  return (
    <div className={row ? "session-list-menu" : "session-menu"} onKeyDown={onKeyDown}>
      {!row && <button
        ref={trigger}
        type="button"
        className="session-menu-btn"
        aria-label="Session actions"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => setOpen(!open)}
      >
        <Icon name="ellipsis-horizontal" size={18} />
      </button>}
      {row ? entries && createPortal(entries, document.body) : entries}
      {like.busy && <span className="session-like-pending" role="status">Starting session…</span>}
      {(error || like.error) && (
        <p className={row ? "form-error list-note" : "form-error session-menu-error"} role="alert">
          <bdi>{error || like.error}</bdi>
        </p>
      )}
      {row ? confirmation && createPortal(confirmation, document.body) : confirmation}
    </div>
  )
}
