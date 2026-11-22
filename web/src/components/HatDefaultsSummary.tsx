import { Fragment, useEffect, useId, useMemo, useState } from 'react'
import { sessionDefaults } from '../api/defaults'
import { messageOf } from '../api/errors'
import { hosts as listHosts } from '../api/manage'
import { useClient } from '../app-client'
import type { HatItem, HatSessionDefaults, HostItem } from '../generated/protocol'
import { useResource } from '../hooks/useResource'
import { agentLabel } from '../lib/agent'
import { flatConfig, valueLabel } from '../lib/defaults'
import { Text, visible } from '../lib/text'
import { Link } from '../router'

export function defaultsAgents(hosts: HostItem[] | null): string[] {
  return [...new Set((hosts ?? []).filter(host => host.revoked_at == null).flatMap(host => Object.keys(host.mcp_delivery ?? {})))]
}

/** Stored overrides, without implying that a host supports them or probing one. */
export default function HatDefaultsSummary({ hatId, agents, compact = false }: { hatId: string; agents: string[]; compact?: boolean }) {
  const client = useClient(), id = useId()
  const [selected, setSelected] = useState<string | null>(null), [other, setOther] = useState(''), [refresh, setRefresh] = useState(0)
  const agent = selected ?? agents[0] ?? ''
  const request = useMemo(() => ({}), [client, hatId, agent, refresh])
  const [result, setResult] = useState<{ request: object; record?: HatSessionDefaults; error?: string } | null>(null)
  useEffect(() => {
    if (!agent) return
    let live = true
    const abort = new AbortController()
    sessionDefaults(client, hatId, agent, abort.signal).then(
      record => { if (live) setResult({ request, record }) },
      err => { if (live) setResult({ request, error: messageOf(err) }) },
    )
    return () => { live = false; abort.abort() }
  }, [client, hatId, agent, request])
  const current = result?.request === request ? result : null
  const entries = current?.record ? Object.entries(flatConfig(current.record.config)) : null
  const options = [...new Set([...agents, ...(agent ? [agent] : [])])]
  return <div className={`hat-defaults-summary${compact ? ' compact-defaults' : ''}`}>
    <details className="defaults-agent-picker" open={!compact || undefined}><summary>Choose defaults agent</summary>
    <label className="field"><span className="field-label">Defaults agent</span><select value={agent} onChange={event => setSelected(event.target.value)}>
      {!agent && <option value="">Choose an agent</option>}{options.map(value => <option key={value} value={value}>{visible(agentLabel(value))}</option>)}
    </select></label>
    <details><summary>Other configured agent</summary><form className="inline-form" onSubmit={event => { event.preventDefault(); if (other.trim()) setSelected(other.trim()) }}>
      <label className="field" htmlFor={id}>Configured agent name<input id={id} className="text-input" value={other} onChange={event => setOther(event.target.value)} /></label>
      <button className="btn btn-secondary btn-sm" disabled={!other.trim()}>Load saved defaults</button>
    </form></details>
    </details>
    {!agent && <p className="hint">Choose a configured agent to inspect its saved overrides.</p>}
    {agent && !current && <p role="status">Loading saved defaults…</p>}
    {current?.error && <p role="alert">Unable to load saved defaults. <Text>{current.error}</Text> <button type="button" className="btn btn-ghost btn-sm" onClick={() => setRefresh(value => value + 1)}>Retry saved defaults</button></p>}
    {entries && (entries.length ? <dl className="facts">{entries.map(([key, value]) => <Fragment key={key}><dt><Text>{key === 'protocol' ? 'ACP protocol' : key === 'model' ? 'Model' : key === 'mode' ? 'Permission mode' : key}</Text></dt><dd><Text>{valueLabel(null, key, value)}</Text></dd></Fragment>)}</dl> : <p>No saved overrides for this agent.</p>)}
    <p className="hint">Saved overrides only. Availability and inherited values depend on the host; preview them in Defaults.</p>
    <Link className="defaults-link" to={`/defaults?hat=${encodeURIComponent(hatId)}${agent ? `&agent=${encodeURIComponent(agent)}` : ''}`}>Session defaults <span aria-hidden="true">→</span></Link>
  </div>
}

export function DefaultsSettings({ hats }: { hats: HatItem[] | null }) {
  const client = useClient(), hosts = useResource(() => listHosts(client), [client])
  const [selected, setSelected] = useState('')
  const hat = hats?.find(item => item.id === selected) ?? hats?.[0]
  return <section className="card" aria-labelledby="defaults-settings-heading">
    <h2 id="defaults-settings-heading">Session defaults</h2>
    {!hats && <p role="status">Loading hats…</p>}
    {hats?.length === 0 && <p>No hats available.</p>}
    {hosts.error && <p role="alert">Unable to load configured agents. <button className="btn btn-ghost" onClick={hosts.reload}>Retry configured agents</button></p>}
    {hat && <><label className="field"><span className="field-label">Defaults hat</span><select value={hat.id} onChange={event => setSelected(event.target.value)}>{hats?.map(item => <option key={item.id} value={item.id}>{visible(item.name)}</option>)}</select></label>
      <HatDefaultsSummary key={hat.id} hatId={hat.id} agents={defaultsAgents(hosts.data)} /></>}
  </section>
}
