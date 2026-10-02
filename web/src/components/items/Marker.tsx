// A divider for what happened to the session rather than in its
// conversation (frontend spec §6.1): one plain label per marker kind, the
// reason in words where it is known (else the code as sent), and any text
// behind a disclosure, as text.
import type { MarkerKind } from '../../generated/view'
import { clockTime } from '../../lib/time'
import { CutNote } from './parts'
import type { ItemEnv, ItemOf } from './types'

type MarkerItem = ItemOf<'marker'>

export const MARKER_LABEL: Record<MarkerKind, string> = {
  parked: 'Parked',
  resumed: 'Resumed',
  closed: 'Closed',
  host_restarted: 'The host restarted',
  host_offline: 'Host offline',
  host_back: 'Host back',
  turn_interrupted: 'The turn was interrupted',
  turn_failed: 'The turn failed',
  turn_cancelled: 'The turn was stopped',
  turn_not_delivered: 'The turn was not delivered',
  start_not_delivered: 'The start was not delivered',
  start_failed: 'The session failed to start',
  adapter_exited: 'The agent’s process exited',
  transcript_gap: 'Part of the transcript is missing',
  host_note: 'A note from the host',
  conflict: 'The host sent a conflicting update',
  hat_reassigned: 'Moved to another hat',
  elided: 'This turn holds more than is shown here',
}

/** Known reason codes, in words, by marker kind. */
const REASON: Partial<Record<MarkerKind, Record<string, string>>> = {
  parked: {
    idle: 'it was idle',
    operator: 'on request',
    adapter_exited: 'the agent’s process exited',
  },
  host_offline: {
    host_offline: 'the host went offline',
    host_revoked: 'the host was revoked',
  },
  start_failed: {
    agent_has_no_record: 'the agent has no record of this session',
    agent_not_logged_in: 'the agent is not logged in on the host',
    load_unsupported: 'the agent cannot open an earlier session',
    start_failed: 'the agent could not start',
  },
  host_note: {
    config_failed: 'a setting did not take',
    reapply_failed: 'a setting could not be applied again',
    cancel_unanswered: 'a question was left unanswered when the turn stopped',
    replay_unknown_dropped: 'history of an unknown kind was left out',
    agent_home_moved: 'the agent’s data directory moved',
  },
  elided: {
    items: 'too many items',
    bytes: 'too much text',
    questions: 'too many questions',
  },
}

/** The reason in words; an unknown code is shown as sent. */
export function reasonWords(kind: MarkerKind, reason: string): string {
  // Own keys only, at both levels: a kind of `__proto__` must not reach
  // the prototype's `toString`.
  const known = Object.hasOwn(REASON, kind) ? REASON[kind] : undefined
  if (known && Object.hasOwn(known, reason)) return known[reason]
  if (kind === 'transcript_gap') return `host events ${reason}`
  if (kind === 'conflict') return `at host event ${reason}`
  return reason
}

export function markerLabel(kind: string): string {
  return Object.hasOwn(MARKER_LABEL, kind) ? MARKER_LABEL[kind as MarkerKind] : 'Something happened'
}

export default function Marker({ item, env }: { item: MarkerItem; env: ItemEnv }) {
  const clock = clockTime(item.ts)
  const known = Object.hasOwn(MARKER_LABEL, item.marker)
  const hat = (id: string | undefined) => (id ? (env.hatName?.(id) ?? id) : 'no hat')
  return (
    <div className={'marker fade-in marker-' + (known ? item.marker : 'unknown')}>
      <div className="divider">
        <span>
          {markerLabel(item.marker)}
          {!known && (
            <>
              {' '}
              (<bdi>{item.marker}</bdi>)
            </>
          )}
          {item.marker === 'hat_reassigned' && (
            <>
              : from <bdi>{hat(item.from)}</bdi> to <bdi>{hat(item.to)}</bdi>
            </>
          )}
          {item.reason && (
            <>
              {' '}
              (<bdi>{reasonWords(item.marker, item.reason)}</bdi>)
            </>
          )}
          {clock && <span className="marker-when"> · {clock}</span>}
        </span>
        {item.marker === 'turn_not_delivered' && env.onSendAgain && item.about_turn && (
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => env.onSendAgain?.(item)}>
            Send again
          </button>
        )}
      </div>
      {item.text && (
        <details className="marker-text">
          <summary>Details</summary>
          <pre>{item.text}</pre>
        </details>
      )}
      {item.marker === 'elided' && <CutNote sessionId={env.sessionId}>The rest is in the session’s events.</CutNote>}
    </div>
  )
}
