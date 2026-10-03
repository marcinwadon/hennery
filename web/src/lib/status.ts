// A session's status marker in the list and in the session's header
// (frontend spec §5; plan 4c decisions 7 and 8): one rule for both. It
// reads the server's two axes, lifecycle and activity, plus
// `question_waits` and `presumed_parked`, directly: there is no staleness
// heuristic and no "actionable" guess (F-9).

/** The marker's colour: `attn` is the strongest and means only "waiting on
 *  a question"; `fail` is a hollow red ring. */
export type Tone = 'attn' | 'run' | 'wait' | 'idle' | 'fail'

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
}

/**
 * The marker for a session.
 *
 * Waiting on a question wins over every lifecycle, so a row's marker always
 * agrees with the waiting count (tab title, header badge), which counts
 * `blocked || question_waits` whatever the lifecycle. The wording is never
 * "needs you": the summary cannot tell an answer already in flight.
 */
export function statusOf(s: StatusInput): Marker {
  if (s.activity === 'blocked' || s.question_waits) return { tone: 'attn', label: 'Waiting on a question', resume: false }
  switch (s.lifecycle) {
    case 'failed':
      return { tone: 'fail', label: 'Failed', resume: false }
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
      return s.activity === 'running'
        ? { tone: 'run', label: 'Running', resume: false }
        : { tone: 'idle', label: 'Idle', resume: false }
    default:
      // A lifecycle from a newer server: shown by its name, never hidden.
      return { tone: 'idle', label: s.lifecycle || 'Unknown', resume: false }
  }
}
