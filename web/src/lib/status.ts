// A session's status marker in the list and in the session's header
// (frontend spec §5; plan 4c decisions 7 and 8): one rule for both. It
// uses the server's operator attention separately from factual pending
// questions. Older summaries without counts retain their original marker.

/** Operator attention is strongest; `fail` is a hollow red ring. */
export type Tone = 'attn' | 'run' | 'wait' | 'idle' | 'fail' | 'background'

export interface Marker {
  tone: Tone
  label: string
  /** The row offers "Resume" (a parked session). */
  resume: boolean
}

export interface StatusInput {
  lifecycle: string
  activity?: string
  question_waits: boolean
  presumed_parked: boolean
  operator_attention?: number
  review?: { event_id: number }
  unseen?: { event_id: number }
  parent_handled_questions?: number
  family?: { child_counts: { active: number } }
  background?: { active: number; done: number; all: number; stale?: boolean } | null
}

/**
 * The marker for a session.
 *
 * Operator work wins over every lifecycle. A parent-handled question and
 * an accepted answer awaiting delivery remain visible without adding to
 * the operator badge. Waiting on children describes only an idle parent.
 */
export function statusOf(s: StatusInput): Marker {
  if (s.operator_attention !== undefined ? s.operator_attention > 0 : s.activity === 'blocked' || s.question_waits) return { tone: 'attn', label: s.operator_attention !== undefined ? 'Waiting on you' : 'Waiting on a question', resume: false }
  if ((s.parent_handled_questions ?? 0) > 0) return { tone: 'wait', label: 'Parent is handling', resume: false }
  if (s.operator_attention === 0 && s.question_waits) return { tone: 'wait', label: 'Answer pending delivery', resume: false }
  if (s.lifecycle === 'active' && s.activity === 'idle' && (s.family?.child_counts.active ?? 0) > 0) return { tone: 'wait', label: 'Waiting on children', resume: false }
  switch (s.lifecycle) {
    case 'failed':
      return { tone: 'fail', label: 'Failed', resume: false }
    case 'archived':
      return { tone: 'idle', label: 'Archived', resume: false }
    case 'closed':
      return { tone: 'idle', label: 'Closed', resume: false }
    case 'parked':
      // Parked only because its host went quiet: it may still be running
      // there. It is still parked, so Resume stays (the footer adds the
      // "host offline" note, decision 35).
      return s.presumed_parked
        ? { tone: 'idle', label: 'Host offline', resume: true }
        : { tone: 'idle', label: 'Parked', resume: true }
    case 'starting':
      return { tone: 'wait', label: 'Starting', resume: false }
    case 'active':
      if (s.activity === 'unknown' || s.activity === 'busy') return { tone: 'run', label: 'Busy', resume: false }
      if (s.activity !== 'running' && (s.background?.active ?? 0) > 0) return { tone: 'background', label: 'Background', resume: false }
      return s.activity === 'running'
        ? { tone: 'run', label: 'Running', resume: false }
        : { tone: 'idle', label: 'Idle', resume: false }
    default:
      // A lifecycle from a newer server: shown by its name, never hidden.
      return { tone: 'idle', label: s.lifecycle || 'Unknown', resume: false }
  }
}

export const STATUS_FILTERS = ['all', 'attention', 'unseen', 'running', 'background', 'idle', 'parked', 'failed'] as const
export type StatusFilter = typeof STATUS_FILTERS[number]
export const STATUS_LABEL: Record<StatusFilter, string> = { all: 'All', attention: 'Needs you', unseen: 'Unseen', running: 'Running', background: 'Background', idle: 'Idle', parked: 'Parked / offline', failed: 'Failed' }
/** Modern operator counts are authoritative, including closed status-only work. */
export function needsOperator(s: StatusInput): boolean { return (s.operator_attention ?? 0) > 0 }
export function matchesStatus(s: StatusInput, filter: StatusFilter): boolean {
  switch (filter) {
    case 'all': return true
    case 'attention': return needsOperator(s)
    case 'unseen': return !!s.unseen
    case 'running': return s.lifecycle === 'active' && ['running', 'busy', 'unknown'].includes(s.activity ?? '')
    case 'background': return s.lifecycle === 'active' && (s.activity === 'idle' || s.activity === 'background') && (s.background?.active ?? 0) > 0
    case 'idle': return s.lifecycle === 'active' && s.activity === 'idle' && (s.background?.active ?? 0) === 0
    case 'parked': return s.lifecycle === 'parked' || s.presumed_parked
    case 'failed': return s.lifecycle === 'failed'
  }
}
