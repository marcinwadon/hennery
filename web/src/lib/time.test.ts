import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { relTime, clockTime, bucketOf, BUCKET_ORDER, BUCKET_LABEL } from './time'

describe('relTime', () => {
  beforeEach(() => vi.useFakeTimers().setSystemTime(new Date('2026-06-09T12:00:00Z')))
  afterEach(() => vi.useRealTimers())
  it('returns just now under 5s', () => expect(relTime('2026-06-09T11:59:58Z')).toBe('just now'))
  it('seconds', () => expect(relTime('2026-06-09T11:59:30Z')).toBe('30s ago'))
  it('minutes', () => expect(relTime('2026-06-09T11:45:00Z')).toBe('15m ago'))
  it('hours', () => expect(relTime('2026-06-09T09:00:00Z')).toBe('3h ago'))
  it('empty -> empty', () => expect(relTime('')).toBe(''))
  it('counts from the `now` it is given', () =>
    expect(relTime('2026-06-09T11:45:00Z', Date.parse('2026-06-09T13:45:00Z'))).toBe('2h ago'))
})
describe('clockTime', () => {
  it('empty -> empty', () => expect(clockTime('')).toBe(''))
  it('unparseable -> empty', () => expect(clockTime('not a time')).toBe(''))
  it('hours and minutes in the browser’s own form, the same on every call', () => {
    const iso = '2026-06-09T11:45:00Z'
    const expected = new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
    expect(clockTime(iso)).toBe(expected)
    expect(clockTime('2026-06-09T13:05:00Z')).toBe(new Date('2026-06-09T13:05:00Z').toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }))
    expect(clockTime(iso)).toBe(expected)
  })
})
describe('bucketOf', () => {
  // Local-time strings (no trailing Z) on purpose: buckets are local-calendar,
  // not rolling windows.
  const now = new Date('2026-09-11T09:00:00').getTime()

  it('same calendar day is today', () =>
    expect(bucketOf('2026-09-11T00:30:00', now)).toBe('today'))

  it('00:10 is still today when read at 00:30 (the rolling-24h bug)', () =>
    expect(
      bucketOf('2026-09-11T00:10:00', new Date('2026-09-11T00:30:00').getTime()),
    ).toBe('today'))

  it('23:59 yesterday is yesterday even 31 minutes later', () =>
    expect(
      bucketOf('2026-09-10T23:59:00', new Date('2026-09-11T00:30:00').getTime()),
    ).toBe('yesterday'))

  it('2 to 6 days ago is this week', () =>
    expect(bucketOf('2026-09-07T12:00:00', now)).toBe('week'))

  it('6 days ago is still this week', () =>
    expect(bucketOf('2026-09-05T12:00:00', now)).toBe('week'))

  it('7 days ago is this month', () =>
    expect(bucketOf('2026-09-04T12:00:00', now)).toBe('month'))

  it('29 days ago is still this month', () =>
    expect(bucketOf('2026-08-13T12:00:00', now)).toBe('month'))

  it('30 days ago is earlier', () =>
    expect(bucketOf('2026-08-12T12:00:00', now)).toBe('earlier'))

  it('empty is earlier', () => expect(bucketOf('', now)).toBe('earlier'))
  it('unparseable is earlier', () => expect(bucketOf('nope', now)).toBe('earlier'))
  it('a future timestamp is today, not earlier', () =>
    expect(bucketOf('2026-09-12T00:00:00', now)).toBe('today'))

  // The DST tests below mean something only in a zone that shifts on these
  // dates: vite.config.ts sets TZ to Europe/Warsaw. A runner that ignores it
  // fails here instead of passing them vacuously.
  it('runs in a zone with daylight saving time', () => {
    expect(new Date('2026-03-29T00:00:00').getTimezoneOffset()).not.toBe(
      new Date('2026-03-30T00:00:00').getTimezoneOffset(),
    )
  })

  // Europe/Warsaw springs forward 2026-03-29, so 03-29 midnight to 03-30
  // midnight is 23h. Math.floor(23h / 24h) is 0, which would label yesterday
  // "today". Math.round is why the first passes; the fall-back day is 25h,
  // which floors to 1 anyway.
  it('survives a DST spring-forward boundary', () =>
    expect(
      bucketOf('2026-03-29T12:00:00', new Date('2026-03-30T09:00:00').getTime()),
    ).toBe('yesterday'))

  it('survives a DST fall-back boundary', () =>
    expect(
      bucketOf('2026-10-25T12:00:00', new Date('2026-10-26T09:00:00').getTime()),
    ).toBe('yesterday'))
})

describe('BUCKET_ORDER / BUCKET_LABEL', () => {
  it('labels every bucket in display order', () => {
    expect(BUCKET_ORDER.map((b) => BUCKET_LABEL[b])).toEqual([
      'Today',
      'Yesterday',
      'This week',
      'This month',
      'Earlier',
    ])
  })
})
