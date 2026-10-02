// The session list, as a pure reducer and the functions that read it
// (plan 4c decision 5; frontend spec §5).
//
// - `all` holds every summary received, from pages and from the list
//   stream, which sends every session of the owner's (O-5). The header
//   reads it.
// - The shown list is `all` filtered here by hat, lifecycle filter and
//   Hide closed, newest `last_event_at` first, ties by id, highest first,
//   as the server's pages are ordered (F-7).
// - While a search is active the shown list is the server's result: an
//   upsert updates a row already found and never adds one; only the hat
//   applies (F-10).
// - No event ever refetches (F-4): the stream's summaries are whole.
// - `waiting` is the server's count, the hat's whatever the query: from
//   the first page, then each `waiting_changed`; a further page never
//   changes it.
import type { SummaryQuery } from '../api/view'
import type { SessionSummary, SummaryPage } from '../generated/view'

/** The server's lifecycle names. */
export const LIFECYCLES = ['starting', 'active', 'parked', 'closed', 'failed'] as const

export interface ListFilters {
  /** A hat's id, or none for every hat. */
  hat?: string | null
  /** Only these lifecycles. When set, it alone decides: Hide closed applies
   *  only with no lifecycle filter (so picking `closed` shows closed). */
  lifecycle?: readonly string[]
  /** Hide `closed` sessions (never parked ones). */
  hideClosed: boolean
  /** A search over every lifecycle: the volume filters are bypassed. */
  q?: string
}

export interface ListState {
  all: Map<string, SessionSummary>
  /** While searching, the ids the server found; otherwise `null`. */
  found: Set<string> | null
  /** Where the next page starts; absent on the last one. */
  nextCursor?: string
  /** The first page's anchor: where the list stream resumes. */
  epoch: string
  revision: number
  /** The ids removed since this query's first page: a further page read
   *  before a removal never puts one back. A first page starts it over. */
  removed: ReadonlySet<string>
  /** The server's count of the hat's sessions waiting on a question,
   *  whatever the query: the first page's, then each `waiting_changed`'s.
   *  Absent when the first page had none: the loaded rows are counted. */
  waiting?: number
}

export type ListAction =
  /** The first page, or a resync's: replaces everything. */
  | { type: 'first'; page: SummaryPage; search: boolean }
  /** A further page: added; the anchor stays the first page's. */
  | { type: 'more'; page: SummaryPage }
  | { type: 'upsert'; summary: SessionSummary }
  | { type: 'removed'; id: string }
  /** `waiting_changed`: the server's new count. */
  | { type: 'waiting'; count: number }

export const EMPTY_LIST: ListState = { all: new Map(), found: null, removed: new Set(), epoch: '', revision: 0 }

export function isSummary(value: unknown): value is SessionSummary {
  if (!value || typeof value !== 'object') return false
  const v = value as Record<string, unknown>
  return (
    typeof v.session_id === 'string' &&
    typeof v.hat_id === 'string' &&
    typeof v.lifecycle === 'string' &&
    typeof v.last_event_at === 'string' &&
    typeof v.question_waits === 'boolean'
  )
}

export function isSummaryPage(value: unknown): value is SummaryPage {
  if (!value || typeof value !== 'object') return false
  const v = value as Record<string, unknown>
  return (
    Array.isArray(v.sessions) &&
    typeof v.epoch === 'string' &&
    typeof v.revision === 'number' &&
    (v.next_cursor === undefined || v.next_cursor === null || typeof v.next_cursor === 'string')
  )
}

/** A count the server could have sent: a whole number, never negative. */
export function isWaitingCount(value: unknown): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= 0
}

/** A first page: a page whose `waiting`, when there, is a count. A further
 *  page's `waiting` is never read, so it is never checked. */
export function isFirstPage(value: unknown): value is SummaryPage {
  if (!isSummaryPage(value)) return false
  const waiting = (value as { waiting?: unknown }).waiting
  return waiting === undefined || isWaitingCount(waiting)
}

export function searching(filters: ListFilters): boolean {
  return (filters.q ?? '').trim() !== ''
}

/** The page query for `filters`: a search sends `q` and the hat only. */
export function serverQuery(filters: ListFilters): SummaryQuery {
  const hat = filters.hat || undefined
  if (searching(filters)) return { q: filters.q!.trim(), hat }
  if (filters.lifecycle && filters.lifecycle.length > 0) return { hat, lifecycle: filters.lifecycle }
  if (filters.hideClosed) return { hat, lifecycle: LIFECYCLES.filter((name) => name !== 'closed') }
  return { hat }
}

/** Newest `last_event_at` first (a fixed RFC 3339 form: text order is time
 *  order), then by id, highest first: the server's order
 *  (`last_event_at DESC, id DESC`), so tied rows keep their place across
 *  pages. */
export function compareSummaries(a: SessionSummary, b: SessionSummary): number {
  if (a.last_event_at !== b.last_event_at) return a.last_event_at < b.last_event_at ? 1 : -1
  return a.session_id < b.session_id ? 1 : a.session_id > b.session_id ? -1 : 0
}

