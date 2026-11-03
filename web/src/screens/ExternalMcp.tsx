import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useClient } from '../app-client'
import { messageOf } from '../api/errors'
import { Link, useLocation } from '../router'

interface ScopeOption { id: string; name: string }
interface Pending {
  request: { request_id: string; client_id: string; client_name: string; redirect_uri: string; scope: string; resource: string; expires_at: number }
  hats: ScopeOption[]
  hosts: ScopeOption[]
}
interface Grant {
  client_id: string; label: string; hat_id: string; roots: { host_id: string; root: string }[]
  read_metadata: boolean; existing_sessions: boolean; expires_at: number; revoked: boolean
  read_transcript?: boolean; control_sessions?: boolean
}
function callbackHost(uri: string): string { try { return new URL(uri).host } catch { return 'Invalid callback' } }

export default function ExternalMcp({ consent = false }: { consent?: boolean }) {
  const client = useClient()
  const { search } = useLocation()
  const requestId = new URLSearchParams(search).get('request') ?? ''
  const [pending, setPending] = useState<Pending | null>(null)
  const [grants, setGrants] = useState<Grant[]>([])
  const [after, setAfter] = useState<string | null>(null)
  const [hat, setHat] = useState('')
  const [host, setHost] = useState('')
  const [root, setRoot] = useState('')
  const [metadata, setMetadata] = useState(false)
  const [transcript, setTranscript] = useState(false)
  const [control, setControl] = useState(false)
  const [existing, setExisting] = useState(false)
  const [days, setDays] = useState(7)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const mutation = useRef<AbortController | null>(null)
  const locked = useRef<number | null>(null)
  const generation = useRef(0)
  useEffect(() => {
    const epoch = ++generation.current
    const abort = new AbortController()
    locked.current = null
    setBusy(false); setPending(null); setGrants([]); setAfter(null); setError(''); setHat(''); setHost(''); setRoot(''); setExisting(false); setMetadata(false); setTranscript(false); setControl(false); setDays(7)
    const operation = consent
      ? client.request<Pending>('GET', '/api/external-oauth/requests/' + encodeURIComponent(requestId), undefined, { signal: abort.signal }).then(p => {
        if (abort.signal.aborted || epoch !== generation.current) return
        setPending(p); setMetadata(p.request.scope.split(' ').includes('hennery:sessions:read'))
      })
      : client.request<Grant[]>('GET', '/api/external-clients', undefined, { signal: abort.signal }).then(rows => { if (!abort.signal.aborted && epoch === generation.current) { setGrants(rows); setAfter(rows.length === 100 ? rows.at(-1)!.client_id : null) } })
    void operation.catch(err => { if (!abort.signal.aborted && epoch === generation.current) setError(messageOf(err)) })
    return () => { abort.abort(); mutation.current?.abort(); generation.current++ }
  }, [client, consent, requestId])

  const decide = async (allow: boolean) => {
    if (locked.current !== null || !pending || pending.request.request_id !== requestId) return
    locked.current = generation.current; setBusy(true); setError('')
    const epoch = generation.current
    const controller = new AbortController(); mutation.current = controller
    try {
      const result = await client.request<{ redirect_uri: string }>('POST', '/api/external-oauth/consent', {
        request_id: requestId, allow, hat_id: hat, roots: [{ host_id: host, root }],
        read_metadata: metadata, existing_sessions: existing && metadata, expires_in_days: days,
        ...(transcript && metadata ? { read_transcript: true } : {}),
        ...(control && metadata ? { control_sessions: true } : {}),
      }, { signal: controller.signal })
      if (!controller.signal.aborted && epoch === generation.current) window.location.assign(result.redirect_uri)
    } catch (err) { if (!controller.signal.aborted && epoch === generation.current) setError(messageOf(err)) }
    finally { if (locked.current === epoch) locked.current = null; if (!controller.signal.aborted && epoch === generation.current) setBusy(false) }
  }
  const revoke = async (id: string) => {
    if (locked.current !== null) return
    locked.current = generation.current; setBusy(true); setError('')
    const epoch = generation.current
    const controller = new AbortController(); mutation.current = controller
    try {
      await client.request('DELETE', '/api/external-clients/' + encodeURIComponent(id), undefined, { signal: controller.signal })
      if (!controller.signal.aborted && epoch === generation.current) setGrants(rows => rows.map(g => g.client_id === id ? { ...g, revoked: true } : g))
    } catch (err) { if (!controller.signal.aborted && epoch === generation.current) setError(messageOf(err)) }
    finally { if (locked.current === epoch) locked.current = null; if (!controller.signal.aborted && epoch === generation.current) setBusy(false) }
  }
  const submit = (event: FormEvent) => { event.preventDefault(); void decide(true) }
  const more = async () => {
    if (locked.current !== null || !after) return
    const epoch = generation.current; locked.current = epoch; setBusy(true); setError('')
    const controller = new AbortController(); mutation.current = controller
    try {
      const rows = await client.request<Grant[]>('GET', '/api/external-clients?after=' + encodeURIComponent(after), undefined, { signal: controller.signal })
      if (!controller.signal.aborted && epoch === generation.current) {
        setGrants(old => [...old, ...rows.filter(r => !old.some(g => g.client_id === r.client_id))])
        setAfter(rows.length === 100 ? rows.at(-1)!.client_id : null)
      }
    } catch (err) { if (!controller.signal.aborted && epoch === generation.current) setError(messageOf(err)) }
    finally { if (locked.current === epoch) locked.current = null; if (!controller.signal.aborted && epoch === generation.current) setBusy(false) }
  }
  return <main className="auth" style={{ alignItems: 'flex-start', overflowY: 'auto', minHeight: 0, flex: 1 }}><section className="auth-card" aria-label="External MCP access" style={{ overflowWrap: 'anywhere' }}>
    <div className="brand-name">hennery</div>
    <h1>{consent ? 'Connect an external MCP client' : 'External MCP clients'}</h1>
    <p>Choose whether this client may inspect sessions, read conversations, or create and control agents within the selected project. Permissions requested by agents still require your approval in Hennery.</p>
    {error && <p role="alert">{error}</p>}
    {consent && !pending && !error && <p role="status">Loading request…</p>}
    {consent && pending && pending.request.request_id === requestId && <>
      <h2><bdi>{pending.request.client_name}</bdi></h2>
      <p>Application identity: <bdi>{pending.request.client_id}</bdi></p>
      <p>Returns authorization to: <strong><bdi>{callbackHost(pending.request.redirect_uri)}</bdi></strong></p>
      <p><bdi>{pending.request.redirect_uri}</bdi></p>
      {pending.request.redirect_uri.startsWith('http:') && <p>Loopback callback: another local process could be using this port. Confirm you started this connection.</p>}
      <p>Requested scopes: <bdi>{pending.request.scope}</bdi></p>
      <p>Only continue if you started this connection and recognize its destination. Application names are supplied by the client.</p>
      <form onSubmit={submit}>
        <label className="field"><span className="field-label">Hat</span><select aria-label="Hat" required disabled={busy} value={hat} onChange={e => setHat(e.target.value)}><option value="">Choose a hat</option>{pending.hats.map(h => <option key={h.id} value={h.id}>{h.name}</option>)}</select></label>
        <label className="field"><span className="field-label">Host</span><select aria-label="Host" required disabled={busy} value={host} onChange={e => setHost(e.target.value)}><option value="">Choose a host</option>{pending.hosts.map(h => <option key={h.id} value={h.id}>{h.name}</option>)}</select></label>
        <label className="field"><span className="field-label">Workspace root</span><input className="text-input" required disabled={busy} value={root} onChange={e => setRoot(e.target.value)} placeholder="/workspace/project" /></label>
        <label className="field"><span className="field-label">Access expires in</span><select aria-label="Access expires in" disabled={busy} value={days} onChange={e => setDays(Number(e.target.value))}>{[1, 7, 30, 90].map(d => <option key={d} value={d}>{d} days</option>)}</select></label>
        <label className="field"><span><input type="checkbox" checked={metadata} disabled={busy || !pending.request.scope.split(' ').includes('hennery:sessions:read')} onChange={e => { setMetadata(e.target.checked); if (!e.target.checked) { setExisting(false); setTranscript(false); setControl(false) } }} /> Read session metadata</span></label>
        <label className="field"><span><input type="checkbox" checked={transcript} disabled={busy || !metadata || !pending.request.scope.split(' ').includes('hennery:sessions:transcript')} onChange={e => setTranscript(e.target.checked)} /> Read session conversations</span></label>
        <label className="field"><span><input type="checkbox" checked={control} disabled={busy || !metadata || !pending.request.scope.split(' ').includes('hennery:sessions:control')} onChange={e => setControl(e.target.checked)} /> Create, prompt and control sessions</span></label>
        {control && <p>Session control includes changing the model and permission mode, answering ordinary forms, sending images, editing queued prompts, and renaming, pinning or cloning sessions. Agent actions follow that mode; permission requests still require approval in Hennery.</p>}
        <label className="field"><span><input type="checkbox" checked={existing} disabled={busy || !metadata} onChange={e => setExisting(e.target.checked)} /> Also allow existing sessions within this hat, host and root</span></label>
        <p>By default, access is limited to sessions created by this client. Including existing sessions also allows the selected read and control actions on those sessions.</p>
        <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => void decide(false)}>Deny</button>
        <button type="submit" className="btn btn-primary" disabled={busy || !hat || !host || !root.startsWith('/')}>Allow selected access</button>
      </form>
    </>}
    {!consent && <ul aria-label="Client grants">{grants.map(g => <li key={g.client_id}>
      <h2><bdi>{g.label}</bdi></h2><p>Hat: <bdi>{g.hat_id}</bdi></p>
      {g.roots.map(r => <p key={r.host_id + r.root}><bdi>{r.host_id}: {r.root}</bdi></p>)}
      <p>{g.existing_sessions ? 'Existing sessions included' : 'Created by this client only'}; {g.read_metadata ? 'metadata enabled' : 'discovery only'}.</p>
      <p>{g.read_transcript ? 'Conversation access enabled' : 'No conversation access'}; {g.control_sessions ? 'session control enabled' : 'no session control'}.</p>
      <p>Expires: {new Date(g.expires_at * 1000).toLocaleString()}</p>
      {g.revoked ? <p>Revoked</p> : <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => void revoke(g.client_id)}>Revoke access to {g.label}</button>}
    </li>)}</ul>}
    {!consent && after && <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => void more()}>Load more clients</button>}
    <p><Link to="/external-mcp">Manage external client grants</Link> · <Link to="/settings">Settings</Link></p>
  </section></main>
}
