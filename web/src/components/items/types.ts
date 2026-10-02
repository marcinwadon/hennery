// What every item renderer is given besides its item: the session it is in,
// what answers its questions, and the composer's seams (its emptiness, its
// prefill, sending a turn again).
import type { Item } from '../../generated/view'
import type { AnswerBook } from '../../store/useAnswer'

export type ItemOf<K extends Item['kind']> = Extract<Item, { kind: K }>

export interface ItemEnv {
  sessionId: string
  /** The agent's label (lib/agent.ts): who speaks for the session. */
  agent: string
  /** A hat's name by its id, when the hats are known. */
  hatName?: (id: string) => string | undefined
  /** Answers the session's questions; none makes every card read-only. */
  answers?: AnswerBook
  /** "Send again" for a turn that was not delivered; absent, none is offered. */
  onSendAgain?: (item: ItemOf<'marker'>) => void
  /** "Answer as a new message" for a question the agent stopped waiting
   *  on; absent, none is offered. `question` is the question's own text,
   *  nothing more: the view words the composer's draft around it
   *  (composerWords' `answerAsMessage`), labelled, the cursor at its end.
   *  Nothing is sent: the operator writes the answer and sends it. */
  onAnswerAsMessage?: (question: string) => void
  /** The composer holds no draft (no text, no image): only then may a newly
   *  opened answerable card take the focus (brief item 25, §10). Absent,
   *  no card ever does. */
  composerEmpty?: () => boolean
}

/** The session's raw events, where a cut or elided item can be read whole. */
export function rawEventsHref(sessionId: string): string {
  return `/api/sessions/${encodeURIComponent(sessionId)}/events`
}
