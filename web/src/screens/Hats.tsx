import SessionName from '../components/SessionNames'
import { sourcePolicyLabel } from '../components/McpSourcePolicy'
import AgentSourcePolicies, { sourceAgents } from '../components/AgentSourcePolicies'
import { sourcePolicyFor, type SourcePolicyPatch } from '../lib/mcpSourcePolicy'
import { agentLabel } from '../lib/agent'
// Hats (frontend spec §8, kernel spec §5): create, rename and recolour hats,
// pick the default for new hosts, edit each host's path rules with a live
// "this path resolves to" tester, and purge a hat after seeing what goes.
// Every change but creating needs a fresh step-up, which the client asks
// for when the server refuses.
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { messageOf } from '../api/errors'
import { createHat, hats as listHats, hosts as listHosts, pathRules, purgeHat, purgePreview, updateHat } from '../api/manage'
import { useClient } from '../app-client'
import ConfirmDialog from '../components/ConfirmDialog'
import PathRules from '../components/PathRules'
import HatDefaultsSummary, { defaultsAgents } from '../components/HatDefaultsSummary'
import { connections } from '../api/mcp'
import { useMcpScope } from '../components/McpScope'
import type { HatItem, HostItem, PurgePreview, PurgeResult } from '../generated/protocol'
import { useResource } from '../hooks/useResource'
import { count, isDefault, purgeState, safeColour } from '../lib/manage'
import { Text } from '../lib/text'
import { Link } from '../router'
import { useSessionScope } from '../components/SessionScope'
import '../manage.css'

/** The colour a new hat starts with, as the server's own default. */
const NEW_COLOUR = '#64748b'

export default function Hats() {
  const client = useClient()
  const hats = useResource(() => listHats(client))
  const hosts = useResource(() => listHosts(client))
  const sessions = useSessionScope()
  const scope = useMcpScope()
  const localMcp = useResource(() => scope ? Promise.resolve([]) : connections(client), [!!scope])
  const mcp = scope ?? localMcp
  const [purge, setPurge] = useState<{ hat: HatItem; preview: PurgePreview } | null>(null)
  const [purged, setPurged] = useState<{ name: string; result: PurgeResult } | null>(null)
  const [error, setError] = useState<string | null>(null)
  // A purge deletes the hat's path rules on the server: every purge tried,
  // whether it finished or not, has the rules read again.
  const [purges, setPurges] = useState(0)
  const [ruleSaves, setRuleSaves] = useState(0)
  const rules = useResource(async () => Promise.all((hosts.data ?? []).filter(host => !host.revoked_at).map(async host => ({ host, rules: await pathRules(client, host.host_id) }))), [hosts.data, purges, ruleSaves])
  const purgeTried = useRef(false)

  // As functions of the list held, so two changes landing together both
  // stay.
  const replace = (hat: HatItem) =>
    hats.set((prev) =>
      (prev ?? []).map((h) => {
        if (h.id === hat.id) return hat
        // Only one hat is the default for new hosts.
        return hat.default_for_new_hosts ? { ...h, default_for_new_hosts: false } : h
      }),
    )

  const askPurge = async (hat: HatItem) => {
    setError(null)
    try {
      setPurge({ hat, preview: await purgePreview(client, hat.id) })
    } catch (err) {
      setError(messageOf(err))
    }
  }

  const serverSummary = (hatId: string) => {
    const hostRows = hosts.data, connectionRows = mcp.data
    if (!hostRows || !connectionRows) return null
    return connectionRows.filter(c => c.hat_id === hatId).map(c => {
      const mounted = hostRows.filter(h => !h.revoked_at && c.mounts.includes(h.host_id)).map(h => h.name)
      return `${c.label} — ${mounted.length ? `on ${mounted.join(', ')}` : 'not mounted on a paired host'}`
    })
  }

  return (
    <div className="manage hats-screen">
      <header className="manage-head">
        <h1>Hats</h1>
        <button className="btn btn-secondary btn-sm" onClick={() => { const panel = document.getElementById('new-hat-panel') as HTMLDetailsElement | null; if (panel) { panel.open = true; panel.querySelector('input')?.focus() } }}>+ New hat</button>
      </header>
      <p className="manage-subtitle">Path rules choose a hat. Each hat has its own servers, session defaults and notification privacy.</p>
      {[hats.error, hosts.error, mcp.error, rules.error, error].map(
        (shown, i) =>
          shown && (
            <p key={i} className="form-error" role="alert">
              <Text>{shown}</Text>
            </p>
          ),
      )}
      {purged && (
        <PurgeOutcome name={purged.name} result={purged.result} onClose={() => setPurged(null)} />
      )}
      <ul className="cards hat-grid" aria-label="Hats">
        {(hats.data ?? []).map((hat) => (
          <HatCard
            key={hat.id}
            hat={hat}
            isDefault={isDefault(hat, hosts.data ?? [])}
            defaultOn={hosts.data ? hosts.data.filter(h => !h.revoked_at && h.default_hat_id === hat.id).map(h => h.name) : null}
            servers={serverSummary(hat.id)}
            rules={rules.data ? rules.data.flatMap(row => row.rules.filter(rule => rule.hat_id === hat.id).map(rule => `${rule.prefix} · ${row.host.name}`)) : null}
            sessions={sessions ? `${sessions.list.shown.filter(row => row.hat_id === hat.id).length} shown` : '—'}
            agents={defaultsAgents(hosts.data)}
            hosts={hosts.data}
            onChanged={replace}
            onPurge={() => askPurge(hat)}
          />
        ))}
      </ul>
      <details id="new-hat-panel" className="hat-create"><summary>New hat</summary><NewHat hosts={hosts.data} onCreated={(hat) => hats.set((prev) => [...(prev ?? []), hat])} /></details>
      <details className="hat-rules-editor"><summary>Manage path rules</summary>
      <PathRules
        hosts={(hosts.data ?? []).filter((h) => h.revoked_at === undefined)}
        hats={hats.data ?? []}
        purges={purges}
        onSaved={() => setRuleSaves(value => value + 1)}
      />
      </details>
      {purge && (
        <PurgeDialog
          hat={purge.hat}
          preview={purge.preview}
          hosts={hosts.data ?? []}
          onTried={() => (purgeTried.current = true)}
          onPurged={(result) => setPurged({ name: purge.hat.name, result })}
          onClose={() => {
            setPurge(null)
            // A purge that failed may have frozen the hat ("Resume purge")
            // and deleted its rules already: both are read again.
            if (purgeTried.current) {
              purgeTried.current = false
              hats.reload()
              setPurges((n) => n + 1)
            }
          }}
        />
      )}
    </div>
  )
}

