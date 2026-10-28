import { useEffect, useRef, useState } from 'react'
import { statusOf } from '../lib/status'
import { relTime } from '../lib/time'
import { useNow } from '../hooks/useNow'
import { Link } from '../router'
import { agentLabel } from '../lib/agent'
import { useSessionScope } from './SessionScope'
import AttentionPreview, { AttentionPreviews } from './AttentionPreview'
import { moveListFocus } from './SessionList'
import { HatChip } from './ui'
import SessionName, { useSessionNames } from './SessionNames'
export default function NeedsYouQueue({ embedded = false }: { embedded?: boolean }) {
  const scope = useSessionScope(), now = useNow(60_000)
  const scan = useRef({ key: '', remaining: 2 })
  const list = scope?.list
  const names = useSessionNames(scope?.attentionRows ?? [])
  const [showDismissed, setShowDismissed] = useState(false)
  useEffect(() => setShowDismissed(false), [scope?.hat])
  useEffect(() => {
    if (!list) return
    if (scan.current.key !== list.queryKey) scan.current = { key: list.queryKey, remaining: 2 }
    if (list.loading || list.loadingMore || list.error || !list.hasMore || scan.current.remaining === 0) return
    scan.current.remaining--
    void list.loadMore()
  }, [list?.queryKey, list?.loading, list?.loadingMore, list?.hasMore, list?.error, list?.loadMore])
  if (!scope || !list) return null
  const rows = scope.attentionRows
  const dismissedCount = rows.filter(row => scope.isAttentionDismissed(row.session_id)).length
  const shownCount = rows.length - dismissedCount
  const total = list.counts.waiting
  const complete = !list.loading && !list.hasMore && !list.error
  return <section className={'needs-queue' + (embedded ? ' queue-embedded' : '')} aria-label="Attention queue" onKeyDown={moveListFocus}>
    {embedded && <h2 className="queue-title">Needs you <span>Requests and alerts that need your attention</span></h2>}
    <div className="queue-heading"><span>{list.loading ? 'Checking attention…' : complete ? `${total} ${total === 1 ? 'session needs' : 'sessions need'} you` : `${total} sessions needing you shown`}</span><span>Most recent first</span></div>
    {list.error && <p role="alert" className="form-error"><bdi>{list.error}</bdi></p>}
    {list.stream === 'reconnecting' && <p role="status">Reconnecting…</p>}
    {list.loading && <p role="status">Loading attention…</p>}
    {!list.loading && !shownCount && <p className="queue-empty">{complete ? 'Nothing needs you' : 'No requests to show yet'}</p>}
    <div className="attention-grid"><AttentionPreviews>{rows.map(session => {
      const hat = scope.hats?.find(h => h.id === session.hat_id)
      const dismissed = scope.isAttentionDismissed(session.session_id)
      return <article key={session.session_id} className={'attention-card' + (dismissed ? ' attention-dismissed' : '') + (session.unseen ? ' session-unseen' : '')} hidden={dismissed && !showDismissed} aria-label={names(session).text}>
        <div className="attention-card-meta"><span className="attention-status">{statusOf(session).label}</span>{session.lifecycle === 'closed' && <span className="attention-closed">Closed</span>}<span>{relTime(session.last_event_at, now)}</span></div>
        <h2><SessionName session={session} /></h2>
        <div className="attention-origin"><HatChip colour={hat?.colour}><bdi>{hat?.name || 'Unknown hat'}</bdi></HatChip><bdi>{scope.hostNames.get(session.host_id) || 'Unknown host'}</bdi><bdi>{agentLabel(session.agent)}</bdi></div>
        {session.family?.parent && <p className="attention-parent">Child of <SessionName session={session.family.parent} /></p>}
        <AttentionPreview session={session} />
      </article>
    })}</AttentionPreviews></div>
    {!list.loading && list.hasMore && <button type="button" className="btn btn-ghost" disabled={list.loadingMore} onClick={() => { scan.current.remaining = 2; void list.loadMore() }}>{list.loadingMore ? 'Loading…' : 'Load more'}</button>}
    {dismissedCount > 0 && <button type="button" className="btn btn-ghost queue-dismissed-toggle" aria-expanded={showDismissed} onClick={() => setShowDismissed(value => !value)}>{showDismissed ? 'Hide' : 'Show'} {dismissedCount} dismissed</button>}
    {shownCount > 0 && <p className="queue-help queue-keys">J / K moves between focused cards. Enter opens the current request.</p>}
    {!embedded && <Link className="queue-summary" to="/sessions">View sessions · {list.shown.filter(s => s.lifecycle === 'active' && s.activity === 'running').length} running · {list.shown.filter(s => s.lifecycle === 'active' && s.activity === 'idle').length} idle · {list.shown.filter(s => s.lifecycle === 'parked').length} parked{!complete ? ' (more sessions to check)' : ''}</Link>}
  </section>
}
