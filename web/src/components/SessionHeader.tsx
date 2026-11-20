// A session's header (frontend spec §6.2): its title, agent, host, branch,
// where it stands in words, model and mode, and the newest step list. On a
// phone it carries the way back to the list, and the steps start closed.
// The session's menu (Park, Close, Delete) sits at the end of its title row.
import type { SessionItem, SessionDetail, McpSessionDelivery } from '../generated/protocol'
import { useRef, useState, type ReactNode, type RefObject } from 'react'
import type { FamilySummary, PlanEntry, ReviewResult } from '../generated/view'
import { agentLabel } from '../lib/agent'
import { statusOf } from '../lib/status'
import { basename } from '../lib/time'
import { Icon } from '../lib/ui'
import { Link } from '../router'
import { useSessionScope, WaitingBadge } from './SessionScope'
import { Icon as Glyph } from './ui'
import { sessionPath } from './SessionRow'
import { reasonWords } from './items/Marker'
import SessionName, { useSessionName } from './SessionNames'
import RenameSession from './RenameSession'
import BackgroundTasks from './BackgroundTasks'

/** What the header reads: a list summary or a session's detail. */
export type HeaderInfo = SessionItem & { question_waits?: boolean; operator_attention?: number; review?: ReviewResult; parent_handled_questions?: number; family?: FamilySummary }

export function titleOf(info: HeaderInfo | undefined, id: string): string {
  return info?.title || basename(info?.cwd ?? '') || id
}

interface Props {
  renaming?: boolean
  onRename?: () => void
  finishRename?: () => void
  id: string
  info?: HeaderInfo
  hostName?: string
  plan?: { entries: PlanEntry[]; truncated?: boolean }
  /** Under 768 px: the steps start closed. */
  narrow: boolean
  /** The session's menu, at the end of the title row. */
  menu?: ReactNode
  detailsOpen?: boolean
  toggleDetails?: () => void
  openFamily?: () => void
  detailsTrigger?: RefObject<HTMLButtonElement | null>
  hatName?: string
  hostConnected?: boolean
  delivery?: McpSessionDelivery
}

