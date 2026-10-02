// What every item renderer is given besides its item: the session it is in
// and the seams later tasks fill (answering a question, sending a turn
// again).
import type { ReactNode } from 'react'
import type { Item } from '../../generated/view'

export type ItemOf<K extends Item['kind']> = Extract<Item, { kind: K }>

export interface ItemEnv {
  sessionId: string
  /** The agent's label (lib/agent.ts): who speaks for the session. */
  agent: string
  /** A hat's name by its id, when the hats are known. */
  hatName?: (id: string) => string | undefined
  /** A question card's actions; none makes the card read-only. */
  questionActions?: (item: ItemOf<'question'>) => ReactNode
  /** "Send again" for a turn that was not delivered; absent, none is offered. */
  onSendAgain?: (item: ItemOf<'marker'>) => void
}

/** The session's raw events, where a cut or elided item can be read whole. */
export function rawEventsHref(sessionId: string): string {
  return `/api/sessions/${encodeURIComponent(sessionId)}/events`
}
