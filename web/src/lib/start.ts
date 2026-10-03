// Starting a session (frontend spec §7, ACP core §9 `POST /api/sessions`):
// what a refused start says, and how the New Session screen hands the
// session view a notice about a start or a first prompt that did not
// fully go through; and the `/new?host=…&cwd=…` link that opens New Session
// on a project (a failed session's "Start a new session in this project").
import { ApiFailure, messageOf } from '../api/errors'

/** A refused `POST /api/sessions`, in words. Nothing was started, except
 *  for a 503 `delivery_unknown` with a `session_id`, which the caller
 *  handles before this. */
export function startRefusal(err: unknown): string {
  if (!(err instanceof ApiFailure)) return messageOf(err)
  switch (err.code) {
    case 'hat_ambiguous':
      return `${err.message} (${err.serverMessage})`
    case 'host_offline':
      // The server's words: on a host that never connected they say that
      // its first start installs its agents first.
      return err.serverMessage
    case 'invalid_cwd':
      // The server's words name the path: `"<path>" is not a directory on that host`.
      return err.serverMessage
    case 'unknown_host':
      return 'That host is no longer paired. Pick another host.'
    case 'delivery_unknown':
      return 'The host went away while the session was starting; its outcome is not known.'
  }
  if (err.status === 502) return `The host refused to start the session (${err.code}): ${err.serverMessage}`
  return err.message
}

/** Why the session view opens with a notice:
 *  - `start_unknown`: the start's delivery is unknown (503 with
 *    `session_id`); the session exists and may still start; `kept`: a first
 *    prompt was typed, and waits as the session's draft;
 *  - `prompt_failed`: the session started, but the first prompt was
 *    refused (its code travels as `code`). */
export type StartNotice = { kind: 'start_unknown'; kept: boolean } | { kind: 'prompt_failed'; code: string; message: string }

/** `/sessions/<id>`, with the notice as a query flag.
 *
 *  The flag is a query string, not history state, so it needs nothing
 *  from the router: `?notice=start_unknown` (`start_unknown_draft` when a
 *  first prompt was kept), or `?notice=prompt_failed&code=<code>`. The session view reads it with
 *  `readStartNotice(search)` and then drops it with
 *  `navigate(pathname, {replace: true})`, so a reload does not show it
 *  again. Only a code travels, never a server's message: the reader
 *  words it. */
export function sessionHref(
  id: string,
  notice?: { kind: 'start_unknown'; kept: boolean } | { kind: 'prompt_failed'; code: string },
): string {
  const path = `/sessions/${encodeURIComponent(id)}`
  if (!notice) return path
  const query = new URLSearchParams({ notice: notice.kind === 'start_unknown' && notice.kept ? 'start_unknown_draft' : notice.kind })
  if (notice.kind === 'prompt_failed') query.set('code', notice.code)
  return `${path}?${query.toString()}`
}

/** What a refused first prompt says, by its code. */
const PROMPT_REFUSED: Record<string, string> = {
  host_offline: 'The host went offline before the first prompt reached it; send it again once it is back.',
  not_attached: 'The session was no longer running when the first prompt arrived; resume it and send it again.',
  turn_in_progress: 'A turn was already running; send the first prompt again once it ends.',
  delivery_unknown: 'Delivery of the first prompt is unknown: the outcome shows when the host reconnects.',
  content_too_large: 'The first prompt was too large to send.',
  body_too_large: 'The first prompt was too large to send.',
  empty_prompt: 'The first prompt was empty.',
}

/** What an error code looks like. */
const CODE = /^[a-z0-9_]{1,64}$/

/** The notice `search` (a `?…` query) carries, if any. */
export function readStartNotice(search: string): StartNotice | null {
  const query = new URLSearchParams(search)
  const kind = query.get('notice')
  if (kind === 'start_unknown') return { kind, kept: false }
  if (kind === 'start_unknown_draft') return { kind: 'start_unknown', kept: true }
  if (kind === 'prompt_failed') {
    // A link is anyone's to write: only a code's shape is shown, never
    // free text, and only the table's own keys pick a wording.
    const given = query.get('code') ?? ''
    const code = CODE.test(given) ? given : 'unknown'
    const message = Object.hasOwn(PROMPT_REFUSED, code) ? PROMPT_REFUSED[code] : `The first prompt was not sent (${code}).`
    return { kind, code, message }
  }
  return null
}

/** A notice in words, for the session view. */
export function startNoticeText(notice: StartNotice): string {
  if (notice.kind === 'start_unknown') {
    const started = 'The host went away while this session was starting: it may still start, and shows here when the host reconnects.'
    return notice.kept ? `${started} A first prompt was not sent: it waits as this session’s draft.` : started
  }
  return `${notice.message} It waits as this session’s draft.`
}

/** `/new?host=<host>&cwd=<cwd>`: New Session with the host and the project
 *  filled in. Both are encoded; `readNewSessionPrefill` reads them back. */
export function newSessionHref(host: string, cwd: string): string {
  return `/new?${new URLSearchParams({ host, cwd }).toString()}`
}

/** The host and the project a `/new` link names (`newSessionHref`). */
export function readNewSessionPrefill(search: string): { host?: string; cwd?: string } {
  const query = new URLSearchParams(search)
  return { host: query.get('host') ?? undefined, cwd: query.get('cwd') ?? undefined }
}
