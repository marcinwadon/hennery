import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { Client } from '../../api/client'
import type { Item } from '../../generated/view'
import { json, stubServer, type Answer } from '../../test-server'
import { AnswerBook } from '../../store/useAnswer'
import { ItemView } from '../Transcript'
import type { ItemEnv, ItemOf } from './types'

type Q = ItemOf<'question'>
// The query as the spec words it, not the module's constant: a test that
// imported it would follow a wrong one.
const DESKTOP = '(min-width: 768px) and (pointer: fine)'
const TS = '2026-10-02T10:00:00.000Z'
const SESSION = 's/1'
const PATH = (pending: string) => `/api/sessions/s%2F1/pending/${encodeURIComponent(pending)}/answer`

const OPTIONS = [
  { option_id: 'a', name: 'Yes, always', option_kind: 'allow_always' },
  { option_id: 'b', name: 'Allow', option_kind: 'allow_once' },
  { option_id: 'c', name: 'Allow (really reject)', option_kind: 'reject_once' },
  { option_id: 'd', name: 'Never', option_kind: 'reject_always' },
  { option_id: 'e', name: 'Reject (really new)', option_kind: 'shiny_new_kind' },
]

function permission(patch: Partial<Q> = {}): Q {
  return {
    id: 'question:p/1',
    version: 1,
    ts: TS,
    turn_id: 't1',
    kind: 'question',
    pending_id: 'p/1',
    question_kind: 'permission',
    request: { type: 'permission', title: 'Run rm -rf build?', options: OPTIONS },
    answerable: true,
    state: 'open',
    answered: false,
    ...patch,
  } as Q
}

function elicitation(request: Partial<Extract<Q['request'], { type: 'elicitation' }>>, patch: Partial<Q> = {}): Q {
  return permission({
    id: 'question:e1',
    pending_id: 'e1',
    question_kind: 'elicitation',
    request: { type: 'elicitation', message: 'Tabs or spaces?', fields: [], form_supported: true, ...request },
    ...patch,
  })
}

/** A deferred response: the test decides when the server answers. */
function later() {
  let settle!: (r: Response) => void
  const promise = new Promise<Response>((r) => (settle = r))
  return { answer: () => promise, settle }
}

function setup(routes: Record<string, Answer | Answer[]> = {}) {
  const server = stubServer(routes)
  const client = new Client({ fetch: server.fetch, navigate: vi.fn(), here: () => ({ pathname: '/sessions/s', search: '' }), stepUp: vi.fn(async () => {}) })
  const book = new AnswerBook(client, SESSION)
  const posts = () => server.sent.filter((s) => s.method === 'POST')
  return { server, client, book, posts }
}

function show(item: Item, env: ItemEnv) {
  const view = render(<ItemView item={item} env={env} />)
  return { ...view, again: (next: Item, nextEnv: ItemEnv = env) => view.rerender(<ItemView item={next} env={nextEnv} />) }
}

const card = () => screen.getByRole('region', { name: /Question from/ })