const inHat = (s: SessionSummary, hat: string | null | undefined) => !hat || s.hat_id === hat

export function isShown(state: ListState, s: SessionSummary, filters: ListFilters): boolean {
  if (!inHat(s, filters.hat)) return false
  if (state.found) return state.found.has(s.session_id)
  if (filters.lifecycle && filters.lifecycle.length > 0) return filters.lifecycle.includes(s.lifecycle)
  return !(filters.hideClosed && s.lifecycle === 'closed')
}

export function shownOf(state: ListState, filters: ListFilters): SessionSummary[] {
  return [...state.all.values()].filter((s) => isShown(state, s, filters)).sort(compareSummaries)
}

/**
 * The sessions of the hat waiting on a question (`blocked || question_waits`):
 * the tab title's and the header badge's number, read by both through
 * `useSessionList().counts.waiting`. It is the hat's number, never a query's:
 * a search or a lifecycle filter leaves it as it was.
 *
 * The server's count (`SummaryPage.waiting`, then `waiting_changed`) is used
 * whenever it was sent (`waitingCount`): it is the hat's whatever the query.
 * THE FALLBACK, for a page with no `waiting`, counts the rows loaded, and
 * the security review allowed that only with its A4: a search or a lifecycle
 * filter starts `all` over with its own rows only, so while one is set
 * `useSessionList` carries the ids counted from the hat's unfiltered list,
 * brought up to date by every summary received since (`carryWaiting`). A
 * session removed meanwhile stays counted until the filter is cleared.
 */
export function waitingIds(state: ListState, hat: string | null | undefined): Set<string> {
  const ids = new Set<string>()
  for (const s of state.all.values()) {
    if (inHat(s, hat) && isWaiting(s)) ids.add(s.session_id)
  }
  return ids
}

/** The server's count when it sent one (`state.waiting`, already the
 *  hat's: `state` is the hat's query's); otherwise the loaded rows'. */
export function waitingCount(state: ListState, hat: string | null | undefined): number {
  return state.waiting ?? waitingIds(state, hat).size
}

/** `held`, the ids counted from the hat's unfiltered list, with every
 *  summary `state` holds applied: each came after the count. */
export function carryWaiting(held: ReadonlySet<string>, state: ListState, hat: string | null | undefined): Set<string> {
  const ids = new Set(held)
  for (const s of state.all.values()) {
    if (inHat(s, hat) && isWaiting(s)) ids.add(s.session_id)
    else ids.delete(s.session_id)
  }
  return ids
}

const isWaiting = (s: SessionSummary) => s.activity === 'blocked' || s.question_waits

/** `held` with a page's rows: a row the stream sent later is kept. */
function merge(held: Map<string, SessionSummary>, rows: readonly unknown[]): { all: Map<string, SessionSummary>; ids: string[] } {
  const all = new Map(held)
  const ids: string[] = []
  for (const row of rows) {
    if (!isSummary(row)) continue
    ids.push(row.session_id)
    const before = all.get(row.session_id)
    // Strictly newer only: the stream's copy came after the page was read,
    // so it wins a tie (a title or `presumed_parked` changed, the time not).
    if (!before || row.last_event_at > before.last_event_at) all.set(row.session_id, row)
  }
  return { all, ids }
}

export function listReducer(state: ListState, action: ListAction): ListState {
  switch (action.type) {
    case 'first': {
      if (!isFirstPage(action.page)) return state
      const { all, ids } = merge(new Map(), action.page.sessions)
      return {
        all,
        found: action.search ? new Set(ids) : null,
        // A new query's pages hold what the server says.
        removed: new Set(),
        nextCursor: action.page.next_cursor ?? undefined,
        epoch: action.page.epoch,
        revision: action.page.revision,
        // The new page's, or none: never the page's before.
        waiting: action.page.waiting,
      }
    }
    case 'more': {
      if (!isSummaryPage(action.page)) return state
      // A row read before its session was removed is never put back.
      const rows = action.page.sessions.filter((row) => !(isSummary(row) && state.removed.has(row.session_id)))
      const { all, ids } = merge(state.all, rows)
      const found = state.found ? new Set([...state.found, ...ids]) : null
      return { ...state, all, found, nextCursor: action.page.next_cursor ?? undefined }
    }
    case 'upsert': {
      if (!isSummary(action.summary)) return state
      const all = new Map(state.all)
      all.set(action.summary.session_id, action.summary)
      return { ...state, all }
    }
    case 'removed': {
      if (typeof action.id !== 'string') return state
      const held = state.all.has(action.id)
      if (!held && state.removed.has(action.id)) return state
      // Out of `all` is out of the shown list, searching or not. Kept in
      // `removed` even when not held: a further page in flight, read
      // before the removal, may still carry it.
      let all = state.all
      if (held) {
        all = new Map(state.all)
        all.delete(action.id)
      }
      return { ...state, all, removed: new Set(state.removed).add(action.id) }
    }
    case 'waiting': {
      if (!isWaitingCount(action.count) || action.count === state.waiting) return state
      return { ...state, waiting: action.count }
    }
    default:
      return state
  }
}
