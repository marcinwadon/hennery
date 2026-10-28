// Delivery is a last-start/resume decision, never a live health check.
import type { HostItem } from '../generated/protocol'

export type DeliveryMode = 'unknown' | 'isolated' | 'managed_plus_native' | 'unisolated' | 'fallback' | 'unsupported'

export function deliveryWords(mode: unknown): string {
  switch (mode) {
    case 'managed_plus_native':
      return 'This hat’s managed MCP servers were offered alongside runtime-native MCP sources.'
    case 'isolated':
      return "This agent was given only this hat's MCP servers."
    case 'unisolated':
      return 'The MCP servers are not isolated per hat; the agent also loads your own MCP configuration.'
    case 'fallback':
      return 'No hennery MCP servers: this hat is not the host’s default for this agent.'
    case 'unsupported':
      return 'No hennery MCP servers: the host does not support their delivery.'
    default:
      return 'MCP delivery is unknown.'
  }
}

/** Unknown/missing reports never imply isolation. An accepted capability-
 *  negative hello is distinct from a host that has never reported. */
export function predictDelivery(host: HostItem | undefined, agent: string, hat: string | undefined): DeliveryMode {
  if (!host || !agent || !hat) return 'unknown'
  if (!host.capabilities.includes('mcp_servers')) {
    if (host.last_seen_at === undefined) return 'unknown'
    return 'unsupported'
  }
  const map = host.mcp_delivery
  if (!map || !Object.hasOwn(map, agent)) return 'unknown'
  if (map[agent] === 'isolated') return 'isolated'
  // An unknown future agent-delivery value is read as default_hat_only.
  if (hat === host.default_hat_id) return 'unisolated'
  return 'fallback'
}

export function predictionWords(mode: DeliveryMode): string {
  return 'Prediction from the host’s last report: ' + deliveryWords(mode)
}

export const CODEX_MCP_NOTE =
  'This session’s MCP servers are not isolated per hat. Codex also loads your own ~/.codex MCP servers.'

/** The caveat follows the recorded/predicted mode; unknown is conditional. */
export function ownConfigNote(mode: DeliveryMode, agent?: string): string | undefined {
  if (mode === 'unisolated') return agent === 'codex' ? CODEX_MCP_NOTE : 'The agent also loads your own MCP configuration.'
  if (mode === 'unknown' && agent === 'codex') return 'Codex may also load your own ~/.codex MCP servers.'
  return undefined
}

/** Only a non-running → running transition invalidates delivery metadata. */
export function runningLifecycle(lifecycle: string | undefined): boolean {
  return lifecycle === 'starting' || lifecycle === 'active'
}

/** A resolution of another input must never name this draft's hat. */
export function predictionHat(resolved: { hostId: string; path: string; hat: string } | undefined, hostId: string, path: string): string | undefined {
  if (resolved?.hostId === hostId && resolved.path === path) return resolved.hat
  return undefined
}
