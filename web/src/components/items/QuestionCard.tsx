// A question the agent asked (frontend spec §6.3), read-only here: the
// request, its options or fields, and where it stands, in words. The
// answering controls come in through `actions`; without them the card only
// shows. Options are styled by their kind, never by their agent-chosen names.
import type { ReactNode } from 'react'
import type { PendingReason } from '../../generated/protocol'
import type { Field } from '../../generated/view'
import type { ItemOf } from './types'

type Question = ItemOf<'question'>

export const REASON_WORDS: Record<PendingReason, string> = {
  turn_cancelled: 'the turn was stopped',
  session_closed: 'the session was closed',
  session_parked: 'the session was parked',
  adapter_lost: 'the agent’s process was lost',
  host_restarted: 'the host restarted',
  agent_withdrew: 'the agent withdrew the question',
  host_revoked: 'the host was revoked',
}

/** Where a question stands, from the item alone (the table in §6.3 without
 *  its local states: an answer in flight, an answer refused). */
export function questionStateText(q: Question): string {
  if (q.delivered === true) return 'Answered'
  if (q.state === 'cancelled') {
    // Own keys only: a reason of `constructor` must read as sent.
    const why = q.reason ? (Object.hasOwn(REASON_WORDS, q.reason) ? REASON_WORDS[q.reason] : q.reason) : undefined
    return why ? `The agent stopped waiting (${why})` : 'The agent stopped waiting'
  }
  if (q.delivered === false) return 'Sent, but the agent was no longer waiting'
  if (q.state === 'delivered') return 'Answered'
  if (q.answered) return 'Sent'
  if (q.answerable) return 'Needs your answer'
  return 'Open'
}

/** A permission option's style, from its kind alone. */
export function optionClass(kind: string): string {
  if (kind.startsWith('reject')) return 'q-opt q-opt-reject'
  if (kind === 'allow_always') return 'q-opt q-opt-always'
  return 'q-opt q-opt-allow'
}

function FieldView({ field }: { field: Field }) {
  return (
    <div className="elic-field">
      <span className="elic-label">{field.label || field.key}</span>
      {field.hint && <span className="elic-hint">{field.hint}</span>}
      {field.options && field.options.length > 0 && (
        <ul className="elic-opts">
          {field.options.map((option, i) => (
            <li key={i} className="elic-opt">
              <span className="elic-opt-value">{option.label || option.value}</span>
              {option.description && <span className="elic-opt-desc">{option.description}</span>}
            </li>
          ))}
        </ul>
      )}
      {field.field_kind === 'unsupported' && <span className="elic-unsupported-msg">This field cannot be filled in here.</span>}
    </div>
  )
}

interface Props {
  item: Question
  /** The agent's label: who is asking. */
  agent: string
  /** Controls that answer it; absent, the card is read-only. */
  actions?: ReactNode
}

export default function QuestionCard({ item, agent, actions }: Props) {
  const { request } = item
  const state = questionStateText(item)
  const live = item.answerable
  return (
    <section className={'ask fade-in' + (live ? '' : ' stale')} aria-label={`Question from ${agent}`}>
      <div className="ask-eyebrow">
        <span className="e-tag">
          {request.type === 'permission' ? `${agent} asks for permission` : `${agent} asks`}
        </span>
      </div>
      {request.type === 'permission' ? (
        <>
          <p className="ask-q">{request.title || 'Permission'}</p>
          {request.options.length === 0 ? (
            <p className="item-note">This question cannot be answered here: stop, park or close the session.</p>
          ) : (
            <ul className="q-opts">
              {request.options.map((option) => (
                <li key={option.option_id} className={optionClass(option.option_kind)}>
                  {option.name}
                </li>
              ))}
            </ul>
          )}
        </>
      ) : (
        <>
          <p className="ask-q">{request.message}</p>
          {request.fields.length > 0 && (
            <div className="elic-fields">
              {request.fields.map((field) => (
                <FieldView key={field.key} field={field} />
              ))}
            </div>
          )}
          {!request.form_supported && (
            <p className="item-note">This form cannot be filled in here: it can only be declined or cancelled.</p>
          )}
        </>
      )}
      <p className="q-state">{state}</p>
      {actions && <div className="ask-actions">{actions}</div>}
    </section>
  )
}