export default function SessionHeader({ id, info, hostName, menu, detailsOpen, toggleDetails, openFamily, detailsTrigger, hatName, hostConnected, delivery, narrow, renaming, onRename, finishRename }: Props) {
  const titleButton = useRef<HTMLButtonElement>(null)
  const [savedName, setSavedName] = useState<{ before: HeaderInfo | undefined; session: SessionDetail } | null>(null)
  const named = savedName && savedName.before === info ? { ...info, title: savedName.session.title, title_source: savedName.session.title_source } : info
  const titleName = useSessionName({ ...named, session_id: id })
  const finish = () => { finishRename?.(); requestAnimationFrame(() => titleButton.current?.focus()) }
  const scope = useSessionScope()
  // Share the queue's recent-first summary order, never its overview filters.
  const waiting = (scope?.attentionRows ?? []).filter(s => !scope?.isAttentionDismissed(s.session_id))
  const position = waiting.findIndex(s => s.session_id === id)
  const complete = scope !== null && !scope.list.loading && !scope.list.hasMore && !scope.list.error && !scope.query && !scope.lifecycle && !scope.hideClosed
  const previous = position > 0 ? waiting[position - 1] : undefined
  const next = position >= 0 ? waiting[position + 1] : undefined
  // The list's marker, word for word (plan 4c decision 8): the header and
  // the row never disagree.
  const status = info
    ? statusOf({
        operator_attention: info.operator_attention,
        review: info.review,
        parent_handled_questions: info.parent_handled_questions,
        family: info.family,
        lifecycle: info.lifecycle,
        activity: info.activity,
        background: info.background,
        question_waits: !!info.question_waits,
        presumed_parked: !!info.presumed_parked,
      })
    : null
  return (
    <>
    <header className="conv-head session-head">
      <div className="conv-title-row">
        <Link to="/sessions" className="back-btn" aria-label="Back to sessions">
          <Icon.Chevron size={18} className="rot-180" />
        </Link>
        {hatName && <span className="session-hat"><span aria-hidden="true" /><bdi>{hatName}</bdi></span>}
        {renaming ? <RenameSession key={id} id={id} title={named?.title ?? ''} canReset={named?.title_source === 'operator'} saved={session => { setSavedName({ before: info, session }); finish() }} cancel={finish} /> : <h1 className="conv-name" aria-label={titleName.text}>{onRename && info ? <button ref={titleButton} type="button" className="session-title-button" aria-label="Rename session" title="Rename session" onClick={onRename}><SessionName session={{ ...named, session_id: id }} /></button> : <SessionName session={{ ...named, session_id: id }} />}</h1>}
        {status && (
          <span className={'badge badge-' + status.tone}>
            {status.tone === 'background' ? <span className="background-ring" aria-hidden="true" /> : <Glyph name={status.tone === 'attn' ? 'hand-raised' : status.tone === 'run' ? 'play-circle' : status.tone === 'fail' ? 'exclamation-circle' : 'clock'} size={14} />}
            {status.label}
          </span>
        )}
        {info && <BackgroundTasks value={info.background} stale={!!info.presumed_parked || hostConnected === false} />}
        {position >= 0 && <nav className="waiting-navigation" aria-label="Waiting sessions">
          <span>{position + 1} / {waiting.length}{complete ? ' waiting' : ' loaded waiting'}</span>
          {previous ? <Link to={sessionPath(previous.session_id)} aria-label="Previous waiting session">↑</Link> : <button type="button" aria-label="Previous waiting session" disabled>↑</button>}
          {next ? <Link to={sessionPath(next.session_id)} aria-label="Next waiting session">↓</Link> : <button type="button" aria-label="Next waiting session" disabled>↓</button>}
        </nav>}
        {toggleDetails && <button ref={detailsTrigger} className="btn btn-ghost btn-sm session-details-toggle" type="button" aria-label="Session details" aria-pressed={detailsOpen} onClick={toggleDetails}><Glyph name="view-columns" size={16} /><span>Details</span>{narrow && <WaitingBadge className="session-wait-count" />}</button>}
        {menu}
      </div>
      {info && (
        <div className="conv-meta">
          {narrow && hatName && <span className="session-mobile-hat"><bdi>{hatName}</bdi></span>}
          <span className="meta-pill" aria-label="Agent">
            {agentLabel(info.agent)}
          </span>
          {hostName && (
            <span className="meta-pill" aria-label="Host">
              <Icon.Cpu size={12} /> <bdi>{hostName}</bdi>{hostConnected !== undefined && <span className={hostConnected ? 'host-connected' : 'host-disconnected'}>{hostConnected ? 'Online' : 'Offline'}</span>}
            </span>
          )}
          <span className="meta-pill meta-cwd" title={info.cwd} aria-label="Project"><bdi>{info.cwd}</bdi></span>
          {info.git_branch && (
            <span className="meta-pill" aria-label="Branch">
              <Icon.Branch size={12} /> <bdi>{info.git_branch}</bdi>
              {info.git_dirty && <b className="meta-dirty"> (changed)</b>}
            </span>
          )}
          {info.model && (
            <span className="meta-pill" aria-label="Model">
              <bdi>{info.model}</bdi>
            </span>
          )}
          {info.mode && (
            <span className="meta-pill" aria-label="Mode">
              <bdi>{info.mode}</bdi>
            </span>
          )}
          {info.acp_protocol && <span className="meta-pill" title={info.acp_fallback || undefined}>ACP v{info.acp_protocol}{info.acp_fallback ? ` · ${info.acp_fallback}` : ''}</span>}
          {delivery && <span className="meta-pill">{delivery.servers} MCP servers</span>}
          {info.family && (info.family.parent || info.family.child_counts.total > 0) && <button type="button" className="meta-toggle" onClick={openFamily}>Family{info.family.child_counts.total > 0 ? ` · ${info.family.child_counts.total}` : ''}</button>}
          {info.lifecycle === 'failed' && info.failure_reason && (
            <span className="meta-item">
              Reason: <bdi>{reasonWords('start_failed', info.failure_reason)}</bdi>
            </span>
          )}
        </div>
      )}
    </header>
    </>
  )
}
