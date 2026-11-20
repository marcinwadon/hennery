// A hook set's JSON: a name (only when creating), the `hooks` object as text,
// and, for the events it names, which agents act on them. The JSON is checked
// for syntax here; the server checks the rest and its reason is shown as sent.
import { useState } from 'react'
import { messageOf } from '../api/errors'
import type { HookEventSupport, HookSetConfig } from '../generated/protocol'
import { Text } from '../lib/text'

interface Props {
  initial?: HookSetConfig
  events: HookEventSupport[]
  onSubmit(name: string, hooks: HookSetConfig): Promise<void>
  busy: boolean
}

function agents(support: HookEventSupport | undefined): string {
  if (!support) return 'Unknown event'
  if (support.claude && support.codex) return 'Claude and Codex'
  if (support.claude) return 'Claude only'
  if (support.codex) return 'Codex only'
  return 'Neither agent'
}

function parse(text: string): { hooks: HookSetConfig } | { error: string } {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch (err) {
    return { error: `The hooks are not valid JSON: ${err instanceof Error ? err.message : 'unreadable'}` }
  }
  // Any other shape goes to the server, whose reason is shown as sent.
  return { hooks: value as HookSetConfig }
}

export default function HookSetEditor({ initial, events, onSubmit, busy }: Props) {
  const replacing = initial !== undefined
  const [name, setName] = useState('')
  const [text, setText] = useState(replacing ? JSON.stringify(initial, null, 2) : '{\n  \n}')
  const [error, setError] = useState<string | null>(null)
  const parsed = parse(text)
  const named =
    'hooks' in parsed && parsed.hooks !== null && typeof parsed.hooks === 'object' && !Array.isArray(parsed.hooks)
      ? Object.keys(parsed.hooks)
      : []

  const submit = async () => {
    if (busy) return
    if ('error' in parsed) return
    if (!replacing && name.trim() === '') return setError('A hook set needs a name.')
    setError(null)
    try {
      await onSubmit(name.trim(), parsed.hooks)
    } catch (err) {
      setError(messageOf(err))
    }
  }

  return (
    <form
      className="hook-set-editor"
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      {!replacing && (
        <label className="field">
          Name
          <input value={name} onChange={(e) => setName(e.target.value)} maxLength={64} autoComplete="off" />
        </label>
      )}
      <label className="field">
        Hooks (JSON)
        <textarea
          className="hook-set-json"
          rows={12}
          spellCheck={false}
          value={text}
          onChange={(e) => setText(e.target.value)}
          aria-invalid={'error' in parsed}
        />
      </label>
      {'error' in parsed && <p className="hint warn">{parsed.error}</p>}
      {named.length > 0 && (
        <table className="hook-set-events">
          <caption>Events in this set</caption>
          <thead>
            <tr><th scope="col">Event</th><th scope="col">Acted on by</th></tr>
          </thead>
          <tbody>
            {named.map((event) => (
              <tr key={event}>
                <th scope="row"><Text>{event}</Text></th>
                <td>{agents(events.find((e) => e.event === event))}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {error && (
        <p className="form-error" role="alert"><Text>{error}</Text></p>
      )}
      <div className="card-actions">
        <button type="submit" className="btn btn-primary btn-sm" aria-disabled={busy}>
          {replacing ? 'Save hooks' : 'Create hook set'}
        </button>
      </div>
    </form>
  )
}
