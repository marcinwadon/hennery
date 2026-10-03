// The session header's menu (frontend spec §6.6): Park, Close and "Delete
// session".
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
import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react'
import { ApiFailure } from '../api/errors'
import { close, deleteSession, park } from '../api/turns'
import { useClient } from '../app-client'
import type { DeleteResult } from '../generated/protocol'
import { Icon } from '../lib/ui'
import ConfirmDialog from './ConfirmDialog'
import { closeRefusal, deleteRefusal, parkRefusal } from './sessionWords'

interface Props {
  id: string
  /** The session's lifecycle; undefined while unknown (Delete only). */
  lifecycle?: string
  /** The session's host announced the `park` capability. */
  canPark: boolean
  /** The session was deleted (`undefined`: a server that answers 204, or
   *  a session already gone). */
  onDeleted: (result: DeleteResult | undefined) => void
  /** Where focus goes when the dialog closes and this menu is gone (the
   *  deleted session's view). */
  focusAfterDelete?: () => HTMLElement | null
}

export default function SessionMenu({ id, lifecycle, canPark, onDeleted, focusAfterDelete }: Props) {
  const client = useClient()
  const menuId = useId()
  const [open, setOpen] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const box = useRef<HTMLDivElement>(null)

  const showPark = lifecycle === 'active' && canPark
  const showClose = lifecycle !== undefined && lifecycle !== 'closed'

  // A click anywhere outside closes the menu.
  useEffect(() => {
    if (!open) return
    const outside = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', outside)
    return () => document.removeEventListener('mousedown', outside)
  }, [open])

  // Opened: focus its first entry.
  useEffect(() => {
    if (open) box.current?.querySelector<HTMLElement>('.session-menu-list button')?.focus()
  }, [open])

  const dismiss = () => {
    setOpen(false)
    trigger.current?.focus()
  }

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape' && open) {
      e.preventDefault()
      e.stopPropagation()
      dismiss()
    }
  }

  const act = async (run: () => Promise<unknown>, refusal: (err: unknown) => string) => {
    dismiss()
    setBusy(true)
    setError(null)
    try {
      await run()
    } catch (err) {
      setError(refusal(err))
    } finally {
      setBusy(false)
    }
  }

  // A delete is an action like the others: while it runs, no entry starts
  // another (a second Delete included).
  const remove = async () => {
    let result: DeleteResult | undefined
    setBusy(true)
    try {
      result = await deleteSession(client, id)
    } catch (err) {
      // Gone already: as deleted, with nothing to say about the host.
      if (err instanceof ApiFailure && err.status === 404) return onDeleted(undefined)
      throw new Error(deleteRefusal(err))
    } finally {
      setBusy(false)
    }
    onDeleted(result)
  }

  return (
    <div className="session-menu" ref={box} onKeyDown={onKeyDown}>
      <button
        ref={trigger}
        type="button"
        className="session-menu-btn"
        aria-label="Session actions"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => setOpen((o) => !o)}
      >
        <Icon.Menu size={18} />
      </button>
      {open && (
        <div className="session-menu-list" id={menuId}>
          {showPark && (
            <button type="button" disabled={busy} onClick={() => void act(() => park(client, id), parkRefusal)}>
              Park
            </button>
          )}
          {showClose && (
            <button type="button" disabled={busy} onClick={() => void act(() => close(client, id), closeRefusal)}>
              Close
            </button>
          )}
          <button
            type="button"
            className="danger"
            disabled={busy}
            onClick={() => {
              setOpen(false)
              setError(null)
              setConfirming(true)
            }}
          >
            Delete session
          </button>
        </div>
      )}
      {error && (
        <p className="form-error session-menu-error" role="alert">
          <bdi>{error}</bdi>
        </p>
      )}
      {/* The page behind this dialog is not made inert (as on Hosts and Hats;
          plan 4c amendment of brief item 36, MUST-3): a question card opening
          behind it cannot take the focus, or a digit, because a card moves
          the focus only while it is free (QuestionCard's focusIsFree). */}
      {confirming && (
        <ConfirmDialog
          title="Delete this session?"
          confirm="Delete"
          danger
          action={remove}
          onClose={() => setConfirming(false)}
          returnFocus={() => (trigger.current?.isConnected ? trigger.current : (focusAfterDelete?.() ?? null))}
        >
          <p>
            Its transcript, its questions and the images only it holds are deleted here for good. A running session is
            closed first.
          </p>
          <p>The agent’s own transcript on the host is removed too, where the host can.</p>
        </ConfirmDialog>
      )}
    </div>
  )
}
