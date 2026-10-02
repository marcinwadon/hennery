// The session view answers its questions, through its real composer: the
// book is wired, a question opened on the stream takes the focus only when
// the composer is empty, a verdict that comes back as `delivered: false`
// never undoes "Answered", the host's connection comes from the view's one
// hosts list, and a question that can be answered is never held above the
// transcript's window.
import '@testing-library/jest-dom/vitest'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Item } from '../generated/view'
import { forgetAllAttachments } from '../lib/attachments'
import { json } from '../test-stream'
import SessionView from './Session'
import { FAST, message, sessionServer, type Opts } from './test-session'

const ID = 's1'
const TS = '2026-10-02T10:00:00.000Z'
const ANSWER = '/api/sessions/s1/pending/p1/answer'
const WAIT = { timeout: 5000 }

const question = (patch: object = {}): Item =>
  ({
    id: 'question:p1',
    version: 5,
    ts: TS,
    turn_id: 't1',
    kind: 'question',
    pending_id: 'p1',
    question_kind: 'permission',
    request: { type: 'permission', title: 'Run it?', options: [{ option_id: 'a', name: 'Allow', option_kind: 'allow_once' }] },
    answerable: true,
    state: 'open',
    answered: false,
    ...patch,
  }) as Item

const form = (patch: object = {}): Item =>
  question({
    question_kind: 'elicitation',
    request: { type: 'elicitation', message: 'Your name?', fields: [{ key: 'name', label: 'Name', field_kind: 'text' }], form_supported: true },
    ...patch,
  })

/** A cheap row: the operator's words, no Markdown. */
const said = (n: number): Item =>
  ({ id: `u${n}`, version: 1, ts: TS, turn_id: 't1', kind: 'user_turn', content: [{ type: 'text', text: `row ${n}` }] }) as Item

function server(items: Item[], opts: Opts = {}) {
  return sessionServer({
    items: () => items,
    detail: { agent: 'codex' },
    hosts: () => json([{ host_id: 'h1', name: 'build-box', connected: true, capabilities: ['images'] }]),
    ...opts,
  })
}

const card = () => screen.getByRole('region', { name: /Question from/ })
const textarea = () => screen.getByLabelText('Prompt') as HTMLTextAreaElement
const hostCalls = (s: ReturnType<typeof server>) => s.of('/api/hosts')

beforeEach(() => {
  sessionStorage.clear()
  forgetAllAttachments()
  URL.createObjectURL = vi.fn(() => 'blob:u')
  URL.revokeObjectURL = vi.fn()
})

