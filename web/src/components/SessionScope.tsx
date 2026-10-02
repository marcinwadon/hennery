// The session list's state for the whole signed-in app (frontend spec §5;
// plan 4c decisions 8–11): one list store and one list stream, the filters,
// the hats and the host names. The rail, the mobile list screen and the
// session view's header all read it through `useSessionScope()`.
//
// It also owns what the list decides on its own:
// - the tab title `(N) hennery`, N = the hat's sessions waiting on a question;
// - selection (F-11): the route's id is the selection, honoured even
//   outside the hat; a hat switch clears it; on a desktop, `/sessions` with no id opens the newest VISIBLE row.
import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'
import { useClient } from '../app-client'
import type { HatItem, HostItem } from '../generated/protocol'
import { usePersistentToggle } from '../hooks/usePersistentToggle'
import { readHat, themeOf, writeHat } from '../lib/hats'
import { saveTheme } from '../lib/theme'
import { navigate, type Route } from '../router'
import { useSessionList, type SessionList } from '../store/useSessionList'

export const HIDE_CLOSED_KEY = 'hennery.hideClosed'
/** How long typing rests before a search goes to the server. */
export const SEARCH_DEBOUNCE_MS = 250

export interface SessionScopeValue {
  list: SessionList
  /** The hats, or `null` until (or unless) they load. */
  hats: HatItem[] | null
  /** The selected hat's id, `''` for every hat. */
  hat: string
  chooseHat: (hat: string) => void
  /** Host names by host id, for the rows' second line. */
  hostNames: ReadonlyMap<string, string>
  hideClosed: boolean
  setHideClosed: (on: boolean) => void
  /** One lifecycle, or `''` for any. */
  lifecycle: string
  setLifecycle: (lifecycle: string) => void
  /** The search box as typed (the server gets it after a pause). */
  query: string
  setQuery: (q: string) => void
  /** The route's session id: the selection. */
  selectedId: string | null
}

const ScopeContext = createContext<SessionScopeValue | null>(null)

/** The session list's state, or `null` outside a deployment with sessions. */
export function useSessionScope(): SessionScopeValue | null {
  return useContext(ScopeContext)
}

function useDebounced(value: string, ms: number): string {
  const [settled, setSettled] = useState(value)
  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), ms)
    return () => clearTimeout(timer)
  }, [value, ms])
  return settled
}

export function titleOf(waiting: number): string {
  return waiting > 0 ? `(${waiting}) hennery` : 'hennery'
}

export default function SessionScope({ route, desktop, children }: { route: Route; desktop: boolean; children: ReactNode }) {
  const client = useClient()
  const [hat, setHat] = useState(readHat)
  const [hats, setHats] = useState<HatItem[] | null>(null)
  const [hostNames, setHostNames] = useState<ReadonlyMap<string, string>>(new Map())
  const [hideClosed, setHideClosed] = usePersistentToggle(HIDE_CLOSED_KEY, true)
  const [lifecycle, setLifecycle] = useState('')
  const [query, setQuery] = useState('')
  const q = useDebounced(query, SEARCH_DEBOUNCE_MS)
  const list = useSessionList({ hat: hat || null, hideClosed, lifecycle: lifecycle ? [lifecycle] : undefined, q })
  const selectedId = route.name === 'session' && route.id !== undefined ? route.id : null

  // Hats and hosts: once, on mount. No event refetches them.
  useEffect(() => {
    let live = true
    client.request<HatItem[]>('GET', '/api/hats').then(
      (items) => live && Array.isArray(items) && setHats(items),
      () => {
        // Without the hats the switch offers every hat only, and the
        // colours stay as saved.
      },
    )
    client.request<HostItem[]>('GET', '/api/hosts').then(
      (items) => {
        if (!live || !Array.isArray(items)) return
        setHostNames(new Map(items.map((h) => [h.host_id, h.name])))
      },
      () => {
        // The rows show no host name.
      },
    )
    return () => {
      live = false
    }
  }, [client])

  // A remembered hat that no longer exists falls back to every hat.
  useEffect(() => {
    if (hats && hat !== '' && !hats.some((h) => h.id === hat)) {
      setHat('')
      writeHat('')
    }
  }, [hats, hat])

  // The hat's colour, once the hats are known (never before: that would
  // wipe the saved colours on every load).
  useEffect(() => {
    if (!hats) return
    saveTheme(themeOf(hats.find((h) => h.id === hat)?.colour))
  }, [hats, hat])

  // The hat's count, kept through a search or a lifecycle filter: the
  // server's, or the rows' that the list store carries (`waitingCount`).
  const waiting = list.counts.waiting
  useEffect(() => {
    document.title = titleOf(waiting)
  }, [waiting])
  useEffect(() => () => void (document.title = 'hennery'), [])

  // A hat switch clears the selection, whatever hat the session is in: back
  // to `/sessions`, where a desktop then opens the new hat's newest row.
  const chooseHat = useCallback(
    (next: string) => {
      if (next === hat) return
      if (selectedId !== null) navigate('/sessions')
      setHat(next)
      writeHat(next)
    },
    [hat, selectedId],
  )

  // Desktop auto-select: only on `/sessions` with no id, only a visible row.
  const first = list.shown[0]?.session_id
  useEffect(() => {
    if (!desktop || route.name !== 'sessions' || first === undefined) return
    navigate(`/sessions/${encodeURIComponent(first)}`, { replace: true })
  }, [desktop, route.name, first])

  const value = useMemo<SessionScopeValue>(
    () => ({
      list,
      hats,
      hat,
      chooseHat,
      hostNames,
      hideClosed,
      setHideClosed,
      lifecycle,
      setLifecycle,
      query,
      setQuery,
      selectedId,
    }),
    [list, hats, hat, chooseHat, hostNames, hideClosed, setHideClosed, lifecycle, query, selectedId],
  )
  return <ScopeContext.Provider value={value}>{children}</ScopeContext.Provider>
}

/** The rail's and the top bar's count of sessions waiting on a question. */
export function WaitingBadge() {
  const scope = useSessionScope()
  const n = scope?.list.counts.waiting ?? 0
  if (n === 0) return null
  const text = `${n} waiting on a question`
  return (
    <span className="badge badge-attn" role="status" aria-label={text} title={text}>
      {n}
    </span>
  )
}
