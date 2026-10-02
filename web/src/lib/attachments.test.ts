import { beforeEach, describe, expect, it } from 'vitest'
import {
  MAX_IMAGES,
  MAX_MARKER,
  MAX_TOTAL_BYTES,
  admit,
  forgetAllAttachments,
  forgetAttachments,
  heldFor,
  highestMarker,
  hold,
  inertMarkers,
  live,
  marker,
  promptBlocks,
  splice,
  withoutMarker,
  type Attachment,
} from './attachments'
import { MAX_IMAGE_BYTES } from './image'

const MiB = 1024 * 1024

/** A file that claims `size` bytes without holding them. */
function sized(name: string, size: number, type = 'image/png'): File {
  const file = new File(['x'], name, { type })
  Object.defineProperty(file, 'size', { value: size })
  return file
}

function held(sizes: number[]): Attachment[] {
  return sizes.map((size, i) => ({ n: i + 1, file: sized(`h${i + 1}.png`, size) }))
}

beforeEach(() => forgetAllAttachments())

describe('admit: image limits', () => {
  it('limit type: takes png, jpeg, gif and webp, and refuses an svg or a pdf', () => {
    const files = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/svg+xml', 'application/pdf'].map((t, i) =>
      sized(`f${i}`, 10, t),
    )
    const { accepted, refused } = admit([], files, 1)
    expect(accepted.map((a) => a.file.type)).toEqual(['image/png', 'image/jpeg', 'image/gif', 'image/webp'])
    expect(refused).toEqual(['f4 is not a PNG, JPEG, GIF or WebP image.', 'f5 is not a PNG, JPEG, GIF or WebP image.'])
  })

  it('limit size: takes an image of exactly 5 MiB, refuses one byte more', () => {
    const { accepted, refused } = admit([], [sized('ok.png', MAX_IMAGE_BYTES), sized('big.png', MAX_IMAGE_BYTES + 1)], 1)
    expect(accepted.map((a) => a.file.name)).toEqual(['ok.png'])
    expect(refused).toEqual(['big.png is over 5 MiB.'])
  })

  it('limit count: takes the 20th image, refuses the 21st', () => {
    const { accepted, refused } = admit(held(Array(19).fill(10)), [sized('20th.png', 10), sized('21st.png', 10)], 20)
    expect(MAX_IMAGES).toBe(20)
    expect(accepted.map((a) => a.file.name)).toEqual(['20th.png'])
    expect(refused).toEqual(['21st.png: a prompt takes at most 20 images.'])
  })

  it('limit total: takes images up to exactly 16 MiB in all, refuses one byte more', () => {
    expect(MAX_TOTAL_BYTES).toBe(16 * MiB)
    const three = held([5 * MiB, 5 * MiB, 5 * MiB])
    const exact = admit(three, [sized('last.png', MiB)], 4)
    expect(exact.accepted.map((a) => a.file.name)).toEqual(['last.png'])
    const over = admit(three, [sized('over.png', MiB + 1)], 4)
    expect(over.accepted).toEqual([])
    expect(over.refused).toEqual(['over.png: a prompt takes at most 16 MiB of images in all.'])
  })

  it('limit number: takes image #999999, refuses a seventh digit, saying why', () => {
    const { accepted, refused } = admit([], [sized('last.png', 1), sized('over.png', 1)], MAX_MARKER)
    expect(accepted.map((a) => a.n)).toEqual([999_999])
    expect(refused).toEqual(['over.png: this draft has no image number left; send it first.'])
  })

  it('numbers what it takes from the next number, skipping the refused', () => {
    const { accepted } = admit([], [sized('a', 1), sized('b', 1, 'text/plain'), sized('c', 1)], 7)
    expect(accepted.map((a) => [a.file.name, a.n])).toEqual([
      ['a', 7],
      ['c', 8],
    ])
  })
})

