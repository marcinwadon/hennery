import { describe, expect, it } from 'vitest'
import { statusOf, type StatusInput } from './status'

const base: StatusInput = { lifecycle: 'active', activity: 'idle', question_waits: false, presumed_parked: false }
const of = (patch: Partial<StatusInput>) => statusOf({ ...base, ...patch })

describe('statusOf', () => {
  it('blocked: waiting on a question, the strongest marker', () => {
    expect(of({ activity: 'blocked' })).toEqual({ tone: 'attn', label: 'Waiting on a question', resume: false })
  })

  it('a question open outside a turn: waiting on a question too', () => {
    expect(of({ activity: 'idle', question_waits: true })).toEqual({
      tone: 'attn',
      label: 'Waiting on a question',
      resume: false,
    })
  })

  it('waiting wins over the lifecycle, so the marker agrees with the count', () => {
    expect(of({ lifecycle: 'parked', question_waits: true }).tone).toBe('attn')
  })

  it('never says "needs you"', () => {
    for (const s of [of({ activity: 'blocked' }), of({ question_waits: true })]) expect(s.label).not.toMatch(/needs you/i)
  })

  it('running: animates', () => {
    expect(of({ activity: 'running' })).toEqual({ tone: 'run', label: 'Running', resume: false })
  })

  it('active and idle', () => {
    expect(of({ activity: 'idle' })).toEqual({ tone: 'idle', label: 'Idle', resume: false })
    expect(of({ activity: undefined })).toEqual({ tone: 'idle', label: 'Idle', resume: false })
  })

  it('starting', () => {
    expect(of({ lifecycle: 'starting', activity: undefined })).toEqual({ tone: 'wait', label: 'Starting', resume: false })
  })

  it('parked: offers Resume', () => {
    expect(of({ lifecycle: 'parked', activity: undefined })).toEqual({ tone: 'idle', label: 'Parked', resume: true })
  })

  it('presumed parked: the host is offline, Resume still offered', () => {
    expect(of({ lifecycle: 'parked', activity: undefined, presumed_parked: true })).toEqual({
      tone: 'idle',
      label: 'Host offline',
      resume: true,
    })
  })

  it('failed', () => {
    expect(of({ lifecycle: 'failed', activity: undefined })).toEqual({ tone: 'fail', label: 'Failed', resume: false })
  })

  it('closed: no Resume on the row', () => {
    expect(of({ lifecycle: 'closed', activity: undefined })).toEqual({ tone: 'idle', label: 'Closed', resume: false })
  })

  it('a lifecycle it does not know: shown by name', () => {
    expect(of({ lifecycle: 'future-state', activity: undefined })).toEqual({ tone: 'idle', label: 'future-state', resume: false })
    expect(of({ lifecycle: '', activity: undefined }).label).toBe('Unknown')
  })
})

it('filters only actual operator work and keeps lifecycle/status predicates separate', async () => {
  const { needsOperator, matchesStatus } = await import('./status')
  const base = { lifecycle: 'closed', question_waits: true, presumed_parked: false, operator_attention: 0, parent_handled_questions: 1 }
  expect(needsOperator(base)).toBe(false)
  expect(needsOperator({ ...base, operator_attention: 1 })).toBe(true)
  expect(needsOperator({ ...base, operator_attention: undefined })).toBe(false)
  expect(matchesStatus(base, 'all')).toBe(true)
  expect(matchesStatus({ ...base, operator_attention: 1 }, 'attention')).toBe(true)
  expect(matchesStatus({ ...base, lifecycle: 'active', activity: 'running' }, 'running')).toBe(true)
  expect(matchesStatus(base, 'running')).toBe(false)
  expect(matchesStatus({ ...base, lifecycle: 'active', activity: 'idle' }, 'idle')).toBe(true)
  expect(matchesStatus(base, 'idle')).toBe(false)
  expect(matchesStatus({ ...base, lifecycle: 'parked' }, 'parked')).toBe(true)
  expect(matchesStatus(base, 'parked')).toBe(false)
  expect(matchesStatus({ ...base, lifecycle: 'failed' }, 'failed')).toBe(true)
  expect(matchesStatus(base, 'failed')).toBe(false)
  const unseen = { ...base, unseen: { event_id: 10 }, review: { event_id: 10 } }
  expect(needsOperator(unseen)).toBe(false)
  expect(matchesStatus(unseen, 'attention')).toBe(false)
  expect(matchesStatus(unseen, 'unseen')).toBe(true)
  expect(matchesStatus(base, 'unseen')).toBe(false)
  expect(statusOf({ ...unseen, question_waits: false, parent_handled_questions: 0, lifecycle: 'active', activity: 'running' }).label).toBe('Running')
  expect(needsOperator({ ...unseen, operator_attention: 1 })).toBe(true)
})

it('shows background independently of foreground and filters quiet idle only', async () => {
  const { matchesStatus } = await import('./status')
  const work = { ...base, background: { active: 2, done: 3, all: 5 } }
  expect(statusOf(work)).toEqual({ tone: 'background', label: 'Background', resume: false })
  expect(statusOf({ ...work, activity: 'running' }).label).toBe('Running')
  expect(statusOf({ ...work, operator_attention: 1 }).label).toBe('Waiting on you')
  expect(statusOf({ ...work, lifecycle: 'parked', presumed_parked: true }).label).toBe('Host offline')
  expect(matchesStatus(work, 'background')).toBe(true)
  expect(matchesStatus(work, 'idle')).toBe(false)
  expect(matchesStatus({ ...work, background: { active: 0, done: 5, all: 5 } }, 'idle')).toBe(true)
})

it('never presents an unrecognized foreground state as confirmed idle', () => {
  expect(of({ activity: 'unknown' })).toEqual({ tone: 'run', label: 'Busy', resume: false })
  expect(of({ activity: 'busy' }).label).toBe('Busy')
})
