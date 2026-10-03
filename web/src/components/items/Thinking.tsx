// The agent's reasoning: collapsed until asked for, then Markdown.
import { useState } from 'react'
import { Icon } from '../../lib/ui'
import Markdown from '../Markdown'
import { CutNote, Speaker } from './parts'
import type { ItemEnv, ItemOf } from './types'

export default function Thinking({ item, env }: { item: ItemOf<'thinking'>; env: ItemEnv }) {
  const [open, setOpen] = useState(false)
  return (
    <Speaker who="agent" label={env.agent} ts={item.ts}>
      <button type="button" className="tool-line think-line" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Icon.Sparkle size={16} />
        <span className="tool-name">Thinking</span>
        <span className="tool-summary">{open ? 'Hide' : 'Show reasoning'}</span>
      </button>
      {open && (
        <div className="think-body">
          <Markdown>{item.text}</Markdown>
        </div>
      )}
      {item.truncated && <CutNote sessionId={env.sessionId}>This reasoning was cut.</CutNote>}
    </Speaker>
  )
}
