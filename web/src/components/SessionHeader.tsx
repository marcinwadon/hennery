// A session's header (frontend spec §6.2): its title, agent, host, branch,
// where it stands in words, model and mode, and the newest step list. On a
// phone it carries the way back to the list, and the steps start closed.
import type { SessionItem } from '../generated/protocol'
import type { PlanEntry } from '../generated/view'
import { agentLabel } from '../lib/agent'
import { statusOf } from '../lib/status'
import { basename } from '../lib/time'
import { Icon } from '../lib/ui'
import { Link } from '../router'
import StepList from './StepList'

/** What the header reads: a list summary or a session's detail. */
export type HeaderInfo = SessionItem & { question_waits?: boolean }

export function titleOf(info: HeaderInfo | undefined, id: string): string {
  return info?.title || basename(info?.cwd ?? '') || id
}

interface Props {
  id: string
  info?: HeaderInfo
  hostName?: string
  plan?: { entries: PlanEntry[]; truncated?: boolean }
  /** Under 768 px: the steps start closed. */
  narrow: boolean
}

export default function SessionHeader({ id, info, hostName, plan, narrow }: Props) {
  // The list's marker, word for word (plan 4c decision 8): the header and
  // the row never disagree.
  const status = info
    ? statusOf({
        lifecycle: info.lifecycle,
        activity: info.activity,
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
        {status && (
          <span className={'badge badge-' + status.tone}>
            <span className="led" />
            {status.label}
          </span>
        )}
        <h1 className="conv-name">
          <bdi>{titleOf(info, id)}</bdi>
        </h1>
      </div>
      {info && (
        <div className="conv-meta">
          <span className="meta-pill" aria-label="Agent">
            {agentLabel(info.agent)}
          </span>
          {hostName && (
            <span className="meta-pill" aria-label="Host">
              <Icon.Cpu size={12} /> <bdi>{hostName}</bdi>
            </span>
          )}
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
          {info.lifecycle === 'failed' && info.failure_reason && (
            <span className="meta-item">
              Reason: <bdi>{info.failure_reason}</bdi>
            </span>
          )}
        </div>
      )}
    </header>
      {plan && <StepList entries={plan.entries} truncated={plan.truncated} open={narrow ? false : undefined} />}
    </>
  )
}
