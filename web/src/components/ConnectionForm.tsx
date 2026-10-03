// The form a connection is created or edited in (frontend spec §8, gateway
// spec §2). The slug and the hat are asked for only on create: neither can
// change afterwards. The token is not part of it: it is set on its own,
// write-only (TokenForm).
import { useId, useState, type FormEvent } from 'react'
import type { HatItem } from '../generated/protocol'
import { kindLabel, serverName, toolList, type ConnectionForm as Form } from '../lib/mcp'
import { Text, visible } from '../lib/text'

interface Props {
  initial: Form
  /** On create: the hats to pick from, and the one picked first. */
  create?: { hats: HatItem[]; hatId: string }
  submit: string
  busy: boolean
  error: string | null
  onSubmit: (form: Form, slug: string, hatId: string) => void
  onCancel: () => void
}

export default function ConnectionForm({ initial, create, submit, busy, error, onSubmit, onCancel }: Props) {
  const [form, setForm] = useState(initial)
  const [slug, setSlug] = useState('')
  const [hatId, setHatId] = useState(create?.hatId ?? '')
  const ids = useId()
  const set = <K extends keyof Form>(key: K, value: Form[K]) => setForm((f) => ({ ...f, [key]: value }))
  const noTools = !form.everyTool && toolList(form.tools).length === 0

  // While a request runs the buttons say so but stay focusable: a disabled
  // button that has focus drops it to the page's body (and the step-up
  // dialog returns focus to it). The screen ignores a second send, and
  // Cancel waits for the answer: a request already sent still lands.
  const send = (e: FormEvent) => {
    e.preventDefault()
    onSubmit(form, slug, hatId)
  }

  return (
    <form className="connection-form" onSubmit={send}>
      <label className="field">
        <span className="field-label">Name</span>
        {/* The form opens with focus here: what opened it is disabled or
            gone. */}
        <input
          className="text-input"
          value={form.label}
          maxLength={64}
          autoFocus
          onChange={(e) => set('label', e.target.value)}
        />
      </label>
      {create && (
        <>
          <label className="field">
            <span className="field-label">Slug</span>
            <input
              className="text-input mono"
              value={slug}
              maxLength={48}
              autoCapitalize="off"
              spellCheck={false}
              aria-describedby={`${ids}-slug`}
              onChange={(e) => setSlug(e.target.value)}
            />
          </label>
          <p className="hint" id={`${ids}-slug`}>
            Agents see it as <code>{visible(serverName(slug.trim() || '<slug>'))}</code>. Lower-case letters, digits and
            hyphens; it cannot change later.
          </p>
        </>
      )}
      <label className="field">
        <span className="field-label">Server URL</span>
        <input
          className="text-input mono"
          value={form.url}
          inputMode="url"
          autoCapitalize="off"
          spellCheck={false}
          placeholder="https://mcp.example.com/mcp"
          onChange={(e) => set('url', e.target.value)}
        />
      </label>
      {create && (
        <>
          <label className="select-field">
            <span className="field-label">Hat</span>
            <select value={hatId} aria-describedby={`${ids}-hat`} onChange={(e) => setHatId(e.target.value)}>
              {create.hats.map((h) => (
                <option key={h.id} value={h.id}>
                  {visible(h.name)}
                </option>
              ))}
            </select>
          </label>
          <p className="hint" id={`${ids}-hat`}>
            Only sessions in this hat get it, and it stays in this hat.
          </p>
        </>
      )}
      <label className="select-field">
        <span className="field-label">Authentication</span>
        <select value={form.credKind} onChange={(e) => set('credKind', e.target.value as Form['credKind'])}>
          <option value="none">None</option>
          <option value="static">Token</option>
          {form.credKind !== 'none' && form.credKind !== 'static' && (
            <option value={form.credKind} disabled>
              {kindLabel(form.credKind)}
            </option>
          )}
        </select>
      </label>
      {form.credKind === 'static' && (
        <>
          <div className="field-row">
            <label className="field">
              <span className="field-label">Header</span>
              <input
                className="text-input mono"
                value={form.header}
                maxLength={64}
                autoCapitalize="off"
                spellCheck={false}
                onChange={(e) => set('header', e.target.value)}
              />
            </label>
            <label className="field">
              <span className="field-label">Prefix</span>
              <input
                className="text-input mono"
                value={form.prefix}
                maxLength={32}
                autoCapitalize="off"
                spellCheck={false}
                onChange={(e) => set('prefix', e.target.value)}
              />
            </label>
          </div>
          <p className="hint">
            The token is sent as <code>{visible(form.header.trim() || 'Header')}: {visible(form.prefix)}&lt;token&gt;</code>.
            The prefix may be empty; mind its trailing space. You set the token after saving.
          </p>
        </>
      )}
      <label className="check">
        <input type="checkbox" checked={form.everyTool} onChange={(e) => set('everyTool', e.target.checked)} />
        <span>Every tool</span>
      </label>
      {!form.everyTool && (
        <>
          <label className="field">
            <span className="field-label">Allowed tools</span>
            <textarea
              className="text-input mono"
              rows={3}
              value={form.tools}
              spellCheck={false}
              onChange={(e) => set('tools', e.target.value)}
            />
          </label>
          <p className="hint">One tool name per line, or separated by commas.</p>
          {noTools && <p className="hint warn">No tool is allowed: agents see this server with none.</p>}
        </>
      )}
      <label className="check">
        <input
          type="checkbox"
          checked={form.internalNetwork}
          onChange={(e) => set('internalNetwork', e.target.checked)}
        />
        <span>Internal network</span>
      </label>
      {form.internalNetwork && (
        <p className="hint warn">
          The gateway may then reach addresses on your own network for this connection, and plain http to them.
          Turn it on only for a server you run there.
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          <Text>{error}</Text>
        </p>
      )}
      <div className="card-actions">
        <button type="submit" className="btn btn-primary btn-sm" aria-disabled={busy}>
          {submit}
        </button>
        <button type="button" className="btn btn-ghost btn-sm" onClick={() => busy || onCancel()} aria-disabled={busy}>
          Cancel
        </button>
      </div>
    </form>
  )
}
