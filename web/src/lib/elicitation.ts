// An elicitation form's draft (frontend spec §6.3), as pure functions: what
// the operator chose or typed, per field key, and the answer it makes.
//
// - A draft holds the text as typed (an input shows what it holds); only
//   the answer trims it, and whitespace alone is no answer.
// - A text field paired `exclusive` with a select answers instead of it
//   (the adapter gives the text precedence on the wire): choosing an option
//   clears the text, and typing a real answer clears the option. A `note`
//   is free text alongside and clears nothing.
// - The answer carries only real answers: `{key: string | string[]}`.
import type { ElicitationAction } from '../generated/protocol'
import type { Field } from '../generated/view'

export type Draft = Readonly<Record<string, string | readonly string[]>>

/** The keys of the text fields paired `exclusive` with `key`. */
function exclusiveTexts(fields: readonly Field[], key: string): string[] {
  return fields.filter((f) => f.pairing?.kind === 'exclusive' && f.pairing.with === key).map((f) => f.key)
}

function without(draft: Draft, keys: readonly string[]): Record<string, string | readonly string[]> {
  const next = { ...draft }
  for (const key of keys) delete next[key]
  return next
}

/** A single select's choice: clears the text paired `exclusive` with it. */
export function choose(fields: readonly Field[], draft: Draft, key: string, value: string): Draft {
  return { ...without(draft, exclusiveTexts(fields, key)), [key]: value }
}

/** A multi select's option on or off: clears the text paired `exclusive`
 *  with it. */
export function toggle(fields: readonly Field[], draft: Draft, key: string, value: string): Draft {
  const held = draft[key]
  const now = Array.isArray(held) ? held : []
  const next = now.includes(value) ? now.filter((v) => v !== value) : [...now, value]
  const rest = without(draft, [key, ...exclusiveTexts(fields, key)])
  return next.length > 0 ? { ...rest, [key]: next } : rest
}

/** Text typed into `key`: held as typed. A real answer in a text paired
 *  `exclusive` clears the select it pairs with; whitespace does not. */
export function typeText(fields: readonly Field[], draft: Draft, key: string, text: string): Draft {
  const field = fields.find((f) => f.key === key)
  const pair = field?.pairing?.kind === 'exclusive' && text.trim() !== '' ? [field.pairing.with] : []
  return { ...without(draft, pair), [key]: text }
}

/** The draft's real answer for `key`, if any: trimmed text, a non-empty
 *  choice list. */
function answerOf(draft: Draft, key: string): string | string[] | undefined {
  const value = draft[key]
  if (typeof value === 'string') return value.trim() === '' ? undefined : value.trim()
  if (Array.isArray(value)) return value.length > 0 ? [...value] : undefined
  return undefined
}

/** The accepted form's `content`: every real answer, and nothing else. */
export function contentOf(fields: readonly Field[], draft: Draft): Record<string, string | string[]> {
  const content: Record<string, string | string[]> = {}
  for (const field of fields) {
    const answer = answerOf(draft, field.key)
    // Defined, never assigned: the keys are the agent's, and `__proto__`
    // assigned would set the object's prototype and drop the answer.
    if (answer !== undefined) {
      Object.defineProperty(content, field.key, { value: answer, enumerable: true, writable: true, configurable: true })
    }
  }
  return content
}

/** Whether a required `key` is answered: by its own value, or by the real
 *  text of a field paired `exclusive` with it (that text answers instead). */
function requiredMet(fields: readonly Field[], draft: Draft, key: string): boolean {
  if (answerOf(draft, key) !== undefined) return true
  return exclusiveTexts(fields, key).some((k) => answerOf(draft, k) !== undefined)
}

/** Whether two of the agent's fields share a key. The answer's `content`
 *  is keyed by field, so such a form cannot carry an answer to each, and
 *  the card does not fill it in: it can only be declined or cancelled. */
export function hasDuplicateKeys(fields: readonly Field[]): boolean {
  const seen = new Set<string>()
  for (const field of fields) {
    if (seen.has(field.key)) return true
    seen.add(field.key)
  }
  return false
}

/** Send is offered once one real answer exists and every required key has
 *  one; and only for a form the card can fill (supported, each key once). */
export function canSend(fields: readonly Field[], required: readonly string[] | undefined, formSupported: boolean, draft: Draft): boolean {
  if (!formSupported || hasDuplicateKeys(fields)) return false
  if (Object.keys(contentOf(fields, draft)).length === 0) return false
  return (required ?? []).every((key) => requiredMet(fields, draft, key))
}

/** The answer's body for each action: content only with `accept`. */
export function elicitationBody(
  action: ElicitationAction,
  content?: Record<string, string | string[]>,
): { action: ElicitationAction; content?: Record<string, string | string[]> } {
  return action === 'accept' ? { action, content: content ?? {} } : { action }
}
