// One session (frontend spec §6): its header and its transcript, read-only.
//
// - The items come from the item store (`useSessionItems`): the first page,
//   then the stream. The header reads the list's summary when the caller has
//   it, else the session's detail, fetched once.
// - The transcript opens at its end and stays there while the reader is at
//   the end. "Load earlier" (or scrolling to the top) prepends the turns
//   before, keeping what the reader sees where it was.
// - The stream's state shows as "Reconnecting…", and "Resynced" for a moment
//   after the items were replaced.
// - A deleted session says so, and nothing more is fetched.
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { hatList, hostList, namesOf } from '../api/names'
import { sessionDetail } from '../api/view'
import { useClient } from '../app-client'
import SessionHeader, { type HeaderInfo } from '../components/SessionHeader'
import Transcript from '../components/Transcript'
import type { ItemEnv } from '../components/items/types'
import type { SessionSummary } from '../generated/view'
import type { Item } from '../generated/view'
import { useMediaQuery } from '../hooks/useMediaQuery'
import { agentLabel } from '../lib/agent'
import { Icon } from '../lib/ui'
import { Link } from '../router'
import { useSessionItems, type Timing } from '../store/useSessionItems'

/** Within this many pixels of the end, the reader is at the end. */
const STICK_PX = 80
/** Within this many pixels of the top, older turns are fetched. */
const TOP_PX = 120

interface Props {
  id: string
  /** The list store's summary of the session, when it has one. */
  summary?: SessionSummary
  /** The item store's timing (tests). */
  timing?: Partial<Timing>
  /** Seams for later tasks: question actions and "Send again". */
  env?: Pick<ItemEnv, 'questionActions' | 'onSendAgain'>
}

/** The newest plan among the loaded items. */
function latestPlan(items: Item[]): Extract<Item, { kind: 'plan' }> | undefined {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]
    if (item.kind === 'plan') return item
  }
  return undefined
}

/** The session's detail, once, unless the caller has its summary. */
function useHeaderInfo(id: string, summary: SessionSummary | undefined): HeaderInfo | undefined {
  const client = useClient()
  const [detail, setDetail] = useState<HeaderInfo | undefined>(undefined)
  const needed = summary === undefined
  useEffect(() => {
    if (!needed) return
    let live = true
    sessionDetail(client, id).then(
      (d) => live && setDetail(d),
      // The item store says when the session is gone; the header stays bare.
      () => {},
    )
    return () => {
      live = false
    }
  }, [client, id, needed])
  return summary ?? detail
}

/** `id → name` from a list fetched once, when `wanted`; on failure, none. */
function useNames(fetch: 'hosts' | 'hats', wanted: boolean): Map<string, string> {
  const client = useClient()
  const [names, setNames] = useState<Map<string, string>>(() => new Map())
  useEffect(() => {
    if (!wanted) return
    let live = true
    const request = fetch === 'hosts' ? hostList(client) : hatList(client)
    request.then(
      (list) => live && setNames(namesOf(list, fetch === 'hosts' ? 'host_id' : 'id')),
      () => {},
    )
    return () => {
      live = false
    }
  }, [client, fetch, wanted])
  return names
}

export default function SessionView({ id, summary, timing, env: seams }: Props) {
  const s = useSessionItems(id, timing)
  const info = useHeaderInfo(id, summary)
  const narrow = useMediaQuery('(max-width: 767px)')
  const hosts = useNames('hosts', true)
  const wantsHats = useMemo(() => s.items.some((i) => i.kind === 'marker' && i.marker === 'hat_reassigned'), [s.items])
  const hats = useNames('hats', wantsHats)
  const plan = useMemo(() => latestPlan(s.items), [s.items])

  const env: ItemEnv = useMemo(
    () => ({
      sessionId: id,
      agent: agentLabel(info?.agent),
      hatName: (hat: string) => hats.get(hat),
      ...seams,
    }),
    [id, info?.agent, hats, seams],
  )

  if (s.removed) {
    return (
      <div className="session">
        <div className="welcome" role="status">
          <h1>This session was deleted</h1>
          <p>
            <bdi>{id}</bdi>
          </p>
          <Link to="/sessions" className="btn btn-ghost">
            Back to sessions
          </Link>
        </div>
      </div>
    )
  }

  return (
    <div className="session">
      <SessionHeader id={id} info={info} hostName={info ? hosts.get(info.host_id) : undefined} plan={plan} narrow={narrow} />
      <div className="stream-state" role="status">
        {s.stream === 'reconnecting' ? 'Reconnecting…' : s.resynced ? 'Resynced' : ''}
      </div>
      {s.error && (
        <p className="form-error session-error">
          <bdi>{s.error}</bdi>
        </p>
      )}
      <Scroller items={s.items} older={s.older} loadingOlder={s.loadingOlder} loadOlder={s.loadOlder}>
        {s.loading ? <p className="transcript-empty">Loading…</p> : <Transcript items={s.items} env={env} />}
      </Scroller>
    </div>
  )
}

interface ScrollerProps {
  items: Item[]
  older: boolean
  loadingOlder: boolean
  loadOlder: () => Promise<void>
  children: React.ReactNode
}

/** The transcript's scrolling box: opens at the end, follows the end while
 *  the reader is there, and keeps the reader's place when older turns are
 *  prepended. */
function Scroller({ items, older, loadingOlder, loadOlder, children }: ScrollerProps) {
  const ref = useRef<HTMLDivElement>(null)
  const atEnd = useRef(true)
  const before = useRef<{ first?: string; height: number }>({ height: 0 })

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const first = items[0]?.id
    const was = before.current
    const prepended = was.first !== undefined && first !== was.first && items.some((i) => i.id === was.first)
    if (prepended) el.scrollTop += el.scrollHeight - was.height
    else if (atEnd.current) el.scrollTop = el.scrollHeight
    before.current = { first, height: el.scrollHeight }
  }, [items])

  const onScroll = () => {
    const el = ref.current
    if (!el) return
    atEnd.current = el.scrollTop + el.clientHeight >= el.scrollHeight - STICK_PX
    if (el.scrollTop < TOP_PX && older && !loadingOlder) void loadOlder()
  }

  return (
    <div className="transcript" ref={ref} onScroll={onScroll}>
      <div className="transcript-inner">
        {older && (
          <div className="load-earlier">
            <button type="button" className="btn btn-ghost btn-sm" disabled={loadingOlder} onClick={() => void loadOlder()}>
              {loadingOlder ? (
                'Loading…'
              ) : (
                <>
                  <Icon.ArrowUp size={14} /> Load earlier
                </>
              )}
            </button>
          </div>
        )}
        {children}
      </div>
    </div>
  )
}
