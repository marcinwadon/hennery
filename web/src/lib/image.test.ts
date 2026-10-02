import { describe, it, expect } from 'vitest'
import { isAllowedImage, MAX_IMAGE_BYTES, fileToBase64 } from './image'

describe('isAllowedImage', () => {
  it('accepts png/jpeg/gif/webp under the size cap', () => {
    expect(isAllowedImage(new File(['x'], 'a.png', { type: 'image/png' }))).toBe(null)
  })
  it('accepts each allowed type, and an image of exactly the cap', () => {
    for (const type of ['image/png', 'image/jpeg', 'image/gif', 'image/webp']) {
      expect(isAllowedImage(new File(['x'], 'a', { type }))).toBe(null)
    }
    const max = new File([new Uint8Array(MAX_IMAGE_BYTES)], 'a.png', { type: 'image/png' })
    expect(isAllowedImage(max)).toBe(null)
  })
  it('rejects an svg, which is an image but not an allowed one', () => {
    expect(isAllowedImage(new File(['<svg/>'], 'a.svg', { type: 'image/svg+xml' }))).toMatch(/type/i)
  })
  it('rejects non-image types', () => {
    const err = isAllowedImage(new File(['x'], 'a.pdf', { type: 'application/pdf' }))
    expect(err).toMatch(/type/i)
  })
  it('rejects oversize images', () => {
    const big = new File([new Uint8Array(MAX_IMAGE_BYTES + 1)], 'a.png', { type: 'image/png' })
    expect(isAllowedImage(big)).toMatch(/large/i)
  })
})

describe('fileToBase64', () => {
  it('strips the data: prefix', async () => {
    const b64 = await fileToBase64(new File(['ABC'], 'a.png', { type: 'image/png' }))
    expect(b64).toBe(btoa('ABC'))
  })
})
