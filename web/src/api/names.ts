// The names a session view shows in place of ids: hosts (`GET /api/hosts`)
// and hats (`GET /api/hats`). Both answer a bare array.
import type { HatItem, HostItem } from '../generated/protocol'
import type { Client } from './client'

export function hostList(client: Client): Promise<HostItem[]> {
  return client.request<HostItem[]>('GET', '/api/hosts')
}

export function hatList(client: Client): Promise<HatItem[]> {
  return client.request<HatItem[]>('GET', '/api/hats')
}

/** `id → name` of a list that may not be one: anything else is no names. */
export function namesOf(list: unknown, key: 'host_id' | 'id'): Map<string, string> {
  const names = new Map<string, string>()
  if (!Array.isArray(list)) return names
  for (const entry of list) {
    if (!entry || typeof entry !== 'object') continue
    const e = entry as Record<string, unknown>
    if (typeof e[key] === 'string' && typeof e.name === 'string') names.set(e[key] as string, e.name)
  }
  return names
}
