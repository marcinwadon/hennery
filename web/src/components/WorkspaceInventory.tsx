import ConfirmDialog from './ConfirmDialog'
import { useEffect, useState } from 'react'
import { useClient } from '../app-client'
interface Workspace { id: string; root: string; branch: string; state: string; users: number }
export default function WorkspaceInventory() {
  const client = useClient()
  const [rows, setRows] = useState<Workspace[]>([])
  const [error, setError] = useState(false)
  const [discard, setDiscard] = useState<Workspace | null>(null)
  useEffect(() => {
    let live = true
    const read = () => { void client.request<Workspace[]>('GET', '/api/workspaces').then(value => { if (live && Array.isArray(value)) { setRows(value); setError(false) } }).catch(() => { if (live) setError(true) }) }
    read(); const timer = setInterval(read, 15000)
    return () => { live = false; clearInterval(timer) }
  }, [client])
  return <section><h2>Managed worktrees</h2><p>Cleanup waits for all sessions to release a workspace. Local changes and preparation files block removal; preserve or remove those files before retrying. Branches are retained.</p>{error && <p role="status">Workspace inventory unavailable.</p>}{rows.map(row => <div key={row.id}><p><bdi>{row.branch}</bdi> · {row.state} · {row.users} retaining session(s)</p><p><bdi>{row.root}</bdi></p>{row.state === 'blocked' && row.users === 0 && <button type="button" onClick={() => setDiscard(row)}>Discard local files and remove worktree</button>}</div>)}{discard && <ConfirmDialog title="Discard workspace files?" confirm="Discard and remove" danger action={async () => {
    await client.request('POST', `/api/workspaces/${encodeURIComponent(discard.id)}/discard`)
    setRows(rows => rows.filter(row => row.id !== discard.id))
  }} onClose={() => setDiscard(null)}><p>Remove all uncommitted, untracked and ignored files in <bdi>{discard.root}</bdi>. This cannot be undone. The branch <bdi>{discard.branch}</bdi> and committed work are retained.</p></ConfirmDialog>}</section>
}
