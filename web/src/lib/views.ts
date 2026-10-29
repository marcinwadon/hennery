import type { DeploymentMode } from '../generated/protocol'
import type { RouteName } from '../router'
export type View = 'needs-you' | 'sessions' | 'new' | 'hosts' | 'mcp' | 'hats' | 'defaults' | 'settings' | 'more'
const BY_MODE: Record<DeploymentMode, readonly View[]> = {
  full: ['needs-you', 'sessions', 'new', 'defaults', 'hosts', 'mcp', 'hats', 'settings', 'more'],
  gateway: ['mcp', 'settings', 'more'],
}
export function viewsOf(mode: DeploymentMode): readonly View[] { return BY_MODE[mode] }
export function viewOf(route: RouteName): View | null {
  if (route === 'session') return 'sessions'
  return route === 'setup' || route === 'login' || route === 'external_mcp' || route === 'external_mcp_consent' || route === 'not_found' ? null : route
}
export const LABEL: Record<View, string> = { 'needs-you': 'Needs you', sessions: 'Sessions', new: 'New session', defaults: 'Defaults', hosts: 'Hosts', mcp: 'MCP', hats: 'Hats', settings: 'Settings', more: 'More' }
export const PATH: Record<View, string> = { 'needs-you': '/needs-you', sessions: '/sessions', new: '/new', defaults: '/defaults', hosts: '/hosts', mcp: '/mcp', hats: '/hats', settings: '/settings', more: '/more' }
export function tabsOf(mode: DeploymentMode): View[] { return mode === 'full' ? ['needs-you', 'sessions', 'new', 'hosts', 'more'] : ['mcp', 'more'] }
