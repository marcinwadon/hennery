import { useId } from 'react'

/** Structural subset shared by list summaries and session details. */
export interface BackgroundInfo {
  active: number
  done: number
  all: number
  stale?: boolean
  items?: { id: string; title?: string; status?: string; kind?: string }[]
}

const taskStates: Record<string, string> = { pending: 'Pending', paused: 'Paused', in_progress: 'Running', running: 'Running', completed: 'Completed', succeeded: 'Succeeded', failed: 'Failed', cancelled: 'Cancelled', unknown: 'Unknown' }
const taskState = (value?: string) => value && Object.hasOwn(taskStates, value) ? taskStates[value] : value || 'Active'

export function BackgroundCount({ value, stale = false }: { value?: BackgroundInfo | null; stale?: boolean }) {
  if (!value || value.all === 0) return null
  return <span className={'background-count' + (stale || value.stale ? ' background-stale' : '')} title="Active / done / all background tasks"><span>Background tasks</span> <b>{value.active} / {value.done} / {value.all}</b>{(stale || value.stale) && <span> · last known</span>}</span>
}

export default function BackgroundTasks({ value, stale = false }: { value?: BackgroundInfo | null; stale?: boolean }) {
  const id = useId()
  if (!value || value.all === 0) return null
  return <details className="background-tasks"><summary aria-controls={id}><BackgroundCount value={value} stale={stale} /></summary><div id={id} className="background-task-list">
    <p className="hint">Active / done / all. Active includes pending and paused tasks.</p>
    {(stale || value.stale) && <p role="status">Last known tasks. Current activity is unavailable.</p>}
    {value.items?.length ? <ul>{value.items.map(item => <li key={item.id}><bdi>{item.title || item.id}</bdi><span>{taskState(item.status)}</span></li>)}</ul> : <p className="hint">Task details are not reported.</p>}
  </div></details>
}
