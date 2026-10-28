// One row of the session list (frontend spec §5; plan 4c decision 7).
//
// Line 1: status marker, agent, title (fallback: the project directory's
// name), relative time. Line 2: the branch (fallback: the directory's name,
// never repeated when it is the title), the host's name, and "Resume" for a
// parked session. The row is a link to the session, so a new tab works.
// A failed session's reason is on the marker's hover, and a tap on "Why"
// beside the link shows it (a button inside the link would open the session).
import { memo, useState } from 'react'
import type { SessionSummary } from '../generated/view'
import { agentLabel, modelLabel } from '../lib/agent'
import { statusOf } from '../lib/status'
import { basename, relTime } from '../lib/time'
import { Link } from '../router'
import { Icon } from './ui'
import { useSessionName } from './SessionNames'
import SessionRowMenu from './SessionRowMenu'
import { useListSessionActions } from './SessionListActions'

export function sessionPath(id: string): string {
  return `/sessions/${encodeURIComponent(id)}`
}

/** The row's title and its second line's place. */
export function rowText(s: SessionSummary): { title: string; where: string } {
  const dir = basename(s.cwd)
  const title = s.title || dir || s.session_id
  const where = s.git_branch || dir
  return { title, where: where === title ? '' : where }
}

interface Props {
  session: SessionSummary
  selected: boolean
  hostName?: string
  /** The list's clock (`useNow`): the relative time moves with it. */
  now: number
  table?: boolean
  indent?: boolean
  childrenCount?: number
  childrenStatus?: string
  expanded?: boolean
  onToggleChildren?: (id: string) => void
}

function SessionRow({ session, selected, hostName, now, table, indent, childrenCount = 0, childrenStatus, expanded = false, onToggleChildren }: Props) {
  const [why, setWhy] = useState(false)
  const actions = useListSessionActions()
  const status = statusOf(session)
  const name = useSessionName(session)
  const title = name.text, place = session.git_branch || basename(session.cwd)
  const where = place === title ? '' : place
  const reason = status.tone === 'fail' ? session.failure_reason : undefined
  const markerTitle = reason ? `${status.label}: ${reason}` : status.label
  const content = (
    <div className={'sess-item' + (indent ? ' sess-child' : '')} onContextMenu={event => {
      const trigger = event.currentTarget.querySelector<HTMLButtonElement>('.session-row-actions button')
      if (actions && trigger) { event.preventDefault(); actions.show(session, trigger) }
    }}>
      <Link
        to={sessionPath(session.session_id)}
        data-session-link
        className={'sess' + (selected ? ' active' : '') + (session.unseen ? ' session-unseen' : '')}
        aria-current={selected ? 'page' : undefined}
      >
        <div className="sess-row1">
          {indent && <span className="sess-branch" aria-hidden="true">↳</span>}
          <span className={'sess-led led-' + status.tone} role="img" aria-label={status.label} title={markerTitle}><Icon name={status.tone === 'attn' ? 'hand-raised' : status.tone === 'run' ? 'play-circle' : status.tone === 'fail' ? 'exclamation-circle' : 'clock'} size={14} /></span>
          <span className="sess-kind sess-agent">{agentLabel(session.agent)}</span>
          <span className="sess-name">
            {session.unseen && <span className="unseen-dot" role="img" aria-label="Unseen" title="New activity you have not seen" />}
            <bdi className={name.provisional ? 'session-name-provisional' : undefined}>{title}</bdi>
          </span>
          <span className="sess-time">{relTime(session.last_event_at, now)}</span>
        </div>
        <div className="sess-row2">
          {where && (
            <span className="sess-where">
              <bdi>{where}</bdi>
            </span>
          )}
          {hostName && (
            <span className="sess-machine">
              <bdi>{hostName}</bdi>
            </span>
          )}
          {(status.tone === 'attn' || status.tone === 'fail' || status.label === 'Host offline' || status.label === 'Waiting on children' || status.label === 'Parent is handling' || status.label === 'Answer pending delivery') && (
            <span className={'sess-flag sess-flag-' + status.tone}>{status.label}</span>
          )}
          {status.resume && <span className="sess-resume">Resume</span>}
        </div>
      </Link>
      <SessionRowMenu session={session} />
      {childrenCount > 0 && <button type="button" className="sess-children" aria-expanded={expanded} aria-label={`↳ ${childrenCount} ${childrenCount === 1 ? 'child' : 'children'}`} aria-description={childrenStatus || undefined} title={childrenStatus || 'Open the family to review child sessions'} onClick={() => onToggleChildren?.(session.session_id)}><span>↳ {childrenCount} {childrenCount === 1 ? 'child' : 'children'}</span>{childrenStatus && <span className="sess-children-status">{childrenStatus}</span>}</button>}
      {reason && (
        <>
          <button type="button" className="sess-why" aria-expanded={why} onClick={() => setWhy((open) => !open)}>
            Why it failed
          </button>
          {why && (
            <p className="sess-reason">
              <bdi>{reason}</bdi>
            </p>
          )}
        </>
      )}
    </div>
  )
  return table ? <tr data-session-id={session.session_id}><td data-label="Session">{content}</td><td data-label="Doing"><span className={'row-status row-status-' + status.tone}>{status.label}</span></td><td data-label="Host"><bdi className="sess-cell-truncate" title={hostName}>{hostName || 'Unknown host'}</bdi></td><td data-label="Agent / model"><bdi className="sess-cell-truncate" title={session.model}>{agentLabel(session.agent)}{session.model ? ` · ${modelLabel(session.model)}` : ''}</bdi></td><td data-label="Last">{relTime(session.last_event_at, now)}</td></tr> : content
}
export default memo(SessionRow)
