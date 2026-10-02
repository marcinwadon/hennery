import { describe, it, expect } from 'vitest'
import { tokJSON } from './highlight'

describe('tokJSON', () => {
  it('classes keys, strings, redaction, numbers, punctuation', () => {
    const segs = tokJSON('  "model": "opus",')
    expect(segs.find((s) => s.c === 'tk-key')?.t).toBe('"model"')
    expect(segs.find((s) => s.c === 'tk-str')?.t).toBe('"opus"')
  })
  it('marks a redacted value with tk-redact', () => {
    const segs = tokJSON('"key": "•••• redacted"')
    expect(segs.some((s) => s.c === 'tk-redact')).toBe(true)
  })
})
