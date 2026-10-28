import type { HostItem, McpSourcePolicy as Policy, McpSourceState } from '../generated/protocol'

export const sourcePolicyLabel = (policy: Policy | undefined): string => {
  switch (policy) {
    case 'managed_only': return 'Only MCP configured in Hennery'
    case 'managed_plus_native': return 'Hennery and runtime-native MCP'
    default: return 'Existing / adapter defaults'
  }
}

export function SourcePolicySelect({ value, onChange, label = 'MCP sources', allowed = ['managed_only', 'managed_plus_native'] }: { value: Policy; onChange: (policy: Policy) => void; label?: string; allowed?: Policy[] }) {
  return <label className="field">
    <span className="field-label">{label}</span>
    <select aria-label={label} className="text-input" value={value} onChange={event => onChange(event.target.value as Policy)}>
      <option value="legacy">{sourcePolicyLabel('legacy')}</option>
      {(['managed_only', 'managed_plus_native'] as const).filter(policy => policy === value || allowed.includes(policy)).map(policy =>
        <option key={policy} value={policy} disabled={!allowed.includes(policy)}>{sourcePolicyLabel(policy)}</option>)}
    </select>
    <span className="hint">Applies on the next start or resume for this ACP. Adapter defaults preserve its existing behavior. Runtime-native MCP can include project, plug-in and account sources shared across hats.</span>
  </label>
}

export function SourcePolicyPrediction({ policy = 'legacy', host, agent }: { policy?: Policy; host?: HostItem; agent: string }) {
  const supported = policy === 'legacy' || (Object.hasOwn(host?.mcp_source_policies ?? {}, agent) && host?.mcp_source_policies?.[agent]?.includes(policy))
  return <section aria-label="MCP sources prediction">
    <p>{sourcePolicyLabel(policy)}</p>
    {!supported && <p role="status">This host and agent do not report support for this MCP source policy. Starting will be refused.</p>}
  </section>
}

export default function McpSourcePolicy({ state }: { state?: McpSourceState }) {
  if (!state) return null
  const changed = state.applied ? state.policy !== state.applied.policy || state.revision !== state.applied.revision : state.policy !== 'legacy'
  const waiting = state.requested && state.requested.request_id !== state.applied?.request_id
  return <section aria-label="MCP sources">
    <p>Hat setting: {sourcePolicyLabel(state.policy)}</p>
    <p>Last confirmed attach: {state.applied ? sourcePolicyLabel(state.applied.policy) : 'Not reported'}</p>
    {waiting && <p>Requested at attach: {sourcePolicyLabel(state.requested!.policy)}. Awaiting host confirmation.</p>}
    {changed && <p>The hat setting applies on the next start or resume; the current process keeps its confirmed setting.</p>}
    {state.applied?.policy === 'managed_plus_native' && <p>Runtime-native sources are permitted outside Hennery’s per-hat filtering. Availability and sign-in depend on the runtime.</p>}
  </section>
}
