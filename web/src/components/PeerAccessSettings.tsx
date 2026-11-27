import { useEffect, useId, useRef, useState } from 'react'
import { peerPolicy, setPeerPolicy } from '../api/peers'
import { messageOf, StepUpCancelled } from '../api/errors'
import { useClient } from '../app-client'
import type { HatItem, PeerAccess } from '../generated/protocol'
import { useResource } from '../hooks/useResource'
import { Text, visible } from '../lib/text'

export default function PeerAccessSettings({ hats }: { hats: HatItem[] | null }) {
  const id = useId()
  const [selected, setSelected] = useState(''), [busy, setBusy] = useState(false)
  const hat = hats?.find(h => h.id === selected) ?? hats?.find(h => !h.purging) ?? hats?.[0]
  return <section className="card" aria-labelledby="peer-access-heading">
    <h2 id="peer-access-heading" tabIndex={-1}>Session collaboration by hat</h2>
    {!hats && <p role="status">Loading hats…</p>}
    {hats?.length === 0 && <p>No hats available.</p>}
    {hat && <><label htmlFor={id}>Session collaboration hat<select id={id} value={hat.id} disabled={busy} onChange={e => setSelected(e.target.value)}>{hats?.map(h => <option key={h.id} value={h.id}>{visible(h.name)}</option>)}</select></label>
      <HatPeerAccess key={hat.id} hat={hat} busy={busy} setBusy={setBusy} /></>}
  </section>
}

function HatPeerAccess({ hat, busy, setBusy }: { hat: HatItem; busy: boolean; setBusy: (value: boolean) => void }) {
  const client = useClient(), id = useId(), help = useId()
  const policy = useResource(() => peerPolicy(client, hat.id), [client, hat.id])
  const [error, setError] = useState<string | null>(null), [notice, setNotice] = useState<string | null>(null)
  const running = useRef(false), live = useRef(true)
  useEffect(() => { live.current = true; return () => { live.current = false } }, [])
  const change = async (access: PeerAccess) => {
    if (running.current || hat.purging) return
    running.current = true; setBusy(true); setError(null); setNotice(null)
    let writing = false
    try {
      await client.confirm()
      if (!live.current) return
      writing = true
      const saved = await setPeerPolicy(client, hat.id, access, { stepUp: false })
      if (live.current) {
        policy.set(saved)
        if (saved.access !== access) throw new Error('The collector did not save the requested session access.')
        setNotice('Session collaboration saved. Applies at the next start or resume.')
      }
    } catch (err) {
      if (live.current) {
        setError(err instanceof StepUpCancelled ? 'Change cancelled. The saved setting is unchanged.' : messageOf(err))
        if (writing) {
          try { const saved = await peerPolicy(client, hat.id); if (live.current) policy.set(saved) } catch { /* Keep last confirmed policy. */ }
        }
      }
    } finally { running.current = false; if (live.current) setBusy(false) }
  }
  return <div>
    {hat.purging && <p role="alert">This hat is being purged. Session collaboration is read-only.</p>}
    {policy.error && <p role="alert">Unable to load session collaboration. <Text>{policy.error}</Text> <button className="btn btn-ghost" onClick={policy.reload}>Retry session collaboration</button></p>}
    {!policy.data && !policy.error && <p role="status">Loading session collaboration…</p>}
    {policy.data && <label htmlFor={id}>Session access<select id={id} aria-describedby={help} value={policy.data.access} disabled={busy || hat.purging} onChange={e => void change(e.target.value as PeerAccess)}>
      <option value="off">Off</option><option value="read">Read other sessions</option><option value="read_write">Read and message other sessions</option>
    </select></label>}
    <p id={help}>Agents can discover and read conversations in this hat across hosts, including retained closed sessions. Messages identify the sending session. Parent and child session permissions are configured separately.</p>
    <p className="hint">Takes effect at the next start or resume. Changing access invalidates previously delivered collaboration access.</p>
    {error && <p role="alert"><Text>{error}</Text></p>}{notice && <p role="status">{notice}</p>}
  </div>
}
