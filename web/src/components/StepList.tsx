// The session's own step list, as the agent keeps it (ACP `plan`; frontend
// spec §6.2): the newest snapshot only. The adapter shows its plan instead
// of the tool calls that make it, so this is the only place the steps are.
//
// Open while work is in progress and closed once every step is done, unless
// the caller says (the header keeps it closed on a phone).
import type { PlanEntry } from '../generated/view'

interface Props {
  entries: PlanEntry[]
  /** Steps were left out or cut. */
  truncated?: boolean
  /** Overrides the open-while-in-progress default. */
  open?: boolean
  className?: string
}

export default function StepList({ entries, truncated, open, className }: Props) {
  if (entries.length === 0) return null
  const done = entries.filter((e) => e.status === 'completed').length
  const current = entries.find((e) => e.status === 'in_progress')
  return (
    <details className={'steps' + (className ? ` ${className}` : '')} open={open ?? done < entries.length}>
      <summary>
        <span className="steps-count">
          {done} / {entries.length}
        </span>
        <span className="steps-current">{current ? current.content || 'Untitled step' : 'Steps'}</span>
      </summary>
      <ol>
        {entries.map((e, i) => (
          <li key={i} className={e.status === 'in_progress' ? 'on' : e.status === 'completed' ? 'done' : ''}>
            {e.content || 'Untitled step'}
          </li>
        ))}
      </ol>
      {truncated && <p className="item-note">Some steps were cut or left out.</p>}
    </details>
  )
}
