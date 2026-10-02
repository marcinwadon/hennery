import { describe, expect, it } from 'vitest'
import type { PendingReason } from '../generated/protocol'
import type { Item } from '../generated/view'
import {
  GONE,
  HOST_AWAY_NOTE,
  NOT_OPEN,
  NO_OPTIONS,
  REASON_WORDS,
  TAKEN,
  deliveryOf,
  digitIndex,
  foldVerdict,
  freshQuestions,
  isEditable,
  optionTone,
  questionText,
  type LocalAnswer,
} from './delivery'

type Q = Extract<Item, { kind: 'question' }>
const TS = '2026-10-02T10:00:00.000Z'

function q(patch: Partial<Q> = {}): Q {
  return {
    id: 'question:p1',
    version: 3,
    ts: TS,
    turn_id: 't1',
    kind: 'question',
    pending_id: 'p1',
    question_kind: 'permission',
    request: { type: 'permission', title: 'Run it?', options: [{ option_id: 'a', name: 'Allow', option_kind: 'allow_once' }] },
    answerable: true,
    state: 'open',
    answered: false,
    ...patch,
  } as Q
}

const msg = (id: string): Item => ({ id, version: 1, ts: TS, turn_id: 't1', kind: 'message', text: id }) as Item

describe('deliveryOf: the §6.3 table', () => {
  it('pending, live: the controls, “Needs your answer”', () => {
    expect(deliveryOf(q(), undefined)).toMatchObject({ text: 'Needs your answer', controls: true, busy: false, answerAsMessage: false })
  })

  it('answer queued, no verdict yet: “Sent”', () => {
    const d = deliveryOf(q({ answered: true, answerable: false }), undefined)
    expect(d).toMatchObject({ text: 'Sent', controls: false })
    expect(d.note).toBeUndefined()
  })

  it('answer queued while the host is away: “Sent” and when it delivers', () => {
    expect(deliveryOf(q({ answered: true, answerable: false }), undefined, true)).toMatchObject({ text: 'Sent', note: HOST_AWAY_NOTE })
  })

  it('no host note once a verdict is in, or with no answer', () => {
    expect(deliveryOf(q({ answered: true, answerable: false, delivered: true }), undefined, true).note).toBeUndefined()
    expect(deliveryOf(q(), undefined, true).note).toBeUndefined()
  })

  it('delivered: “Answered”', () => {
    expect(deliveryOf(q({ answered: true, answerable: false, delivered: true }), undefined).text).toBe('Answered')
  })

  it('verdict delivered false: “Sent, but the agent was no longer waiting”', () => {
    expect(deliveryOf(q({ answered: true, answerable: false, delivered: false }), undefined).text).toBe('Sent, but the agent was no longer waiting')
  })

  it('cancelled: “The agent stopped waiting”, and a new message offered', () => {
    expect(deliveryOf(q({ state: 'cancelled', answerable: false }), undefined)).toMatchObject({ text: 'The agent stopped waiting', answerAsMessage: true, controls: false })
  })

  it('cancelled with a verdict not delivered: still stopped waiting', () => {
    expect(deliveryOf(q({ state: 'cancelled', reason: 'host_restarted', answered: true, delivered: false, answerable: false }), undefined)).toMatchObject({
      text: 'The agent stopped waiting (the host restarted)',
      answerAsMessage: true,
    })
  })

  it('cancelled with delivered true: “Answered”, no new message', () => {
    expect(deliveryOf(q({ state: 'cancelled', reason: 'turn_cancelled', answered: true, delivered: true, answerable: false }), undefined)).toMatchObject({
      text: 'Answered',
      answerAsMessage: false,
    })
  })

  it('a permission with no option ids: no controls, and why', () => {
    const none = q({ request: { type: 'permission', options: [] } })
    expect(deliveryOf(none, undefined)).toMatchObject({ text: NO_OPTIONS, controls: false })
  })

  it('state delivered with no verdict: “Answered”', () => {
    expect(deliveryOf(q({ state: 'delivered', answerable: false }), undefined).text).toBe('Answered')
  })

  it('open, not answerable, nothing queued: “Open”', () => {
    expect(deliveryOf(q({ answerable: false }), undefined)).toMatchObject({ text: 'Open', controls: false })
  })
})

