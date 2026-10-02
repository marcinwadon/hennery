import { describe, expect, it } from 'vitest'
import type { TurnContent } from '../generated/view'
import { json, routed } from '../test-stream'
import { undeliveredDraft, type DraftPart } from './sendAgain'

const SHA = 'ab/c?d'
const TURN = '/api/view/sessions/s%2F1/turns/t%231'

/** Bytes past one chunk of an encoder, none of them alike in a row. */
const BYTES = Uint8Array.from({ length: 0x8000 * 2 + 17 }, (_, i) => (i * 31 + 7) % 256)

function server(turn: TurnContent, attachment: () => Response = () => new Response(BYTES, { status: 200 })) {
  return routed(async (call) => {
    if (call.path === TURN) return json(turn)
    if (call.path.startsWith('/api/attachments/')) return attachment()
    return json({ code: 'not_found', message: 'no' }, 404)
  })
}

async function shown(parts: DraftPart[]) {
  return Promise.all(
    parts.map(async (p) =>
      p.type === 'text' ? p : { type: 'image', mimeType: p.file.type, bytes: new Uint8Array(await p.file.arrayBuffer()) },
    ),
  )
}

describe('undeliveredDraft', () => {
  it('rebuilds the stored prompt in order, its images fetched back by hash, and sends nothing', async () => {
    const s = server({
      turn_id: 't#1',
      content: [
        { type: 'text', text: 'look' },
        { type: 'image', mimeType: 'image/png', sha256: SHA, size: BYTES.length },
        { type: 'text', text: 'again' },
      ],
    })
    const parts = await undeliveredDraft(s.client, 's/1', 't#1')
    expect(await shown(parts)).toEqual([
      { type: 'text', text: 'look' },
      { type: 'image', mimeType: 'image/png', bytes: BYTES },
      { type: 'text', text: 'again' },
    ])
    expect(s.calls.map((c) => c.path)).toEqual([TURN, '/api/attachments/ab%2Fc%3Fd'])
    expect(s.calls.every((c) => c.method === 'GET')).toBe(true)
  })

  it('rejects when an image is gone', async () => {
    const s = server(
      { turn_id: 't#1', content: [{ type: 'image', mimeType: 'image/png', sha256: SHA, size: 1 }] },
      () => json({ code: 'not_found', message: 'no' }, 404),
    )
    await expect(undeliveredDraft(s.client, 's/1', 't#1')).rejects.toMatchObject({ code: 'attachment_gone' })
  })

  it('rejects, reading no image, when a stored image is not of a type a prompt takes', async () => {
    const s = server({
      turn_id: 't#1',
      content: [
        { type: 'image', mimeType: 'image/png', sha256: SHA, size: 1 },
        { type: 'image', mimeType: 'image/svg+xml', sha256: SHA, size: 1 },
      ],
    })
    await expect(undeliveredDraft(s.client, 's/1', 't#1')).rejects.toThrow(/cannot be sent again/)
    expect(s.calls.some((c) => c.path.startsWith('/api/attachments/'))).toBe(false)
  })

  it('rejects with the refusal of the turn itself', async () => {
    const t = routed(async () => json({ code: 'not_found', message: 'srv' }, 404))
    await expect(undeliveredDraft(t.client, 's/1', 't#1')).rejects.toMatchObject({ status: 404 })
  })
})
