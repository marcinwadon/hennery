import { beforeEach, describe, expect, it } from 'vitest'
import { forgetAllSends, sendingFor, track } from './sending'

beforeEach(() => forgetAllSends())

describe('sending', () => {
  it('holds a send per session until it ends, and is gone before its outcome is handed on', async () => {
    let finish!: (v: string) => void
    const pending = track('s1', () => new Promise<string>((resolve) => (finish = resolve)))
    expect(sendingFor('s1')).toBe(pending)
    expect(sendingFor('s2')).toBeUndefined()
    const seen = pending.then((outcome) => ({ outcome, held: sendingFor('s1') }))
    finish('sent')
    expect(await seen).toEqual({ outcome: 'sent', held: undefined })
  })

  it('a send that ends after a newer one began leaves the newer one held', async () => {
    let first!: (v: string) => void
    const older = track('s1', () => new Promise<string>((resolve) => (first = resolve)))
    const newer = track('s1', () => new Promise<string>(() => {}))
    first('sent')
    await older
    expect(sendingFor('s1')).toBe(newer)
  })
})
