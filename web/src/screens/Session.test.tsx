import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import type { SessionDetail } from '../generated/protocol'
import type { Item, ItemPage, SessionSummary } from '../generated/view'
import { json, liveStream, routed, type Call, type LiveStream } from '../test-stream'
import SessionView, { TAIL } from './Session'

const ID = 's1'
const PAGE = '/api/view/sessions/s1'
const STREAM = '/api/stream/view/sessions/s1'
const DETAIL = '/api/sessions/s1'
const FAST = { retryMs: () => 5, resyncedMs: 300 }

function message(id: string, turn: string, text = id): Item {
  return { id, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: turn, kind: 'message', text } as Item
}

function page(items: Item[], older = false, revision = 10): ItemPage {
  return { items, older, epoch: 'e1', revision }
}

function detail(agent: string, patch: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session_id: ID,
    host_id: 'h1',
    agent,
    cwd: '/srv/work/project',
    hat_id: 'hat1',
    lifecycle: 'active',
    activity: 'idle',
    presumed_parked: false,
    created_at: '2026-10-02T09:00:00.000Z',
    last_event_at: '2026-10-02T10:00:00.000Z',
    pending: [],
    ...patch,
  } as SessionDetail
}

type Answer = Response | Promise<Response>

/** A server for one session. `pages` are served in turn (the last repeats);
 *  `older` by `before_turn`. */
function server(opts: {
  pages: (() => Answer)[]
  older?: Record<string, () => Answer>
  detail?: () => Answer
  hats?: () => Answer
}) {
  const streams: LiveStream[] = []
  const pages = [...opts.pages]
  const t = routed((call: Call) => {
    const url = new URL(call.path, 'http://h')
    if (url.pathname === PAGE) {
      const before = url.searchParams.get('before_turn')
      if (before !== null) return opts.older?.[before]?.() ?? json({ code: 'x', message: 'x' }, 500)
      return (pages.length > 1 ? pages.shift()! : pages[0])()
    }
    if (url.pathname === STREAM) {
      const live = liveStream()
      streams.push(live)
      return live.response
    }
    if (url.pathname === DETAIL) return opts.detail?.() ?? json(detail('claude'))
    if (url.pathname === `${DETAIL}/catalog`) return json({ session_id: ID, config_options: [], commands: [] })
    if (url.pathname === '/api/hosts') return json([{ host_id: 'h1', name: 'build-box' }])
    if (url.pathname === '/api/hats') return opts.hats?.() ?? json({ code: 'not_found', message: 'no' }, 404)
    return json({ code: 'not_found', message: 'no' }, 404)
  })
  const of = (path: string) => t.calls.filter((c) => new URL(c.path, 'http://h').pathname === path)
  return { ...t, streams, of }
}

// jsdom lays nothing out: the transcript's height is 100 px per row, its
// window 300 px, and its scroll position is held and clamped as a browser
// would.
const ROW = 100
const VIEW = 300
const tops = new WeakMap<Element, number>()
function isScroller(el: Element) {
  return el.classList.contains('transcript')
}
function heightOf(el: Element) {
  return el.querySelectorAll('.msg, .marker, .ask').length * ROW
}
const saved: Record<string, PropertyDescriptor | undefined> = {}
beforeAll(() => {
  for (const key of ['scrollHeight', 'clientHeight', 'scrollTop']) {
    saved[key] = Object.getOwnPropertyDescriptor(Element.prototype, key)
  }
  Object.defineProperty(Element.prototype, 'scrollHeight', {
    configurable: true,
    get(this: Element) {
      return isScroller(this) ? heightOf(this) : 0
    },
  })
  Object.defineProperty(Element.prototype, 'clientHeight', {
    configurable: true,
    get(this: Element) {
      return isScroller(this) ? VIEW : 0
    },
  })
  Object.defineProperty(Element.prototype, 'scrollTop', {
    configurable: true,
    get(this: Element) {
      return tops.get(this) ?? 0
    },
    set(this: Element, value: number) {
      tops.set(this, Math.max(0, Math.min(value, heightOf(this) - VIEW)))
    },
  })
})
afterAll(() => {
  for (const [key, d] of Object.entries(saved)) if (d) Object.defineProperty(Element.prototype, key, d)
})

