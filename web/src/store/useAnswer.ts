// Answering a session's questions (frontend spec §6.3): one book per
// session view, outside React's tree, so what a card did survives the card
// being rendered again or remounted.
//
// - One answer in flight per question: a second one is not sent.
// - Each answer's outcome (202, 409, 404, another refusal) is held with the
//   item's version when it was sent; the card shows it until the item's
//   next upsert (lib/delivery.ts).
// - Whether the host is away: the session is presumed parked, or its host
//   is not connected (the view's one `GET /api/hosts`, read when it opens).
// - Each form's draft, by question: a card mounted again (the window moved,
//   the transcript was rendered anew) finds what the operator had filled in.
// - Which questions opened at the tail since the first page: the one card
//   that may take the focus (§10), once.
// - What the view's one live region says last: a question that opened
//   without taking the focus.
import { useEffect, useLayoutEffect, useMemo, useSyncExternalStore } from 'react'
import { answerQuestion } from '../api/answer'
import type { Client } from '../api/client'
import { ApiFailure, messageOf } from '../api/errors'
import { useClient } from '../app-client'
import type { AnswerRequest, SessionItem } from '../generated/protocol'
import type { Item } from '../generated/view'
import { freshQuestions, type LocalAnswer } from '../lib/delivery'
import type { Draft } from '../lib/elicitation'

/** No draft: one object, so a card's snapshot stays the same. */
const NO_DRAFT: Draft = Object.freeze({})

type Question = Extract<Item, { kind: 'question' }>

export class AnswerBook {
  private readonly local = new Map<string, LocalAnswer>()
  private readonly inFlight = new Set<string>()
  private readonly listeners = new Set<() => void>()
  private away = false
  private known: Set<string> | null = null
  private lastId: string | undefined
  private readonly fresh = new Set<string>()
  private readonly drafts = new Map<string, Draft>()
  private said = ''

  constructor(
    private readonly client: Client,
    readonly sessionId: string,
  ) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  private emit() {
    for (const listener of [...this.listeners]) listener()
  }

  private set(id: string, answer: LocalAnswer) {
    this.local.set(id, answer)
    this.emit()
  }

  /** What this tab did with the question `id`. */
  localOf(id: string): LocalAnswer | undefined {
    return this.local.get(id)
  }

  /** The form's draft of the question `id`: the same object until it
   *  changes. */
  draftOf(id: string): Draft {
    return this.drafts.get(id) ?? NO_DRAFT
  }

  /** Change the draft of the question `id`, from the one held now. */
  updateDraft(id: string, next: (draft: Draft) => Draft) {
    const held = this.draftOf(id)
    const draft = next(held)
    if (draft === held) return
    this.drafts.set(id, draft)
    this.emit()
  }

  get hostAway(): boolean {
    return this.away
  }

  setHostAway(away: boolean) {
    if (away === this.away) return
    this.away = away
    this.emit()
  }

  /** Send `body` as the answer to `item`, unless one is in flight already. */
  async answer(item: Question, body: AnswerRequest): Promise<void> {
    if (this.inFlight.has(item.id)) return
    this.inFlight.add(item.id)
    const version = item.version
    this.set(item.id, { phase: 'sending', version })
    try {
      await answerQuestion(this.client, this.sessionId, item.pending_id, body)
      this.set(item.id, { phase: 'sent', version })
    } catch (err) {
      this.set(item.id, outcomeOf(err, version))
    } finally {
      this.inFlight.delete(item.id)
    }
  }

  /** Take in the session's items once its first page is loaded: the
   *  questions that opened at the tail since the last look are fresh. */
  observe(items: readonly Item[]) {
    for (const id of freshQuestions(items, this.known, this.lastId)) this.fresh.add(id)
    const known = this.known ?? new Set<string>()
    for (const item of items) if (item.kind === 'question') known.add(item.id)
    this.known = known
    this.lastId = items.length > 0 ? items[items.length - 1].id : undefined
  }

  /** Whether the card of question `id` may take the focus: once, and only
   *  for a question that opened at the tail. */
  claimFocus(id: string): boolean {
    return this.fresh.delete(id)
  }

  /** What the view's live region says: the last question that opened
   *  without taking the focus. */
  get announcement(): string {
    return this.said
  }

  /** Say `text` in the view's live region. The same words twice in a row
   *  differ by a trailing no-break space, so the region's text changes and
   *  a screen reader says them again. */
  announce(text: string) {
    this.said = this.said === text ? text + '\u00a0' : text
    this.emit()
  }
}

function outcomeOf(err: unknown, version: number): LocalAnswer {
  if (err instanceof ApiFailure) {
    if (err.status === 409 && err.code === 'already_answered') return { phase: 'taken', version }
    if (err.status === 409 && err.code === 'not_open') return { phase: 'closed', version }
    if (err.status === 404) return { phase: 'gone', version }
  }
  return { phase: 'failed', version, message: messageOf(err) }
}

/** The card's view of the book: what this tab did, and whether the host is
 *  away. Without a book, nothing. */
export function useAnswerState(book: AnswerBook | undefined, id: string): { local?: LocalAnswer; hostAway: boolean } {
  const subscribe = book ? book.subscribe : noSubscribe
  const local = useSyncExternalStore(subscribe, () => book?.localOf(id))
  const hostAway = useSyncExternalStore(subscribe, () => book?.hostAway ?? false)
  return { local, hostAway }
}

const noSubscribe = () => () => {}

/** A form's draft in the book, and how to change it. Without a book, an
 *  empty draft that does not change. */
export function useFormDraft(book: AnswerBook | undefined, id: string): [Draft, (next: (draft: Draft) => Draft) => void] {
  const subscribe = book ? book.subscribe : noSubscribe
  const draft = useSyncExternalStore(subscribe, () => book?.draftOf(id) ?? NO_DRAFT)
  const update = useMemo(() => (next: (draft: Draft) => Draft) => book?.updateDraft(id, next), [book, id])
  return [draft, update]
}

/** The session view's book: one per session, fed its items once the first
 *  page is in, and told whether the host is away. `connected`: what the
 *  view's hosts list says of the session's host (undefined: unknown). */
export function useAnswering(
  sessionId: string,
  info: Pick<SessionItem, 'presumed_parked'> | undefined,
  items: readonly Item[],
  loading: boolean,
  connected: boolean | undefined,
): AnswerBook {
  const client = useClient()
  const book = useMemo(() => new AnswerBook(client, sessionId), [client, sessionId])
  // Before the cards' effects (a layout effect runs first): a fresh card
  // finds itself fresh when it mounts.
  useLayoutEffect(() => {
    if (!loading) book.observe(items)
  }, [book, items, loading])
  const away = Boolean(info?.presumed_parked) || connected === false
  useEffect(() => book.setHostAway(away), [book, away])
  return book
}
