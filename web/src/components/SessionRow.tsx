// One row of the session list (frontend spec §5; plan 4c decision 7).
//
// Line 1: status marker, agent, title (fallback: the project directory's
// name), relative time. Line 2: the branch (fallback: the directory's name,
// never repeated when it is the title), the host's name, and "Resume" for a
// parked session. The row is a link to the session, so a new tab works.
// A failed session's reason is on the marker's hover, and a tap on "Why"
// beside the link shows it (a button inside the link would open the session).
import { useState } from 'react'
import type { SessionSummary } from '../generated/view'
import { agentLabel } from '../lib/agent'
import { statusOf } from '../lib/status'
import { basename, relTime } from '../lib/time'
import { Link } from '../router'

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
}

export default function SessionRow({ session, selected, hostName }: Props) {
  const [why, setWhy] = useState(false)
  const status = statusOf(session)
  const { title, where } = rowText(session)
  const reason = status.tone === 'fail' ? session.failure_reason : undefined
  const markerTitle = reason ? `${status.label}: ${reason}` : status.label
  return (
    <div className="sess-item">
      <Link
        to={sessionPath(session.session_id)}
        className={'sess' + (selected ? ' active' : '')}
        aria-current={selected ? 'page' : undefined}
      >
        <div className="sess-row1">
          <span className={'sess-led led-' + status.tone} role="img" aria-label={status.label} title={markerTitle} />
          <span className="sess-kind sess-agent">{agentLabel(session.agent)}</span>
          <span className="sess-name">
            <bdi>{title}</bdi>
          </span>
          <span className="sess-time">{relTime(session.last_event_at)}</span>
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
          {(status.tone === 'attn' || status.tone === 'fail' || status.label === 'Host offline') && (
            <span className={'sess-flag sess-flag-' + status.tone}>{status.label}</span>
          )}
          {status.resume && <span className="sess-resume">Resume</span>}
        </div>
      </Link>
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
}
