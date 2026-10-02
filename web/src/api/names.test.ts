import { describe, expect, it } from 'vitest'
import { namesOf } from './names'

describe('namesOf', () => {
  it('maps each entry’s id to its name', () => {
    const hosts = namesOf([{ host_id: 'h1', name: 'build-box' }, { host_id: 'h2', name: 'laptop' }], 'host_id')
    expect([...hosts]).toEqual([['h1', 'build-box'], ['h2', 'laptop']])
    expect([...namesOf([{ id: 'hat1', name: 'Work' }], 'id')]).toEqual([['hat1', 'Work']])
  })

  it.each([null, undefined, {}, { code: 'not_found' }, 'hosts', 3])('gives no names for %o, which is not a list', (value) => {
    expect(namesOf(value, 'id').size).toBe(0)
  })

  it('skips entries without a string id and name', () => {
    const names = namesOf([null, 'x', { id: 1, name: 'n' }, { id: 'a' }, { id: 'b', name: 'B' }], 'id')
    expect([...names]).toEqual([['b', 'B']])
  })
})