describe('deliveryOf: what this tab did', () => {
  const at = (phase: LocalAnswer['phase'], version = 3): LocalAnswer =>
    phase === 'failed' ? { phase, version, message: 'nope' } : ({ phase, version } as LocalAnswer)

  it('in flight: the controls, disabled', () => {
    expect(deliveryOf(q(), at('sending'))).toMatchObject({ text: 'Sending…', controls: true, busy: true })
  })

  it('in flight holds over an upsert that leaves it answerable', () => {
    expect(deliveryOf(q({ version: 4 }), at('sending', 3))).toMatchObject({ controls: true, busy: true })
  })

  it('in flight gives way to an upsert that answered it', () => {
    expect(deliveryOf(q({ version: 4, answered: true, answerable: false }), at('sending', 3))).toMatchObject({ text: 'Sent', busy: false, controls: false })
  })

  it('a 202: “Sent” until the next upsert', () => {
    expect(deliveryOf(q(), at('sent'))).toMatchObject({ text: 'Sent', controls: false })
    expect(deliveryOf(q(), at('sent'), true).note).toBe(HOST_AWAY_NOTE)
    expect(deliveryOf(q({ version: 4 }), at('sent', 3)).text).toBe('Needs your answer')
  })

  it('a 409: “Already answered from another device” until the next upsert', () => {
    expect(deliveryOf(q(), at('taken'))).toMatchObject({ text: TAKEN, controls: false })
    expect(deliveryOf(q({ version: 4, answered: true, answerable: false }), at('taken', 3)).text).toBe('Sent')
  })

  it('a 409 not_open: “This question is no longer open”, never another device, until the next upsert', () => {
    expect(deliveryOf(q(), at('closed'))).toMatchObject({ text: NOT_OPEN, controls: false })
    expect(deliveryOf(q(), at('closed')).text).not.toBe(TAKEN)
    expect(deliveryOf(q({ version: 4, state: 'cancelled', reason: 'turn_cancelled', answerable: false }), at('closed', 3)).text).toBe(
      'The agent stopped waiting (the turn was stopped)',
    )
  })

  it('a 404: “This question is gone”', () => {
    expect(deliveryOf(q(), at('gone'))).toMatchObject({ text: GONE, controls: false })
  })

  it('another refusal: its message, and the controls again', () => {
    expect(deliveryOf(q(), at('failed'))).toMatchObject({ text: 'Needs your answer', controls: true, busy: false, error: 'nope' })
  })
})

describe('cancel reasons in words', () => {
  const words: [PendingReason, string][] = [
    ['turn_cancelled', 'the turn was stopped'],
    ['session_closed', 'the session was closed'],
    ['session_parked', 'the session was parked'],
    ['adapter_lost', 'the agent’s process was lost'],
    ['host_restarted', 'the host restarted'],
    ['agent_withdrew', 'the agent withdrew the question'],
    ['host_revoked', 'the host was revoked'],
  ]

  it('covers all 7 reasons, each once', () => {
    expect(Object.keys(REASON_WORDS).sort()).toEqual(words.map(([r]) => r).sort())
  })

  it.each(words)('reason %s reads “%s”', (reason, text) => {
    expect(deliveryOf(q({ state: 'cancelled', reason, answerable: false }), undefined).text).toBe(`The agent stopped waiting (${text})`)
  })

  it('shows a reason it does not know as sent', () => {
    const reason = 'brand_new' as PendingReason
    expect(deliveryOf(q({ state: 'cancelled', reason, answerable: false }), undefined).text).toBe('The agent stopped waiting (brand_new)')
  })
})

describe('the verdict is monotonic', () => {
  it('a held delivered true is never replaced by false', () => {
    const folded = foldVerdict(q({ answered: true, answerable: false, delivered: true, version: 3 }), q({ answered: true, answerable: false, delivered: false, version: 4 }))
    expect(folded).toMatchObject({ delivered: true, version: 4 })
  })

  it('a held delivered true survives a cancellation', () => {
    const folded = foldVerdict(q({ answered: true, delivered: true, answerable: false }), q({ state: 'cancelled', reason: 'turn_cancelled', answerable: false, version: 4 }))
    expect(folded).toMatchObject({ delivered: true, state: 'cancelled' })
    expect(deliveryOf(folded as Q, undefined).text).toBe('Answered')
  })

  it('a held true makes the question not answerable', () => {
    expect(foldVerdict(q({ delivered: true }), q({ version: 4 }))).toMatchObject({ delivered: true, answerable: false })
  })

  it('false gives way to true, and no verdict to any', () => {
    const t = q({ delivered: true, version: 4 })
    expect(foldVerdict(q({ delivered: false }), t)).toBe(t)
    const f = q({ delivered: false, version: 4 })
    expect(foldVerdict(q(), f)).toBe(f)
  })

  it('leaves other kinds alone', () => {
    const next = msg('a')
    expect(foldVerdict(msg('a'), next)).toBe(next)
  })
})

