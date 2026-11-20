// The skill catalogue's routes (skills spec), typed. Writes need a fresh
// step-up: the client opens the dialog on 403 `step_up_required` and retries
// once, so nothing here asks for it.
import type { HatHostsResponse, SkillDetail, SkillHostsResponse, SkillItem } from '../generated/protocol'
import type { Client } from './client'

const id = (value: string) => encodeURIComponent(value)

export function listSkills(client: Client, signal?: AbortSignal): Promise<SkillItem[]> {
  return client.request('GET', '/api/skills', undefined, { signal })
}

export function skill(client: Client, skillId: string): Promise<SkillDetail> {
  return client.request('GET', `/api/skills/${id(skillId)}`)
}

/** Step-up. `data` is the base64 of a tar.gz. */
export function createSkill(client: Client, data: string): Promise<SkillItem> {
  return client.request('POST', '/api/skills', { data })
}

/** Step-up. The new archive must carry the same skill name. */
export function replaceSkillArchive(client: Client, skillId: string, data: string): Promise<SkillItem> {
  return client.request('PUT', `/api/skills/${id(skillId)}/archive`, { data })
}

/** Step-up. */
export function deleteSkill(client: Client, skillId: string): Promise<void> {
  return client.request('DELETE', `/api/skills/${id(skillId)}`)
}

export function skillHosts(client: Client, skillId: string): Promise<SkillHostsResponse> {
  return client.request('GET', `/api/skills/${id(skillId)}/hosts`)
}

/** Step-up. The whole set of hosts, replacing the one before. */
export function setSkillHosts(client: Client, skillId: string, hostIds: string[]): Promise<SkillHostsResponse> {
  return client.request('PUT', `/api/skills/${id(skillId)}/hosts`, { host_ids: hostIds })
}

/** The hosts that have sessions of a hat. */
export function hatHosts(client: Client, hatId: string): Promise<HatHostsResponse> {
  return client.request('GET', `/api/skills/hat-hosts/${id(hatId)}`)
}
