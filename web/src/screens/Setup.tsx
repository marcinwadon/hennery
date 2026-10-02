// The one-time setup (frontend spec §3, kernel spec §3.1): the owner's
// password, `public_url` (this page's origin unless changed) and the default
// hat's name, with the token the link carried; then the offer of a passkey,
// which setup's stepped-up session can register without a second password.
import { useState, type FormEvent } from 'react'
import { registerPasskey, setUp } from '../api/auth'
import { ApiFailure, messageOf } from '../api/errors'
import { HOME } from '../api/next'
import { passkeysSupported, wasDismissed } from '../api/webauthn'
import { useClient } from '../app-client'
import { Link, navigate } from '../router'
import { forgetSetupToken, setupToken } from '../setup-token'

const MIN_PASSWORD = 12

/** Answers after which the token is of no further use. */
const FINAL = ['invalid_setup_token', 'already_set_up']

export default function Setup() {
  const client = useClient()
  // Whether the form is offered: only a boolean, never a copy of the token,
  // which lives in `setup-token` alone so that forgetting it there forgets it.
  // A final answer withdraws the form: the token it would send is dead.
  const [offered, setOffered] = useState(() => setupToken() !== null)
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [publicUrl, setPublicUrl] = useState(() => location.origin)
  const [hatName, setHatName] = useState('Personal')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [done, setDone] = useState(false)
  const [alreadySetUp, setAlreadySetUp] = useState(false)

  if (done) return <PasskeyOffer />

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    if (password !== confirm) {
      setError('The two passwords differ.')
      return
    }
    // Read at the moment of sending, so a forgotten token is never sent.
    const token = setupToken()
    if (token === null) {
      setOffered(false)
      return
    }
    setError(null)
    setBusy(true)
    try {
      await setUp(client, { token, password, publicUrl, defaultHatName: hatName })
      forgetSetupToken()
      setDone(true)
    } catch (err) {
      if (err instanceof ApiFailure && FINAL.includes(err.code)) {
        forgetSetupToken()
        setOffered(false)
      }
      if (err instanceof ApiFailure && err.code === 'already_set_up') setAlreadySetUp(true)
      setError(messageOf(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <main className="auth">
      <div className="auth-card">
        <div className="brand-name">hennery</div>
        <h1>Set up hennery</h1>
        {!offered && error ? (
          <>
            <p className="form-error" role="alert">
              <bdi>{error}</bdi>
            </p>
            {alreadySetUp && (
              <p>
                <Link to="/login">Sign in</Link>
              </p>
            )}
          </>
        ) : !offered ? (
          <p className="form-error" role="alert">
            This page needs the setup link. Run <code>hennery admin setup-url</code> on the collector and open the link
            it prints.
          </p>
        ) : (
          <form onSubmit={submit}>
            <label className="field">
              <span className="field-label">
                Password <span className="hint">at least {MIN_PASSWORD} characters</span>
              </span>
              <input
                className="text-input"
                type="password"
                autoComplete="new-password"
                minLength={MIN_PASSWORD}
                required
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
            <label className="field">
              <span className="field-label">Password again</span>
              <input
                className="text-input"
                type="password"
                autoComplete="new-password"
                minLength={MIN_PASSWORD}
                required
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
              />
            </label>
            <label className="field">
              <span className="field-label">
                Public URL <span className="hint">where your browser reaches hennery</span>
              </span>
              <input
                className="text-input"
                type="url"
                required
                value={publicUrl}
                onChange={(e) => setPublicUrl(e.target.value)}
              />
            </label>
            <label className="field">
              <span className="field-label">
                Default hat <span className="hint">sessions belong to it unless a path rule says otherwise</span>
              </span>
              <input
                className="text-input"
                maxLength={64}
                required
                value={hatName}
                onChange={(e) => setHatName(e.target.value)}
              />
            </label>
            {error && (
              <p className="form-error" role="alert">
                <bdi>{error}</bdi>
              </p>
            )}
            <button type="submit" className="btn btn-primary" disabled={busy}>
              Set up
            </button>
          </form>
        )}
      </div>
    </main>
  )
}

/** After setup: add a passkey now, or skip. */
function PasskeyOffer() {
  const client = useClient()
  const [label, setLabel] = useState('This device')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [added, setAdded] = useState(false)
  const supported = passkeysSupported()

  const add = async (e: FormEvent) => {
    e.preventDefault()
    setError(null)
    setBusy(true)
    try {
      await registerPasskey(client, label)
      setAdded(true)
    } catch (err) {
      if (!wasDismissed(err)) setError(messageOf(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <main className="auth">
      <div className="auth-card">
        <div className="brand-name">hennery</div>
        <h1>hennery is set up</h1>
        <p role="status">You are signed in.</p>
        {added ? (
          <p role="status">Your passkey is added: sign in with it next time.</p>
        ) : supported ? (
          <form onSubmit={add}>
            <p>Add a passkey, to sign in without your password.</p>
            <label className="field">
              <span className="field-label">Passkey name</span>
              <input
                className="text-input"
                maxLength={64}
                required
                value={label}
                onChange={(e) => setLabel(e.target.value)}
              />
            </label>
            {error && (
              <p className="form-error" role="alert">
                <bdi>{error}</bdi>
              </p>
            )}
            <button type="submit" className="btn btn-primary" disabled={busy}>
              Add a passkey
            </button>
          </form>
        ) : null}
        <p>
          <button type="button" className="btn btn-ghost" onClick={() => navigate(HOME)}>
            {added || !supported ? 'Continue' : 'Skip'}
          </button>
        </p>
      </div>
    </main>
  )
}
