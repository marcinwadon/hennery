import type { HatItem, HostItem, McpSourcePolicy } from '../generated/protocol'
import { agentLabel } from '../lib/agent'
import { sourcePolicyFor, type SourcePolicyPatch } from '../lib/mcpSourcePolicy'
import { Text, visible } from '../lib/text'
import { SourcePolicySelect, sourcePolicyLabel } from './McpSourcePolicy'

export function sourceAgents(hat: HatItem | undefined, hosts: HostItem[] | null): string[] {
  return [...new Set(['claude', 'codex', ...Object.keys(hat?.mcp_source_overrides ?? {}),
    ...(hosts ?? []).filter(h => !h.revoked_at).flatMap(h => [...Object.keys(h.mcp_delivery ?? {}), ...Object.keys(h.mcp_source_policies ?? {})])])]
}

export default function AgentSourcePolicies({ hat, hosts, patch, onChange }: {
  hat?: HatItem; hosts: HostItem[] | null; patch: SourcePolicyPatch; onChange: (patch: SourcePolicyPatch) => void
}) {
  return <section className="agent-source-policies" aria-label="MCP sources by ACP">
    {sourceAgents(hat, hosts).map(agent => {
      const draft = Object.hasOwn(patch, agent) ? patch[agent] : undefined
      const value = draft === undefined ? sourcePolicyFor(hat, agent).policy : draft ?? hat?.mcp_source_policy ?? 'legacy'
      const allowed: McpSourcePolicy[] = [...new Set((hosts ?? []).filter(h => !h.revoked_at).flatMap(h =>
        Object.hasOwn(h.mcp_source_policies ?? {}, agent) ? h.mcp_source_policies?.[agent] ?? [] : []))]
      const overridden = draft === undefined ? hat?.mcp_source_overrides?.[agent]?.policy != null : draft != null
      return <fieldset key={agent} aria-label={`${visible(agentLabel(agent))} MCP sources`}>
        <legend><Text>{agentLabel(agent)}</Text></legend>
        <SourcePolicySelect label={`${visible(agentLabel(agent))} MCP sources`} value={value} allowed={allowed} onChange={policy => onChange({ ...patch, [agent]: policy })} />
        {!overridden && <p className="hint">Using hat defaults: {sourcePolicyLabel(hat?.mcp_source_policy)}</p>}
        {value !== 'legacy' && !allowed.includes(value) && <p role="status">No paired host reports support for this ACP setting. Starting will be refused on an unsupported host.</p>}
        {overridden && <button type="button" className="btn btn-ghost btn-sm" onClick={() => onChange({ ...patch, [agent]: null })}>Reset <Text>{agentLabel(agent)}</Text> MCP sources</button>}
      </fieldset>
    })}
  </section>
}
