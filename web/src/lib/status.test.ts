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
    expect(of({ lifecycle: 'archived', activity: undefined })).toEqual({ tone: 'idle', label: 'archived', resume: false })
    expect(of({ lifecycle: '', activity: undefined }).label).toBe('Unknown')
  })
})
