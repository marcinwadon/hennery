// Where a question stands (frontend spec §6.3), read from the item and what
// this tab did with it. Pure: the card renders what these return.
//
// - The item says what the server knows: `answerable`, `state`, `reason`,
//   `answered` and the verdict `delivered`.
// - The tab adds what only it knows: an answer in flight, a 202 not yet
//   shown by an upsert, a 409 or a 404. Each of those holds only while the
//   item is still at the version it was answered at: the next upsert of the
//   item says the rest.
// - The verdict is monotonic (ACP core §4.6): a held `delivered: true` is
//   never replaced by `false`.
import type { PendingReason } from '../generated/protocol'
import type { Item, Request } from '../generated/view'

type Question = Extract<Item, { kind: 'question' }>

export const REASON_WORDS: Record<PendingReason, string> = {
  turn_cancelled: 'the turn was stopped',
  session_closed: 'the session was closed',
  session_parked: 'the session was parked',
  adapter_lost: 'the agent’s process was lost',
  host_restarted: 'the host restarted',
  agent_withdrew: 'the agent withdrew the question',
  host_revoked: 'the host was revoked',
}

/** What this tab did with a question. `version` is the item's when it was
 *  answered: an outcome holds only while the item is at it. */
export type LocalAnswer =
  | { phase: 'sending'; version: number }
  | { phase: 'sent'; version: number }
  /** 409 `already_answered`: another tab or device answered first. */
  | { phase: 'taken'; version: number }
  /** 409 `not_open`: the question closed before the answer reached it. */
  | { phase: 'closed'; version: number }
  /** 404: the question no longer exists. */
  | { phase: 'gone'; version: number }
  /** Any other refusal: the message, and the controls again. */
  | { phase: 'failed'; version: number; message: string }

export interface Delivery {
  /** The state in words. */
  text: string
  /** A second line: "delivers when the host reconnects". */
  note?: string
  /** The answering controls are shown. */
  controls: boolean
  /** They are shown but disabled: an answer is in flight. */
  busy: boolean
  /** A refusal's message, beside the controls. */
  error?: string
  /** "Answer as a new message" is offered. */
  answerAsMessage: boolean
}

export const NO_OPTIONS = 'This question cannot be answered here: stop, park or close the session.'
export const TAKEN = 'Already answered from another device'
export const NOT_OPEN = 'This question is no longer open'
export const GONE = 'This question is gone'
export const HOST_AWAY_NOTE = 'delivers when the host reconnects'

/** A permission with no option ids: nothing here can answer it, whatever
 *  the server's `answerable` says (it does not look at the options). */
export function hasNoOptions(request: Request): boolean {
  return request.type === 'permission' && request.options.length === 0
}

/** Whether the card can answer it now: the server's pending set says so
 *  (F-15), and it has something to answer with. */
export function isAnswerable(q: Question): boolean {
  return q.answerable && !hasNoOptions(q.request)
}

/** The state from the item alone: the §6.3 table, top row first. */
function serverState(q: Question): { text: string; awaiting: boolean; cancelled: boolean } {
  const plain = { awaiting: false, cancelled: false }
  if (q.delivered === true) return { text: 'Answered', ...plain }
  if (q.state === 'cancelled') {
    // Own keys only: a reason named like an Object member (`constructor`)
    // is shown as sent, never as that member's text.
    const why = q.reason ? (Object.hasOwn(REASON_WORDS, q.reason) ? REASON_WORDS[q.reason] : q.reason) : undefined
    const text = why ? `The agent stopped waiting (${why})` : 'The agent stopped waiting'
    return { text, awaiting: false, cancelled: true }
  }
  if (q.delivered === false) return { text: 'Sent, but the agent was no longer waiting', ...plain }
  if (q.state === 'delivered') return { text: 'Answered', ...plain }
  if (q.answered) return { text: 'Sent', awaiting: true, cancelled: false }
  if (hasNoOptions(q.request)) return { text: NO_OPTIONS, ...plain }
  if (q.answerable) return { text: 'Needs your answer', ...plain }
  return { text: 'Open', ...plain }
}

