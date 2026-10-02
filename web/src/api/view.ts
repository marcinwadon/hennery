// The view API, typed (client view spec §4): a session's items a page of
// whole turns at a time, the session list's summaries, an undelivered turn,
// and the streams' paths. Every id in a path is encoded: ids are server
// data, never trusted to be path-safe.
import type { Client } from './client'
import type { ItemPage, SummaryPage, TurnContent } from '../generated/view'
import type { SessionCatalog, SessionDetail } from '../generated/protocol'

const enc = encodeURIComponent

/** `?a=1&b=2` of the params that are set, or nothing. */
function query(params: Record<string, string | number | undefined>): string {
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) search.set(key, String(value))
  }
  const text = search.toString()
  return text ? `?${text}` : ''
}

export interface ItemPageQuery {
  /** The groups before this turn's (the first loaded item's `turn_id`). */
  beforeTurn?: string
  /** Groups per page; the server's default (20) when absent. */
  limit?: number
}

/** `GET /api/view/sessions/{id}`: the last `limit` groups, or those before
 *  `beforeTurn`. */
export function itemPage(client: Client, id: string, q: ItemPageQuery = {}): Promise<ItemPage> {
  return client.request<ItemPage>(
    'GET',
    `/api/view/sessions/${enc(id)}${query({ before_turn: q.beforeTurn, limit: q.limit })}`,
  )
}

export interface SummaryQuery {
  cursor?: string
  limit?: number
  q?: string
  /** A hat's id; never empty (the server refuses `hat=`). */
  hat?: string
  /** Lifecycle names (`starting`, `active`, `parked`, `closed`, `failed`). */
  lifecycle?: readonly string[]
}

/** `GET /api/view/sessions`: one page of summaries, newest first. */
export function summaries(client: Client, q: SummaryQuery = {}): Promise<SummaryPage> {
  return client.request<SummaryPage>(
    'GET',
    `/api/view/sessions${query({
      cursor: q.cursor,
      limit: q.limit,
      q: q.q || undefined,
      hat: q.hat || undefined,
      lifecycle: q.lifecycle && q.lifecycle.length > 0 ? q.lifecycle.join(',') : undefined,
    })}`,
  )
}

/** `GET /api/view/sessions/{id}/turns/{turn_id}`: a prompt that was not
 *  delivered, whole, to send again. */
export function undeliveredTurn(client: Client, id: string, turnId: string): Promise<TurnContent> {
  return client.request<TurnContent>('GET', `/api/view/sessions/${enc(id)}/turns/${enc(turnId)}`)
}

/** `GET /api/sessions/{id}/catalog`: the config options and slash commands. */
export function catalog(client: Client, id: string): Promise<SessionCatalog> {
  return client.request<SessionCatalog>('GET', `/api/sessions/${enc(id)}/catalog`)
}

/** `GET /api/sessions/{id}`: the session, its open turn and pending requests. */
export function sessionDetail(client: Client, id: string): Promise<SessionDetail> {
  return client.request<SessionDetail>('GET', `/api/sessions/${enc(id)}`)
}

/** The item stream of one session. It resumes from `Last-Event-ID`. */
export function itemStreamPath(id: string): string {
  return `/api/stream/view/sessions/${enc(id)}`
}

/** The session list's stream. It sends every session's summaries; `hat`
 *  scopes only its `waiting_changed` count. Never sent empty: the server
 *  refuses `hat=`. */
export function listStreamPath(hat?: string | null): string {
  return `/api/stream/sessions${query({ hat: hat || undefined })}`
}

/** Where a stream resumes after a page: `<epoch>:<revision>`, opaque. */
export function anchorOf(page: { epoch: string; revision: number }): string {
  return `${page.epoch}:${page.revision}`
}
