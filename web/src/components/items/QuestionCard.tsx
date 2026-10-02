// A question the agent asked (frontend spec §6.3): the request, its options
// or form, and where it stands, in words (lib/delivery.ts).
//
// - Only an item the server holds `answerable` can be answered (F-15), and
//   only through the session's answer book (`env.answers`); without one the
//   card only shows.
// - A permission's options are buttons in the adapter's order, styled by
//   their kind, never by their agent-chosen names. Keys 1–9 pick one, but
//   only while the card itself has the focus, never from a text field, and
//   only on a desktop (F-16): the listener is on the card, not the window.
// - A newly opened question takes the focus only when the composer is empty
//   (§10).
// - A question the agent stopped waiting on offers "Answer as a new
//   message": the question's text goes to `env.onAnswerAsMessage`, and the
//   view words the composer's draft around it.
// - A form's draft lives in the answer book, by question: the card can be
//   mounted again without losing it.
import { useEffect, useRef, type KeyboardEvent } from 'react'
import type { AnswerRequest } from '../../generated/protocol'
import { useMediaQuery } from '../../hooks/useMediaQuery'
import { deliveryOf, digitIndex, optionTone, questionText, type OptionTone } from '../../lib/delivery'
import { useAnswerState, useFormDraft } from '../../store/useAnswer'
import Elicitation from './Elicitation'
import type { ItemEnv, ItemOf } from './types'

type Question = ItemOf<'question'>

/** Where the digit shortcuts work: a wide screen with a fine pointer. */
export const DESKTOP = '(min-width: 768px) and (pointer: fine)'

const BUTTON_TONE: Record<OptionTone, string> = {
  allow: 'btn-primary',
  always: 'btn-ghost',
  reject: 'btn-danger',
}

interface Props {
  item: Question
  env: ItemEnv
}

export default function QuestionCard({ item, env }: Props) {
  const { request } = item
  const book = env.answers
  const { local, hostAway } = useAnswerState(book, item.id)
  const [draft, onDraft] = useFormDraft(book, item.id)
  const d = deliveryOf(item, book ? local : undefined, hostAway)
  // Live: this card can answer now.
  const live = book !== undefined && d.controls
  const desktop = useMediaQuery(DESKTOP)
  const ref = useRef<HTMLElement>(null)
  const { composerEmpty } = env

  useEffect(() => {
    if (!live || !book) return
    // Claimed once, whatever the composer holds: a question opened while
    // the operator typed never takes the focus later.
    if (!book.claimFocus(item.id)) return
    if (composerEmpty?.() === true) ref.current?.focus({ preventScroll: true })
  }, [live, book, item.id, composerEmpty])

  const answer = (body: AnswerRequest) => {
    if (book && !d.busy) void book.answer(item, body)
  }

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (!live || !desktop || d.busy || request.type !== 'permission') return
    const at = digitIndex(e)
    if (at === undefined || at >= request.options.length) return
    e.preventDefault()
    answer({ option_id: request.options[at].option_id })
  }

  const agent = env.agent
  return (
    <section
      ref={ref}
      className={'ask fade-in' + (live ? '' : ' stale')}
      aria-label={`Question from ${agent}`}
      tabIndex={live ? 0 : undefined}
      onKeyDown={live ? onKeyDown : undefined}
    >
      <div className="ask-eyebrow">
        <span className="e-tag">{request.type === 'permission' ? `${agent} asks for permission` : `${agent} asks`}</span>
      </div>
      {request.type === 'permission' ? (
        <>
          <p className="ask-q">{request.title || 'Permission'}</p>
          {request.options.length > 0 &&
            (live ? (
              <div className="q-opts" role="group" aria-label="Options">
                {request.options.map((option, i) => {
                  const tone = optionTone(option.option_kind)
                  return (
                    <button
                      key={option.option_id}
                      type="button"
                      className={`btn btn-sm ${BUTTON_TONE[tone]} q-opt q-opt-${tone}`}
                      disabled={d.busy}
                      onClick={() => answer({ option_id: option.option_id })}
                    >
                      {option.name}
                      {desktop && i < 9 && (
                        <kbd className="q-key" aria-hidden="true">
                          {i + 1}
                        </kbd>
                      )}
                    </button>
                  )
                })}
              </div>
            ) : (
              <ul className="q-opts">
                {request.options.map((option) => (
                  <li key={option.option_id} className={`q-opt q-opt-${optionTone(option.option_kind)}`}>
                    {option.name}
                  </li>
                ))}
              </ul>
            ))}
        </>
      ) : (
        <>
          <p className="ask-q">{request.message}</p>
          <Elicitation request={request} live={live} busy={d.busy} draft={draft} onDraft={onDraft} onAnswer={answer} />
        </>
      )}
      <p className="q-state">
        <span>{d.text}</span>
        {d.note && <span className="q-note">{d.note}</span>}
      </p>
      {d.error && (
        <p className="form-error q-error" role="alert">
          <bdi>{d.error}</bdi>
        </p>
      )}
      {d.answerAsMessage && env.onAnswerAsMessage && (
        <div className="ask-actions">
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => env.onAnswerAsMessage?.(questionText(request))}>
            Answer as a new message
          </button>
        </div>
      )}
    </section>
  )
}
