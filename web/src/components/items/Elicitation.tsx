// An elicitation's form (frontend spec §6.3): its fields, read-only or to
// fill in, and the answers it can send.
//
// - Single selects are radios, multi selects checkboxes, text an input; an
//   unsupported field says so. Every field's hint shows.
// - A form the card cannot fill (`form_supported: false`) offers Decline and
//   Cancel only, and never invents a value.
// - Send appears once one real answer exists and every required key has
//   one. Nothing is chosen for the operator, and nothing is sent on its own:
//   no choice and no key sends, only the buttons.
import { useId } from 'react'
import type { ElicitationAction } from '../../generated/protocol'
import type { Field, FieldOption } from '../../generated/view'
import { canSend, choose, contentOf, elicitationBody, toggle, typeText, type Draft } from '../../lib/elicitation'

type Request = { fields: Field[]; required?: string[]; form_supported: boolean }
type Body = ReturnType<typeof elicitationBody>

/** An option's text. Its name is the bare label (§10): the description is
 *  its description, not part of its name. */
function OptionText({ option, id }: { option: FieldOption; id: string }) {
  const descId = `${id}-d`
  return (
    <>
      <span className="elic-opt-value" id={id}>
        {option.label || option.value}
      </span>
      {option.description && (
        <span className="elic-opt-desc" id={descId}>
          {option.description}
        </span>
      )}
    </>
  )
}

function FieldHead({ field, inputId }: { field: Field; inputId?: string }) {
  return (
    <>
      {inputId ? (
        <label className="elic-label" htmlFor={inputId}>
          {field.label || field.key}
        </label>
      ) : (
        <span className="elic-label">{field.label || field.key}</span>
      )}
      {field.hint && <span className="elic-hint">{field.hint}</span>}
    </>
  )
}

/** A select's legend and hint. */
function GroupHead({ field }: { field: Field }) {
  return (
    <>
      <legend className="elic-label">{field.label || field.key}</legend>
      {field.hint && <span className="elic-hint">{field.hint}</span>}
    </>
  )
}

/** A field as it was asked: its options listed, nothing to fill in. */
export function FieldView({ field }: { field: Field }) {
  return (
    <div className="elic-field">
      <FieldHead field={field} />
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

interface FieldInputProps {
  field: Field
  draft: Draft
  disabled: boolean
  /** The field's own prefix for names and ids: from the form's id and the
   *  field's place, never its key (a key may hold spaces, which an id
   *  reference cannot). */
  prefix: string
  onDraft: (next: (draft: Draft) => Draft) => void
  fields: readonly Field[]
}

function FieldInput({ field, draft, disabled, prefix, onDraft, fields }: FieldInputProps) {
  const name = prefix
  const options = field.options ?? []
  switch (field.field_kind) {
    case 'single':
      return (
        <fieldset className="elic-field">
          <GroupHead field={field} />
          <div className="elic-opts">
            {options.map((option, i) => {
              const checked = draft[field.key] === option.value
              const id = `${name}-o${i}`
              return (
                <label key={i} className={'elic-opt' + (checked ? ' sel' : '')}>
                  <input
                    type="radio"
                    name={name}
                    value={option.value}
                    checked={checked}
                    disabled={disabled}
                    aria-labelledby={id}
                    aria-describedby={option.description ? `${id}-d` : undefined}
                    onChange={() => onDraft((d) => choose(fields, d, field.key, option.value))}
                  />
                  <OptionText option={option} id={id} />
                </label>
              )
            })}
          </div>
        </fieldset>
      )
    case 'multi': {
      const held = draft[field.key]
      const chosen = Array.isArray(held) ? held : []
      return (
        <fieldset className="elic-field">
          <GroupHead field={field} />
          <div className="elic-opts">
            {options.map((option, i) => {
              const checked = chosen.includes(option.value)
              const id = `${name}-o${i}`
              return (
                <label key={i} className={'elic-opt' + (checked ? ' sel' : '')}>
                  <input
                    type="checkbox"
                    name={name}
                    value={option.value}
                    checked={checked}
                    disabled={disabled}
                    aria-labelledby={id}
                    aria-describedby={option.description ? `${id}-d` : undefined}
                    onChange={() => onDraft((d) => toggle(fields, d, field.key, option.value))}
                  />
                  <OptionText option={option} id={id} />
                </label>
              )
            })}
          </div>
        </fieldset>
      )
    }
    case 'text': {
      const value = draft[field.key]
      return (
        <div className="elic-field">
          <FieldHead field={field} inputId={name} />
          <input
            id={name}
            type="text"
            className="elic-text"
            value={typeof value === 'string' ? value : ''}
            disabled={disabled}
            onChange={(e) => {
              const text = e.target.value
              onDraft((d) => typeText(fields, d, field.key, text))
            }}
          />
        </div>
      )
    }
    default:
      return <FieldView field={field} />
  }
}

interface Props {
  request: Request
  /** The card can answer it now. */
  live: boolean
  /** An answer is in flight. */
  busy: boolean
  /** What the operator filled in: held by the card's owner (the answer
   *  book), never by this component, so a remount keeps it. */
  draft: Draft
  onDraft: (next: (draft: Draft) => Draft) => void
  onAnswer: (body: Body) => void
}

export default function Elicitation({ request, live, busy, draft, onDraft, onAnswer }: Props) {
  const prefix = useId()
  const { fields, required, form_supported: supported } = request
  if (!live || !supported) {
    return (
      <>
        {fields.length > 0 && (
          <div className="elic-fields">
            {fields.map((field) => (
              <FieldView key={field.key} field={field} />
            ))}
          </div>
        )}
        {!supported && <p className="item-note">This form cannot be filled in here: it can only be declined or cancelled.</p>}
        {live && <Actions busy={busy} send={false} onAction={(action) => onAnswer(elicitationBody(action))} />}
      </>
    )
  }
  const sendable = canSend(fields, required, supported, draft)
  return (
    <>
      {fields.length > 0 && (
        <div className="elic-fields">
          {fields.map((field, i) => (
            <FieldInput key={field.key} field={field} draft={draft} disabled={busy} prefix={`${prefix}f${i}`} onDraft={onDraft} fields={fields} />
          ))}
        </div>
      )}
      <Actions
        busy={busy}
        send={sendable}
        onAction={(action) => onAnswer(action === 'accept' ? elicitationBody('accept', contentOf(fields, draft)) : elicitationBody(action))}
      />
    </>
  )
}

function Actions({ busy, send, onAction }: { busy: boolean; send: boolean; onAction: (action: ElicitationAction) => void }) {
  return (
    <div className="ask-actions">
      {send && (
        <button type="button" className="btn btn-primary btn-sm" disabled={busy} onClick={() => onAction('accept')}>
          Send
        </button>
      )}
      <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => onAction('decline')}>
        Decline
      </button>
      <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => onAction('cancel')}>
        Cancel
      </button>
    </div>
  )
}
