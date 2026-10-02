// A session's items, in the order the store holds them (the collector folds
// the events, D1): one renderer per kind, each in its own error boundary
// keyed by the item's id.
//
// A row renders again only when its item (a new object for each version the
// store takes) or the session's environment changes: an upsert at the tail
// does not parse every message's Markdown again.
import { memo } from 'react'
import type { Item } from '../generated/view'
import ItemBoundary from './ItemBoundary'
import AgentMessage from './items/AgentMessage'
import Marker from './items/Marker'
import PlanItem from './items/PlanItem'
import QuestionCard from './items/QuestionCard'
import Thinking from './items/Thinking'
import ToolCall from './items/ToolCall'
import Unrecognised from './items/Unrecognised'
import UserTurn from './items/UserTurn'
import type { ItemEnv } from './items/types'

export function ItemView({ item, env }: { item: Item; env: ItemEnv }) {
  switch (item.kind) {
    case 'user_turn':
      return <UserTurn item={item} env={env} />
    case 'message':
      return <AgentMessage item={item} env={env} />
    case 'thinking':
      return <Thinking item={item} env={env} />
    case 'tool_call':
      return <ToolCall item={item} env={env} />
    case 'plan':
      return <PlanItem item={item} env={env} />
    case 'question':
      return <QuestionCard item={item} env={env} />
    case 'marker':
      return <Marker item={item} env={env} />
    case 'unrecognised':
      return <Unrecognised item={item} env={env} />
    default: {
      // A kind from a newer server: named, never dropped.
      const kind = String((item as { kind?: unknown }).kind)
      return (
        <details className="unrec fade-in">
          <summary>
            Unsupported item (<bdi>{kind}</bdi>)
          </summary>
        </details>
      )
    }
  }
}

/** A tool call's `fabricated` as text, read outside its renderer so the
 *  boundary can show it whatever the renderer does. */
export function fabricatedOf(item: Item): string | undefined {
  if (item.kind !== 'tool_call') return undefined
  const fabricated = (item as { fabricated?: unknown }).fabricated
  if (fabricated === undefined) return undefined
  return typeof fabricated === 'string' ? fabricated : ''
}

const Row = memo(function Row({ item, env }: { item: Item; env: ItemEnv }) {
  return (
    <ItemBoundary version={item.version} fabricated={fabricatedOf(item)}>
      <ItemView item={item} env={env} />
    </ItemBoundary>
  )
})

export default function Transcript({ items, env }: { items: Item[]; env: ItemEnv }) {
  if (items.length === 0) return <p className="transcript-empty">Nothing in this session yet.</p>
  return (
    <>
      {items.map((item) => (
        <Row key={item.id} item={item} env={env} />
      ))}
    </>
  )
}
