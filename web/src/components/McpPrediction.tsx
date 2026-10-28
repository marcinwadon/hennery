import type { HostItem, HatItem } from '../generated/protocol'
import { sourcePolicyFor } from '../lib/mcpSourcePolicy'
import { ownConfigNote, predictDelivery, predictionWords } from '../lib/mcpDelivery'
import { agentLabel } from '../lib/agent'
import { Text } from '../lib/text'
import { SourcePolicyPrediction } from './McpSourcePolicy'
import type { McpSourcePolicy } from '../generated/protocol'

export default function McpPrediction({ host, agent, hatId, hat, policy: legacyPolicy }: { host?: HostItem; agent: string; hatId?: string; hat?: HatItem; policy?: McpSourcePolicy }) {
  const policy = hat ? sourcePolicyFor(hat, agent).policy : legacyPolicy
  const mode = predictDelivery(host, agent, hatId)
  const note = ownConfigNote(mode, agent)
  if (policy && policy !== 'legacy') return <section className="mcp-prediction" aria-label="MCP prediction">
    <p><Text>{agentLabel(agent)}</Text></p>
    <SourcePolicyPrediction policy={policy} host={host} agent={agent} />
  </section>
  return (
    <section className="mcp-prediction" aria-label="MCP prediction">
      <p><Text>{agentLabel(agent)}</Text>: {predictionWords(mode)}</p>
      {note && <p>{note}</p>}
    </section>
  )
}