describe('option tones, by kind only', () => {
  it.each([
    ['allow_once', 'allow'],
    ['allow_always', 'always'],
    ['reject_once', 'reject'],
    ['reject_always', 'reject'],
    ['something_else', 'allow'],
    ['', 'allow'],
  ])('kind %s is %s', (kind, tone) => {
    expect(optionTone(kind)).toBe(tone)
  })
})

describe('the question’s text, for “Answer as a new message”', () => {
  it('is a permission’s title, as asked', () => {
    expect(questionText(q().request)).toBe('Run it?')
  })

  it('names an untitled permission', () => {
    expect(questionText({ type: 'permission', options: [] })).toBe('Permission')
  })

  it('is an elicitation’s message, as asked', () => {
    expect(questionText({ type: 'elicitation', message: 'Which?', fields: [], form_supported: true })).toBe('Which?')
  })
})

describe('digit keys', () => {
  const key = (k: string, target: EventTarget | null = document.body, mods: Partial<Record<'ctrlKey' | 'metaKey' | 'altKey', boolean>> = {}) =>
    digitIndex({ key: k, ctrlKey: false, metaKey: false, altKey: false, target, ...mods })

  it('1–9 pick an option', () => {
    expect(key('1')).toBe(0)
    expect(key('9')).toBe(8)
  })

  it('0, letters and longer keys pick none', () => {
    for (const k of ['0', 'a', 'F1', 'Enter', '10']) expect(key(k)).toBeUndefined()
  })

  it('a modifier picks none', () => {
    expect(key('1', document.body, { ctrlKey: true })).toBeUndefined()
    expect(key('1', document.body, { metaKey: true })).toBeUndefined()
    expect(key('1', document.body, { altKey: true })).toBeUndefined()
  })

  it.each(['input', 'textarea', 'select'])('a %s is editable', (tag) => {
    const el = document.createElement(tag)
    expect(isEditable(el)).toBe(true)
    expect(key('1', el)).toBeUndefined()
  })

  it('anything inside a contenteditable is editable', () => {
    const box = document.createElement('div')
    box.setAttribute('contenteditable', 'true')
    const inner = document.createElement('span')
    box.appendChild(inner)
    expect(isEditable(box)).toBe(true)
    expect(isEditable(inner)).toBe(true)
    expect(key('1', inner)).toBeUndefined()
  })

  it('contenteditable="false" and a button are not', () => {
    const off = document.createElement('div')
    off.setAttribute('contenteditable', 'false')
    expect(isEditable(off)).toBe(false)
    expect(isEditable(document.createElement('button'))).toBe(false)
    expect(isEditable(null)).toBe(false)
  })
})

describe('freshQuestions: what opened at the tail', () => {
  const open = (id: string, patch: Partial<Q> = {}) => q({ id, ...patch })

  it('none on a first load', () => {
    expect(freshQuestions([msg('a'), open('q1')], null, undefined)).toEqual([])
  })

  it('an answerable question appended at the tail', () => {
    expect(freshQuestions([msg('a'), open('q1')], new Set(), 'a')).toEqual(['q1'])
  })

  it('the first item of an empty transcript', () => {
    expect(freshQuestions([open('q1')], new Set(), undefined)).toEqual(['q1'])
  })

  it('none prepended from an older page', () => {
    expect(freshQuestions([open('q0'), msg('a')], new Set(), 'a')).toEqual([])
  })

  it('none after a resync dropped the last item seen', () => {
    expect(freshQuestions([msg('x'), open('q1')], new Set(), 'a')).toEqual([])
  })

  it('none already known, none not answerable, none without options', () => {
    const items = [msg('a'), open('q1'), open('q2', { answerable: false }), open('q3', { request: { type: 'permission', options: [] } })]
    expect(freshQuestions(items, new Set(['q1']), 'a')).toEqual([])
  })
})
