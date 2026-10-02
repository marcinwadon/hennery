// An update this view does not know: never dropped, shown collapsed with its
// raw JSON as text.
import { CutNote } from './parts'
import type { ItemEnv, ItemOf } from './types'

export default function Unrecognised({ item, env }: { item: ItemOf<'unrecognised'>; env: ItemEnv }) {
  return (
    <details className="unrec fade-in">
      <summary>
        Unsupported update (<bdi>{item.update_kind}</bdi>)
      </summary>
      <pre className="tool-io">{item.raw}</pre>
      {item.truncated && <CutNote sessionId={env.sessionId}>This update was cut.</CutNote>}
    </details>
  )
}
