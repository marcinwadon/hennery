import { useEffect, useState, useRef } from 'react'
import { useClient } from '../app-client'
export interface WorkspaceOptions { base: string; files: string[]; command?: string }
export default function WorkspaceOptionsEditor({ value, onChange, source }: { value: WorkspaceOptions | null; onChange: (value: WorkspaceOptions | null) => void; source?: { host_id?: string; cwd?: string; session_id?: string } }) {
  const current = useRef(value)
  current.current = value
  const client = useClient()
  const [preview, setPreview] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const key = source ? new URLSearchParams(Object.entries(source).filter((entry): entry is [string, string] => typeof entry[1] === 'string')).toString() : ''
  useEffect(() => {
    if (!value || !key) return
    let live = true
    void client.request<{ files: string[]; command?: string }>('GET', `/api/workspace-profile?${key}`).then(profile => {
      if (live && Array.isArray(profile.files)) current.current && onChange({ ...current.current, files: profile.files, command: profile.command ?? undefined })
    }).catch(() => { if (live) setNotice('Saved preparation profile unavailable.') })
    return () => { live = false }
  }, [client, key, value !== null])
  useEffect(() => {
    setPreview(null)
    if (!value || !key) return
    let live = true
    const timer = setTimeout(() => {
      void client.request<{ base_commit: string }>('POST', '/api/workspace-preview', { ...source, base: value.base }).then(result => {
        if (live && typeof result.base_commit === 'string') { setPreview(result.base_commit); if (value.base !== result.base_commit) current.current && onChange({ ...current.current, base: result.base_commit }) }
      }).catch(() => { if (live) setNotice('Base revision preview unavailable. Retry or enter an immutable commit SHA before starting.') })
    }, 300)
    return () => { live = false; clearTimeout(timer) }
  }, [client, key, value?.base])
  return <fieldset>
    <legend>Workspace</legend>
    <label><input type="checkbox" checked={value !== null} onChange={event => onChange(event.target.checked ? { base: 'HEAD', files: [] } : null)} /> Create a new worktree</label>
    {value ? <>
      <label>Base branch or commit<input value={value.base} onChange={event => onChange({ ...value, base: event.target.value })} /></label>
      {preview && <p>Resolved base commit: <bdi>{preview}</bdi></p>}
      <p>The selected committed revision is checked out on a new branch. Uncommitted changes are left in the source workspace.</p>
      <label>Preparation files (one relative path per line)<textarea value={value.files.join('\n')} onChange={event => onChange({ ...value, files: event.target.value.split('\n').filter(Boolean) })} /></label>
      <label>Preparation command (optional)<input value={value.command ?? ''} onChange={event => onChange({ ...value, command: event.target.value || undefined })} /></label>
      <p>Only these local files are copied. Existing checked-in files and symlinks are refused. Preparation failure prevents the agent from starting.</p>
      {key && <button type="button" onClick={() => { void client.request('PUT', `/api/workspace-profile?${key}`, { files: value.files, command: value.command ?? null }).then(() => setNotice('Preparation profile saved for this project.')).catch(() => setNotice('Could not save preparation profile.')) }}>Save preparation profile</button>}
      {notice && <p role="status">{notice}</p>}
    </> : <p>Uses the existing directory, including its uncommitted changes.</p>}
  </fieldset>
}
