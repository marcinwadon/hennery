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
//   and the focus is free (§10, "never steal typing"): nothing holds it, or
//   a control in the transcript that takes no typing does. Never from
//   another open card, a text field, the session menu, a dialog or the
//   list's search: the next key there would be a digit that answers the
//   agent's first option. One that opens without taking the focus is said
//   in the view's live region instead (the answer book's announcement).
// - A question the agent stopped waiting on offers "Answer as a new
//   message": the question's text goes to `env.onAnswerAsMessage`, and the
//   view words the composer's draft around it.
// - A form's draft lives in the answer book, by question: the card can be
//   mounted again without losing it.
import { useEffect, useRef, type KeyboardEvent } from 'react'
import type { AnswerRequest } from '../../generated/protocol'
import { useMediaQuery } from '../../hooks/useMediaQuery'
import { deliveryOf, digitIndex, isEditable, optionTone, questionText, type OptionTone } from '../../lib/delivery'
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

/** Whether a card may take the focus from where it is now: an allowlist.
 *  Nothing focused, or a control inside the card's own transcript that
 *  takes no typing ("Load earlier", a closed card's button). Everything
 *  else keeps it: another live card (its next digit answers that card, not
 *  this one), the composer, any field (a form card's included), the
 *  header's menu, a dialog over the page, the session list. */
function focusIsFree(card: HTMLElement): boolean {
  const active = document.activeElement
  if (active === null || active === document.body) return true
  if (isEditable(active)) return false
  const holder = active.closest('section.ask:not(.stale)')
  if (holder !== null && holder !== card) return false
  const transcript = card.closest('.transcript')
  return transcript !== null && transcript.contains(active)
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
  const { composerEmpty, agent } = env
  const kind = request.type

  useEffect(() => {
    if (!live || !book) return
    // Claimed once, whatever the composer holds: a question opened while
    // the operator typed never takes the focus later.
    if (!book.claimFocus(item.id)) return
    const el = ref.current
    if (el && composerEmpty?.() === true && focusIsFree(el)) el.focus({ preventScroll: true })
    else book.announce(`New question: ${agent} ${kind === 'permission' ? 'asks for permission' : 'asks for an answer'}`)
  }, [live, book, item.id, composerEmpty, agent, kind])

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