/** A matchMedia that matches only `matching` queries. */
function media(matching: string[]) {
  window.matchMedia = ((query: string) => ({
    matches: matching.includes(query),
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
}

afterEach(() => {
  // @ts-expect-error clear a stub between tests
  delete window.matchMedia
})

describe('permission card', () => {
  it('answers with the option clicked, posting its id to the encoded route', async () => {
    const t = setup({ [`POST ${PATH('p/1')}`]: json(202, { pending_id: 'p/1', request_id: 'r1' }) })
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    fireEvent.click(screen.getByRole('button', { name: 'Never' }))
    await screen.findByText('Sent')
    expect(t.posts()).toEqual([{ method: 'POST', path: PATH('p/1'), body: { option_id: 'd' } }])
  })

  it('lists the options as buttons in the adapter’s order', () => {
    const t = setup()
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    const names = within(screen.getByRole('group', { name: 'Options' }))
      .getAllByRole('button')
      .map((b) => b.textContent)
    expect(names).toEqual(OPTIONS.map((o) => o.name))
  })

  it.each([
    ['allow_once', 'Allow', 'btn-primary', 'q-opt-allow'],
    ['allow_always', 'Yes, always', 'btn-ghost', 'q-opt-always'],
    ['reject_once', 'Allow (really reject)', 'btn-danger', 'q-opt-reject'],
    ['reject_always', 'Never', 'btn-danger', 'q-opt-reject'],
    ['an unknown kind', 'Reject (really new)', 'btn-primary', 'q-opt-allow'],
  ])('styles option kind %s by its kind, never its name', (_kind, name, button, tone) => {
    const t = setup()
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    const el = screen.getByRole('button', { name })
    expect(el).toHaveClass(button, tone)
    for (const other of ['btn-primary', 'btn-ghost', 'btn-danger'].filter((c) => c !== button)) expect(el).not.toHaveClass(other)
  })

  it('keeps one answer in flight: a second click sends nothing', async () => {
    const reply = later()
    const t = setup({ [`POST ${PATH('p/1')}`]: reply.answer })
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }))
    expect(screen.getByRole('button', { name: 'Never' })).toBeDisabled()
    fireEvent.click(screen.getByRole('button', { name: 'Never' }))
    await t.book.answer(permission(), { option_id: 'd' })
    expect(t.posts()).toHaveLength(1)
    reply.settle(json(202, { pending_id: 'p/1', request_id: 'r1' }))
    await screen.findByText('Sent')
  })

  it('stays disabled while in flight even when the item is upserted meanwhile', async () => {
    const reply = later()
    const t = setup({ [`POST ${PATH('p/1')}`]: reply.answer })
    const v = show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }))
    v.again(permission({ version: 2 }))
    expect(screen.getByRole('button', { name: 'Allow' })).toBeDisabled()
    expect(screen.getByText('Sending…')).toBeInTheDocument()
    await act(async () => reply.settle(json(202, { pending_id: 'p/1', request_id: 'r1' })))
  })

  it('shows a 202 as “Sent” until the item’s next upsert, then the item’s state', async () => {
    const t = setup({ [`POST ${PATH('p/1')}`]: json(202, { pending_id: 'p/1', request_id: 'r1' }) })
    const v = show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }))
    await screen.findByText('Sent')
    expect(screen.queryByRole('button', { name: 'Allow' })).toBeNull()
    v.again(permission({ version: 2, answered: true, answerable: false, delivered: true }))
    expect(screen.getByText('Answered')).toBeInTheDocument()
  })

  it.each([
    ['already_answered', 'Already answered from another device', 'This question is no longer open'],
    ['not_open', 'This question is no longer open', 'Already answered from another device'],
  ])('shows a 409 %s in its own words until the next upsert', async (code, words, never) => {
    const t = setup({ [`POST ${PATH('p/1')}`]: json(409, { code, message: 'x' }) })
    const v = show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }))
    await screen.findByText(words)
    expect(screen.queryByText(never)).toBeNull()
    expect(screen.queryByRole('button', { name: 'Allow' })).toBeNull()
    // The same version again is no upsert: the message stays.
    v.again(permission())
    expect(screen.getByText(words)).toBeInTheDocument()
    v.again(permission({ version: 2, answered: true, answerable: false, delivered: false }))
    expect(screen.queryByText(words)).toBeNull()
    expect(screen.getByText('Sent, but the agent was no longer waiting')).toBeInTheDocument()
  })

  it('shows a 404 as “This question is gone”', async () => {
    const t = setup({ [`POST ${PATH('p/1')}`]: json(404, { code: 'not_found', message: 'no such pending request' }) })
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }))
    await screen.findByText('This question is gone')
    expect(screen.queryByRole('button', { name: 'Allow' })).toBeNull()
  })

  it('shows another refusal’s message and offers the options again', async () => {
    const t = setup({ [`POST ${PATH('p/1')}`]: json(400, { code: 'invalid', message: 'the request offers no option z' }) })
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('the request offers no option z')
    expect(screen.getByRole('button', { name: 'Allow' })).toBeEnabled()
  })

  it('never asks for a step-up: a 403 is a message, not a dialog', async () => {
    const t = setup({ [`POST ${PATH('p/1')}`]: json(403, { code: 'step_up_required', message: 'x' }) })
    const stepUp = vi.fn(async () => {})
    const client = new Client({ fetch: t.server.fetch, navigate: vi.fn(), here: () => ({ pathname: '/', search: '' }), stepUp })
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: new AnswerBook(client, SESSION) })
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }))
    await screen.findByRole('alert')
    expect(stepUp).not.toHaveBeenCalled()
    expect(t.posts()).toHaveLength(1)
  })

  it('offers no buttons for a permission with no options, even one the server holds answerable', () => {
    const t = setup()
    show(permission({ request: { type: 'permission', title: 'Do it?', options: [] } }), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    expect(screen.queryByRole('button')).toBeNull()
    expect(screen.getByText('This question cannot be answered here: stop, park or close the session.')).toBeInTheDocument()
    expect(screen.queryByText('Needs your answer')).toBeNull()
    expect(card()).not.toHaveAttribute('tabindex')
  })

  it('takes actionability from the item: not answerable, no buttons', () => {
    const t = setup()
    show(permission({ answerable: false }), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    expect(screen.queryByRole('button')).toBeNull()
  })

  it('names the session’s agent as the one asking', () => {
    const t = setup()
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    expect(screen.getByText('Codex asks for permission')).toBeInTheDocument()
    expect(card()).toHaveAccessibleName('Question from Codex')
  })
})

