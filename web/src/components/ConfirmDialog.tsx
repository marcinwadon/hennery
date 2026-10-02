// A confirmation before an action that cannot be taken back (frontend spec
// §8: renaming or revoking a host, purging a hat, removing a passkey or a
// device). The action runs from here: while it runs the dialog stays open,
// so a step-up dialog can open over it, and a refusal is shown in it, as
// text. Focus moves in, stays in (Tab cycles within it), and goes back
// where it was when it closes.
import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { messageOf } from '../api/errors'
import { Text } from '../lib/text'

interface Props {
  title: string
  children: ReactNode
  /** The button that runs `action`. */
  confirm: string
  danger?: boolean
  /** The action cannot run now; the body says why. */
  disabled?: boolean
  /** Runs on the confirm button; the dialog closes when it resolves. */
  action: () => Promise<void>
  onClose: () => void
}

export default function ConfirmDialog({ title, children, confirm, danger, disabled, action, onClose }: Props) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const dialog = useRef<HTMLDivElement>(null)
  const live = useRef(true)

  useEffect(() => {
    live.current = true
    const before = document.activeElement as HTMLElement | null
    dialog.current?.querySelector<HTMLElement>('[data-autofocus]')?.focus()
    return () => {
      live.current = false
      before?.focus?.()
    }
  }, [])

  const run = async () => {
    // Its buttons are disabled while the action runs: focus is held by the
    // dialog itself, so a step-up opened over it returns focus here.
    dialog.current?.focus()
    setBusy(true)
    setError(null)
    try {
      await action()
      if (live.current) onClose()
    } catch (err) {
      if (live.current) setError(messageOf(err))
    } finally {
      if (live.current) setBusy(false)
    }
  }

  // Tab and Shift+Tab cycle through the dialog's own controls: nothing
  // behind the overlay can be reached while it is open.
  const trap = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape' && !busy) {
      onClose()
      return
    }
    if (e.key !== 'Tab') return
    const controls = [
      ...(dialog.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ) ?? []),
    ]
    if (controls.length === 0) {
      e.preventDefault()
      return
    }
    const first = controls[0]
    const last = controls[controls.length - 1]
    const at = document.activeElement
    if (e.shiftKey && (at === first || !controls.includes(at as HTMLElement))) {
      e.preventDefault()
      last.focus()
    } else if (!e.shiftKey && (at === last || !controls.includes(at as HTMLElement))) {
      e.preventDefault()
      first.focus()
    }
  }

  return (
    <div className="overlay" onKeyDown={trap}>
      <div className="modal dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-title" ref={dialog} tabIndex={-1}>
        <div className="modal-head">
          <h2 className="modal-title" id="confirm-title">
            {title}
          </h2>
        </div>
        <div className="modal-body">
          {children}
          {error && (
            <p className="form-error" role="alert">
              <Text>{error}</Text>
            </p>
          )}
        </div>
        <div className="modal-foot">
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={busy} data-autofocus>
            Cancel
          </button>
          <span className="spacer" />
          <button type="button" className={danger ? 'btn btn-danger' : 'btn btn-primary'} onClick={run} disabled={busy || disabled}>
            {confirm}
          </button>
        </div>
      </div>
    </div>
  )
}
