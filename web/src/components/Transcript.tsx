// A session's items, in the order the store holds them (the collector folds
// the events, D1): one renderer per kind, each in its own error boundary
// keyed by the item's id.
//
// A row renders again only when its item (a new object for each version the
// store takes) or the session's environment changes: an upsert at the tail
// does not parse every message's Markdown again.
import { Fragment, memo } from 'react'
import { clockTime } from '../lib/time'
import type { Item, ReviewResult } from '../generated/view'
import ResultReadObserver from './ResultReadObserver'
import ResultReview from './ResultReview'
import ItemBoundary from './ItemBoundary'
import AgentMessage from './items/AgentMessage'
import Marker from './items/Marker'
import PlanItem from './items/PlanItem'
import PluginContext from './items/PluginContext'
import QuestionCard from './items/QuestionCard'
import Thinking from './items/Thinking'
import ToolCall from './items/ToolCall'
import Unrecognised from './items/Unrecognised'
import UserTurn from './items/UserTurn'
import { ArtifactCard } from './SessionFiles'
import type { ItemEnv } from './items/types'

export function ItemView({ item, env }: { item: Item; env: ItemEnv }) {
  switch (item.kind) {
    case 'artifact': return <ArtifactCard artifact={item.artifact} />
    case 'plugin_context':
      return <PluginContext item={item} />
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

export default function Transcript({ items, env, turnEnds, waiting = false, review }: { items: Item[]; env: ItemEnv; turnEnds?: ReadonlyMap<string, string>; waiting?: boolean; review?: ReviewResult }) {
  if (items.length === 0) return <p className="transcript-empty">Nothing in this session yet.</p>
  return (
    <>
      {items.map((item, index) => {
        const end = item.turn_id && item.turn_id !== items[index + 1]?.turn_id ? turnEnds?.get(item.turn_id) : undefined
        const reviewEnd = review && item.turn_id === review.turn_id && item.turn_id !== items[index + 1]?.turn_id
        return <Fragment key={item.id}>
          <Row item={item} env={env} />
          {end && <div className="divider turn-complete"><span>Turn complete{waiting && index === items.length - 1 ? ' — waiting for input' : ''} · <time dateTime={end}>{clockTime(end)}</time></span></div>}
          {reviewEnd && <div className="result-review-footer">
            <ResultReview key={review.event_id} id={env.sessionId} review={review} />
            <ResultReadObserver key={review.event_id} id={env.sessionId} review={review} lastItem={item} />
          </div>}
        </Fragment>
      })}
    </>
  )
}
