// The routes the Hosts and Hats screens use (kernel spec §4, §5, §8), typed.
// Which of them need a fresh step-up is the server's to say: the client
// opens the dialog on 403 `step_up_required` and retries once.
import type {
  CreateHatRequest,
  HatItem,
  HatResolution,
  HostItem,
  PairingCodeResponse,
  PathRuleInput,
  PathRuleItem,
  PurgePreview,
  PurgeResult,
  SettingsResponse,
  UpdateHatRequest,
  UpdateHostRequest,
} from '../generated/protocol'
import type { Client } from './client'

const id = (value: string) => encodeURIComponent(value)

export function hosts(client: Client): Promise<HostItem[]> {
  return client.request('GET', '/api/hosts')
}

export function settings(client: Client): Promise<SettingsResponse> {
  return client.request('GET', '/api/settings')
}

/** Step-up. */
export function mintPairingCode(client: Client): Promise<PairingCodeResponse> {
  return client.request('POST', '/api/hosts/pairing-codes')
}

/** Rename, or change the default hat. Step-up. */
export function updateHost(client: Client, hostId: string, change: UpdateHostRequest): Promise<HostItem> {
  return client.request('PATCH', `/api/hosts/${id(hostId)}`, change)
}

/** Step-up. */
export function revokeHost(client: Client, hostId: string): Promise<HostItem> {
  return client.request('DELETE', `/api/hosts/${id(hostId)}`)
}

export function hats(client: Client): Promise<HatItem[]> {
  return client.request('GET', '/api/hats')
}

export function createHat(client: Client, hat: CreateHatRequest): Promise<HatItem> {
  return client.request('POST', '/api/hats', hat)
}

/** Rename, recolour, or make the default for new hosts. Step-up. */
export function updateHat(client: Client, hatId: string, change: UpdateHatRequest): Promise<HatItem> {
  return client.request('PATCH', `/api/hats/${id(hatId)}`, change)
}

export function pathRules(client: Client, hostId: string): Promise<PathRuleItem[]> {
  return client.request('GET', `/api/hosts/${id(hostId)}/path-rules`)
}

/** The host's whole set, replacing the one before; the answer is the set as
 *  stored. Step-up, and the host must be connected. */
export function replacePathRules(client: Client, hostId: string, rules: PathRuleInput[]): Promise<PathRuleItem[]> {
  return client.request('PUT', `/api/hosts/${id(hostId)}/path-rules`, { rules })
}

/** Which hat `path` resolves to on the host, under the saved rules. */
export function resolveHat(client: Client, hostId: string, path: string, signal?: AbortSignal): Promise<HatResolution> {
  return client.request('POST', '/api/hats/resolve', { host_id: hostId, path }, { signal })
}

export function purgePreview(client: Client, hatId: string): Promise<PurgePreview> {
  return client.request('GET', `/api/hats/${id(hatId)}/purge`)
}

/** Step-up. */
export function purgeHat(client: Client, hatId: string): Promise<PurgeResult> {
  return client.request('POST', `/api/hats/${id(hatId)}/purge`)
}
