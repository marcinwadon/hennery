// A draft's images (frontend spec §6.5, ACP core §7 and §11): the limits,
// the `[Image #N]` markers, the blocks a prompt is sent as, and where each
// session's images wait while another session is shown (F-17).
//
// The text is the source of truth: an image is sent only while its marker
// is in the text, at the place of its first marker.
import type { PromptBlock } from '../api/turns'
import { ALLOWED_IMAGE_TYPES, MAX_IMAGE_BYTES, fileToBase64 } from './image'

export const MAX_IMAGES = 20
export const MAX_TOTAL_BYTES = 16 * 1024 * 1024 // 16 MiB decoded per prompt

export interface Attachment {
  /** Its number in `[Image #N]`; never reused within a draft. */
  n: number
  file: File
}

export function marker(n: number): string {
  return `[Image #${n}]`
}

const MARKER = /\[Image #(\d+)\]/g

/** The greatest N any marker in `text` names (0 for none): a draft restored
 *  from storage numbers its next image past it, so a marker left in the
 *  text never comes to name a new image. */
export function highestMarker(text: string): number {
  let max = 0
  for (const m of text.matchAll(MARKER)) max = Math.max(max, Number(m[1]))
  return max
}

export interface Admitted {
  accepted: Attachment[]
  /** One sentence per file refused, in the order given. */
  refused: string[]
}

/** Which of `files` may join `held`, numbered from `nextN`: an allowed type,
 *  at most 5 MiB each, at most 20 images and 16 MiB in all. */
export function admit(held: readonly Attachment[], files: readonly File[], nextN: number): Admitted {
  const accepted: Attachment[] = []
  const refused: string[] = []
  let count = held.length
  let total = held.reduce((sum, a) => sum + a.file.size, 0)
  let n = nextN
  for (const file of files) {
    const name = file.name || 'image'
    if (!ALLOWED_IMAGE_TYPES.has(file.type)) {
      refused.push(`${name} is not a PNG, JPEG, GIF or WebP image.`)
    } else if (file.size > MAX_IMAGE_BYTES) {
      refused.push(`${name} is over 5 MiB.`)
    } else if (count >= MAX_IMAGES) {
      refused.push(`${name}: a prompt takes at most ${MAX_IMAGES} images.`)
    } else if (total + file.size > MAX_TOTAL_BYTES) {
      refused.push(`${name}: a prompt takes at most 16 MiB of images in all.`)
    } else {
      accepted.push({ n: n++, file })
      count++
      total += file.size
    }
  }
  return { accepted, refused }
}

/** `text` with `inserted` placed between `start` and `end` (the selection),
 *  and where the cursor goes after it. */
export function splice(text: string, start: number, end: number, inserted: string): { text: string; cursor: number } {
  const a = Math.max(0, Math.min(start, text.length))
  const b = Math.max(a, Math.min(end, text.length))
  return { text: text.slice(0, a) + inserted + text.slice(b), cursor: a + inserted.length }
}

/** `text` without the first `[Image #n]` marker (and one space after it). */
export function withoutMarker(text: string, n: number): string {
  const at = text.indexOf(marker(n))
  if (at < 0) return text
  let end = at + marker(n).length
  if (text[end] === ' ') end++
  return text.slice(0, at) + text.slice(end)
}

/** The attachments whose marker is still in `text`. */
export function live(text: string, held: readonly Attachment[]): Attachment[] {
  return held.filter((a) => text.includes(marker(a.n)))
}

/** The prompt as ordered blocks: runs of text, and each live image in place
 *  of its first marker. A marker that names no live image, or repeats one,
 *  stays text. Runs of only whitespace between images are dropped. */
export async function promptBlocks(text: string, held: readonly Attachment[]): Promise<PromptBlock[]> {
  const byN = new Map(held.map((a) => [a.n, a]))
  const placed = new Set<number>()
  const blocks: PromptBlock[] = []
  let run = ''
  let from = 0
  const flush = () => {
    if (run.trim() !== '') blocks.push({ type: 'text', text: run })
    run = ''
  }
  for (const m of text.matchAll(MARKER)) {
    const n = Number(m[1])
    const attachment = byN.get(n)
    run += text.slice(from, m.index)
    from = m.index + m[0].length
    if (!attachment || placed.has(n)) {
      run += m[0]
      continue
    }
    placed.add(n)
    flush()
    blocks.push({ type: 'image', mimeType: attachment.file.type, data: await fileToBase64(attachment.file) })
  }
  run += text.slice(from)
  flush()
  return blocks
}

// Each session's images while the composer shows another session: the
// composer is remounted per session (`key={sessionId}`), so its state goes,
// and the images wait here, in memory only (they never reach storage).
interface Held {
  attachments: Attachment[]
  nextN: number
}

const bySession = new Map<string, Held>()

export function heldFor(sessionId: string): Held {
  return bySession.get(sessionId) ?? { attachments: [], nextN: 1 }
}

export function hold(sessionId: string, held: Held): void {
  if (held.attachments.length === 0 && held.nextN <= 1) bySession.delete(sessionId)
  else bySession.set(sessionId, held)
}

/** Drop a session's images (it was sent, or the session is gone). */
export function forgetAttachments(sessionId: string): void {
  bySession.delete(sessionId)
}

/** Drop every session's images (sign-out; tests). */
export function forgetAllAttachments(): void {
  bySession.clear()
}
