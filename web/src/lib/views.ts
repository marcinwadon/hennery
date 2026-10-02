// Which views a deployment shows (frontend spec §2): the whole cockpit, or
// the gateway alone. The mode comes from `GET /api/capabilities`.
import type { DeploymentMode } from '../generated/protocol'
import type { RouteName } from '../router'

export type View = 'sessions' | 'new' | 'hosts' | 'mcp' | 'hats' | 'settings'

const BY_MODE: Record<DeploymentMode, readonly View[]> = {
  full: ['sessions', 'new', 'hosts', 'mcp', 'hats', 'settings'],
  gateway: ['mcp', 'settings'],
}

export function viewsOf(mode: DeploymentMode): readonly View[] {
  return BY_MODE[mode]
}

/** The view a route belongs to; the sign-in screens and unknown paths
 *  belong to none. */
export function viewOf(route: RouteName): View | null {
  switch (route) {
    case 'sessions':
    case 'session':
      return 'sessions'
    case 'new':
    case 'hosts':
    case 'mcp':
    case 'hats':
    case 'settings':
      return route
    default:
      return null
  }
}

export const LABEL: Record<View, string> = {
  sessions: 'Sessions',
  new: 'New session',
  hosts: 'Hosts',
  mcp: 'MCP',
  hats: 'Hats',
  settings: 'Settings',
}

export const PATH: Record<View, string> = {
  sessions: '/sessions',
  new: '/new',
  hosts: '/hosts',
  mcp: '/mcp',
  hats: '/hats',
  settings: '/settings',
}

/** The bottom tab bar under 768 px (frontend spec §2): Sessions, New,
 *  Hosts (MCP in gateway mode), Settings — those the mode shows. */
export function tabsOf(mode: DeploymentMode): View[] {
  const shown = viewsOf(mode)
  const tabs: View[] = mode === 'gateway' ? ['mcp', 'settings'] : ['sessions', 'new', 'hosts', 'settings']
  return tabs.filter((v) => shown.includes(v))
}
