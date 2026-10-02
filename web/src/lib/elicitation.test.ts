import { describe, expect, it } from 'vitest'
import type { Field } from '../generated/view'
import { canSend, choose, contentOf, elicitationBody, toggle, typeText, type Draft } from './elicitation'

// The shape `question.rs` reads from the adapters: a select with one agent's
// exclusive custom answer, a multi select with Codex's note and answer.
const FIELDS: Field[] = [
  { key: 'zeta', label: 'Colour', field_kind: 'single', options: [{ value: 'Red', label: 'Rouge' }, { value: 'Blue' }] },
  { key: 'zeta_custom', label: 'Other', field_kind: 'text', pairing: { with: 'zeta', kind: 'exclusive' } },
  { key: 'alpha', label: 'alpha', field_kind: 'multi', options: [{ value: 'A' }, { value: 'B' }] },
  { key: 'note', label: 'note', field_kind: 'text', pairing: { with: 'alpha', kind: 'note' } },
  { key: 'other', label: 'other', field_kind: 'text', pairing: { with: 'alpha', kind: 'exclusive' } },
]
const REQUIRED = ['zeta', 'alpha']

describe('exclusive pairing', () => {
  it('choosing an option clears its paired text', () => {
    let d: Draft = typeText(FIELDS, {}, 'zeta_custom', 'green')
    d = choose(FIELDS, d, 'zeta', 'Red')
    expect(d).toEqual({ zeta: 'Red' })
  })

  it('typing a real answer clears the option it pairs with', () => {
    let d: Draft = choose(FIELDS, {}, 'zeta', 'Red')
    d = typeText(FIELDS, d, 'zeta_custom', 'green')
    expect(d).toEqual({ zeta_custom: 'green' })
  })

  it('whitespace clears nothing, and is held as typed', () => {
    let d: Draft = choose(FIELDS, {}, 'zeta', 'Red')
    d = typeText(FIELDS, d, 'zeta_custom', '  ')
    expect(d).toEqual({ zeta: 'Red', zeta_custom: '  ' })
  })

  it('toggling a multi option clears its paired text', () => {
    let d: Draft = typeText(FIELDS, {}, 'other', 'C')
    d = toggle(FIELDS, d, 'alpha', 'A')
    expect(d).toEqual({ alpha: ['A'] })
    d = typeText(FIELDS, d, 'other', 'C')
    expect(d).toEqual({ other: 'C' })
  })
})

describe('note pairing', () => {
  it('a note and its select live side by side', () => {
    let d: Draft = toggle(FIELDS, {}, 'alpha', 'B')
    d = typeText(FIELDS, d, 'note', 'why')
    d = toggle(FIELDS, d, 'alpha', 'A')
    expect(d).toEqual({ alpha: ['B', 'A'], note: 'why' })
  })
})

describe('multi select', () => {
  it('toggles off, and an empty list is no answer', () => {
    let d: Draft = toggle(FIELDS, {}, 'alpha', 'A')
    d = toggle(FIELDS, d, 'alpha', 'A')
    expect(d).toEqual({})
  })
})

describe('content', () => {
  it('carries real answers only, text trimmed', () => {
    const d: Draft = { zeta: 'Red', zeta_custom: '   ', alpha: [], note: ' why ', unknown: 'x' }
    expect(contentOf(FIELDS, d)).toEqual({ zeta: 'Red', note: 'why' })
  })

  it('a list stays a list', () => {
    expect(contentOf(FIELDS, { alpha: ['A', 'B'] })).toEqual({ alpha: ['A', 'B'] })
  })
})

describe('Send', () => {
  it('needs one real answer', () => {
    expect(canSend(FIELDS, undefined, true, {})).toBe(false)
    expect(canSend(FIELDS, undefined, true, { note: '   ' })).toBe(false)
    expect(canSend(FIELDS, undefined, true, { note: 'x' })).toBe(true)
  })

  it('needs every required key', () => {
    expect(canSend(FIELDS, REQUIRED, true, { zeta: 'Red' })).toBe(false)
    expect(canSend(FIELDS, REQUIRED, true, { zeta: 'Red', alpha: ['A'] })).toBe(true)
  })

  it('takes an exclusive text as the answer to the required key it pairs with', () => {
    expect(canSend(FIELDS, REQUIRED, true, { zeta_custom: 'green', alpha: ['A'] })).toBe(true)
    expect(canSend(FIELDS, REQUIRED, true, { zeta_custom: '  ', alpha: ['A'] })).toBe(false)
  })

  it('never takes a note as the answer to a required key', () => {
    expect(canSend(FIELDS, REQUIRED, true, { zeta: 'Red', note: 'why' })).toBe(false)
  })

  it('is never offered for a form the card cannot fill', () => {
    expect(canSend(FIELDS, undefined, false, { note: 'x' })).toBe(false)
  })
})

describe('bodies', () => {
  it('accept carries the content', () => {
    expect(elicitationBody('accept', { zeta: 'Red' })).toEqual({ action: 'accept', content: { zeta: 'Red' } })
  })

  it.each(['decline', 'cancel'] as const)('%s carries no content key', (action) => {
    const body = elicitationBody(action, { zeta: 'Red' })
    expect(body).toEqual({ action })
    expect('content' in body).toBe(false)
  })
})
