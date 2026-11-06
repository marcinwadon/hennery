// The view API, typed (client view spec §4): a session's items a page of
// whole turns at a time, the session list's summaries, an undelivered turn,
// and the streams' paths. Every id in a path is encoded: ids are server
// data, never trusted to be path-safe.
import type { Client } from './client'
import type { FamilyPage, Item, ItemPage, SessionWork, SummaryPage, TurnContent } from '../generated/view'
import type { SessionCatalog, SessionDetail } from '../generated/protocol'

const enc = encodeURIComponent

export function questionCard(client: Client, id: string, pending: string, signal?: AbortSignal): Promise<Item> {
  return client.request<Item>('GET', `/api/view/sessions/${enc(id)}/questions/${enc(pending)}`, undefined, { signal })
}

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

/** The items of one turn, walking back past turns that started after it (a
 *  queued prompt or a wake can start the next turn at once). `revision` is the
 *  newest page's. Empty when the turn is not within `pages` groups. */
export async function turnItems(client: Client, id: string, turnId: string, pages = 6): Promise<{ items: Item[]; revision: number }> {
  let page = await itemPage(client, id, { limit: 1 })
  const revision = page.revision
  for (let i = 1; ; i++) {
    const items = page.items.filter(item => item.turn_id === turnId)
    const first = page.items[0]?.turn_id
    if (items.length || !page.older || !first || i >= pages) return { items, revision }
    page = await itemPage(client, id, { beforeTurn: first, limit: 1 })
  }
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
export function summaries(client: Client, q: SummaryQuery = {}, signal?: AbortSignal): Promise<SummaryPage> {
  return client.request<SummaryPage>(
    'GET',
    `/api/view/sessions${query({
      cursor: q.cursor,
      limit: q.limit,
      q: q.q || undefined,
      hat: q.hat || undefined,
      lifecycle: q.lifecycle && q.lifecycle.length > 0 ? q.lifecycle.join(',') : undefined,
    })}`,
    undefined, { signal },
  )
}

/** Bounded session observations; reading does not acknowledge activity. */
export function sessionWork(client: Client, id: string, signal?: AbortSignal): Promise<SessionWork> {
  return client.request<SessionWork>('GET', `/api/view/sessions/${enc(id)}/work`, undefined, { signal })
}

/** `GET /api/view/sessions/{id}/turns/{turn_id}`: a prompt that was not
 *  delivered, whole, to send again. `signal` aborts the request. */
export function undeliveredTurn(client: Client, id: string, turnId: string, signal?: AbortSignal): Promise<TurnContent> {
  return client.request<TurnContent>('GET', `/api/view/sessions/${enc(id)}/turns/${enc(turnId)}`, undefined, { signal })
}

/** `GET /api/sessions/{id}/catalog`: the config options and slash commands. */
export function catalog(client: Client, id: string): Promise<SessionCatalog> {
  return client.request<SessionCatalog>('GET', `/api/sessions/${enc(id)}/catalog`)
}

/** `GET /api/sessions/{id}`: the session, its open turn and pending requests. */
const detailReads = new WeakMap<Client, Map<string, Promise<SessionDetail>>>()

export function sessionDetail(client: Client, id: string, signal?: AbortSignal): Promise<SessionDetail> {
  if (signal) return client.request<SessionDetail>('GET', `/api/sessions/${enc(id)}`, undefined, { signal })
  let reads = detailReads.get(client)
  if (!reads) { reads = new Map(); detailReads.set(client, reads) }
  const held = reads.get(id)
  if (held) return held
  const pending = client.request<SessionDetail>('GET', `/api/sessions/${enc(id)}`).finally(() => reads.delete(id))
  reads.set(id, pending)
  return pending
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

/** Family pages are owner-authorized by the collector, like the view itself. */
export function familyPage(client: Client, id: string, after?: string): Promise<FamilyPage> {
  return client.request<FamilyPage>('GET', `/api/view/sessions/${enc(id)}/family${query({ after, limit: 20 })}`)
}

export function takeOver(client: Client, id: string): Promise<void> {
  return client.request<void>('POST', `/api/view/sessions/${enc(id)}/takeover`, {})
}

/** A captured result only; a stale browser cannot acknowledge newer work. */
export function markResultRead(client: Client, id: string, eventId: number): Promise<void> {
  return client.request<void>('POST', `/api/view/sessions/${enc(id)}/read`, { event_id: eventId })
}

export function markSessionSeen(client: Client, id: string, eventId: number): Promise<void> {
  return client.request<void>('POST', `/api/view/sessions/${enc(id)}/seen`, { event_id: eventId })
}
