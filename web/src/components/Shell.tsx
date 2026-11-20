import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react'
import { capabilities } from '../api/auth'
import { messageOf } from '../api/errors'
import { useClient } from '../app-client'
import { useRegisterAction, useRegisterContext, type AppActionId } from '../app-actions'
import type { CapabilitiesResponse } from '../generated/protocol'
import { LABEL, PATH, viewOf, viewsOf, type View } from '../lib/views'
import { Link, navigate, type Route } from '../router'
import Hats from '../screens/Hats'
import Hosts from '../screens/Hosts'
import Mcp from '../screens/Mcp'
import Placeholder from '../screens/Placeholder'
import Skills from '../screens/Skills'
import Sessions from '../screens/Sessions'
import NeedsYou from '../screens/NeedsYou'
import More from '../screens/More'
import PushForeground from './PushForeground'
import SessionView from '../screens/Session'
import SessionScope, { useSessionScope, WaitingBadge } from './SessionScope'
import McpScope, { McpBadge } from './McpScope'
import NavigationRail from './NavigationRail'
import BottomNavigation from './BottomNavigation'
import { CollectorVersionContext } from './VersionInfo'
import { usePersistentToggle } from '../hooks/usePersistentToggle'
import '../styles/shell.css'
const Settings = lazy(() => import('../screens/Settings'))
const Defaults = lazy(() => import('../screens/Defaults'))
const NewSessionFrame = lazy(() => import('./NewSessionFrame'))
const NewSession = lazy(() => import('../screens/NewSession'))
export const DESKTOP = '(min-width: 768px)'
export const COLLECTOR_REFRESH_MS = 150
function SessionRoute({ id }: { id: string }) {
  const list = useSessionScope()?.list
  return <SessionView id={id} summary={list?.all.get(id)} awaitSummary={!!list && list.loading && !list.error} onRemoved={list?.remove} />
}
function NavigationAction({ view, enabled }: { view: View; enabled: boolean }) {
  const id = view === 'new' ? 'new-session' : `open-${view}` as AppActionId
  const run = useCallback(() => navigate(PATH[view]), [view])
  useRegisterAction(id, { label: LABEL[view], enabled, run }, -1)
  return null
}
function Frame({ caps, route }: { caps: CapabilitiesResponse; route: Route }) {
  const [collapsed, setCollapsed] = usePersistentToggle('hennery.railCollapsed', false)
  const views = viewsOf(caps.mode)
  const advertised = views.filter(view => (view !== 'defaults' || caps.features.includes('session_defaults')) && (view !== 'skills' || caps.features.includes('skills')))
  const current = viewOf(route.name)
  const shown = current !== null && views.includes(current)
  const scope = useSessionScope()
  useRegisterContext({ hatId: scope?.hat || undefined }, -1)
  useRegisterAction('toggle-rail', { label: 'Toggle navigation', enabled: true, run: () => setCollapsed(!collapsed) }, -1)
  useRegisterAction('add-host', { label: 'Add host', enabled: views.includes('hosts'), run: () => navigate('/hosts?add=1') })
  useRegisterAction('search-sessions', { label: 'Search sessions', enabled: caps.mode === 'full', run: () => {
    navigate('/sessions?search=1')
    // A request on another route focuses only after the overview has mounted.
    requestAnimationFrame(() => document.querySelector<HTMLInputElement>('[aria-label="Search sessions"]')?.focus())
  } }, -1)
  return <div className={'app' + (collapsed ? ' app-collapsed' : '') + (route.name === 'session' ? ' app-session' : '')}>
    {(['needs-you', 'sessions', 'new', 'hosts', 'mcp', 'skills', 'hats', 'defaults', 'settings'] as const).map(view => <NavigationAction key={view} view={view} enabled={views.includes(view) && (view !== 'mcp' || caps.features.includes('mcp_connections')) && (view !== 'skills' || caps.features.includes('skills')) && (view !== 'defaults' || caps.features.includes('session_defaults'))} />)}
    {caps.features.includes('web_push') && <PushForeground />}
    <NavigationRail views={advertised} current={current} collapsed={collapsed} toggle={() => setCollapsed(!collapsed)} />
    <div className="shell">{!['sessions', 'needs-you', 'session', 'more'].includes(route.name) && <header className="mtopbar"><span className="mtb-title">{current ? LABEL[current] : 'hennery'}</span><McpBadge link /></header>}{route.name === 'session' && (scope?.list.counts.waiting ?? 0) > 0 && <div className="session-scope-status"><Link to="/needs-you">Needs you <WaitingBadge /></Link></div>}<main className="main">
      <Suspense fallback={<p role="status">Loading…</p>}>
      {route.name === 'not_found' ? <Placeholder title="Not found" text="No page lives at this address." />
      : !shown ? <Placeholder title={current ? LABEL[current] : 'hennery'} text="This view is not part of this deployment." />
      : route.name === 'sessions' ? <Sessions />
      : route.name === 'needs-you' ? <NeedsYou />
      : route.name === 'more' ? <More views={advertised} />
      : route.name === 'defaults' ? caps.features.includes('session_defaults') ? <Defaults /> : <Placeholder title="Defaults" text="Per-hat session defaults are not part of this deployment." />
      : route.name === 'hosts' ? <Hosts />
      : route.name === 'hats' ? <Hats />
      : route.name === 'settings' ? <Settings push={caps.features.includes('web_push')} sessions={caps.mode === 'full'} />
      : route.name === 'new' ? <NewSessionFrame><NewSession /></NewSessionFrame>
      : route.name === 'mcp' ? caps.features.includes('mcp_connections') ? <Mcp stdio={caps.features.includes('mcp_stdio')} oauth={caps.features.includes('mcp_oauth')} /> : <Placeholder title="MCP" text="The MCP gateway is not part of this deployment." />
      : route.name === 'skills' ? caps.features.includes('skills') ? <Skills hookSets={caps.features.includes('hook_sets')} /> : <Placeholder title="Skills" text="Skills are not part of this deployment." />
      : route.name === 'session' ? <SessionRoute key={route.id} id={route.id ?? ''} /> : null}
      </Suspense>
    </main><BottomNavigation mode={caps.mode} current={current} /></div>
  </div>
}
export default function Shell({ route }: { route: Route }) {
  const client = useClient()
  const [caps, setCaps] = useState<CapabilitiesResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [attempt, setAttempt] = useState(0)
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const refresh = useCallback(() => {
    clearTimeout(refreshTimer.current)
    refreshTimer.current = setTimeout(() => { refreshTimer.current = undefined; setAttempt(n => n + 1) }, COLLECTOR_REFRESH_MS)
  }, [])
  useEffect(() => {
    let live = true; setError(null)
    capabilities(client).then(c => { if (live) setCaps(c) }, err => { if (live && !(err instanceof Error && err.name === 'Unauthenticated')) setError(messageOf(err)) })
    return () => { live = false }
  }, [client, attempt])
  useEffect(() => { if (caps?.mode === 'gateway' && route.name === 'sessions') navigate('/mcp', { replace: true }) }, [caps?.mode, route.name])
  useEffect(() => {
    const visible = () => { if (document.visibilityState === 'visible') refresh() }
    window.addEventListener('online', refresh)
    window.addEventListener('focus', refresh)
    document.addEventListener('visibilitychange', visible)
    return () => {
      window.removeEventListener('online', refresh)
      window.removeEventListener('focus', refresh)
      document.removeEventListener('visibilitychange', visible)
      clearTimeout(refreshTimer.current)
    }
  }, [refresh])
  if (!caps) return <main className="auth"><div className="auth-card" role="status">{error ? <><p className="form-error"><bdi>{error}</bdi></p><button type="button" className="btn btn-ghost" onClick={() => setAttempt(n => n + 1)}>Try again</button></> : 'Loading…'}</div></main>
  const frame = <CollectorVersionContext.Provider value={caps}><Frame caps={caps} route={route} /></CollectorVersionContext.Provider>
  const scoped = caps.mode === 'full' ? <SessionScope route={route} onReconnect={refresh}>{frame}</SessionScope> : frame
  return caps.features.includes('mcp_connections') && caps.features.includes('mcp_oauth') ? <McpScope active={route.name === 'mcp'}>{scoped}</McpScope> : scoped
}
