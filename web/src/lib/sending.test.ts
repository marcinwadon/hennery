import { beforeEach, describe, expect, it } from 'vitest'
import { forgetAllSends, sendingFor, track } from './sending'

beforeEach(() => forgetAllSends())

describe('sending', () => {
  it('holds a send per session until it ends, and is gone before its outcome is handed on', async () => {
    let finish!: (v: string) => void
    const pending = track('s1', () => new Promise<string>((resolve) => (finish = resolve)), String)
    expect(sendingFor('s1')).toBe(pending)
    expect(sendingFor('s2')).toBeUndefined()
    const seen = pending.then((outcome) => ({ outcome, held: sendingFor('s1') }))
    finish('sent')
    expect(await seen).toEqual({ outcome: 'sent', held: undefined })
  })

  it('a send that ends after a newer one began leaves the newer one held', async () => {
    let first!: (v: string) => void
    const older = track('s1', () => new Promise<string>((resolve) => (first = resolve)), String)
    const newer = track('s1', () => new Promise<string>(() => {}), String)
    first('sent')
    await older
    expect(sendingFor('s1')).toBe(newer)
  })

  it('a send whose work rejects ends too, with the outcome its error is given', async () => {
    const pending = track('s1', () => Promise.reject(new Error('boom')), (err) => `refused: ${(err as Error).message}`)
    expect(await pending).toBe('refused: boom')
    expect(sendingFor('s1')).toBeUndefined()
  })

  it('a send let go of still ends, and leaves nothing held', async () => {
    let finish!: (v: string) => void
    const pending = track('s1', () => new Promise<string>((resolve) => (finish = resolve)), String)
    forgetAllSends()
    expect(sendingFor('s1')).toBeUndefined()
    finish('sent')
    expect(await pending).toBe('sent')
    expect(sendingFor('s1')).toBeUndefined()
  })
})
