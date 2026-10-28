// Which hosts a connection is mounted on (gateway spec §1, §9): every host
// that is not revoked, ticked or not. A tick saves at once, as the whole set
// (no step-up: a mount reaches only a host the owner paired). The set sent
// is the connection's own with one host changed, so a host this list does
// not show keeps its mount.
import { useRef, useState } from 'react'
import { messageOf } from '../api/errors'
import { replaceMounts } from '../api/mcp'
import { useClient } from '../app-client'
import type { HostItem, McpConnectionItem, McpSourcePolicy } from '../generated/protocol'
import { mountsWith } from '../lib/mcp'
import { Text } from '../lib/text'
import McpPrediction from './McpPrediction'

interface Props {
  connection: McpConnectionItem
  /** `null` while they load or when they could not be read. */
  hosts: HostItem[] | null
  onChanged: (connection: McpConnectionItem) => void
  compact?: boolean
  policy?: McpSourcePolicy
}

export default function Mounts({ connection, hosts, onChanged, compact = false, policy }: Props) {
  const client = useClient()
  // A ref, not state: two ticks in one frame both see the first.
  const running = useRef(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const live = (hosts ?? []).filter((h) => !h.revoked_at)

  // While a set is being saved, a tick is ignored rather than the boxes
  // disabled: disabling the box that has focus drops focus to the page.
  const toggle = async (hostId: string, on: boolean) => {
    if (running.current) return
    running.current = true
    setBusy(true)
    setError(null)
    try {
      onChanged(await replaceMounts(client, connection.id, mountsWith(connection.mounts, hostId, on)))
    } catch (err) {
      setError(messageOf(err))
    } finally {
      running.current = false
      setBusy(false)
    }
  }

  return (
    <fieldset className={`mounts${compact ? ' mounts-compact' : ''}`} disabled={hosts === null} aria-busy={busy}>
      <legend>Hosts whose sessions get it</legend>
      {hosts !== null && live.length === 0 && <p className="hint">No host is paired yet.</p>}
      {live.map((host) => (
        <div key={host.host_id}>
        <label className="check">
          <input
            type="checkbox"
            checked={connection.mounts.includes(host.host_id)}
            onChange={(e) => toggle(host.host_id, e.target.checked)}
          />
          <Text>{host.name}</Text>
        </label>
        <details className="mount-predictions" open={!compact || undefined}><summary>Delivery details for <Text>{host.name}</Text></summary>
        {Object.keys(host.mcp_delivery ?? {}).length === 0
          ? <p className="hint">MCP delivery is unknown for this host’s agents.</p>
          : Object.keys(host.mcp_delivery ?? {}).map((agent) => (
            <McpPrediction key={agent} host={host} agent={agent} hatId={connection.hat_id} policy={policy} />
          ))}</details>
        </div>
      ))}
      {error && (
        <p className="form-error" role="alert">
          <Text>{error}</Text>
        </p>
      )}
    </fieldset>
  )
}