describe('digit shortcuts', () => {
  it('answer from the card itself on a desktop', async () => {
    media([DESKTOP])
    const t = setup({ [`POST ${PATH('p/1')}`]: json(202, { pending_id: 'p/1', request_id: 'r1' }) })
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    expect(card()).toHaveAttribute('tabindex', '0')
    fireEvent.keyDown(card(), { key: '2' })
    await screen.findByText('Sent')
    expect(t.posts().map((p) => p.body)).toEqual([{ option_id: 'b' }])
  })

  it('answer from a button inside the card', async () => {
    media([DESKTOP])
    const t = setup({ [`POST ${PATH('p/1')}`]: json(202, { pending_id: 'p/1', request_id: 'r1' }) })
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    fireEvent.keyDown(screen.getByRole('button', { name: 'Never' }), { key: '1' })
    await screen.findByText('Sent')
    expect(t.posts().map((p) => p.body)).toEqual([{ option_id: 'a' }])
  })

  it('are never heard from the window or a field outside the card', () => {
    media([DESKTOP])
    const t = setup()
    render(
      <>
        <textarea aria-label="composer" />
        <ItemView item={permission()} env={{ sessionId: SESSION, agent: 'Codex', answers: t.book }} />
      </>,
    )
    fireEvent.keyDown(window, { key: '1' })
    fireEvent.keyDown(document.body, { key: '1' })
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'composer' }), { key: '1' })
    expect(t.posts()).toHaveLength(0)
  })

  it('are ignored from an editable target inside the card', () => {
    media([DESKTOP])
    const t = setup()
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    const field = document.createElement('input')
    card().appendChild(field)
    fireEvent.keyDown(field, { key: '1' })
    expect(t.posts()).toHaveLength(0)
  })

  it('are off on a phone or a touch screen', () => {
    media(['(min-width: 768px)', '(pointer: fine)'])
    const t = setup()
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    fireEvent.keyDown(card(), { key: '1' })
    expect(t.posts()).toHaveLength(0)
    expect(document.querySelector('.q-key')).toBeNull()
  })

  it('ignore a digit past the options, and a digit with a modifier', () => {
    media([DESKTOP])
    const t = setup()
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    // A digit past the options is not an option: nothing is read for it.
    const thrown: unknown[] = []
    const onError = (e: ErrorEvent) => {
      thrown.push(e.error)
      e.preventDefault()
    }
    window.addEventListener('error', onError)
    try {
      fireEvent.keyDown(card(), { key: '6' })
    } finally {
      window.removeEventListener('error', onError)
    }
    expect(thrown).toEqual([])
    fireEvent.keyDown(card(), { key: '1', metaKey: true })
    fireEvent.keyDown(card(), { key: '1', ctrlKey: true })
    fireEvent.keyDown(card(), { key: '1', altKey: true })
    expect(t.posts()).toHaveLength(0)
  })

  it('show each digit beside its option, and keep its bare name as the option’s name', () => {
    media([DESKTOP])
    const t = setup()
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    expect(screen.getByRole('button', { name: 'Allow' })).toHaveTextContent('Allow2')
    expect(Array.from(document.querySelectorAll('.q-key')).map((k) => k.textContent)).toEqual(['1', '2', '3', '4', '5'])
  })
})

