// A small router for real links (frontend spec §2): a push or a reload opens
// the view its URL names (F-19). History API only; no dependency.
import { useSyncExternalStore, type AnchorHTMLAttributes, type MouseEvent } from 'react'

export type RouteName =
  | 'setup'
  | 'login'
  | 'sessions'
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
  '/sessions': 'sessions',
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

const CHANGE = 'hennery:navigate'

/** Go to `to`, a path of this app: a new history entry, or this one
 *  replaced. */
export function navigate(to: string, options: { replace?: boolean } = {}): void {
  if (options.replace) history.replaceState(null, '', to)
  else history.pushState(null, '', to)
  window.dispatchEvent(new Event(CHANGE))
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener('popstate', onChange)
  window.addEventListener(CHANGE, onChange)
  return () => {
    window.removeEventListener('popstate', onChange)
    window.removeEventListener(CHANGE, onChange)
  }
}

const snapshot = () => location.pathname + location.search

/** The current path and query; re-renders on every navigation. */
export function useLocation(): { pathname: string; search: string } {
  const href = useSyncExternalStore(subscribe, snapshot)
  const at = href.indexOf('?')
  return at < 0 ? { pathname: href, search: '' } : { pathname: href.slice(0, at), search: href.slice(at) }
}

type LinkProps = Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href'> & { to: string }

/** An `<a>` that navigates inside the app, leaving modified clicks (new
 *  tab, new window) to the browser. */
export function Link({ to, onClick, ...rest }: LinkProps) {
  const go = (e: MouseEvent<HTMLAnchorElement>) => {
    onClick?.(e)
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return
    e.preventDefault()
    navigate(to)
  }
  return <a href={to} onClick={go} {...rest} />
}
