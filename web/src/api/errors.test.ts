import { describe, expect, it } from 'vitest'
import { ApiFailure } from './errors'

describe('ApiFailure', () => {
  it('gives its own wording for a code it knows', () => {
    expect(new ApiFailure(409, { code: 'host_offline', message: 'srv' }).message).toBe('The host is offline.')
  })

  it('gives the server’s message for a code named like an Object property', () => {
    for (const code of ['constructor', '__proto__', 'toString', 'hasOwnProperty']) {
      const err = new ApiFailure(400, { code, message: `srv-${code}` })
      expect(err.message).toBe(`srv-${code}`)
      expect(err.code).toBe(code)
    }
  })
})
