// What the Hosts and Hats screens decide from the server's answers. Each
// function is a classifier with one outcome per case the screen shows.
import type { HatItem, HostItem, PurgePreview } from '../generated/protocol'

export type HostState = 'online' | 'offline' | 'revoked'

/** Revoked wins over everything; then connected (and reconciled) or not. */
export function hostState(host: HostItem): HostState {
  if (host.revoked_at !== undefined) return 'revoked'
  return host.connected ? 'online' : 'offline'
}

export const HOST_STATE_LABEL: Record<HostState, string> = {
  online: 'Online',
  offline: 'Offline',
  revoked: 'Revoked',
}

export type PurgeState = 'default' | 'running' | 'resume' | 'ready'

/** Whether a purge can go ahead (kernel spec §5.5), in the server's order:
 *  a default hat is refused first (409 `hat_is_default`), then running
 *  sessions (409 `sessions_running`); a frozen hat resumes its purge. */
export function purgeState(hat: HatItem, hosts: HostItem[], preview: PurgePreview): PurgeState {
  if (isDefault(hat, hosts)) return 'default'
  if (preview.running.length > 0) return 'running'
  if (preview.purging) return 'resume'
  return 'ready'
}

/** The default for new hosts, or the default hat of any host, revoked ones
 *  included: the server refuses to purge either. */
export function isDefault(hat: HatItem, hosts: HostItem[]): boolean {
  return hat.default_for_new_hosts || hosts.some((h) => h.default_hat_id === hat.id)
}

const COLOUR = /^#[0-9a-f]{6}$/

/** A hat's colour as the server guarantees it (`#rrggbb`, lowercase); the
 *  client trusts no more than that before it reaches a style. */
export function safeColour(colour: string): string | undefined {
  return COLOUR.test(colour) ? colour : undefined
}

/** `n thing`, or `n things`. */
export function count(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`
}
