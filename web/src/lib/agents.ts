// Which agents a host can start (frontend spec §7). Everything the New
// Session form knows about a host's agents comes through `agentsFor`.
import type { Client } from '../api/client'
import { agentLabel } from './agent'

// LOCAL TYPES: these mirror the shape of 4d's `GET /api/hosts/{id}/agents`
// (B1-i, shape v2) until its generated types land in
// `generated/protocol.ts`. Then they are deleted and imported from there;
// nothing else changes.

/** Whether the agent is signed in on its host: `unknown` when the host has
 *  not checked. */
export type AgentAuth = 'ok' | 'missing' | 'unknown'

/** Which CLI the agent runs. */
export type AgentCli = 'bundled' | 'override' | 'given'

/** One agent of a host. */
export interface AgentInfo {
  /** The profile name: what `StartSessionRequest.agent` takes. */
  agent: string
  /** Launchable as configured. */
  available: boolean
  auth: AgentAuth
  cli: AgentCli
  adapter_version?: string
  /** `false`: it takes no images. Absent: not probed, so images are
   *  allowed and the server's 409 `images_unsupported` stays the guard. */
  images?: boolean
  /** Why it is unavailable, or a caveat, in the host's words. */
  note?: string
}

/** Where a host's agents come from. */
export type RuntimeSource = 'managed' | 'given'

export interface RuntimeInfo {
  source: RuntimeSource
  set_id?: string
  pinned?: boolean
  held?: boolean
}

/** Where a `HostAgents` report comes from. */
export type AgentsSource = 'none' | 'hello' | 'probe'

/** `GET /api/hosts/{id}/agents[?refresh=1]`: the host's latest report. */
export interface HostAgents {
  host_id: string
  agents: AgentInfo[]
  runtime?: RuntimeInfo
  /** RFC 3339; absent with `source: none`. */
  reported_at?: string
  source: AgentsSource
  /** The host is connected and reconciled now. */
  live: boolean
}

/** What the form may offer on one host. */
export interface HostAgentChoices {
  agents: AgentInfo[]
  /** The form also takes an agent name typed by hand ("Other…"). */
  other: boolean
}

/** The agents to offer on `hostId`.
 *
 *  THE SWITCH POINT: no host reports its agents yet, so today this answers
 *  the two built-in profiles, unchecked (`auth: 'unknown'`, which the
 *  picker allows), plus a name typed by hand. When
 *  `GET /api/hosts/{id}/agents` lands, this becomes
 *  `client.request<HostAgents>('GET', \`/api/hosts/${encodeURIComponent(hostId)}/agents\`)`
 *  mapped to `{agents: answer.agents, other: true}`; nothing else changes. */
export async function agentsFor(client: Client, hostId: string): Promise<HostAgentChoices> {
  void client
  void hostId
  return {
    agents: [
      { agent: 'claude', available: true, auth: 'unknown', cli: 'bundled' },
      { agent: 'codex', available: true, auth: 'unknown', cli: 'bundled' },
    ],
    other: true,
  }
}

/** Images are hidden only for an agent known to take none
 *  (`images === false`); an agent not probed may take them. */
export function takesImages(agent: Pick<AgentInfo, 'images'>): boolean {
  return agent.images !== false
}

/** An agent as the picker shows it. */
export interface AgentOption {
  agent: string
  label: string
  /** Why it cannot be picked; absent when it can. */
  disabled?: string
}

/** The picker's rules: only `available` agents are listed; auth `missing`
 *  is listed but disabled, with its note; `ok` and `unknown` may be
 *  picked. */
export function agentOptions(agents: AgentInfo[]): AgentOption[] {
  return agents
    .filter((a) => a.available)
    .map((a) => ({
      agent: a.agent,
      label: agentLabel(a.agent),
      ...(a.auth === 'missing' ? { disabled: a.note || 'Not signed in on this host.' } : {}),
    }))
}
