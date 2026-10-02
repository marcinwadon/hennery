// The session list (frontend spec §5; plan 4c decisions 6–12): the hat
// selector, a search box, Hide closed and a lifecycle filter, then the rows
// newest first under sticky day headings. In the rail from 768 px; the
// whole screen at `/sessions` under it.
//
// - A search runs on the server across every lifecycle: while it is active
//   Hide closed and the lifecycle filter are set aside, and only the hat
//   applies (F-10).
// - Rows render 50 at a time; past the rows held, "Load more" fetches the
//   next page by `next_cursor`.
import { useState } from 'react'
import type { SessionSummary } from '../generated/view'
import { BUCKET_LABEL, BUCKET_ORDER, bucketOf, type Bucket } from '../lib/time'
import { Icon } from '../lib/ui'
import { LIFECYCLES } from '../store/sessionList'
import HatSwitch from './HatSwitch'
import SessionRow from './SessionRow'
import { useSessionScope } from './SessionScope'

export const PAGE_ROWS = 50

const LIFECYCLE_LABEL: Record<(typeof LIFECYCLES)[number], string> = {
  starting: 'Starting',
  active: 'Active',
  parked: 'Parked',
  closed: 'Closed',
  failed: 'Failed',
}

/** `rows` (newest first) under their day headings, in display order; an
 *  empty day has no heading. */
export function byDay(rows: readonly SessionSummary[], now: number = Date.now()): [Bucket, SessionSummary[]][] {
  const days = new Map<Bucket, SessionSummary[]>()
  for (const s of rows) {
    const b = bucketOf(s.last_event_at, now)
    const list = days.get(b)
    if (list) list.push(s)
    else days.set(b, [s])
  }
  return BUCKET_ORDER.filter((b) => days.has(b)).map((b) => [b, days.get(b)!])
}

export default function SessionList({ screen = false }: { screen?: boolean }) {
  const scope = useSessionScope()
  const [limit, setLimit] = useState(PAGE_ROWS)
  if (!scope) return null
  const { list, hats, hat, chooseHat, hostNames, hideClosed, setHideClosed, lifecycle, setLifecycle, query, setQuery, selectedId } =
    scope
  const searching = list.searching
  const rows = list.shown.slice(0, limit)
  const more = list.shown.length > limit || list.hasMore
  const showMore = () => {
    if (list.shown.length > limit) setLimit((n) => n + PAGE_ROWS)
    else {
      setLimit((n) => n + PAGE_ROWS)
      void list.loadMore()
    }
  }

  return (
    <section className={'slist' + (screen ? ' slist-screen' : '')} aria-label="Session list">
      {screen && <h1 className="slist-title">Sessions</h1>}
      <HatSwitch hats={hats} hat={hat} onHat={chooseHat} />
      <div className="rail-search">
        <Icon.Search size={15} />
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search sessions…"
          aria-label="Search sessions"
        />
      </div>
      <div className="list-filters">
        <button
          type="button"
          className={'toggle' + (hideClosed && !searching ? ' on' : '')}
          aria-pressed={hideClosed}
          disabled={searching}
          onClick={() => setHideClosed(!hideClosed)}
        >
          Hide closed
        </button>
        <select
          className="list-select"
          aria-label="Lifecycle"
          value={lifecycle}
          disabled={searching}
          onChange={(e) => setLifecycle(e.target.value)}
        >
          <option value="">Any lifecycle</option>
          {LIFECYCLES.map((l) => (
            <option key={l} value={l}>
              {LIFECYCLE_LABEL[l]}
            </option>
          ))}
        </select>
      </div>
      {searching && <p className="list-note">Searching every session in this hat.</p>}
      {list.stream === 'reconnecting' && !list.loading && (
        <p className="list-note" role="status">
          Reconnecting…
        </p>
      )}
      {list.error && (
        <p className="form-error list-note" role="alert">
          <bdi>{list.error}</bdi>
        </p>
      )}
      {list.loading ? (
        <p className="list-note" role="status">
          Loading…
        </p>
      ) : rows.length === 0 ? (
        <p className="list-note">{searching ? 'No matches' : 'No sessions'}</p>
      ) : (
        byDay(rows).map(([bucket, day]) => (
          <div className="grp" key={bucket}>
            <h2 className="grp-head">
              <span className="grp-title">{BUCKET_LABEL[bucket]}</span>
              <span className="grp-count">{day.length}</span>
            </h2>
            {day.map((s) => (
              <SessionRow
                key={s.session_id}
                session={s}
                selected={s.session_id === selectedId}
                hostName={hostNames.get(s.host_id)}
              />
            ))}
          </div>
        ))
      )}
      {!list.loading && more && (
        <button type="button" className="btn btn-ghost btn-sm list-more" disabled={list.loadingMore} onClick={showMore}>
          {list.loadingMore ? 'Loading…' : 'Load more'}
        </button>
      )}
    </section>
  )
}
