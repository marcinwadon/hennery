// The operator's prompt, shown exactly as sent: plain text with its line
// breaks (no Markdown), and its images from the attachment store when they
// are of a type the page shows.
import { USER_LABEL } from '../../lib/agent'
import { CutNote, Speaker } from './parts'
import type { ItemEnv, ItemOf } from './types'

/** The image types a page shows; anything else is named, not loaded. */
export const SHOWN_IMAGE_TYPES: ReadonlySet<string> = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp'])

/** The attachment store's address for an image, or `null` when the hash is
 *  not one (64 lowercase hex digits): then nothing is loaded. */
export function attachmentHref(sha256: unknown): string | null {
  return typeof sha256 === 'string' && /^[0-9a-f]{64}$/.test(sha256) ? `/api/attachments/${sha256}` : null
}

function Attachment({ mimeType, sha256 }: { mimeType: string; sha256: unknown }) {
  const href = attachmentHref(sha256)
  if (href === null) {
    return (
      <p className="item-note">
        An image of type <bdi>{mimeType}</bdi>, not shown: its reference is not one the attachment store makes.
      </p>
    )
  }
  return <img className="user-image" src={href} alt="An attached image" />
}

export default function UserTurn({ item, env }: { item: ItemOf<'user_turn'>; env: ItemEnv }) {
  return (
    <Speaker who="user" label={USER_LABEL} ts={item.ts}>
      <div className="bubble user">
        {item.content.map((block, i) =>
          block.type === 'text' ? (
            <p key={i} className="user-text">
              {block.text}
            </p>
          ) : block.type === 'image' && SHOWN_IMAGE_TYPES.has(block.mimeType) ? (
            <Attachment key={i} mimeType={block.mimeType} sha256={block.sha256} />
          ) : (
            <p key={i} className="item-note">
              An attachment of type <bdi>{block.type === 'image' ? block.mimeType : String((block as { type?: unknown }).type)}</bdi>, not shown.
            </p>
          ),
        )}
      </div>
      {item.truncated && <CutNote sessionId={env.sessionId}>This prompt was cut.</CutNote>}
    </Speaker>
  )
}
