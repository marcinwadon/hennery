// The step-up dialog (frontend spec §3, kernel spec §3.4): a refused request
// waits while the owner confirms with a passkey, or else the password; then
// the client sends it once more. It sits over the page, so nothing under it
// unmounts and forms keep their input.
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { finishPasskeyStepUp, getOptions, startPasskeyStepUp, stepUpWithPassword } from '../api/auth'
import { ApiFailure, messageOf } from '../api/errors'
import { passkeysSupported, wasDismissed } from '../api/webauthn'
import type { PasskeyCeremony } from '../generated/protocol'
import { useClient } from '../app-client'

interface Props {
  onDone: () => void
  onCancel: () => void
}

export default function StepUpDialog({ onDone, onCancel }: Props) {
  const client = useClient()
  const [ceremony, setCeremony] = useState<PasskeyCeremony | null>(null)
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const dialog = useRef<HTMLDivElement>(null)

  // The passkey's options are fetched first, so the click that follows can
  // open the browser's prompt at once (a prompt needs the click itself).
  useEffect(() => {
    if (!passkeysSupported()) return
    let live = true
    startPasskeyStepUp(client)
      .then((c) => live && setCeremony(c))
      .catch(() => {
        // No passkeys, or none usable here: the password remains.
      })
    return () => {
      live = false
    }
  }, [client])

  // Focus moves into the dialog and back where it was when it closes.
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null
    dialog.current?.querySelector<HTMLElement>('button, input')?.focus()
    return () => before?.focus?.()
  }, [])

  const withPasskey = async () => {
    if (!ceremony) return
    setError(null)
    let credential: Credential | null
    try {
      credential = await navigator.credentials.get(getOptions(ceremony))
    } catch (err) {
      if (!wasDismissed(err)) setError(messageOf(err))
      return
    }
    setBusy(true)
    try {
      await finishPasskeyStepUp(client, ceremony, credential)
      onDone()
    } catch (err) {
      setError(messageOf(err))
      // A ceremony is single use: another try needs a new one.
      setCeremony(null)
      startPasskeyStepUp(client).then(setCeremony, () => {})
    } finally {
      setBusy(false)
    }
  }

  const withPassword = async (e: FormEvent) => {
    e.preventDefault()
    setError(null)
    setBusy(true)
    try {
      await stepUpWithPassword(client, password)
      onDone()
    } catch (err) {
      setError(
        err instanceof ApiFailure && err.code === 'rate_limited'
          ? `Too many attempts. Try again in ${err.retryAfter ?? 60} s.`
          : messageOf(err),
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="overlay" onKeyDown={(e) => e.key === 'Escape' && onCancel()}>
      <div className="modal dialog" role="dialog" aria-modal="true" aria-labelledby="step-up-title" ref={dialog}>
        <div className="modal-head">
          <div className="modal-eyebrow">Confirm it is you</div>
          <h2 className="modal-title" id="step-up-title">
            This needs a fresh confirmation
          </h2>
        </div>
        <div className="modal-body">
          {ceremony && (
            <p>
              <button type="button" className="btn btn-primary" onClick={withPasskey} disabled={busy}>
                Confirm with passkey
              </button>
            </p>
          )}
          <form onSubmit={withPassword}>
            <label className="field">
              <span className="field-label">{ceremony ? 'Or your password' : 'Your password'}</span>
              <input
                className="text-input"
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
              />
            </label>
            {error && (
              <p className="form-error" role="alert">
                <bdi>{error}</bdi>
              </p>
            )}
            <div className="modal-foot">
              <button type="button" className="btn btn-ghost" onClick={onCancel}>
                Cancel
              </button>
              <span className="spacer" />
              <button type="submit" className="btn btn-primary" disabled={busy || password === ''}>
                Confirm
              </button>
            </div>
          </form>
        </div>
      </div>
    </div>
  )
}
