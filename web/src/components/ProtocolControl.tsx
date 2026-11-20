import { useId } from 'react'
import { protocolValue, type ProtocolPreference } from '../lib/protocol'

export default function ProtocolControl({ value, disabled, inherit = false, onChange }: {
  value?: ProtocolPreference; disabled?: boolean; inherit?: boolean; onChange: (value: ProtocolPreference | undefined) => void
}) {
  const id = useId()
  return <div className="config-select protocol-select"><label htmlFor={id}>ACP protocol</label><select id={id} value={value ?? ''} disabled={disabled} onChange={e => onChange(protocolValue(e.target.value))}>
    <option value="">{inherit ? 'Inherit (v1 unless configured)' : 'Agent default (v1)'}</option>
    <option value="v1">v1</option><option value="v2">v2 · experimental, required</option><option value="v2-v1">v2 / v1 · prefer v2, allow fallback</option>
  </select><p className="hint">v2 reports background work when supported by the agent. The protocol is chosen when the session attaches.</p></div>
}