describe('markers', () => {
  it('splice places text over the selection and puts the cursor after it', () => {
    expect(splice('hello world', 6, 11, '[Image #1] ')).toEqual({ text: 'hello [Image #1] ', cursor: 17 })
    expect(splice('ab', 1, 1, 'X')).toEqual({ text: 'aXb', cursor: 2 })
    expect(splice('ab', 9, 9, 'X')).toEqual({ text: 'abX', cursor: 3 })
  })

  it('withoutMarker takes out the first marker and one space after it', () => {
    expect(withoutMarker('a [Image #2] b [Image #2]', 2)).toBe('a b [Image #2]')
    expect(withoutMarker('a [Image #12] b', 1)).toBe('a [Image #12] b')
  })

  it('highestMarker is past every number a draft names', () => {
    expect(highestMarker('x [Image #3] y [Image #11] [Image #2]')).toBe(11)
    expect(highestMarker('none')).toBe(0)
  })

  it('highestMarker reads six digits at most: a longer number, or a leading zero, is plain text', () => {
    // Past 2^53 every such number is the same to Number: images would share one.
    expect(highestMarker('[Image #99999999999999999999]')).toBe(0)
    expect(highestMarker('[Image #1000000]')).toBe(0)
    expect(highestMarker('[Image #999999] [Image #1000000]')).toBe(999_999)
    expect(highestMarker('[Image #01]')).toBe(0)
  })

  it('inertMarkers keeps the words readable but links no image', async () => {
    const words = 'see [Image #1] and [Image #2]'
    const inert = inertMarkers(words)
    expect(inert.replaceAll('\u2060', '')).toBe(words)
    expect(highestMarker(inert)).toBe(0)
    expect(live(inert, held([1, 1]))).toEqual([])
    expect(await promptBlocks(inert, held([1, 1]))).toEqual([{ type: 'text', text: inert }])
  })

  it('live keeps only images whose marker is in the text', () => {
    const atts = held([1, 1, 1])
    expect(live(`${marker(1)} ${marker(3)}`, atts).map((a) => a.n)).toEqual([1, 3])
  })
})

describe('promptBlocks', () => {
  it('sends text runs and images in order, each image at its first marker', async () => {
    const atts: Attachment[] = [
      { n: 1, file: new File(['ONE'], 'a.png', { type: 'image/png' }) },
      { n: 2, file: new File(['TWO'], 'b.gif', { type: 'image/gif' }) },
    ]
    const blocks = await promptBlocks('look [Image #2] then [Image #1] end [Image #1]', atts)
    expect(blocks).toEqual([
      { type: 'text', text: 'look ' },
      { type: 'image', mimeType: 'image/gif', data: btoa('TWO') },
      { type: 'text', text: ' then ' },
      { type: 'image', mimeType: 'image/png', data: btoa('ONE') },
      { type: 'text', text: ' end [Image #1]' },
    ])
  })

  it('sends no image whose marker was deleted, and keeps a marker naming no image as text', async () => {
    const atts: Attachment[] = [{ n: 1, file: new File(['ONE'], 'a.png', { type: 'image/png' }) }]
    expect(await promptBlocks('only text [Image #9]', atts)).toEqual([{ type: 'text', text: 'only text [Image #9]' }])
  })

  it('keeps a marker with a leading zero or seven digits as text', async () => {
    const atts: Attachment[] = [{ n: 1, file: new File(['ONE'], 'a.png', { type: 'image/png' }) }]
    expect(await promptBlocks('[Image #01] [Image #0000001]', atts)).toEqual([{ type: 'text', text: '[Image #01] [Image #0000001]' }])
  })

  it('drops runs of only whitespace between images', async () => {
    const atts: Attachment[] = [
      { n: 1, file: new File(['A'], 'a.png', { type: 'image/png' }) },
      { n: 2, file: new File(['B'], 'b.png', { type: 'image/png' }) },
    ]
    const blocks = await promptBlocks('[Image #1] [Image #2] ', atts)
    expect(blocks.map((b) => b.type)).toEqual(['image', 'image'])
  })
})

describe('held images per session', () => {
  it('keeps each session’s images apart, and forgets one session only', () => {
    hold('a', { attachments: held([1]), nextN: 2 })
    hold('b', { attachments: held([1, 1]), nextN: 3 })
    expect(heldFor('a').attachments).toHaveLength(1)
    forgetAttachments('a')
    expect(heldFor('a')).toEqual({ attachments: [], nextN: 1 })
    expect(heldFor('b').nextN).toBe(3)
  })

  it('remembers the next number even with no image left', () => {
    hold('a', { attachments: [], nextN: 4 })
    expect(heldFor('a').nextN).toBe(4)
  })
})
