import type { Client, RequestOptions } from './client'
import type { PeerAccess, PeerPolicy } from '../generated/protocol'
const path = (hat: string) => `/api/hats/${encodeURIComponent(hat)}/peer-access`
export function peerPolicy(client: Client, hat: string): Promise<PeerPolicy> {
  return client.request('GET', path(hat))
}
export function setPeerPolicy(client: Client, hat: string, access: PeerAccess, options?: RequestOptions): Promise<PeerPolicy> {
  return client.request('PATCH', path(hat), { access }, options)
}
