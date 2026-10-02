// The app: the route decides between setup, login and the signed-in shell;
// the step-up dialog opens over any of them when a request needs it.
import { useEffect, useMemo, useRef, useState } from 'react'
import { StepUpCancelled } from './api/errors'
import { ClientContext, browserClient } from './app-client'
import Shell from './components/Shell'
import StepUpDialog from './components/StepUpDialog'
import { match, navigate, useLocation } from './router'
import Login from './screens/Login'
import Setup from './screens/Setup'

interface Waiting {
  resolve: () => void
  reject: (err: Error) => void
}

export default function App({ fetchImpl }: { fetchImpl?: typeof fetch }) {
  const [waiting, setWaiting] = useState<Waiting | null>(null)
  const open = useRef<() => Promise<void>>(() => Promise.reject(new StepUpCancelled()))
  open.current = () => new Promise<void>((resolve, reject) => setWaiting({ resolve, reject }))
  const client = useMemo(() => browserClient(() => open.current(), fetchImpl), [fetchImpl])
  const { pathname } = useLocation()
  const route = match(pathname)

  useEffect(() => {
    if (pathname === '/') navigate('/sessions', { replace: true })
  }, [pathname])

  const close = (outcome: 'done' | 'cancelled') => {
    if (!waiting) return
    if (outcome === 'done') waiting.resolve()
    else waiting.reject(new StepUpCancelled())
    setWaiting(null)
  }

  return (
    <ClientContext.Provider value={client}>
      {route.name === 'setup' ? <Setup /> : route.name === 'login' ? <Login /> : <Shell route={route} />}
      {waiting && <StepUpDialog onDone={() => close('done')} onCancel={() => close('cancelled')} />}
    </ClientContext.Provider>
  )
}
