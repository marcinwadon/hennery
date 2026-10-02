// A plan update in the transcript: the step list as it stood then.
import StepList from '../StepList'
import { Speaker } from './parts'
import type { ItemEnv, ItemOf } from './types'

export default function PlanItem({ item, env }: { item: ItemOf<'plan'>; env: ItemEnv }) {
  return (
    <Speaker who="agent" label={env.agent} ts={item.ts}>
      <StepList entries={item.entries} truncated={item.truncated} className="steps-item" />
    </Speaker>
  )
}