function Swatch({ colour }: { colour: string }) {
  const safe = safeColour(colour)
  return <span className="swatch" aria-hidden="true" style={safe ? { background: safe } : undefined} />
}

function HatCard({
  hat,
  isDefault,
  defaultOn,
  servers,
  agents,
  hosts,
  rules,
  sessions,
  onChanged,
  onPurge,
}: {
  hat: HatItem
  isDefault: boolean
  defaultOn: string[] | null
  servers: string[] | null
  agents: string[]
  hosts: HostItem[] | null
  rules: string[] | null
  sessions: string
  onChanged: (hat: HatItem) => void
  onPurge: () => void
}) {
  const client = useClient()
  const [editing, setEditing] = useState(false)
  const [sourcePatch, setSourcePatch] = useState<SourcePolicyPatch>({})
  const [name, setName] = useState(hat.name)
  const [colour, setColour] = useState(safeColour(hat.colour) ?? NEW_COLOUR)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const titleId = `hat-${hat.id}`
  // A closed form or a done "Make default for new hosts" takes the focused
  // control away: focus goes to "Edit", or to the card's title.
  const title = useRef<HTMLHeadingElement>(null)
  const editButton = useRef<HTMLButtonElement>(null)
  const refocus = useRef<'edit' | 'title' | null>(null)
  useEffect(() => {
    if (refocus.current === null) return
    const to = refocus.current === 'edit' ? editButton.current : title.current
    refocus.current = null
    to?.focus()
  })

  const closeForm = () => {
    refocus.current = 'edit'
    setEditing(false)
  }

  const change = async (body: Parameters<typeof updateHat>[2], then: 'edit' | 'title') => {
    setBusy(true)
    setError(null)
    try {
      onChanged(await updateHat(client, hat.id, body))
      refocus.current = then
      setEditing(false)
    } catch (err) {
      setError(messageOf(err))
    } finally {
      setBusy(false)
    }
  }

  const save = (e: FormEvent) => {
    e.preventDefault()
    const body: Parameters<typeof updateHat>[2] = {}
    if (name.trim() !== hat.name) body.name = name.trim()
    if (colour !== hat.colour) body.colour = colour
    const sourceChanges = Object.fromEntries(Object.entries(sourcePatch).filter(([agent, value]) => value !== (hat.mcp_source_overrides?.[agent]?.policy ?? null)))
    if (Object.keys(sourceChanges).length) body.mcp_source_overrides = sourceChanges
    if (Object.keys(body).length === 0) closeForm()
    else change(body, 'edit')
  }

  return (
    <li className="card hat" aria-labelledby={titleId}>
      <div className="card-head">
        <Swatch colour={hat.colour} />
        <h2 className="card-title" id={titleId} ref={title} tabIndex={-1}>
          <Text>{hat.name}</Text>
        </h2>
        <span className="hat-session-count">{sessions}</span>
        {hat.default_for_new_hosts && <span className="tag">Default for new hosts</span>}
        {hat.purging && <span className="tag tag-warn">Being purged</span>}
      </div>
      <dl className="facts hat-summary">
        <dt>Path rules</dt><dd>{rules === null ? 'Not loaded' : rules.length ? rules.map(rule => <div key={rule}><Text className="mono">{rule}</Text></div>) : 'No path rules'}</dd>
        <dt>Default on</dt><dd>{defaultOn === null ? 'Not loaded' : defaultOn.length ? <Text>{defaultOn.join(', ')}</Text> : 'No hosts'}</dd>
        <dt>Gateway servers</dt><dd>{servers === null ? 'Not loaded' : servers.length ? <Text>{servers.join(', ')}</Text> : 'None'}</dd>
      </dl>
      {sourceAgents(hat, hosts).map(agent => <p key={agent}><Text>{agentLabel(agent)}</Text> MCP sources: {sourcePolicyLabel(sourcePolicyFor(hat, agent).policy)}</p>)}
      <HatDefaultsSummary hatId={hat.id} agents={agents} compact />
      {editing ? (
        <form className="inline-form" onSubmit={save}>
          <label className="field">
            <span className="field-label">Name</span>
            <input className="text-input" value={name} maxLength={64} required onChange={(e) => setName(e.target.value)} />
          </label>
          <label className="field">
            <span className="field-label">Colour</span>
            <input type="color" className="colour-input" value={colour} onChange={(e) => setColour(e.target.value.toLowerCase())} />
          </label>
          <AgentSourcePolicies hat={hat} hosts={hosts} patch={sourcePatch} onChange={setSourcePatch} />
          <button type="submit" className="btn btn-primary btn-sm" disabled={busy || name.trim() === ''}>
            Save
          </button>
          <button type="button" className="btn btn-ghost btn-sm" onClick={closeForm} disabled={busy}>
            Cancel
          </button>
        </form>
      ) : (
        <div className="card-actions">
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            ref={editButton}
            onClick={() => {
              setSourcePatch({})
              setName(hat.name)
              setColour(safeColour(hat.colour) ?? NEW_COLOUR)
              setEditing(true)
            }}
            disabled={hat.purging}
          >
            Edit
          </button>
          {!hat.default_for_new_hosts && !hat.purging && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              onClick={() => change({ default_for_new_hosts: true }, 'title')}
              disabled={busy}
            >
              Make default for new hosts
            </button>
          )}
          <span className="spacer" />
          <button type="button" className="btn btn-danger btn-sm" onClick={onPurge} disabled={isDefault && !hat.purging}>
            {hat.purging ? 'Resume purge' : 'Purge'}
          </button>
        </div>
      )}
      {isDefault && !hat.purging && (
        <p className="hint">A default hat cannot be purged: make another hat the default first.</p>
      )}
      {error && (
        <p className="form-error" role="alert">
          <Text>{error}</Text>
        </p>
      )}
    </li>
  )
}

