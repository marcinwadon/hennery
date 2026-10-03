// "Send again" for a `turn_not_delivered` marker (frontend spec §6.5, 4a-ii
// O-6): the prompt as it was stored, whole, put back into the composer as a
// draft for the operator to send. Nothing is sent from here: a refill never
// sends on its own. Its images are the owner's attachments, fetched back by
// hash as files; the composer encodes them when the draft is sent.
// Nothing comes back unless every block could be rebuilt.
import type { Client } from '../api/client'
import { ApiFailure } from '../api/errors'
import { undeliveredTurn } from '../api/view'
import { ALLOWED_IMAGE_TYPES } from './image'

/** A block of the turn as the composer takes it back: a run of text, or an
 *  image as a file. */
export type DraftPart = { type: 'text'; text: string } | { type: 'image'; file: File }

/** `GET /api/attachments/{sha256}` as a file of type `mimeType`. */
async function attachment(client: Client, sha256: string, mimeType: string, n: number, signal: AbortSignal): Promise<File> {
  const response = await client.open(`/api/attachments/${encodeURIComponent(sha256)}`, { Accept: 'image/*' }, signal)
  if (!response.ok) {
    void response.body?.cancel().catch(() => {})
    throw new ApiFailure(response.status, {
      code: response.status === 404 ? 'attachment_gone' : `http_${response.status}`,
      message: `An image of this prompt could not be read (${response.status}).`,
    })
  }
  const bytes = await response.arrayBuffer()
  return new File([bytes], `image-${n}`, { type: mimeType })
}

/** The undelivered turn `turnId` of session `sessionId`, rebuilt as draft
 *  parts in order. Rejects, having read no image, when a block cannot be
 *  sent again; rejects when the turn or one of its images cannot be read. */
export async function undeliveredDraft(
  client: Client,
  sessionId: string,
  turnId: string,
  signal: AbortSignal = new AbortController().signal,
): Promise<DraftPart[]> {
  const turn = await undeliveredTurn(client, sessionId, turnId, signal)
  for (const block of turn.content) {
    if (block.type === 'text') continue
    if (block.type === 'image' && ALLOWED_IMAGE_TYPES.has(block.mimeType)) continue
    throw new Error('This prompt holds content that cannot be sent again.')
  }
  const parts: DraftPart[] = []
  let n = 0
  for (const block of turn.content) {
    if (block.type === 'text') parts.push({ type: 'text', text: block.text })
    else if (block.type === 'image') {
      parts.push({ type: 'image', file: await attachment(client, block.sha256, block.mimeType, ++n, signal) })
    }
  }
  return parts
}
