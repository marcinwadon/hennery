// The app: the route decides between setup, login and the signed-in shell;
// the step-up dialog opens over any of them when a request needs it.
import { lazy, Suspense, useEffect, useMemo, useRef, useState } from 'react'
import { StepUpCancelled } from './api/errors'
import { ClientContext, browserClient } from './app-client'
import Shell from './components/Shell'
import StepUpDialog from './components/StepUpDialog'
import { match, navigate, useLocation } from './router'
import Login from './screens/Login'
import Setup from './screens/Setup'
import PwaStatus from './components/PwaStatus'
import { completeLogout } from './pwa/logout'
import NotificationNavigation from './components/NotificationNavigation'
import { AppActionsProvider } from './app-actions'
const CommandHost = lazy(() => import('./components/CommandHost'))
const ExternalMcp = lazy(() => import('./screens/ExternalMcp'))

interface Waiting {
  resolve: () => void
  reject: (err: Error) => void
}

export default function App({ fetchImpl }: { fetchImpl?: typeof fetch }) {
  const [waiting, setWaiting] = useState<Waiting | null>(null)
  const focusBeforeStepUp = useRef<HTMLElement | null>(null)
  const open = useRef<() => Promise<void>>(() => Promise.reject(new StepUpCancelled()))
  open.current = () => new Promise<void>((resolve, reject) => {
    // Capture before `inert` blurs the underlying control in real browsers.
    focusBeforeStepUp.current = document.activeElement as HTMLElement | null
    setWaiting({ resolve, reject })
  })
  const client = useMemo(() => browserClient(() => open.current(), fetchImpl), [fetchImpl])
  const { pathname } = useLocation()
  const route = match(pathname)

  useEffect(() => {
    if (pathname === '/') navigate('/sessions', { replace: true })
  }, [pathname])
  useEffect(() => {
    if (typeof BroadcastChannel === 'undefined') return
    const channel = new BroadcastChannel('hennery-auth')
    channel.onmessage = (event: MessageEvent<unknown>) => {
      const data = event.data
      if (data && typeof data === 'object' && 'version' in data && data.version === 1 && 'kind' in data && data.kind === 'logout') void completeLogout(client, false)
    }
    return () => channel.close()
  }, [client])

  const close = (outcome: 'done' | 'cancelled') => {
    if (!waiting) return
    if (outcome === 'done') waiting.resolve()
    else waiting.reject(new StepUpCancelled())
    setWaiting(null)
  }

  return (
    <ClientContext.Provider value={client}>
      <AppActionsProvider>
      <div className="application-root">
      <PwaStatus />
      <NotificationNavigation />
      {/* While the step-up dialog is open, nothing under it can be focused
          or clicked. */}
      <div className="page" inert={waiting !== null} style={{ display: 'contents' }}>
        {route.name === 'setup' ? <Setup /> : route.name === 'login' ? <Login />
          : route.name === 'external_mcp' || route.name === 'external_mcp_consent'
            ? <Suspense fallback={<p role="status">Loading…</p>}><ExternalMcp consent={route.name === 'external_mcp_consent'} /></Suspense>
            : <Shell route={route} />}
      </div>
      {waiting && <StepUpDialog returnTo={focusBeforeStepUp.current} onDone={() => close('done')} onCancel={() => close('cancelled')} />}
      <Suspense fallback={null}><CommandHost /></Suspense>
      </div>
      </AppActionsProvider>
    </ClientContext.Provider>
  )
}
