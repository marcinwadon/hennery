import { createContext, useContext, useEffect, useRef, useState } from 'react'
import { useClient } from '../app-client'
import { ApiFailure } from '../api/errors'
import type { Client } from '../api/client'
import type { ArtifactPage, ArtifactRevision, PublishArtifact, StoredBlock } from '../generated/protocol'
import { attachmentHref } from './items/UserTurn'
import Markdown from './Markdown'

export const ArtifactVersions = createContext<ReadonlyMap<string, number>>(new Map())
export function artifactHref(a: ArtifactRevision): string {
  return `/api/sessions/${encodeURIComponent(a.session_id)}/artifacts/${encodeURIComponent(a.artifact_id)}/${a.revision}`
}
function formatSize(bytes: number) { return bytes < 1024 ? `${bytes} B` : bytes < 1048576 ? `${(bytes / 1024).toFixed(1)} KiB` : `${(bytes / 1048576).toFixed(1)} MiB` }
function message(e: unknown) { return e instanceof Error ? e.message : 'Could not read this file.' }

export function ArtifactCard({ artifact }: { artifact: ArtifactRevision }) {
  const latest = useContext(ArtifactVersions).get(artifact.artifact_id)
  const [preview, setPreview] = useState(false)
  return <div className="artifact-card">
    <strong><bdi>{artifact.title}</bdi></strong>
    <span className="item-note"><bdi>{artifact.name}</bdi> · Version {artifact.revision} · {formatSize(artifact.size)}</span>
    <a className="file-download" href={artifactHref(artifact)} download={artifact.name}>Download version {artifact.revision}</a>
    {latest !== undefined && latest > artifact.revision && <span className="item-note">Version {latest} is available in Files.</span>}
    {['text/markdown', 'text/plain', 'text/csv', 'application/json'].includes(artifact.mime_type) && artifact.size <= 262144 && <button type="button" className="btn btn-ghost btn-sm" aria-expanded={preview} onClick={() => setPreview(v => !v)}>{preview ? 'Hide preview' : 'Preview'}</button>}
    {preview && <ArtifactPreview key={`${artifact.artifact_id}:${artifact.revision}`} artifact={artifact} />}
  </div>
}
function ArtifactPreview({ artifact }: { artifact: ArtifactRevision }) {
  const client = useClient()
  const [text, setText] = useState<string | null>(null)
  const [error, setError] = useState('')
  useEffect(() => {
    const controller = new AbortController()
    void (async () => {
      try {
        const response = await client.open(artifactHref(artifact), { Accept: '*/*' }, controller.signal)
        if (!response.ok) { await response.body?.cancel(); throw new Error('Could not read this revision.') }
        const value = await response.text()
        if (!controller.signal.aborted) setText(value)
      } catch (e) { if (!controller.signal.aborted) setError(message(e)) }
    })()
    return () => controller.abort()
  }, [client, artifact])
  return error ? <p role="alert">{error}</p> : text === null ? <p className="item-note">Loading preview…</p> : <div className="artifact-preview">{artifact.mime_type === 'text/markdown' ? <div className="bubble"><Markdown>{text}</Markdown></div> : <pre>{text}</pre>}</div>
}
interface Files extends ArtifactPage { attachments: StoredBlock[]; attachments_truncated: boolean }
interface History { revisions: ArtifactRevision[]; next_before: number | null }
async function readFiles(client: Client, base: string, pages: number, signal?: AbortSignal): Promise<Files> {
  let result = await client.request<Files>('GET', `${base}/files`, undefined, { signal })
  for (let page = 1; page < pages && result.next_cursor; page++) {
    const next = await client.request<Files>('GET', `${base}/files?cursor=${encodeURIComponent(result.next_cursor)}`, undefined, { signal })
    result = { ...result, artifacts: [...result.artifacts, ...next.artifacts], next_cursor: next.next_cursor }
  }
  return result
}

