import { useEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { useClient } from '../app-client'
import { ApiFailure, Unauthenticated } from '../api/errors'
import type { SessionSummary } from '../generated/view'
import type { HostItem } from '../generated/protocol'
import { useBulkClose } from '../hooks/useBulkClose'
import { useSessionSelection } from '../hooks/useSessionSelection'
import { useNow } from '../hooks/useNow'
import { closeTargets, type BulkCloseTarget, type CloseRetryPlan } from '../lib/bulkClose'
import { sessionPath, type RowSelection } from './SessionRow'
import { Link } from '../router'
import ConfirmDialog from './ConfirmDialog'
import { useListSessionActions } from './SessionListActions'

type Confirmation = { scope: string; targets: readonly BulkCloseTarget[]; retry?: CloseRetryPlan; reads?: ReadonlyMap<string, SessionSummary | undefined> }
interface Controls {
  selectButton: ReactNode
  panel: ReactNode
  busy: boolean
  selectionFor: (session: SessionSummary) => RowSelection | undefined
}
const statusLabel = {
  queued: 'Waiting', closing: 'Closing', checking: 'Checking', closed: 'Closed in Hennery', already_closed: 'Already closed',
  pending_unknown: 'Confirmation pending', failed: 'Failed', unavailable: 'Unavailable', not_sent: 'Not sent',
}

/** A stable owner next to T4's menu owner, outside all filtered/paged rows. */
export default function SessionBulkActions({ enabled, scopeKey, sessions, hosts, visibleIds, children }: {
  enabled: boolean; scopeKey: string; sessions: ReadonlyMap<string, SessionSummary>; visibleIds: ReadonlySet<string>
  hosts: readonly HostItem[] | null
  children: (controls: Controls) => ReactNode
}) {
  const client = useClient(), menu = useListSessionActions()
  const selection = useSessionSelection(scopeKey), bulk = useBulkClose()
  const [selecting, setSelecting] = useState(false)
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null)
  const [preparing, setPreparing] = useState(false)
  const [notice, setNotice] = useState('')
  const [preflightNotes, setPreflightNotes] = useState<string[]>([])
  const [preflightRetryAt, setPreflightRetryAt] = useState(0)
  useNow(bulk.retryAt || preflightRetryAt ? 1000 : 60_000)
  const region = useRef<HTMLDivElement>(null), selectButton = useRef<HTMLButtonElement>(null)
  const closeButton = useRef<HTMLButtonElement>(null)
  const pending = useRef<AbortController | null>(null), liveScope = useRef(scopeKey), latest = useRef(sessions)
  liveScope.current = scopeKey; latest.current = sessions
  const previous = useRef(sessions), previousScope = useRef(scopeKey)
  const previousResults = useRef(new Map<string, string>())
  // A later batch replaces the result panel, but cannot erase the need to
  // check a previously submitted ID before closing its current run again.
  const submittedIds = useRef(new Set<string>())
  useEffect(() => {
    const changed = new Map([...sessions].filter(([id, summary]) => previous.current.get(id) !== summary))
    previous.current = sessions
    if (changed.size) { bulk.reconcile(changed); selection.reconcile(changed) }
  }, [sessions])
  useEffect(() => {
    const terminal = bulk.results.filter(result => ['closed', 'already_closed', 'unavailable'].includes(result.status) && !['closed', 'already_closed', 'unavailable'].includes(previousResults.current.get(result.target.id) ?? ''))
    selection.reconcile(new Map(terminal.filter(result => result.status !== 'unavailable').map(result => [result.target.id, { lifecycle: 'closed' }])))
    for (const result of terminal) if (result.status === 'unavailable' && selection.selectedIds.has(result.target.id)) selection.setChecked(result.target, false)
    previousResults.current = new Map(bulk.results.map(result => [result.target.id, result.status]))
  }, [bulk.results])
  useEffect(() => {
    if (previousScope.current !== scopeKey) {
      setNotice('Selection cleared because filters changed.'); setConfirmation(null)
      previousScope.current = scopeKey
    }
    setPreparing(false)
    return () => { pending.current?.abort(); pending.current = null }
  }, [scopeKey])
  useEffect(() => { if (!confirmation && bulk.busy) region.current?.focus() }, [confirmation, bulk.busy])
  const busy = bulk.busy || preparing
  const cooldown = Math.max(0, Math.ceil((Math.max(bulk.retryAt, preflightRetryAt) - Date.now()) / 1000))
  const targetOf = (session: SessionSummary): BulkCloseTarget => ({ id: session.session_id, label: session.title || session.cwd.split('/').filter(Boolean).at(-1) || session.session_id, lifecycle: session.lifecycle })

  const prepare = async () => {
    if (pending.current || busy || !selection.targets.length || cooldown) return
    const abort = new AbortController(), captured = closeTargets(selection.targets), notes: string[] = [], targets: BulkCloseTarget[] = []
    const reads = new Map<string, SessionSummary | undefined>()
    pending.current = abort; setPreparing(true); setPreflightNotes([])
    try {
      for (const chosen of captured) {
        const current = latest.current.get(chosen.id)
        if (current && !submittedIds.current.has(chosen.id)) { targets.push({ ...chosen, lifecycle: current.lifecycle }); continue }
        // A list summary cannot settle an unknown Close. Repeated submissions
        // always need a new GET and confirmation, even after another batch.
        // A missing cursor row or resync baseline cannot establish deletion.
        const beforeRead = current
        try {
          const read = await client.request<{ session_id: string; lifecycle: string }>('GET', `/api/sessions/${encodeURIComponent(chosen.id)}`, undefined, { signal: abort.signal, timeoutMs: 15_000 })
          if (abort.signal.aborted || liveScope.current !== scopeKey) return
          if (beforeRead !== latest.current.get(chosen.id)) {
            notes.push(`${chosen.label}: State changed while checking; review selection again. No Close sent.`)
            continue
          }
          if (read.session_id !== chosen.id || !['starting', 'active', 'parked', 'closed', 'failed'].includes(read.lifecycle)) throw new Error('State unavailable')
          if (read.lifecycle === 'closed') {
            notes.push(`${chosen.label}: Already closed in Hennery; no Close sent.`)
            selection.setChecked(chosen, false)
            continue
          }
          reads.set(chosen.id, beforeRead)
          targets.push({ ...chosen, lifecycle: read.lifecycle })
        } catch (err) {
          if (abort.signal.aborted || liveScope.current !== scopeKey) return
          notes.push(`${chosen.label}: ${err instanceof ApiFailure && err.status === 404 ? 'Unavailable; no Close sent.' : 'Current state could not be read; no Close sent.'}`)
          if (err instanceof Unauthenticated || err instanceof ApiFailure && err.status === 401) { targets.length = 0; break }
          if (err instanceof ApiFailure && err.status === 429) {
            setPreflightRetryAt(Date.now() + (err.retryAfter ?? 60) * 1000)
            targets.length = 0; break
          }
        }
      }
      if (!abort.signal.aborted && liveScope.current === scopeKey) {
        const ready = targets.filter(target => {
          if (!reads.has(target.id) || reads.get(target.id) === latest.current.get(target.id)) return true
          notes.push(`${target.label}: State changed while checking; review selection again. No Close sent.`)
          return false
        })
        setPreflightNotes(notes)
        if (ready.length) setConfirmation({ scope: scopeKey, targets: closeTargets(ready), reads })
      }
    } finally {
      if (pending.current === abort) { pending.current = null; setPreparing(false) }
    }
  }
  const retry = async () => {
    const scope = scopeKey
    const plan = await bulk.prepareRetry()
    if (liveScope.current !== scope) return
    if (plan) setConfirmation({ scope, targets: plan.targets, retry: plan })
    else setNotice('No retry is ready. Check the per-session results and refresh changed states again.')
  }
  const selectedOutside = selection.targets.filter(target => !visibleIds.has(target.id)).length
  const currentConfirmation = confirmation?.scope === scopeKey ? confirmation : null
  const confirmInvalid = !!currentConfirmation && (!!currentConfirmation.retry && bulk.retryPlan !== currentConfirmation.retry
    || currentConfirmation.targets.some(target => currentConfirmation.reads?.has(target.id) && currentConfirmation.reads.get(target.id) !== sessions.get(target.id)))
  const confirmTargets = currentConfirmation?.targets.map(target => {
    const current = sessions.get(target.id)
    return currentConfirmation.retry || currentConfirmation.reads?.has(target.id) || !current ? target : { ...target, lifecycle: current.lifecycle }
  }) ?? []
  const running = confirmTargets.filter(target => sessions.get(target.id)?.activity === 'running' || sessions.get(target.id)?.activity === 'blocked').length
  const offline = confirmTargets.filter(target => hosts?.find(host => host.host_id === sessions.get(target.id)?.host_id)?.connected === false).length
  const controls: Controls = {
    busy: enabled && busy,
    selectionFor: session => enabled && selecting ? { checked: selection.selectedIds.has(session.session_id), disabled: busy, onCheckedChange: checked => { setNotice(''); selection.setChecked(targetOf(session), checked) } } : undefined,
    selectButton: enabled && !selecting ? <button ref={selectButton} type="button" className="btn btn-ghost" onClick={() => { menu?.setOpen(false); setSelecting(true); setNotice('') }}>Select sessions</button> : null,
    panel: enabled && <>
      {selecting && <div className="bulk-toolbar" role="group" aria-label="Selected session actions" ref={region} tabIndex={-1}>
        <strong role="status">{selection.targets.length} selected</strong>
        <button ref={closeButton} type="button" className="btn btn-primary" disabled={busy || !selection.targets.length || !!cooldown} onClick={() => void prepare()}>Close selected</button>
        <button type="button" className="btn btn-ghost" disabled={busy || !selection.targets.length} onClick={selection.clear}>Clear selection</button>
        <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => { selection.clear(); setSelecting(false); requestAnimationFrame(() => selectButton.current?.focus()) }}>Done</button>
        {busy && <span role="status">{preparing ? 'Checking selection…' : 'Working… Leaving stops requests not yet sent.'}</span>}
      </div>}
      {notice && <p className="list-note" role="status">{notice}</p>}
      {selecting && selection.targets.length > 0 && <section className="bulk-selection list-note" aria-label="Selected sessions">
        <p>{selectedOutside} selected outside current rows. Only the sessions named here will be considered.</p>
        <ul>{selection.targets.map(target => <li key={target.id}><bdi>{target.label}</bdi><button className="btn btn-ghost btn-sm" type="button" aria-label={`Remove ${target.label} from selection`} disabled={busy} onClick={() => selection.setChecked(target, false)}>Remove</button></li>)}</ul>
      </section>}
      {!!preflightNotes.length && <ul className="list-note" aria-label="Selection check results">{preflightNotes.map(note => <li key={note}><bdi>{note}</bdi></li>)}</ul>}
      {!!cooldown && <p className="list-note" role="status">Rate limited. Retry remaining in {cooldown}s.</p>}
      {bulk.results.length > 0 && <section className="bulk-results list-note" aria-label="Close results">
        <p role="status">{bulk.results.filter(result => result.status === 'closed' || result.status === 'already_closed').length} / {bulk.results.length} closed in Hennery</p>
        <ul>{bulk.results.map(result => <li key={result.target.id}><Link to={sessionPath(result.target.id)}><bdi>{result.target.label}</bdi></Link><strong>{statusLabel[result.status]}</strong><p><bdi>{result.message}</bdi></p></li>)}</ul>
        {bulk.results.some(result => result.status === 'pending_unknown') && <button className="btn btn-ghost" type="button" disabled={busy || !!cooldown} onClick={() => void bulk.checkUnknown()}>Check status</button>}
        {bulk.results.some(result => result.status === 'failed' || result.status === 'not_sent') && <button className="btn btn-ghost" type="button" disabled={busy || !!cooldown} onClick={() => void retry()}>Review retry</button>}
      </section>}
    </>,
  }
  return <>{children(controls)}{enabled && currentConfirmation && createPortal(<ConfirmDialog title={`Close ${confirmTargets.length} selected ${confirmTargets.length === 1 ? 'session' : 'sessions'}?`} confirm="Close sessions" disabled={confirmInvalid || busy} onClose={() => setConfirmation(null)} returnFocus={() => closeButton.current && !closeButton.current.disabled ? closeButton.current : region.current}
    action={async () => {
      if (confirmInvalid) return
      for (const target of confirmTargets) submittedIds.current.add(target.id)
      if (currentConfirmation.retry) void bulk.confirmRetry(currentConfirmation.retry)
      else void bulk.start(closeTargets(confirmTargets))
    }}>
    <p>Running turns will be interrupted and open questions cancelled. Unsent messages stop sending and remain available as failed. Transcripts are kept.</p>
    <p>Related sessions are kept unless separately selected. Offline hosts may still run a session until they reconnect.</p>
    <p>{running} running or blocked · {confirmTargets.filter(target => target.lifecycle === 'starting').length} starting · {offline} known offline</p>
    <ul aria-label="Sessions to close">{confirmTargets.map(target => <li key={target.id}><bdi>{target.label}</bdi> · <bdi>{target.id}</bdi> · {target.lifecycle}</li>)}</ul>
    {confirmInvalid && <p role="alert">Session state changed. Cancel and review the selected sessions again.</p>}
  </ConfirmDialog>, document.body)}</>
}
