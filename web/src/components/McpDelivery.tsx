import type { McpSessionDelivery } from '../generated/protocol'
import { ownConfigNote, deliveryWords } from '../lib/mcpDelivery'

export default function McpDelivery({ agent, delivery }: { agent?: string; delivery?: McpSessionDelivery }) {
  const mode = delivery?.mode
  const note = ownConfigNote(
    mode === 'isolated' || mode === 'managed_plus_native' || mode === 'unisolated' || mode === 'fallback' || mode === 'unsupported' ? mode : 'unknown', agent)
  return (
    <section className="mcp-delivery" aria-label="MCP delivery">
      <p>At the last start/resume: {deliveryWords(delivery?.mode)}</p>
      {delivery && <p>{delivery.servers} MCP servers.</p>}
      {note && <p>{note}</p>}
    </section>
  )
}