function NewHat({ onCreated, hosts }: { onCreated: (hat: HatItem) => void; hosts: HostItem[] | null }) {
  const client = useClient()
  const [name, setName] = useState('')
  const [sourcePatch, setSourcePatch] = useState<SourcePolicyPatch>({ claude: 'managed_only' })
  const [colour, setColour] = useState(NEW_COLOUR)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const nameInput = useRef<HTMLInputElement>(null)

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      onCreated(await createHat(client, { name: name.trim(), colour, mcp_source_overrides: sourcePatch }))
      setName('')
      setSourcePatch({ claude: 'managed_only' })
      // "Create" is disabled with the name empty: focus goes to the name,
      // ready for the next hat.
      nameInput.current?.focus()
    } catch (err) {
      setError(messageOf(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="card" aria-labelledby="new-hat-title">
      <h2 className="card-title" id="new-hat-title">
        New hat
      </h2>
      <form className="inline-form" onSubmit={submit}>
        <AgentSourcePolicies hosts={hosts} patch={sourcePatch} onChange={setSourcePatch} />
        <label className="field">
          <span className="field-label">Name</span>
          <input
            ref={nameInput}
            className="text-input"
            value={name}
            maxLength={64}
            required
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        <label className="field">
          <span className="field-label">Colour</span>
          <input type="color" className="colour-input" value={colour} onChange={(e) => setColour(e.target.value.toLowerCase())} />
        </label>
        <button type="submit" className="btn btn-primary btn-sm" disabled={busy || name.trim() === ''}>
          Create
        </button>
      </form>
      {error && (
        <p className="form-error" role="alert">
          <Text>{error}</Text>
        </p>
      )}
    </section>
  )
}

function PurgeDialog({
  hat,
  preview,
  hosts,
  onTried,
  onPurged,
  onClose,
}: {
  hat: HatItem
  preview: PurgePreview
  hosts: HostItem[]
  onTried: () => void
  onPurged: (result: PurgeResult) => void
  onClose: () => void
}) {
  const client = useClient()
  const state = purgeState(hat, hosts, preview)
  const blocked = state === 'default' || state === 'running'
  return (
    <ConfirmDialog
      title={state === 'resume' ? 'Resume this hat’s purge?' : 'Purge this hat?'}
      confirm={state === 'resume' ? 'Resume purge' : 'Purge'}
      danger
      disabled={blocked}
      action={async () => {
        onTried()
        onPurged(await purgeHat(client, hat.id))
      }}
      onClose={onClose}
    >
      <p>
        Purging <Text>{hat.name}</Text> deletes, for good:
      </p>
      <ul className="purge-list">
        <li>{count(preview.sessions, 'session')} of the hat, on every host, with their transcripts</li>
        <li>{count(preview.rules, 'path rule')}</li>
        <li>{count(preview.recents, 'recent project')}</li>
        <li>the hat itself, with its push policy</li>
      </ul>
      {state === 'default' && (
        <p className="form-error">
          This hat is the default for new hosts, or a host’s default hat. Make another hat that default first.
        </p>
      )}
      {state === 'running' && (
        <>
          <p className="form-error">Close these sessions first: they are running on a host hennery reaches.</p>
          <SessionIds ids={preview.running} link />
        </>
      )}
      {state === 'resume' && <p>A purge of this hat began and did not finish; this finishes it.</p>}
      {preview.unassigned_count > 0 && (
        <>
          <p>
            {count(preview.unassigned_count, 'session')} belong to no hat. No purge deletes them; re-assign or delete
            them one by one:
          </p>
          <ul className="purge-list">
            {preview.unassigned.map((s) => (
              <li key={s.session_id}>
                <Link to={`/sessions/${encodeURIComponent(s.session_id)}`}>
                  <SessionName session={s} fallback={s.cwd} />
                </Link>
              </li>
            ))}
          </ul>
        </>
      )}
    </ConfirmDialog>
  )
}

/** Session ids, as links to the sessions when they still exist. */
function SessionIds({ ids, link }: { ids: string[]; link?: boolean }) {
  return (
    <ul className="purge-list mono">
      {ids.map((id) => (
        <li key={id}>
          {link ? (
            <Link to={`/sessions/${encodeURIComponent(id)}`}>
              <Text>{id}</Text>
            </Link>
          ) : (
            <Text>{id}</Text>
          )}
        </li>
      ))}
    </ul>
  )
}

function PurgeOutcome({ name, result, onClose }: { name: string; result: PurgeResult; onClose: () => void }) {
  const t = result.host_transcripts
  // The purged hat's card, and the button that opened the dialog, are
  // gone: focus comes here.
  const title = useRef<HTMLHeadingElement>(null)
  useEffect(() => title.current?.focus(), [])
  return (
    <section className="card notice" aria-labelledby="purged-title" role="status">
      <h2 className="card-title" id="purged-title" ref={title} tabIndex={-1}>
        Purged <Text>{name}</Text>
      </h2>
      <p>
        Deleted {count(result.sessions, 'session')} and {count(result.rules, 'path rule')}.
      </p>
      {result.unconfirmed.length > 0 && (
        <>
          <p>
            {count(result.unconfirmed.length, 'session')} were deleted while their host was away; the host closes them
            when it is back:
          </p>
          <SessionIds ids={result.unconfirmed} />
        </>
      )}
      <p>
        The agents’ own transcripts on the hosts: {t.removed} removed, {t.partial} removed in part, {t.pending} still to
        remove when their host is back.
      </p>
      <div className="card-actions">
        <button type="button" className="btn btn-ghost btn-sm" onClick={onClose}>
          Close
        </button>
      </div>
    </section>
  )
}
