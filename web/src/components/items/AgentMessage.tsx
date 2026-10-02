// The agent's reply, as Markdown.
import Markdown from '../Markdown'
import { CutNote, Speaker } from './parts'
import type { ItemEnv, ItemOf } from './types'

export default function AgentMessage({ item, env }: { item: ItemOf<'message'>; env: ItemEnv }) {
  return (
    <Speaker who="agent" label={env.agent} ts={item.ts}>
      <div className="bubble">
        <Markdown>{item.text}</Markdown>
      </div>
      {item.truncated && <CutNote sessionId={env.sessionId}>This message was cut.</CutNote>}
    </Speaker>
  )
}
