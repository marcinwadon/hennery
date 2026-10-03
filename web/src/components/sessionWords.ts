// What the session's footer and header menu say (frontend spec §6.6, ACP
// core §9): why a session failed, why a resume, a park, a close or a delete
// was refused, and what a delete left on the host. The wording is the
// view's own: each table is looked up first, by the code's own keys, and
// only a code none of them knows falls back to the app-wide message (whose
// words for `hat_mismatch`, `cwd_moved`, `hat_ambiguous` or `host_offline`
// were written for other routes).
import { ApiFailure, messageOf } from '../api/errors'
import type { DeleteResult, RemovalPending } from '../generated/protocol'
import { reasonWords } from './items/Marker'

/** A sentence per code. */
type Words = Record<string, string>

function say(words: Words, err: unknown): string {
  if (err instanceof ApiFailure && Object.hasOwn(words, err.code)) return words[err.code]
  return messageOf(err)
}

/** Why a session failed, in words: its `failure_reason` looked up by own
 *  key; undefined for a reason this client does not know (the footer shows
 *  that one as sent, isolated as the server's text). */
export function failureWords(reason: string): string | undefined {
  const words = reasonWords('start_failed', reason)
  // reasonWords gives an unknown reason back as it came.
  return words === reason ? undefined : words
}

/** How the agent's CLI is signed in on its host: `hennery doctor`'s own
 *  instructions, by agent. */
export function loginSteps(agent: string): string {
  switch (agent) {
    case 'claude':
      return 'Run `claude` in a terminal there and sign in (`/login`), then resume.'
    case 'codex':
      return 'Run `codex login` in a terminal there, then resume.'
    default:
      return 'Sign the agent’s command-line tool in there, then resume. `hennery doctor` on that host checks it.'
  }
}

/** The resume refusals the footer words itself; `hat_mismatch`,
 *  `agent_has_no_record` and `agent_not_logged_in` get more than a
 *  sentence (see the footer). */
const RESUME: Words = {
  hat_mismatch: 'This session’s directory now belongs to another hat than the session’s.',
  cwd_moved:
    'The session’s directory now resolves to another place on its host: it cannot be resumed there. Start a new session in that directory instead.',
  hat_ambiguous:
    'The session’s directory now matches the path rules of more than one hat: change the rules so that one hat claims it, then resume.',
  host_offline: 'The host is offline: resume once it is back.',
  agent_has_no_record: 'The agent has no record of this session, so it cannot be resumed.',
  agent_not_logged_in: 'The agent is not logged in on the host.',
  starting: 'The session is already starting.',
  active: 'The session is already running.',
  invalid_cwd: 'The session’s directory is no longer a directory on its host.',
  delivery_unknown: 'The host went away while the session was resuming: its outcome shows when the host reconnects.',
  load_unsupported: 'The agent cannot open an earlier session, so this one cannot be resumed.',
  start_failed: 'The agent could not start.',
}

/** Why a resume was refused. */
export function resumeRefusal(err: unknown): string {
  return say(RESUME, err)
}

const PARK: Words = {
  not_attached: 'The session is not running, or its host is not ready: there is nothing to park.',
  park_unsupported: 'This host cannot park sessions: update hennery on it.',
}

/** Why a park was refused. */
export function parkRefusal(err: unknown): string {
  return say(PARK, err)
}

const CLOSE: Words = {
  starting: 'The session is still starting: close it once the start settles.',
  delivery_unknown: 'The host went away before it confirmed the close: the session closes when the host is back.',
}

/** Why a close was refused. */
export function closeRefusal(err: unknown): string {
  return say(CLOSE, err)
}

/** The lifecycle codes a delete answers when the session moved while it
 *  was being deleted. */
const MOVED = 'The session changed while it was being deleted, so nothing was deleted: try again.'

const DELETE: Words = {
  starting: 'The session is still starting: delete it once the start settles.',
  delivery_unknown:
    'The host went away before it confirmed the close, so nothing was deleted: try again once the host is back.',
  active: MOVED,
  parked: MOVED,
  closed: MOVED,
  failed: MOVED,
}

/** Why a delete was refused. */
export function deleteRefusal(err: unknown): string {
  return say(DELETE, err)
}

const PENDING: Record<RemovalPending, string> = {
  host_offline: 'the host is offline; it is removed when the host is back',
  host_needs_update: 'the host needs a newer hennery to remove it',
  no_reply: 'the host did not answer in time; it is tried again when the host next connects',
  attached: 'the agent still has it open; it is tried again when the host next connects',
  in_progress: 'a removal is running on the host right now',
}

/** What a delete left of the agent's own transcript on the host, in
 *  sentences: nothing to say when it was removed, or there was none. */
export function deleteNotes(result: DeleteResult | undefined | null): string[] {
  const t = result?.host_transcript
  if (!t) return []
  const notes: string[] = []
  if (t.state === 'partial') {
    const left = (t.remaining ?? []).reduce((sum, item) => sum + (Number.isFinite(item.count) ? item.count : 0), 0)
    notes.push(
      left > 0
        ? `The agent’s own transcript on the host was removed only in part: ${left} ${left === 1 ? 'entry is' : 'entries are'} left there.`
        : 'The agent’s own transcript on the host was removed only in part.',
    )
  } else if (t.state === 'pending') {
    const why = t.pending !== undefined && Object.hasOwn(PENDING, t.pending) ? PENDING[t.pending] : null
    notes.push(
      why
        ? `The agent’s own transcript on the host is not removed yet: ${why}.`
        : 'The agent’s own transcript on the host is not removed yet: it is tried again when the host next connects.',
    )
  }
  return notes
}