describe('delivery states', () => {
  const env = (book: AnswerBook): ItemEnv => ({ sessionId: SESSION, agent: 'Codex', answers: book })

  it('reads “Needs your answer” while answerable', () => {
    show(permission(), env(setup().book))
    expect(screen.getByText('Needs your answer')).toBeInTheDocument()
  })

  it('reads “Sent” for an answer with no verdict yet', () => {
    show(permission({ answered: true, answerable: false }), env(setup().book))
    expect(screen.getByText('Sent')).toBeInTheDocument()
    expect(screen.queryByText('delivers when the host reconnects')).toBeNull()
  })

  it('adds that it delivers when the host reconnects while the host is away', () => {
    const t = setup()
    show(permission({ answered: true, answerable: false }), env(t.book))
    act(() => t.book.setHostAway(true))
    expect(screen.getByText('Sent')).toBeInTheDocument()
    expect(screen.getByText('delivers when the host reconnects')).toBeInTheDocument()
  })

  it('reads “Answered” for a verdict delivered', () => {
    show(permission({ answered: true, answerable: false, delivered: true }), env(setup().book))
    expect(screen.getByText('Answered')).toBeInTheDocument()
  })

  it('reads “Sent, but the agent was no longer waiting” for a verdict not delivered', () => {
    show(permission({ answered: true, answerable: false, delivered: false }), env(setup().book))
    expect(screen.getByText('Sent, but the agent was no longer waiting')).toBeInTheDocument()
  })

  it('reads “Answered” for a question cancelled after its answer was delivered, with no new message offered', () => {
    const onAnswerAsMessage = vi.fn()
    show(permission({ state: 'cancelled', reason: 'turn_cancelled', answered: true, delivered: true, answerable: false }), {
      ...env(setup().book),
      onAnswerAsMessage,
    })
    expect(screen.getByText('Answered')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Answer as a new message' })).toBeNull()
  })

  it('reads “Answered” for a delivered state with no verdict', () => {
    show(permission({ state: 'delivered', answerable: false }), env(setup().book))
    expect(screen.getByText('Answered')).toBeInTheDocument()
  })

  it('offers “Answer as a new message” for a cancelled question, handing the composer the question’s text only', () => {
    const onAnswerAsMessage = vi.fn()
    show(permission({ state: 'cancelled', reason: 'session_parked', answerable: false }), { ...env(setup().book), onAnswerAsMessage })
    expect(screen.getByText('The agent stopped waiting (the session was parked)')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Answer as a new message' }))
    expect(onAnswerAsMessage).toHaveBeenCalledWith('Run rm -rf build?')
  })

  it('hands over an elicitation’s message', () => {
    const onAnswerAsMessage = vi.fn()
    show(elicitation({}, { state: 'cancelled', reason: 'adapter_lost', answerable: false }), { ...env(setup().book), onAnswerAsMessage })
    fireEvent.click(screen.getByRole('button', { name: 'Answer as a new message' }))
    expect(onAnswerAsMessage).toHaveBeenCalledWith('Tabs or spaces?')
  })

  it('offers no new message without the composer’s seam', () => {
    show(permission({ state: 'cancelled', reason: 'session_parked', answerable: false }), env(setup().book))
    expect(screen.queryByRole('button', { name: 'Answer as a new message' })).toBeNull()
  })
})

describe('focus on a newly opened question', () => {
  const items = (...extra: Item[]): Item[] => [
    { id: 'm1', version: 1, ts: TS, turn_id: 't1', kind: 'message', text: 'hi' } as Item,
    ...extra,
  ]

  it('moves to an answerable card opened at the tail when the composer is empty', () => {
    const t = setup()
    t.book.observe(items(), 1)
    t.book.observe(items(permission()), 1)
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book, composerEmpty: () => true })
    expect(card()).toHaveFocus()
  })

  it('stays put while the composer holds text, and does not come back later', () => {
    const t = setup()
    t.book.observe(items(), 1)
    t.book.observe(items(permission()), 1)
    const v = show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book, composerEmpty: () => false })
    expect(card()).not.toHaveFocus()
    v.again(permission({ version: 2 }), { sessionId: SESSION, agent: 'Codex', answers: t.book, composerEmpty: () => true })
    expect(card()).not.toHaveFocus()
  })

  it('never moves without the composer’s seam', () => {
    const t = setup()
    t.book.observe(items(), 1)
    t.book.observe(items(permission()), 1)
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book })
    expect(card()).not.toHaveFocus()
  })

  it('stays in a field inside the transcript that no open card holds, and says the card instead', () => {
    const t = setup()
    t.book.observe(items(), 1)
    t.book.observe(items(permission()), 1)
    const v = render(
      <div className="transcript">
        <input aria-label="Note" />
      </div>,
    )
    const note = screen.getByRole('textbox', { name: 'Note' })
    note.focus()
    v.rerender(
      <div className="transcript">
        <input aria-label="Note" />
        <ItemView item={permission()} env={{ sessionId: SESSION, agent: 'Codex', answers: t.book, composerEmpty: () => true }} />
      </div>,
    )
    expect(note).toHaveFocus()
    expect(t.book.announcement).toBe('New question: Codex asks for permission')
  })

  it('never moves to a card that was there on the first page', () => {
    const t = setup()
    t.book.observe(items(permission()), 1)
    show(permission(), { sessionId: SESSION, agent: 'Codex', answers: t.book, composerEmpty: () => true })
    expect(card()).not.toHaveFocus()
  })
})