describe('SessionView answering', () => {
  it('answers a question through its route, as the session’s agent asks it', async () => {
    const s = server([message('m1', 't1'), question()])
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    fireEvent.click(await screen.findByRole('button', { name: 'Allow' }))
    await screen.findByText('Sent')
    expect(s.posted('/answer')).toEqual([{ path: ANSWER, body: { option_id: 'a' } }])
    expect(await screen.findByText('Codex asks for permission', {}, WAIT)).toBeInTheDocument()
  })

  it('moves the focus to a question opened on the stream while the composer is empty', async () => {
    const s = server([message('m1', 't1')])
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('m1')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    expect(textarea().value).toBe('')
    act(() => s.streams[0].event('item', question()))
    await screen.findByText('Run it?')
    await waitFor(() => expect(card()).toHaveFocus())
  })

  it('leaves the focus in the composer while it holds text', async () => {
    const s = server([message('m1', 't1')])
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('m1')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    textarea().focus()
    fireEvent.change(textarea(), { target: { value: 'half a thought' } })
    act(() => s.streams[0].event('item', question()))
    await screen.findByText('Run it?')
    // A later event: once it shows, the card's effects have run.
    act(() => s.streams[0].event('item', message('m2', 't1')))
    await screen.findByText('m2')
    expect(card()).not.toHaveFocus()
    expect(document.activeElement).toBe(textarea())
  })

  it('never takes the focus for a question that was there when the session opened', async () => {
    const s = server([message('m1', 't1'), question()])
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('Run it?')
    expect(card()).not.toHaveFocus()
  })

  it('keeps “Answered” when a later version says not delivered', async () => {
    const s = server([message('m1', 't1'), question({ answered: true, answerable: false, delivered: true })])
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('Answered')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => {
      s.streams[0].event('item', question({ version: 6, answered: true, answerable: false, delivered: false }))
      // A later event: once it shows, the one before was taken in.
      s.streams[0].event('item', message('m2', 't1'))
    })
    await screen.findByText('m2')
    expect(screen.getByText('Answered')).toBeInTheDocument()
    expect(screen.queryByText('Sent, but the agent was no longer waiting')).toBeNull()
  })

  it('says an answer delivers when the host reconnects while the session is presumed parked', async () => {
    const s = server([message('m1', 't1'), question({ answered: true, answerable: false })], { detail: { agent: 'codex', presumed_parked: true } })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('Sent')
    expect(await screen.findByText('delivers when the host reconnects', {}, WAIT)).toBeInTheDocument()
  })

  it('reads the host’s connection from the view’s one hosts request, shared with the composer', async () => {
    const s = server([message('m1', 't1'), question({ answered: true, answerable: false })], {
      hosts: () => json([{ host_id: 'h1', name: 'build-box', connected: false, capabilities: [] }]),
    })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('Sent')
    // The card used the list: the host is not connected.
    expect(await screen.findByText('delivers when the host reconnects', {}, WAIT)).toBeInTheDocument()
    // The composer used it: the host takes no images.
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Attach images' })).toBeNull(), WAIT)
    // And the header named the host from it.
    expect(screen.getByText('build-box')).toBeInTheDocument()
    expect(hostCalls(s)).toHaveLength(1)
  })

  it('says nothing of the host while it is connected', async () => {
    const s = server([message('m1', 't1'), question({ answered: true, answerable: false })])
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('Sent')
    await screen.findByText('build-box', {}, WAIT)
    expect(screen.queryByText('delivers when the host reconnects')).toBeNull()
  })

  it('answers a question as a new message in the composer, one stop after the question', async () => {
    const s = server([message('m1', 't1'), question({ state: 'cancelled', reason: 'adapter_lost', answerable: false })])
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    fireEvent.click(await screen.findByRole('button', { name: 'Answer as a new message' }))
    await waitFor(() => expect(textarea().value).toBe('You asked: Run it? My answer: '))
    expect(screen.getByText('Answering a question as a new message')).toBeInTheDocument()
    expect(s.posted('/prompt')).toEqual([])
  })
})

describe('SessionView: questions and the transcript’s window', () => {
  // 250 rows, the open question 11th: the newest 200 would start at row 50.
  const long = (q: Item) => [...Array.from({ length: 10 }, (_, n) => said(n)), q, ...Array.from({ length: 239 }, (_, n) => said(n + 11))]

  it('starts the window at a question that can be answered, however far above the tail it is', async () => {
    const s = server(long(question()))
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    expect(await screen.findByRole('button', { name: 'Allow' }, WAIT)).toBeInTheDocument()
    expect(screen.getByText('Needs your answer')).toBeInTheDocument()
    expect(screen.getByText('row 11')).toBeInTheDocument()
    expect(screen.getByText('row 249')).toBeInTheDocument()
    expect(screen.queryByText('row 9')).toBeNull()
    expect(screen.getByRole('button', { name: /Load earlier/ })).toBeInTheDocument()
  })

  it('holds a question that cannot be answered above the window as before', async () => {
    const s = server(long(question({ answered: true, answerable: false, delivered: true })))
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('row 249', {}, WAIT)
    expect(screen.queryByText('Run it?')).toBeNull()
    expect(screen.queryByText('row 49')).toBeNull()
    expect(screen.getByText('row 50')).toBeInTheDocument()
  })

  it('keeps the window where it is once the question is answered', async () => {
    const s = server(long(question()))
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByRole('button', { name: 'Allow' }, WAIT)
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].event('item', question({ version: 6, answered: true, answerable: false, delivered: true })))
    await screen.findByText('Answered')
    expect(screen.getByText('row 11')).toBeInTheDocument()
  })

  it('keeps a half-filled form across a reveal of earlier rows', async () => {
    // The form 61st of 250: inside the newest 200, with rows held above.
    const items = [...Array.from({ length: 60 }, (_, n) => said(n)), form(), ...Array.from({ length: 189 }, (_, n) => said(n + 61))]
    const s = server(items)
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    const name = (await screen.findByRole('textbox', { name: 'Name' }, WAIT)) as HTMLInputElement
    fireEvent.change(name, { target: { value: 'Ad' } })
    expect(screen.queryByText('row 0')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /Load earlier/ }))
    await screen.findByText('row 0')
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveValue('Ad')
    expect(within(card()).getByRole('button', { name: 'Send' })).toBeInTheDocument()
  })
})
