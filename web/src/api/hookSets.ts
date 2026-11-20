// The hook-set catalogue's routes (skills spec §8), typed. Writes need a fresh
// step-up: the client opens the dialog on 403 `step_up_required` and retries
// once, so nothing here asks for it. `invalid_hook_set` is not mapped in
// `errors.ts`: its message names the problem, and is shown as it is.
import type {
  HookEventsResponse,
  HookSetConfig,
  HookSetDetail,
  HookSetHostsResponse,
  HookSetItem,
} from '../generated/protocol'
import type { Client } from './client'

const id = (value: string) => encodeURIComponent(value)

export function listHookSets(client: Client, signal?: AbortSignal): Promise<HookSetItem[]> {
  return client.request('GET', '/api/hook-sets', undefined, { signal })
}

export function hookSet(client: Client, hookSetId: string): Promise<HookSetDetail> {
  return client.request('GET', `/api/hook-sets/${id(hookSetId)}`)
}

/** Step-up. */
export function createHookSet(client: Client, name: string, hooks: HookSetConfig): Promise<HookSetItem> {
  return client.request('POST', '/api/hook-sets', { name, hooks })
}

/** Step-up. */
export function replaceHookSet(client: Client, hookSetId: string, hooks: HookSetConfig): Promise<HookSetItem> {
  return client.request('PUT', `/api/hook-sets/${id(hookSetId)}`, { hooks })
}

/** Step-up. */
export function deleteHookSet(client: Client, hookSetId: string): Promise<void> {
  return client.request('DELETE', `/api/hook-sets/${id(hookSetId)}`)
}

export function hookSetHosts(client: Client, hookSetId: string): Promise<HookSetHostsResponse> {
  return client.request('GET', `/api/hook-sets/${id(hookSetId)}/hosts`)
}

/** Step-up. The whole set of hosts, replacing the one before. */
export function setHookSetHosts(client: Client, hookSetId: string, hostIds: string[]): Promise<HookSetHostsResponse> {
  return client.request('PUT', `/api/hook-sets/${id(hookSetId)}/hosts`, { host_ids: hostIds })
}

/** The events a hook set may name, and which agents act on each. */
export function hookEvents(client: Client): Promise<HookEventsResponse> {
  return client.request('GET', '/api/hook-sets/events')
}