describe('elicitation card', () => {
  const env = (book: AnswerBook): ItemEnv => ({ sessionId: SESSION, agent: 'Codex', answers: book })
  const ok = () => json(202, { pending_id: 'e1', request_id: 'r1' })
  const single = { key: 'indent', label: 'Indent', hint: 'Pick one', field_kind: 'single' as const, options: [{ value: 'tabs', label: 'Tabs', description: 'one char' }, { value: 'spaces' }] }
  const custom = { key: 'indent_custom', label: 'Other', field_kind: 'text' as const, pairing: { with: 'indent', kind: 'exclusive' as const } }
  const multi = { key: 'langs', label: 'Languages', hint: 'Any', field_kind: 'multi' as const, options: [{ value: 'rs', label: 'Rust' }, { value: 'ts', label: 'TypeScript' }] }
  const note = { key: 'langs_note', label: 'Note', field_kind: 'text' as const, pairing: { with: 'langs', kind: 'note' as const } }
  const text = { key: 'name', label: 'Name', hint: 'Your name', field_kind: 'text' as const }

  it('fills a single select as radios, with its hint and descriptions', async () => {
    const t = setup({ [`POST ${PATH('e1')}`]: ok() })
    show(elicitation({ fields: [single] }), env(t.book))
    expect(screen.getByText('Pick one')).toBeInTheDocument()
    expect(screen.getByRole('radio', { name: 'Tabs' })).toHaveAccessibleDescription('one char')
    expect(screen.getByRole('radio', { name: 'spaces' })).not.toBeChecked()
    fireEvent.click(screen.getByRole('radio', { name: 'Tabs' }))
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await screen.findByText('Sent')
    expect(t.posts().map((p) => p.body)).toEqual([{ action: 'accept', content: { indent: 'tabs' } }])
  })

  it('fills a multi select as checkboxes, sending a list', async () => {
    const t = setup({ [`POST ${PATH('e1')}`]: ok() })
    show(elicitation({ fields: [multi] }), env(t.book))
    expect(screen.getByText('Any')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('checkbox', { name: 'TypeScript' }))
    fireEvent.click(screen.getByRole('checkbox', { name: 'Rust' }))
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await screen.findByText('Sent')
    expect(t.posts().map((p) => p.body)).toEqual([{ action: 'accept', content: { langs: ['ts', 'rs'] } }])
  })

  it('fills a text field, sending it trimmed', async () => {
    const t = setup({ [`POST ${PATH('e1')}`]: ok() })
    show(elicitation({ fields: [text] }), env(t.book))
    expect(screen.getByText('Your name')).toBeInTheDocument()
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), { target: { value: '  Ada ' } })
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveValue('  Ada ')
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await screen.findByText('Sent')
    expect(t.posts().map((p) => p.body)).toEqual([{ action: 'accept', content: { name: 'Ada' } }])
  })

  it('sends the answer of a field the agent keyed __proto__', async () => {
    const t = setup({ [`POST ${PATH('e1')}`]: ok() })
    show(elicitation({ fields: [{ key: '__proto__', label: 'Proto', field_kind: 'text' }] }), env(t.book))
    fireEvent.change(screen.getByRole('textbox', { name: 'Proto' }), { target: { value: 'kept' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await screen.findByText('Sent')
    // Parsed from the JSON sent: `__proto__` is an own key there.
    const [body] = t.posts().map((p) => p.body as { content: object })
    expect(Object.getOwnPropertyDescriptor(body.content, '__proto__')?.value).toBe('kept')
    expect(Object.keys(body.content)).toEqual(['__proto__'])
  })

  it('keeps a half-filled form in the answer book: the card mounted again shows it', () => {
    const t = setup()
    const form = elicitation({ fields: [text, single] })
    const first = show(form, env(t.book))
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), { target: { value: 'Ad' } })
    fireEvent.click(screen.getByRole('radio', { name: 'Tabs' }))
    first.unmount()
    show(form, env(t.book))
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveValue('Ad')
    expect(screen.getByRole('radio', { name: 'Tabs' })).toBeChecked()
    expect(screen.getByRole('button', { name: 'Send' })).toBeInTheDocument()
    expect(t.posts()).toEqual([])
  })

  it('keeps each question’s draft apart', () => {
    const t = setup()
    show(elicitation({ fields: [text] }), env(t.book))
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), { target: { value: 'Ada' } })
    show(elicitation({ fields: [text] }, { id: 'question:e2', pending_id: 'e2' }), env(t.book))
    expect(screen.getAllByRole('textbox', { name: 'Name' }).map((i) => (i as HTMLInputElement).value)).toEqual(['Ada', ''])
    // Typing into the second leaves the first's draft as it was.
    fireEvent.change(screen.getAllByRole('textbox', { name: 'Name' })[1], { target: { value: 'Bob' } })
    expect(screen.getAllByRole('textbox', { name: 'Name' }).map((i) => (i as HTMLInputElement).value)).toEqual(['Ada', 'Bob'])
  })

  it('says an unsupported field cannot be filled in, and offers Decline and Cancel only', () => {
    const t = setup()
    show(elicitation({ fields: [text, { key: 'when', label: 'When', field_kind: 'unsupported' }], form_supported: false }), env(t.book))
    expect(screen.getByText('This field cannot be filled in here.')).toBeInTheDocument()
    expect(screen.getByText(/can only be declined or cancelled/)).toBeInTheDocument()
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(screen.getAllByRole('button').map((b) => b.textContent)).toEqual(['Decline', 'Cancel'])
  })

  it('does not fill in a form that names one field twice: Decline and Cancel only, each field shown', () => {
    const t = setup()
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {})
    show(elicitation({ fields: [text, { ...text, label: 'Name again' }] }), env(t.book))
    expect(screen.getByText('This form names one field twice, so it cannot be filled in here: it can only be declined or cancelled.', { exact: true })).toBeInTheDocument()
    expect(screen.getByText('Name', { exact: true })).toBeInTheDocument()
    expect(screen.getByText('Name again', { exact: true })).toBeInTheDocument()
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(screen.getAllByRole('button').map((b) => b.textContent)).toEqual(['Decline', 'Cancel'])
    // React keys by place: no duplicate-key warning for the agent's keys.
    expect(errors.mock.calls.filter((c) => String(c[0]).includes('same key'))).toEqual([])
    errors.mockRestore()
  })

  it.each([
    ['Decline', 'decline'],
    ['Cancel', 'cancel'],
  ])('%s sends its action with no content', async (label, action) => {
    const t = setup({ [`POST ${PATH('e1')}`]: ok() })
    show(elicitation({ fields: [text], form_supported: false }), env(t.book))
    fireEvent.click(screen.getByRole('button', { name: label }))
    await screen.findByText('Sent')
    expect(t.posts().map((p) => p.body)).toEqual([{ action }])
  })

  it('declines a form it can fill without sending what was typed', async () => {
    const t = setup({ [`POST ${PATH('e1')}`]: ok() })
    show(elicitation({ fields: [text] }), env(t.book))
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), { target: { value: 'Ada' } })
    fireEvent.click(screen.getByRole('button', { name: 'Decline' }))
    await screen.findByText('Sent')
    expect(t.posts().map((p) => p.body)).toEqual([{ action: 'decline' }])
  })

  it('pairs an exclusive text with its select: choosing clears the text, typing clears the choice', async () => {
    const t = setup({ [`POST ${PATH('e1')}`]: ok() })
    show(elicitation({ fields: [single, custom], required: ['indent'] }), env(t.book))
    const other = screen.getByRole('textbox', { name: 'Other' })
    fireEvent.change(other, { target: { value: 'two spaces' } })
    fireEvent.click(screen.getByRole('radio', { name: 'Tabs' }))
    expect(other).toHaveValue('')
    fireEvent.change(other, { target: { value: 'three' } })
    expect(screen.getByRole('radio', { name: 'Tabs' })).not.toBeChecked()
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await screen.findByText('Sent')
    expect(t.posts().map((p) => p.body)).toEqual([{ action: 'accept', content: { indent_custom: 'three' } }])
  })

  it('keeps the choice when only whitespace is typed into its exclusive text', () => {
    show(elicitation({ fields: [single, custom] }), env(setup().book))
    fireEvent.click(screen.getByRole('radio', { name: 'Tabs' }))
    fireEvent.change(screen.getByRole('textbox', { name: 'Other' }), { target: { value: '   ' } })
    expect(screen.getByRole('radio', { name: 'Tabs' })).toBeChecked()
  })

  it('keeps a note alongside its select: neither clears the other', async () => {
    const t = setup({ [`POST ${PATH('e1')}`]: ok() })
    show(elicitation({ fields: [multi, note] }), env(t.book))
    fireEvent.click(screen.getByRole('checkbox', { name: 'Rust' }))
    fireEvent.change(screen.getByRole('textbox', { name: 'Note' }), { target: { value: 'mostly' } })
    expect(screen.getByRole('checkbox', { name: 'Rust' })).toBeChecked()
    fireEvent.click(screen.getByRole('checkbox', { name: 'TypeScript' }))
    expect(screen.getByRole('textbox', { name: 'Note' })).toHaveValue('mostly')
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await screen.findByText('Sent')
    expect(t.posts().map((p) => p.body)).toEqual([{ action: 'accept', content: { langs: ['rs', 'ts'], langs_note: 'mostly' } }])
  })

  it('offers Send only once a real answer exists: whitespace is none', () => {
    show(elicitation({ fields: [text] }), env(setup().book))
    expect(screen.queryByRole('button', { name: 'Send' })).toBeNull()
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), { target: { value: '   ' } })
    expect(screen.queryByRole('button', { name: 'Send' })).toBeNull()
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), { target: { value: 'x' } })
    expect(screen.getByRole('button', { name: 'Send' })).toBeInTheDocument()
  })

  it('offers Send only once every required key has an answer', () => {
    show(elicitation({ fields: [text, multi], required: ['langs'] }), env(setup().book))
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), { target: { value: 'Ada' } })
    expect(screen.queryByRole('button', { name: 'Send' })).toBeNull()
    fireEvent.click(screen.getByRole('checkbox', { name: 'Rust' }))
    expect(screen.getByRole('button', { name: 'Send' })).toBeInTheDocument()
  })

  it('sends nothing on its own: no choice and no Enter sends', () => {
    const t = setup()
    show(elicitation({ fields: [single, text] }), env(t.book))
    fireEvent.click(screen.getByRole('radio', { name: 'Tabs' }))
    const name = screen.getByRole('textbox', { name: 'Name' })
    fireEvent.change(name, { target: { value: 'Ada' } })
    fireEvent.keyDown(name, { key: 'Enter' })
    fireEvent.submit(name)
    expect(t.posts()).toHaveLength(0)
    expect(screen.getByText('Needs your answer')).toBeInTheDocument()
  })

  it('chooses nothing for the operator', () => {
    show(elicitation({ fields: [single, multi] }), env(setup().book))
    for (const input of screen.getAllByRole('radio').concat(screen.getAllByRole('checkbox'))) expect(input).not.toBeChecked()
  })

  it('names the session’s agent as the one asking', () => {
    show(elicitation({ fields: [text] }), env(setup().book))
    expect(screen.getByText('Codex asks')).toBeInTheDocument()
  })

  it('takes no digit shortcut', () => {
    media([DESKTOP])
    const t = setup()
    show(elicitation({ fields: [single] }), env(t.book))
    fireEvent.keyDown(card(), { key: '1' })
    expect(t.posts()).toHaveLength(0)
  })
})

describe('a card without an answer book', () => {
  it('only shows', () => {
    show(permission(), { sessionId: SESSION, agent: 'Codex' })
    expect(screen.queryByRole('button')).toBeNull()
    expect(card()).not.toHaveAttribute('tabindex')
  })
})

describe('answering through the book', () => {
  it('waits for the post before it lets another go', async () => {
    const t = setup({ [`POST ${PATH('p/1')}`]: [json(400, { code: 'invalid', message: 'x' }), json(202, { pending_id: 'p/1', request_id: 'r' })] })
    await t.book.answer(permission(), { option_id: 'b' })
    expect(t.book.localOf('question:p/1')).toMatchObject({ phase: 'failed' })
    await t.book.answer(permission(), { option_id: 'b' })
    await waitFor(() => expect(t.book.localOf('question:p/1')).toMatchObject({ phase: 'sent', version: 1 }))
    expect(t.posts()).toHaveLength(2)
  })
})
