// The signed-in app's frame (frontend spec §2): one component tree for every
// width (F-3). From 768 px a rail with the views beside the content; under
// it a top bar and a bottom tab bar. Which views exist comes from
// `GET /api/capabilities`, which also tells a signed-out browser to sign in.
import { useEffect, useState } from 'react'
import { capabilities } from '../api/auth'
import { messageOf } from '../api/errors'
import { useClient } from '../app-client'
import type { CapabilitiesResponse } from '../generated/protocol'
import { LABEL, PATH, tabsOf, viewOf, viewsOf, type View } from '../lib/views'
import { Icon } from '../lib/ui'
import { Link, type Route } from '../router'
import Hats from '../screens/Hats'
import Hosts from '../screens/Hosts'
import NewSession from '../screens/NewSession'
import Placeholder from '../screens/Placeholder'
import SignOut from './SignOut'
import SessionView from '../screens/Session'
import { useMediaQuery } from '../hooks/useMediaQuery'
import SessionList from './SessionList'
import SessionScope, { WaitingBadge, useSessionScope } from './SessionScope'

/** The rail's width and up (frontend spec §2). */
export const DESKTOP = '(min-width: 768px)'

const ICON: Record<View, (p: { size?: number }) => React.JSX.Element> = {
  sessions: Icon.List,
  new: Icon.Plus,
  hosts: Icon.Cpu,
  mcp: Icon.Wrench,
  hats: Icon.Sparkle,
  settings: Icon.Shield,
}

/** `/sessions/:id`: the session, its header fed by the list store's summary
 *  (kept current by the list stream) when the list holds it. While the
 *  list's first page is on its way the header waits for it; only a session
 *  the list does not hold fetches its detail. The address is the selection
 *  (F-11, F-19): shown whatever the hat, and whether or not the list holds it. */
function SessionRoute({ id }: { id: string }) {
  const list = useSessionScope()?.list
  return <SessionView id={id} summary={list?.all.get(id)} awaitSummary={!!list && list.loading && !list.error} />
}

export default function Shell({ route }: { route: Route }) {
  const client = useClient()
  const [caps, setCaps] = useState<CapabilitiesResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [attempt, setAttempt] = useState(0)
  const desktop = useMediaQuery(DESKTOP)

  useEffect(() => {
    let live = true
    setError(null)
    capabilities(client).then(
      (c) => live && setCaps(c),
      (err) => {
        // A 401 has already sent the browser to sign in.
        if (live && !(err instanceof Error && err.name === 'Unauthenticated')) setError(messageOf(err))
      },
    )
    return () => {
      live = false
    }
  }, [client, attempt])

  if (!caps) {
    return (
      <main className="auth">
        <div className="auth-card" role="status">
          {error ? (
            <>
              <p className="form-error">
                <bdi>{error}</bdi>
              </p>
              <button type="button" className="btn btn-ghost" onClick={() => setAttempt((n) => n + 1)}>
                Try again
              </button>
            </>
          ) : (
            'Loading…'
          )}
        </div>
      </main>
    )
  }

  const views = viewsOf(caps.mode)
  const current = viewOf(route.name)
  const shown = current !== null && views.includes(current)
  const title = route.name === 'not_found' ? 'Not found' : current ? LABEL[current] : 'hennery'

  const frame = (
    <div className="app">
      <aside className="rail" aria-label="Views">
        <div className="rail-head">
          <span className="brand-name">hennery</span>
          <WaitingBadge />
        </div>
        {views.includes('new') && (
          <Link to={PATH.new} className="new-btn">
            <Icon.Plus size={17} /> New session
          </Link>
        )}
        <nav className="rail-nav">
          {views
            .filter((v) => v !== 'new')
            .map((v) => {
              const Glyph = ICON[v]
              return (
                <Link
                  key={v}
                  to={PATH[v]}
                  className={'nav-item' + (v === current ? ' on' : '')}
                  aria-current={v === current ? 'page' : undefined}
                >
                  <Glyph size={17} /> {LABEL[v]}
                </Link>
              )
            })}
        </nav>
        <div className="rail-scroll">{desktop && <SessionList />}</div>
        <div className="rail-foot">
          <SignOut />
        </div>
      </aside>
      <div className="shell">
        <header className="mtopbar">
          <div className="mtb-mid">
            <div className="mtb-eyebrow">hennery</div>
            <div className="mtb-title">{title}</div>
          </div>
          <WaitingBadge />
        </header>
        <main className="main">
          {route.name === 'not_found' ? (
            <Placeholder title="Not found" text="No page lives at this address." />
          ) : !shown ? (
            <Placeholder title={title} text="This view is not part of this deployment." />
          ) : route.name === 'hosts' ? (
            <Hosts />
          ) : route.name === 'hats' ? (
            <Hats />
          ) : route.name === 'sessions' && !desktop ? (
            <SessionList screen />
          ) : route.name === 'new' ? (
            <NewSession />
          ) : route.name === 'session' ? (
            <SessionRoute key={route.id} id={route.id ?? ''} />
          ) : (
            <Placeholder title={title} text="This screen arrives in a later part of the web UI." />
          )}
        </main>
        <nav className="tabbar" aria-label="Tabs">
          {tabsOf(caps.mode).map((v) => {
            const Glyph = ICON[v]
            return (
              <Link
                key={v}
                to={PATH[v]}
                className={'tab' + (v === current ? ' on' : '')}
                aria-current={v === current ? 'page' : undefined}
              >
                <Glyph size={20} />
                <span>{v === 'new' ? 'New' : LABEL[v]}</span>
              </Link>
            )
          })}
        </nav>
      </div>
    </div>
  )
  return views.includes('sessions') ? (
    <SessionScope route={route} desktop={desktop}>
      {frame}
    </SessionScope>
  ) : (
    frame
  )
}