export default function SessionFiles({ sessionId, active = true }: { sessionId: string; active?: boolean }) {
  const client = useClient()
  const [files, setFiles] = useState<Files | null>(null)
  const [error, setError] = useState('')
  const [path, setPath] = useState('')
  const [update, setUpdate] = useState('')
  const [busy, setBusy] = useState(false)
  const [history, setHistory] = useState<string | null>(null)
  const [versions, setVersions] = useState<History | null>(null)
  const [retry, setRetry] = useState<PublishArtifact | null>(null)
  const alive = useRef(true)
  const historyRequest = useRef(0)
  const filePages = useRef(1)
  const fileRequest = useRef(0)
  const base = `/api/sessions/${encodeURIComponent(sessionId)}`
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  useEffect(() => {
    if (!active) return
    const abort = new AbortController()
    let loading = false
    async function load() {
      if (loading) return
      loading = true
      const request = ++fileRequest.current
      try {
        const result = await readFiles(client, base, filePages.current, abort.signal)
        if (!abort.signal.aborted && request === fileRequest.current) setFiles(result)
      } catch (e) { if (!abort.signal.aborted && request === fileRequest.current) setError(message(e)) }
      finally { loading = false }
    }
    void load()
    const timer = setInterval(() => void load(), 5000)
    return () => { abort.abort(); clearInterval(timer) }
  }, [client, base, active])
  async function older() {
    if (!files?.next_cursor || busy) return
    setBusy(true)
    ++fileRequest.current
    try { const next = await client.request<Files>('GET', `${base}/files?cursor=${encodeURIComponent(files.next_cursor)}`); if (alive.current) { filePages.current++; setFiles(current => current && ({ ...current, artifacts: [...current.artifacts, ...next.artifacts.filter(a => !current.artifacts.some(r => r.artifact_id === a.artifact_id))], next_cursor: next.next_cursor })) } }
    catch (e) { if (alive.current) setError(message(e)) }
    finally { if (alive.current) setBusy(false) }
  }
  async function showHistory(artifact: string, before?: number) {
    const request = ++historyRequest.current
    if (before === undefined && history === artifact) { setHistory(null); return }
    setHistory(artifact)
    if (before === undefined) setVersions(null)
    try { const next = await client.request<History>('GET', `${base}/artifacts/${encodeURIComponent(artifact)}${before ? `?before=${before}` : ''}`); if (alive.current && request === historyRequest.current) setVersions(current => ({ ...next, revisions: before ? [...(current?.revisions ?? []), ...next.revisions] : next.revisions })) }
    catch (e) { if (alive.current && request === historyRequest.current) setError(message(e)) }
  }
  async function publish() {
    if (busy) return
    const previous = files?.artifacts.find(a => a.artifact_id === update)
    const q: PublishArtifact = retry ?? { path, request_key: crypto.randomUUID(), title: previous?.title ?? null, mime_type: null, artifact_id: previous?.artifact_id ?? null, expected_revision: previous?.revision ?? null }
    setBusy(true); setError(''); setRetry(q)
    ++fileRequest.current
    try {
      await client.request('POST', `${base}/files`, q)
      if (!alive.current) return
      setRetry(null); setPath(''); setUpdate(''); setHistory(null)
      const request = ++fileRequest.current
      const result = await readFiles(client, base, filePages.current)
      if (alive.current && request === fileRequest.current) setFiles(result)
    } catch (e) {
      if (!alive.current) return
      setError(message(e))
      if (e instanceof ApiFailure && e.status >= 400 && e.status < 500) setRetry(null)
    } finally { if (alive.current) setBusy(false) }
  }
  return <section className="session-files" aria-label="Session files">
    <h4>Artifacts</h4>
    {!files ? <p className="item-note">Loading files…</p> : files.artifacts.length === 0 ? <p className="item-note">No files have been published.</p> : files.artifacts.map(a => <div key={a.artifact_id}>
      <ArtifactCard artifact={a} />
      <button type="button" className="btn btn-ghost btn-sm" onClick={() => void showHistory(a.artifact_id)} aria-expanded={history === a.artifact_id}>Version history</button>
      {history === a.artifact_id && (!versions ? <p className="item-note">Loading history…</p> : <><ol className="artifact-history">{versions.revisions.map(r => <li key={r.revision}><a href={artifactHref(r)} download={r.name}>Version {r.revision}</a> · <bdi>{r.name}</bdi> · <time dateTime={r.created_at}>{new Date(r.created_at).toLocaleString()}</time></li>)}</ol>{versions.next_before !== null && <button type="button" className="btn btn-ghost btn-sm" onClick={() => void showHistory(a.artifact_id, versions.next_before!)}>Older versions</button>}</>)}
    </div>)}
    {files?.next_cursor && <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => void older()}>More artifacts</button>}
    <h4>Attachments</h4>
    {files && files.attachments.length === 0 && <p className="item-note">No attachments have been sent.</p>}
    <ul className="session-input-files">{files?.attachments.map((a, i) => a.type === 'file' || a.type === 'image' ? <li key={i}><a href={attachmentHref(a.sha256) ?? undefined} download={a.type === 'file' ? a.name : 'image'}><bdi>{a.type === 'file' ? a.name : 'Image'}</bdi> · {formatSize(a.size)}</a></li> : null)}</ul>
    {files?.attachments_truncated && <p className="item-note">Showing 500 attachments. Earlier files remain available in the transcript.</p>}
    <form onSubmit={e => { e.preventDefault(); void publish() }}>
      <h4>Add a file from the host</h4>
      <label>File path<input value={path} disabled={busy || retry !== null} onChange={e => setPath(e.target.value)} placeholder="reports/analysis.md" required /></label>
      <label>Publish as<select value={update} disabled={busy || retry !== null} onChange={e => setUpdate(e.target.value)}><option value="">New artifact</option>{files?.artifacts.map(a => <option key={a.artifact_id} value={a.artifact_id}>Update {a.title} (v{a.revision})</option>)}</select></label>
      <p className="item-note">A regular file under this session’s working directory, up to 16 MiB. Published copies remain available when the host is offline.</p>
      <button type="submit" className="btn btn-primary btn-sm" disabled={busy || !path.trim()}>{busy ? 'Publishing…' : retry ? 'Retry publication' : update ? 'Publish new version' : 'Publish file'}</button>
      {retry && !busy && <button type="button" className="btn btn-ghost btn-sm" onClick={() => setRetry(null)}>Start a different publication</button>}
    </form>
    {error && <p role="alert" className="form-error">{error}</p>}
  </section>
}
