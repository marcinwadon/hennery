// What the composer says when a prompt, a cancel or a config switch is
// refused (frontend spec §6.5, ACP core §9). The wording is the composer's
// own: these codes mean something else on other routes (`invalid` on the
// session list, say), so they stay out of the app-wide messages.
import { ApiFailure, messageOf } from '../api/errors'
import type { TurnOutcome } from '../generated/protocol'

/** A sentence per code; a function when the server's reason is worth
 *  showing after it, as text. */
type Wording = string | ((serverMessage: string) => string)

const PROMPT: Record<string, Wording> = {
  not_attached: 'The session is not running: resume it to send this.',
  host_offline: 'The host is offline; send again once it is back.',
  turn_in_progress: 'A turn is still running: wait for it to end, or stop it, then send again.',
  images_unsupported: 'This host takes no images: remove them, then send again.',
  empty_prompt: 'There is nothing to send: write something, or attach an image.',
  invalid_content: (why) => `This prompt holds something that cannot be sent (${why}).`,
  invalid: (why) => `The prompt was refused as not valid (${why}).`,
  content_too_large: (why) => `The prompt is too large (${why}).`,
  body_too_large: 'The prompt is too large to send: take out some images, or send them in parts.',
  delivery_unknown: 'Delivery unknown: the outcome shows when the host reconnects.',
}

const CANCEL: Record<string, Wording> = {
  no_open_turn: 'Nothing was running.',
  not_running: 'Nothing was running.',
  not_attached: 'The session is not running: there is nothing to stop.',
  host_offline: 'The host is offline: the turn cannot be stopped from here until it is back.',
}

const CONFIG: Record<string, Wording> = {
  not_attached: 'The session is not running: resume it to change this.',
  unknown_option: 'The agent no longer offers this.',
  invalid: (why) => `The agent refused this value (${why}).`,
  config_failed: (why) => `The agent could not apply it (${why}).`,
  host_offline: 'The host is offline: try again once it is back.',
}

/** What a cancel answers with: the open turn's real outcome. */
export const CANCEL_OUTCOME: Record<TurnOutcome, string> = {
  completed: 'The turn finished before it could be stopped.',
  cancelled: 'Stopped.',
  failed: 'The turn failed before it could be stopped.',
  interrupted: 'The turn was interrupted: the session was parked or closed, or its agent exited.',
}

function say(words: Record<string, Wording>, err: unknown): string {
  // Own keys only: a code is server data (`constructor` is not a wording).
  if (err instanceof ApiFailure && Object.hasOwn(words, err.code)) {
    const wording = words[err.code]
    return typeof wording === 'string' ? wording : wording(err.serverMessage)
  }
  return messageOf(err)
}

/** Why a prompt was not sent. The draft is kept whatever the reason. */
export function promptRefusal(err: unknown): string {
  return say(PROMPT, err)
}

/** Why a cancel stopped nothing. */
export function cancelRefusal(err: unknown): string {
  return say(CANCEL, err)
}

/** Why a config switch was taken back. */
export function configRefusal(err: unknown): string {
  return say(CONFIG, err)
}

/** The draft a question the agent stopped waiting on is answered in, as a
 *  new message (frontend spec §6.3, "Answer as a new message"): the operator
 *  writes the answer after it. A question that ends its own sentence (`?`,
 *  `.`, `!`) gets no second stop. */
export function answerAsMessage(question: string): string {
  const q = question.trimEnd()
  const stop = /[?.!]$/.test(q) ? '' : '.'
  return `You asked: ${q}${stop} My answer: `
}

/** The labels a draft put in for the operator carries until it is sent. */
export const ANSWER_LABEL = 'Answering a question as a new message'
export const SEND_AGAIN_LABEL = 'The turn that was not delivered, back in the draft: send it when ready'
