// Sign in (frontend spec §3, client view spec D6): "Sign in with passkey"
// first, the password second. The passkey's ceremony is started when the
// page loads, so the button's click can open the browser's prompt at once;
// a 409 (`no_passkeys`, `passkeys_unavailable`) hides the button. A 429
// disables the password for its `Retry-After`.
import { useCallback, useEffect, useState, type FormEvent } from 'react'
import { finishPasskeyLogin, getOptions, logIn, startPasskeyLogin } from '../api/auth'
import { ApiFailure, messageOf } from '../api/errors'
import { safeNext } from '../api/next'
import { passkeysSupported, wasDismissed } from '../api/webauthn'
import type { PasskeyCeremony } from '../generated/protocol'
import { useClient } from '../app-client'
import { navigate, useLocation } from '../router'

export default function Login() {
  const client = useClient()
  const { search } = useLocation()
  const [ceremony, setCeremony] = useState<PasskeyCeremony | null>(null)
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [waitUntil, setWaitUntil] = useState(0)
  const [now, setNow] = useState(() => Date.now())

  const signedIn = () => navigate(safeNext(new URLSearchParams(search).get('next'), location.origin), { replace: true })

  const startCeremony = useCallback(
    (live: () => boolean = () => true) => {
      if (!passkeysSupported()) return
      startPasskeyLogin(client).then(
        (c) => live() && setCeremony(c),
        (err) => {
          // 409: no passkey, or none possible here; anything else, the
          // password still works. Either way, no ceremony, so no button.
          if (err instanceof ApiFailure && err.code === 'setup_required' && live()) setError(err.message)
        },
      )
    },
    [client],
  )

  useEffect(() => {
    let live = true
    startCeremony(() => live)
    return () => {
      live = false
    }
  }, [startCeremony])

  // The countdown of a 429.
  useEffect(() => {
    if (waitUntil <= now) return
    const timer = setTimeout(() => setNow(Date.now()), 1000)
    return () => clearTimeout(timer)
  }, [waitUntil, now])
  const waiting = Math.ceil((waitUntil - now) / 1000)

  const withPasskey = async () => {
    if (!ceremony) return
    setError(null)
    let credential: Credential | null
    try {
      // Straight from the click: the prompt needs its user activation.
      credential = await navigator.credentials.get(getOptions(ceremony))
    } catch (err) {
      if (!wasDismissed(err)) setError(messageOf(err))
      return
    }
    setBusy(true)
    try {
      await finishPasskeyLogin(client, ceremony, credential)
      signedIn()
    } catch (err) {
      setError(messageOf(err))
      // Single use: a new ceremony for the next try.
      setCeremony(null)
      startCeremony()
    } finally {
      setBusy(false)
    }
  }

  const withPassword = async (e: FormEvent) => {
    e.preventDefault()
    setError(null)
    setBusy(true)
    try {
      await logIn(client, password)
      signedIn()
    } catch (err) {
      if (err instanceof ApiFailure && err.code === 'rate_limited') {
        const at = Date.now()
        setNow(at)
        setWaitUntil(at + (err.retryAfter ?? 60) * 1000)
      }
      setError(messageOf(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <main className="auth">
      <div className="auth-card">
        <div className="brand-name">hennery</div>
        <h1>Sign in</h1>
        {ceremony && (
          <>
            <button type="button" className="btn btn-primary auth-passkey" onClick={withPasskey} disabled={busy}>
              Sign in with passkey
            </button>
            <p className="auth-or">or with your password</p>
          </>
        )}
        <form onSubmit={withPassword}>
          <label className="field">
            <span className="field-label">Password</span>
            <input
              className="text-input"
              type="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </label>
          {error && (
            <p className="form-error" role="alert">
              <bdi>{error}</bdi>
            </p>
          )}
          {waiting > 0 && <p role="status">You can try again in {waiting} s.</p>}
          <button type="submit" className={ceremony ? 'btn btn-ghost' : 'btn btn-primary'} disabled={busy || waiting > 0}>
            Sign in
          </button>
        </form>
      </div>
    </main>
  )
}
