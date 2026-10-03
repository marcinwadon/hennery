// Sign out (kernel spec §3.2): `POST /api/auth/logout` ends this browser's
// session, and only once it has does the browser go to the login screen. A
// sign-out that failed (no network, a refusal) says so and stays: the
// cookie is still good, and showing the login screen would claim
// otherwise.
import { useState } from 'react'
import { logOut } from '../api/auth'
import { messageOf } from '../api/errors'
import { Text } from '../lib/text'
import { useClient } from '../app-client'
import { navigate } from '../router'

export default function SignOut({ className = 'btn btn-ghost btn-sm' }: { className?: string }) {
  const client = useClient()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const signOut = async () => {
    setBusy(true)
    setError(null)
    try {
      await logOut(client)
    } catch (err) {
      setError(messageOf(err))
      setBusy(false)
      return
    }
    navigate('/login')
  }

  return (
    <>
      <button type="button" className={className} onClick={signOut} disabled={busy}>
        Sign out
      </button>
      {error && (
        <p className="form-error sign-out-error" role="alert">
          Not signed out: this browser is still signed in. <Text>{error}</Text>
        </p>
      )}
    </>
  )
}
