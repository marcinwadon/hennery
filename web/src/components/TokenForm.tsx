// A connection's static token (gateway spec §9: write-only, step-up). The
// field is not React state: it is read once when the form is sent, then
// emptied at once, whether the request succeeds, is refused or its step-up
// is cancelled, so the token is never kept by the page or shown again. A
// refused token is typed again. Password managers are asked to leave it
// alone: it is a vendor's token, not this site's password, and a manager
// that saved it could fill it into another connection's field.
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { messageOf } from '../api/errors'
import { setToken } from '../api/mcp'
import { useClient } from '../app-client'
import type { McpConnectionItem } from '../generated/protocol'
import { originOf } from '../lib/mcp'
import { Text } from '../lib/text'

interface Props {
  connection: McpConnectionItem
  /** The connection's hat, by name, when known. */
  hatName?: string
  /** The token was stored (204). */
  onSaved: () => void
}

export default function TokenForm({ connection, hatName, onSaved }: Props) {
  const client = useClient()
  const field = useRef<HTMLInputElement>(null)
  // A ref, not state: two presses in one frame both see the first.
  const running = useRef(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)

  // A change that deleted the token takes "Token saved." with it.
  useEffect(() => {
    if (!connection.has_credential) setSaved(false)
  }, [connection.has_credential])

  const send = async (e: FormEvent) => {
    // Never a native submit: the token would leave the page in a request
    // of the browser's own.
    e.preventDefault()
    const input = field.current
    // A second send while one runs is ignored; nothing is disabled, so
    // focus stays where it was.
    if (!input || running.current) return
    const token = input.value
    input.value = ''
    setSaved(false)
    if (token === '') {
      setError('Type the token first.')
      return
    }
    setError(null)
    running.current = true
    setBusy(true)
    try {
      await setToken(client, connection.id, token)
      setSaved(true)
      onSaved()
    } catch (err) {
      setError(messageOf(err))
    } finally {
      running.current = false
      setBusy(false)
    }
  }

  const origin = originOf(connection.url) ?? connection.url
  return (
    <form className="inline-form token-form" method="post" onSubmit={send} autoComplete="off">
      <label className="field">
        <span className="field-label">{connection.has_credential ? 'Replace the token' : 'Token'}</span>
        <input
          ref={field}
          className="text-input mono"
          type="password"
          name="mcp-token"
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          data-1p-ignore=""
          data-lpignore="true"
          data-bwignore=""
          data-form-type="other"
        />
      </label>
      <button type="submit" className="btn btn-primary btn-sm" aria-disabled={busy}>
        Save token
      </button>
      <p className="hint">
        Sent only to <Text className="mono">{origin}</Text>
        {hatName !== undefined && (
          <>
            , for sessions in <Text>{hatName}</Text>
          </>
        )}
        .
      </p>
      {saved && (
        <p className="hint" role="status">
          Token saved. It is never shown again.
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          <Text>{error}</Text>
        </p>
      )}
    </form>
  )
}