afterEach(() => {
  // @ts-expect-error clear a stub between tests
  delete window.matchMedia
})

const scroller = () => document.querySelector('.transcript') as HTMLElement

function scrollTo(top: number) {
  const el = scroller()
  el.scrollTop = top
  fireEvent.scroll(el)
}

const rows = (n: number, turn: string, prefix = turn) => Array.from({ length: n }, (_, i) => message(`${prefix}-${i}`, turn))

describe('SessionView', () => {
  it('opens at the end of the transcript', async () => {
    const s = server({ pages: [() => json(page(rows(10, 't5')))] })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-9')
    expect(scroller().scrollTop).toBe(10 * ROW - VIEW)
  })

  it('loads earlier turns from a button, keeping the reader’s place', async () => {
    const s = server({
      pages: [() => json(page(rows(10, 't5'), true))],
      older: { t5: () => json(page(rows(4, 't4'), false)) },
    })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-9')
    scrollTo(300)
    fireEvent.click(screen.getByRole('button', { name: /Load earlier/ }))
    await screen.findByText('t4-0')
    expect(s.of(PAGE).map((c) => new URL(c.path, 'http://h').searchParams.get('before_turn'))).toEqual([null, 't5'])
    // The rows on screen stay where they were: four rows went in above them.
    expect(scroller().scrollTop).toBe(300 + 4 * ROW)
    // Nothing older is left: no button.
    expect(screen.queryByRole('button', { name: /Load earlier/ })).toBeNull()
  })

  it('loads earlier turns on scrolling to the top', async () => {
    const s = server({
      pages: [() => json(page(rows(10, 't5'), true))],
      older: { t5: () => json(page(rows(4, 't4'), false)) },
    })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-9')
    scrollTo(0)
    await screen.findByText('t4-0')
    expect(scroller().scrollTop).toBe(4 * ROW)
  })

  it('does not load earlier turns while scrolling below the top', async () => {
    const s = server({ pages: [() => json(page(rows(10, 't5'), true))] })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-9')
    scrollTo(400)
    expect(s.of(PAGE)).toHaveLength(1)
  })

  it('follows new items while the reader is at the end', async () => {
    const s = server({ pages: [() => json(page(rows(5, 't5')))] })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-4')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].event('item', message('t5-new', 't5')))
    await screen.findByText('t5-new')
    expect(scroller().scrollTop).toBe(6 * ROW - VIEW)
  })

  it('leaves the reader where they are when scrolled up', async () => {
    const s = server({ pages: [() => json(page(rows(8, 't5')))] })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-7')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    scrollTo(200)
    act(() => s.streams[0].event('item', message('t5-new', 't5')))
    await screen.findByText('t5-new')
    expect(scroller().scrollTop).toBe(200)
  })

  it('says “Reconnecting…” while it resyncs, then “Resynced” for a moment', async () => {
    let release!: (r: Response) => void
    const s = server({
      pages: [() => json(page(rows(2, 't5'))), () => new Promise<Response>((r) => (release = r))],
    })
    // "Resynced" shows long enough to be seen on a loaded machine.
    render(<SessionView id={ID} timing={{ ...FAST, resyncedMs: 1500 }} />, { wrapper: s.wrapper })
    await screen.findByText('t5-1')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    expect(screen.queryByText('Reconnecting…')).toBeNull()
    act(() => s.streams[0].event('resync_required', {}))
    expect(await screen.findByText('Reconnecting…')).toBeInTheDocument()
    await act(async () => release(json(page(rows(3, 't6')))))
    expect(await screen.findByText('Resynced')).toBeInTheDocument()
    expect(screen.queryByText('Reconnecting…')).toBeNull()
    expect(screen.getByText('t6-2')).toBeInTheDocument()
    await waitFor(() => expect(screen.queryByText('Resynced')).toBeNull(), { timeout: 5000 })
  })

  it('says a deleted session was deleted, and opens no stream', async () => {
    const s = server({ pages: [() => json({ code: 'not_found', message: 'gone' }, 404)] })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    expect(await screen.findByRole('heading', { name: 'This session was deleted' })).toBeInTheDocument()
    expect(s.of(STREAM)).toHaveLength(0)
  })

  it('says so when the stream says the session was removed', async () => {
    const s = server({ pages: [() => json(page(rows(2, 't5')))] })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-1')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].event('session_removed', { session_id: ID }))
    expect(await screen.findByRole('heading', { name: 'This session was deleted' })).toBeInTheDocument()
    expect(screen.queryByText('t5-1')).toBeNull()
  })

  it.each([
    ['claude', 'Claude'],
    ['codex', 'Codex'],
    ['my-agent', 'my-agent'],
  ])('labels the %s agent “%s”, and the user “You”', async (agent, label) => {
    const user = { id: 'u', version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'user_turn', content: [{ type: 'text', text: 'hello' }] } as Item
    const s = server({ pages: [() => json(page([user, message('m', 't5')]))], detail: () => json(detail(agent)) })
    const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await waitFor(() => expect(Array.from(container.querySelectorAll('.msg-who')).map((e) => e.textContent)).toEqual(['You', label]))
    // The avatars carry a letter of the label, never anyone's initials.
    expect(Array.from(container.querySelectorAll('.avatar')).map((e) => e.textContent)).toEqual(['Y', label[0].toUpperCase()])
  })

  it('reads the header from the session’s detail when no summary is given', async () => {
    const s = server({
      pages: [() => json(page(rows(1, 't5')))],
      detail: () => json(detail('codex', { title: 'Fix the build', git_branch: 'main', model: 'gpt-x', mode: 'auto', activity: 'running' })),
    })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    const head = (await screen.findByRole('heading', { name: 'Fix the build' })).closest('header') as HTMLElement
    const h = within(head)
    await waitFor(() => expect(h.getByLabelText('Host')).toHaveTextContent('build-box'))
    expect(h.getByLabelText('Agent')).toHaveTextContent('Codex')
    expect(h.getByLabelText('Branch')).toHaveTextContent('main')
    expect(h.getByLabelText('Model')).toHaveTextContent('gpt-x')
    expect(h.getByLabelText('Mode')).toHaveTextContent('auto')
    expect(h.getByText('Running')).toBeInTheDocument()
    expect(s.of(DETAIL)).toHaveLength(1)
  })

  it('says “Waiting on a question” from the detail for a question outside a turn', async () => {
    // The detail has no `question_waits` (the real type has none), and a
    // question outside a turn leaves `activity` idle: its pending request
    // says it.
    const pending = {
      pending_id: 'p1',
      session_id: ID,
      kind: 'permission',
      state: 'open',
      payload: {},
      answered: false,
    } as SessionDetail['pending'][number]
    const s = server({ pages: [() => json(page(rows(1, 't5')))], detail: () => json(detail('claude', { activity: 'idle', pending: [pending] })) })
    const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await waitFor(() => expect(container.querySelector('header .badge')?.textContent).toBe('Waiting on a question'))
    expect(container.querySelector('header .badge')?.className).toBe('badge badge-attn')
    expect(s.of(DETAIL)).toHaveLength(1)
  })

  it('reads an idle detail with no pending request as “Idle”', async () => {
    const s = server({ pages: [() => json(page(rows(1, 't5')))], detail: () => json(detail('claude', { activity: 'idle' })) })
    const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await waitFor(() => expect(container.querySelector('header .badge')?.textContent).toBe('Idle'))
  })

  it('reads the header from the summary it is given, without fetching the detail', async () => {
    const summary = { ...detail('claude'), pending: undefined, title: undefined, question_waits: true } as unknown as SessionSummary
    const s = server({ pages: [() => json(page(rows(1, 't5')))] })
    render(<SessionView id={ID} summary={summary} timing={FAST} />, { wrapper: s.wrapper })
    // No title: the project directory's name.
    expect(await screen.findByRole('heading', { name: 'project' })).toBeInTheDocument()
    expect(screen.getByText('Waiting on a question')).toBeInTheDocument()
    await screen.findByText('t5-0')
    expect(s.of(DETAIL)).toHaveLength(0)
  })

  it('follows the summary it is given as it changes', async () => {
    const base = { ...detail('claude'), pending: undefined, title: 'A task' } as unknown as SessionSummary
    const s = server({ pages: [() => json(page(rows(1, 't5')))] })
    const { rerender } = render(<SessionView id={ID} summary={base} timing={FAST} />, { wrapper: s.wrapper })
    const head = (await screen.findByRole('heading', { name: 'A task' })).closest('header') as HTMLElement
    expect(within(head).getByText('Idle')).toBeInTheDocument()
    rerender(<SessionView id={ID} summary={{ ...base, activity: 'running', title: 'A renamed task' }} timing={FAST} />)
    expect(within(head).getByText('Running')).toBeInTheDocument()
    expect(within(head).getByRole('heading', { name: 'A renamed task' })).toBeInTheDocument()
    expect(s.of(DETAIL)).toHaveLength(0)
  })

  it('takes the summary over a detail fetched before it came', async () => {
    const s = server({ pages: [() => json(page(rows(1, 't5')))], detail: () => json(detail('codex', { title: 'From the detail' })) })
    const { rerender } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByRole('heading', { name: 'From the detail' })
    const summary = { ...detail('codex'), pending: undefined, title: 'From the list' } as unknown as SessionSummary
    rerender(<SessionView id={ID} summary={summary} timing={FAST} />)
    expect(await screen.findByRole('heading', { name: 'From the list' })).toBeInTheDocument()
  })

  it('keeps a summary that went over the detail fetched before it, until the detail comes again', async () => {
    let release: (() => void) | undefined
    let calls = 0
    const s = server({
      pages: [() => json(page(rows(1, 't5')))],
      detail: () => {
        calls++
        if (calls === 1) return json(detail('codex', { title: 'From the detail' }))
        // The refetch, once the summary went: held until released.
        return new Promise<Response>((resolve) => {
          release = () => resolve(json(detail('codex', { title: 'From the detail, again' })))
        })
      },
    })
    const { rerender } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByRole('heading', { name: 'From the detail' })
    const summary = { ...detail('codex'), pending: undefined, title: 'From the list' } as unknown as SessionSummary
    rerender(<SessionView id={ID} summary={summary} timing={FAST} />)
    await screen.findByRole('heading', { name: 'From the list' })
    // The summary goes (a new search's results) while the refetch is held.
    rerender(<SessionView id={ID} timing={FAST} />)
    await waitFor(() => expect(s.of(DETAIL)).toHaveLength(2))
    expect(screen.getByRole('heading', { name: 'From the list' })).toBeInTheDocument()
    act(() => release!())
    expect(await screen.findByRole('heading', { name: 'From the detail, again' })).toBeInTheDocument()
  })

  it.each([
    [{ lifecycle: 'starting' }, 'Starting', 'wait'],
    [{ activity: 'blocked' }, 'Waiting on a question', 'attn'],
    [{ lifecycle: 'parked' }, 'Parked', 'idle'],
    [{ lifecycle: 'parked', presumed_parked: true }, 'Host offline', 'idle'],
    [{ lifecycle: 'closed' }, 'Closed', 'idle'],
    [{ lifecycle: 'failed', failure_reason: 'agent_not_logged_in' }, 'Failed', 'fail'],
    // Waiting on a question wins over every lifecycle, as on the row.
    [{ lifecycle: 'parked', question_waits: true }, 'Waiting on a question', 'attn'],
    [{ lifecycle: 'closed', activity: 'blocked' }, 'Waiting on a question', 'attn'],
    [{ lifecycle: 'starting', question_waits: true }, 'Waiting on a question', 'attn'],
  ] as [Partial<SessionDetail> & { question_waits?: boolean }, string, string][])('reads %o as “%s”, as the row does', async (patch, text, tone) => {
    const summary = { ...detail('claude', patch), pending: undefined } as unknown as SessionSummary
    const s = server({ pages: [() => json(page(rows(1, 't5')))] })
    const { container } = render(<SessionView id={ID} summary={summary} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-0')
    expect(container.querySelector('header .badge')?.textContent).toBe(text)
    expect(container.querySelector('header .badge')?.className).toBe(`badge badge-${tone}`)
    // The reason in words, never its code.
    if (patch.failure_reason) {
      expect(container.querySelector('header')?.textContent).toContain('Reason: the agent is not logged in on the host')
      expect(container.querySelector('header')?.textContent).not.toContain(patch.failure_reason)
    }
  })

  it('shows the newest plan’s steps in the header', async () => {
    const plan = (id: string, step: string) =>
      ({ id, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'plan', entries: [{ content: step, status: 'in_progress' }] }) as Item
    const s = server({ pages: [() => json(page([plan('p1', 'old step'), plan('p2', 'new step')]))] })
    const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findAllByText('new step')
    const header = container.querySelector('.session > .steps') as HTMLElement
    expect(within(header).getAllByText('new step').length).toBeGreaterThan(0)
    expect(within(header).queryByText('old step')).toBeNull()
  })

  it('names the hats of a reassignment from the hat list', async () => {
    const moved = { id: 'mv', version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'marker', marker: 'hat_reassigned', from: 'h-a', to: 'h-b' } as Item
    const s = server({
      pages: [() => json(page([moved]))],
      hats: () => json([{ id: 'h-a', name: 'Work' }, { id: 'h-b', name: 'Home' }]),
    })
    const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await waitFor(() => expect(container.querySelector('.marker')?.textContent).toContain('from Work to Home'))
  })

  it('fetches no hats when no item names one', async () => {
    const s = server({ pages: [() => json(page(rows(1, 't5')))] })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-0')
    expect(s.of('/api/hats')).toHaveLength(0)
  })

  it('has a way back to the list', async () => {
    const s = server({ pages: [() => json(page(rows(1, 't5')))] })
    render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    expect(await screen.findByRole('link', { name: 'Back to sessions' })).toHaveAttribute('href', '/sessions')
  })

  it('starts the header’s steps closed on a phone', async () => {
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: query === '(max-width: 767px)',
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia
    const plan = { id: 'p', version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'plan', entries: [{ content: 'a', status: 'in_progress' }] } as Item
    const s = server({ pages: [() => json(page([plan]))] })
    const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findAllByText('a')
    expect((container.querySelector('.session > .steps') as HTMLDetailsElement).open).toBe(false)
    expect((container.querySelector('.transcript .steps') as HTMLDetailsElement).open).toBe(true)
  })
})

/** The transcript's rows, by the text of each message (its id). */
const shown = () => Array.from(document.querySelectorAll('.transcript .bubble')).map((b) => b.textContent)

describe('the tail window', () => {
  it(`renders at most the newest ${TAIL} items on open`, async () => {
    // The windowing measurement's threshold under load.
    expect(TAIL).toBe(200)
    // Markers: a cheap row, so the real window size stays quick under load.
    const markers = Array.from(
      { length: TAIL + 30 },
      (_, i) => ({ id: `m${i}`, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'marker', marker: 'host_back' }) as Item,
    )
    const s = server({ pages: [() => json(page(markers))] })
    const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
    await waitFor(() => expect(container.querySelectorAll('.marker')).toHaveLength(TAIL))
    // Nothing older on the server, but rows are held: the way up stays.
    expect(screen.getByRole('button', { name: /Load earlier/ })).toBeInTheDocument()
    expect(scroller().scrollTop).toBe(TAIL * ROW - VIEW)
  })

  it('reveals held rows before fetching, and fetches only once none is held', async () => {
    const s = server({
      pages: [() => json(page(rows(11, 't5'), true))],
      older: { t5: () => json(page(rows(6, 't4'), false)) },
    })
    render(<SessionView id={ID} tail={4} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-10')
    expect(shown()).toEqual(['t5-7', 't5-8', 't5-9', 't5-10'])
    const earlier = () => fireEvent.click(screen.getByRole('button', { name: /Load earlier/ }))
    earlier()
    await screen.findByText('t5-3')
    expect(shown()).toHaveLength(8)
    earlier()
    await screen.findByText('t5-0')
    expect(shown()).toHaveLength(11)
    // Two reveals, no fetch.
    expect(s.of(PAGE)).toHaveLength(1)
    earlier()
    // Nothing held: now the turns before, and up to a window of them shown.
    await screen.findByText('t4-2')
    expect(s.of(PAGE).map((c) => new URL(c.path, 'http://h').searchParams.get('before_turn'))).toEqual([null, 't5'])
    expect(shown().slice(0, 5)).toEqual(['t4-2', 't4-3', 't4-4', 't4-5', 't5-0'])
    expect(screen.queryByText('t4-1')).toBeNull()
  })

  it('keeps the reader’s place when held rows are revealed, from the button or the top', async () => {
    const s = server({ pages: [() => json(page(rows(15, 't5')))] })
    render(<SessionView id={ID} tail={6} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-14')
    // Below the top: scrolling there reveals nothing by itself.
    scrollTo(200)
    expect(shown()).toHaveLength(6)
    fireEvent.click(screen.getByRole('button', { name: /Load earlier/ }))
    await screen.findByText('t5-3')
    expect(scroller().scrollTop).toBe(200 + 6 * ROW)
    scrollTo(0)
    await screen.findByText('t5-0')
    expect(scroller().scrollTop).toBe(3 * ROW)
    expect(s.of(PAGE)).toHaveLength(1)
  })

  it('pins its first row by id: a new item at the end leaves it, and shows while at the end', async () => {
    const s = server({ pages: [() => json(page(rows(6, 't5')))] })
    render(<SessionView id={ID} tail={4} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-5')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    expect(shown()).toEqual(['t5-2', 't5-3', 't5-4', 't5-5'])
    act(() => s.streams[0].event('item', message('t5-new', 't5')))
    await screen.findByText('t5-new')
    expect(shown()).toEqual(['t5-2', 't5-3', 't5-4', 't5-5', 't5-new'])
    expect(scroller().scrollTop).toBe(5 * ROW - VIEW)
  })

  it('shows nothing new for an upsert of a held row, or a new row of a held turn', async () => {
    const s = server({ pages: [() => json(page([...rows(3, 't4'), ...rows(4, 't5')]))] })
    render(<SessionView id={ID} tail={4} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-3')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].event('item', { ...message('t4-0', 't4', 'changed'), version: 2 }))
    act(() => s.streams[0].event('item', message('t4-new', 't4')))
    act(() => s.streams[0].event('item', message('t5-new', 't5')))
    await screen.findByText('t5-new')
    expect(shown()).toEqual(['t5-0', 't5-1', 't5-2', 't5-3', 't5-new'])
    expect(screen.queryByText('changed')).toBeNull()
    expect(screen.queryByText('t4-new')).toBeNull()
  })

  it('goes back to the tail, and to the end, on a resync', async () => {
    // The same ids come back: the resync itself, not a missing row, resets.
    const s = server({ pages: [() => json(page(rows(10, 't5'))), () => json(page(rows(10, 't5'), false, 20))] })
    render(<SessionView id={ID} tail={5} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-9')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    fireEvent.click(screen.getByRole('button', { name: /Load earlier/ }))
    await screen.findByText('t5-0')
    scrollTo(150)
    act(() => s.streams[0].event('resync_required', {}))
    await waitFor(() => expect(s.streams).toHaveLength(2))
    await waitFor(() => expect(shown()).toEqual(['t5-5', 't5-6', 't5-7', 't5-8', 't5-9']))
    expect(scroller().scrollTop).toBe(5 * ROW - VIEW)
  })

  it('goes to the end on a resync even when the new rows hold the old first one', async () => {
    const again = [...rows(2, 't4'), message('t5-5', 't5'), message('t5-6', 't5')]
    const s = server({ pages: [() => json(page(rows(10, 't5'))), () => json(page(again, false, 20))] })
    render(<SessionView id={ID} tail={5} timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('t5-9')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    scrollTo(150)
    act(() => s.streams[0].event('resync_required', {}))
    await waitFor(() => expect(shown()).toEqual(['t4-0', 't4-1', 't5-5', 't5-6']))
    // Not rows prepended above the reader: the end.
    expect(scroller().scrollTop).toBe(4 * ROW - VIEW)
  })
})
