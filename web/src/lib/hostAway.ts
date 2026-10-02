// Whether a session's host is away, for a question card's "delivers when
// the host reconnects" (decision 22). Pure: the answer book feeds it what
// the view already holds, and nothing here fetches.
//
// - The session's `presumed_parked` (its summary, kept current by the
//   list's `session_upsert`): the host is away.
// - Else the newest host marker on the session's item stream says it:
//   `host_offline` (a presumed park, the host offline or revoked) is away;
//   `host_back` (reattached), `host_restarted` (the host is back after a
//   restart) and `resumed` (a resume, which the server refuses while the
//   host is not connected) are back. Newest by `ts`, a tie to the later
//   item; a `ts` that does not parse is not read.
// - Else the seed: `connected` from the view's one `GET /api/hosts`, read
//   when the view opens. A host known not to be connected is away; one not
//   known is not.
// - Only markers newer than the first page count (`since`: the newest `ts`
//   that page held). The seed was read as the page was, so the page's own
//   markers are history it already says; so are older pages loaded later.
//   A resync's new markers do count.
import type { Item } from '../generated/view'

/** Marker kinds that say the host is back. */
const BACK: ReadonlySet<string> = new Set(['host_back', 'host_restarted', 'resumed'])

/** What says the host is away. */
export interface HostAwayInput {
  /** The session's `presumed_parked`. */
  presumedParked: boolean
  /** `connected` from the view's hosts list; undefined while not known. */
  seed: boolean | undefined
  /** The session's items. */
  items: readonly Item[]
  /** Markers at or before this time (ms) are history the seed says. */
  since: number
}

/** The newest `ts` in `items`, in ms; `-Infinity` when none parses. */
export function newestTs(items: readonly Item[]): number {
  let newest = -Infinity
  for (const item of items) {
    const at = Date.parse(item.ts)
    if (at > newest) newest = at
  }
  return newest
}

/** Whether the host is away: see the rules above. */
export function hostAway({ presumedParked, seed, items, since }: HostAwayInput): boolean {
  if (presumedParked) return true
  let newest: { at: number; away: boolean } | undefined
  for (const item of items) {
    if (item.kind !== 'marker') continue
    const away = item.marker === 'host_offline'
    if (!away && !BACK.has(item.marker)) continue
    const at = Date.parse(item.ts)
    if (Number.isNaN(at) || at <= since) continue
    if (newest === undefined || at >= newest.at) newest = { at, away }
  }
  return newest !== undefined ? newest.away : seed === false
}
