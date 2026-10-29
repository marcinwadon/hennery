// Prompts and family messages retain their plain text and attachments.
// Parent instructions and child reports start collapsed, like tool calls.
import { useState } from 'react'
import { Link } from '../../router'
import { sessionPath } from '../SessionRow'
import { USER_LABEL } from '../../lib/agent'
import { Icon } from '../../lib/ui'
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
  const [open, setOpen] = useState(false)
  const origin = item.origin
  const familyMessage = origin?.kind === 'parent' || origin?.kind === 'child_report'
  const source = origin?.kind === 'parent' ? origin.session_id : origin?.kind === 'child_report' ? origin.child_id : undefined
  const label = origin?.kind === 'parent' ? 'Parent instruction' : origin?.kind === 'child_report' ? 'Child report' : origin?.kind === 'plugin_context' ? 'Plug-in context' : USER_LABEL
  return (
    <Speaker who={source ? 'agent' : 'user'} label={label} ts={item.ts}>
      {source && <Link className="item-note" to={sessionPath(source)}>Source session</Link>}
      {familyMessage && (
        <button type="button" className="tool-line" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          <Icon.Branch size={16} />
          <span className="tool-name">{open ? 'Hide message' : 'Show message'}</span>
        </button>
      )}
      {(!familyMessage || open) && <div className="bubble user">
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
      </div>}
      {item.truncated && <CutNote sessionId={env.sessionId}>{origin?.kind === 'child_report' ? 'This report was cut.' : origin?.kind === 'parent' ? 'This instruction was cut.' : 'This prompt was cut.'}</CutNote>}
    </Speaker>
  )
}
