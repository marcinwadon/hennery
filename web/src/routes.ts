export type RouteName =
  | 'setup'
  | 'login'
  | 'external_mcp'
  | 'external_mcp_consent'
  | 'sessions'
  | 'needs-you'
  | 'more'
  | 'defaults'
  | 'session'
  | 'new'
  | 'hosts'
  | 'mcp'
  | 'hats'
  | 'settings'
  | 'not_found'

export interface Route {
  name: RouteName
  /** `/sessions/:id`'s id, decoded. */
  id?: string
}

const STATIC: Record<string, RouteName> = {
  '/setup': 'setup',
  '/login': 'login',
  '/external-mcp': 'external_mcp',
  '/external-mcp/consent': 'external_mcp_consent',
  '/sessions': 'sessions',
  '/needs-you': 'needs-you',
  '/more': 'more',
  '/defaults': 'defaults',
  '/new': 'new',
  '/hosts': 'hosts',
  '/mcp': 'mcp',
  '/hats': 'hats',
  '/settings': 'settings',
}

/** The route `pathname` names. A trailing slash is the same route. */
export function match(pathname: string): Route {
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname
  if (path === '/') return { name: 'sessions' }
  const name = STATIC[path]
  if (name) return { name }
  const session = /^\/sessions\/([^/]+)$/.exec(path)
  if (session) {
    try {
      return { name: 'session', id: decodeURIComponent(session[1]) }
    } catch {
      return { name: 'not_found' }
    }
  }
  return { name: 'not_found' }
}

/** App documents only; API, auth and MCP proxy documents never get replaced. */
export function updateRoute(pathname: string): boolean {
  if (pathname === '/api' || pathname.startsWith('/api/') || pathname.startsWith('/mcp/')) return false
  return !['setup', 'login', 'external_mcp', 'external_mcp_consent', 'not_found'].includes(match(pathname).name)
}