/** Where the question stands, as the card shows it. `hostAway`: the session
 *  is presumed parked, or its host is not connected. */
export function deliveryOf(q: Question, local: LocalAnswer | undefined, hostAway = false): Delivery {
  // An answer in flight holds whatever the item's version: an upsert
  // meanwhile must not offer the controls again. Its outcome holds only
  // until the next upsert.
  const held = local && (local.phase === 'sending' || local.version === q.version) ? local : undefined
  const none: Delivery = { text: '', controls: false, busy: false, answerAsMessage: false }
  const awayNote = hostAway ? { note: HOST_AWAY_NOTE } : {}
  switch (held?.phase) {
    case 'sending':
      if (isAnswerable(q)) return { ...none, text: 'Sending…', controls: true, busy: true }
      break
    case 'sent':
      return { ...none, text: 'Sent', ...awayNote }
    case 'taken':
      return { ...none, text: TAKEN }
    case 'closed':
      return { ...none, text: NOT_OPEN }
    case 'gone':
      return { ...none, text: GONE }
    case 'failed':
      if (isAnswerable(q)) return { ...none, text: 'Needs your answer', controls: true, error: held.message }
      break
  }
  const s = serverState(q)
  return {
    ...none,
    text: s.text,
    ...(s.awaiting && q.state === 'open' ? awayNote : {}),
    controls: isAnswerable(q),
    answerAsMessage: s.cancelled,
  }
}

/** The question in one line: a permission's title, an elicitation's
 *  message. "Answer as a new message" hands this, and only this, to the
 *  composer, which words the draft around it. */
export function questionText(request: Request): string {
  return request.type === 'permission' ? request.title || 'Permission' : request.message
}

/** A permission option's tone, from its kind alone, never its name:
 *  `reject_*` destructive, `allow_always` ghost, anything else primary. */
export type OptionTone = 'reject' | 'always' | 'allow'

export function optionTone(kind: string): OptionTone {
  if (kind.startsWith('reject_')) return 'reject'
  if (kind === 'allow_always') return 'always'
  return 'allow'
}

/** The store's upsert of a held item by a newer one: a question's verdict
 *  `delivered: true` sticks, and with it the question is not answerable. */
export function foldVerdict(held: Item, next: Item): Item {
  if (held.kind !== 'question' || next.kind !== 'question') return next
  if (held.delivered !== true || next.delivered === true) return next
  return { ...next, delivered: true, answerable: false }
}

/** A digit 1–9 as an option's index, when the key may answer: no modifier,
 *  not typed into an editable target. */
export function digitIndex(e: {
  key: string
  ctrlKey: boolean
  metaKey: boolean
  altKey: boolean
  target: EventTarget | null
}): number | undefined {
  if (e.ctrlKey || e.metaKey || e.altKey) return undefined
  if (!/^[1-9]$/.test(e.key)) return undefined
  if (isEditable(e.target)) return undefined
  return Number(e.key) - 1
}

/** Whether typing in `target` is text: an input, a textarea, a select, or
 *  anything inside a contenteditable. */
export function isEditable(target: EventTarget | null): boolean {
  if (!target || typeof (target as Element).closest !== 'function') return false
  const el = target as Element
  const tag = el.tagName
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true
  const editable = el.closest('[contenteditable]')
  return editable !== null && editable.getAttribute('contenteditable') !== 'false'
}

/** The question ids that opened at the tail since `known` was taken: new,
 *  and after `lastId` (the last item before; none: the transcript was
 *  empty). Nothing on a first load (`known` null), or when `lastId` is not
 *  held any more (a resync replaced the items); never a question prepended
 *  from an older page. */
export function freshQuestions(items: readonly Item[], known: ReadonlySet<string> | null, lastId: string | undefined): string[] {
  if (known === null) return []
  const after = lastId === undefined ? -1 : items.findIndex((item) => item.id === lastId)
  if (lastId !== undefined && after < 0) return []
  const fresh: string[] = []
  for (let i = after + 1; i < items.length; i++) {
    const item = items[i]
    if (item.kind === 'question' && !known.has(item.id) && isAnswerable(item)) fresh.push(item.id)
  }
  return fresh
}
