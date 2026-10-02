// The session list in the app: day headings, search, filters, hats,
// selection (F-10, F-11), the waiting count and the host names.
import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import type { HatItem, HostItem } from '../generated/protocol'
import type { SessionSummary, SummaryPage } from '../generated/view'
import { THEME_KEY } from '../lib/theme'
import { json, liveStream, type LiveStream } from '../test-stream'
import { PAGE_ROWS, byDay } from './SessionList'
import { navigate } from '../router'
import { host } from '../test-fixtures'
import { DESKTOP } from './Shell'

const LIST = '/api/view/sessions'
const STREAM = '/api/stream/sessions'

function summary(id: string, patch: Partial<SessionSummary> = {}): SessionSummary {
  return {
    session_id: id,
    host_id: 'h1',
    agent: 'claude',
    cwd: `/srv/work/${id}`,
    hat_id: 'hat-a',
    lifecycle: 'active',
    activity: 'idle',
    presumed_parked: false,
    created_at: '2026-10-01T00:00:00.000Z',
    last_event_at: '2026-10-02T10:00:00.000Z',
    question_waits: false,
    ...patch,
  }
}

/** A page; with no `waiting`, a server that does not count (the rows are
 *  counted instead: the fallback). */
const page = (sessions: SessionSummary[], next?: string, waiting?: number): SummaryPage =>
  ({ sessions, next_cursor: next, epoch: 'e1', revision: 1, ...(waiting === undefined ? {} : { waiting }) }) as SummaryPage

function hat(id: string, name: string, colour: string): HatItem {
  return { id, name, colour, created_at: '2026-10-01T00:00:00Z', default_for_new_hosts: false, purging: false }
}

const HATS = [hat('hat-a', 'Work', '#112233'), hat('hat-b', 'Home', 'not-a-colour')]
const HOSTS = [{ host_id: 'h1', name: 'laptop' } as HostItem]

interface Options {
  /** The list page for a query (the search params as a string), or the
   *  server's whole answer. */
  list?: (params: URLSearchParams) => SummaryPage | Response
  hats?: HatItem[] | null
  /** Read at each request: a test may change it. */
  hosts?: HostItem[]
}

function server({ list = () => page([]), hats = HATS, hosts = HOSTS }: Options = {}) {
  const calls: URL[] = []
  const streams: LiveStream[] = []
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = new URL(String(input), 'http://h')
    calls.push(url)
    switch (url.pathname) {
      case '/api/capabilities':
        return json({ mode: 'full', features: [] })
      case '/api/hats':
        return hats ? json(hats) : json({ code: 'internal', message: 'no' }, 500)
      case '/api/hosts':
        return json(hosts)
      case LIST: {
        const answer = list(url.searchParams)
        return answer instanceof Response ? answer : json(answer)
      }
      case STREAM: {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      default:
        return json({ code: 'not_found', message: 'no' }, 404)
    }
  })
  const of = (path: string) => calls.filter((u) => u.pathname === path)
  return { fetch: fetch as unknown as typeof globalThis.fetch, calls, streams, of }
}

function width(desktop: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: query === DESKTOP && desktop,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
}

function at(path: string) {
  history.replaceState(null, '', path)
}

const names = (scope: HTMLElement) =>
  within(scope)
    .queryAllByRole('link')
    .filter((a) => a.classList.contains('sess'))
    .map((a) => a.querySelector('.sess-name')?.textContent)

async function list() {
  return screen.findByRole('region', { name: 'Session list' })
}

beforeEach(() => {
  localStorage.clear()
  document.title = 'hennery'
  width(false)
})

afterEach(() => {
  at('/')
  // @ts-expect-error jsdom has none; each test sets its own
  delete window.matchMedia
  vi.useRealTimers()
})

describe('day headings', () => {
  it('are headings, in order, from calendar-local midnights', async () => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(new Date(2026, 9, 2, 12, 0))
    const local = (d: number, h = 10) => new Date(2026, 9, d, h, 0).toISOString()
    at('/sessions')
    const s = server({
      list: () =>
        page([
          summary('today', { last_event_at: local(2, 0) }),
          summary('yesterday', { last_event_at: local(1, 23) }),
          summary('week', { last_event_at: new Date(2026, 8, 29, 10).toISOString() }),
          summary('earlier', { last_event_at: '2026-06-01T10:00:00.000Z' }),
          summary('month', { last_event_at: new Date(2026, 8, 20, 10).toISOString() }),
          summary('broken', { last_event_at: 'not a time' }),
        ]),
    })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(within(region).getAllByRole('heading', { level: 2 }).length).toBeGreaterThan(0))
    const headings = within(region)
      .getAllByRole('heading', { level: 2 })
      .map((h) => h.querySelector('.grp-title')?.textContent)
    expect(headings).toEqual(['Today', 'Yesterday', 'This week', 'This month', 'Earlier'])
  })

  it('move on with the clock on an idle list: across midnight Today becomes Yesterday, and 29m ago 31m ago', async () => {
    // No list event and no fetch after the first page: only the clock moves.
    vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
    vi.setSystemTime(new Date(2026, 9, 2, 23, 59))
    at('/sessions')
    const s = server({ list: () => page([summary('late', { last_event_at: new Date(2026, 9, 2, 23, 30).toISOString() })]) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['late']))
    const headings = () =>
      within(region)
        .getAllByRole('heading', { level: 2 })
        .map((h) => h.querySelector('.grp-title')?.textContent)
    const time = () => region.querySelector('.sess-time')?.textContent
    expect(headings()).toEqual(['Today'])
    expect(time()).toBe('29m ago')
    act(() => vi.advanceTimersByTime(2 * 60_000))
    expect(headings()).toEqual(['Yesterday'])
    expect(time()).toBe('31m ago')
    expect(s.of(LIST)).toHaveLength(1)
  })

  it('put an empty or unparseable time under Earlier, and a future one under Today', () => {
    const now = new Date(2026, 9, 2, 12, 0).getTime()
    const days = byDay(
      [
        summary('future', { last_event_at: new Date(2026, 9, 5, 9, 0).toISOString() }),
        summary('empty', { last_event_at: '' }),
        summary('broken', { last_event_at: 'not a time' }),
      ],
      now,
    )
    expect(days.map(([b, rows]) => [b, rows.map((s) => s.session_id)])).toEqual([
      ['today', ['future']],
      ['earlier', ['empty', 'broken']],
    ])
  })

  it('put a session from before a 23 h day under Yesterday', () => {
    // Europe/Warsaw (vite.config.ts): 2026-03-29 has 23 hours.
    const now = new Date(2026, 2, 30, 0, 30).getTime()
    const days = byDay([summary('a', { last_event_at: new Date(2026, 2, 29, 0, 10).toISOString() })], now)
    expect(days.map(([b]) => b)).toEqual(['yesterday'])
  })
})

describe('filters', () => {
  it('Hide closed is on by default and never hides a parked session', async () => {
    at('/sessions')
    const s = server({
      list: () =>
        page([
          summary('live'),
          summary('parked', { lifecycle: 'parked', activity: undefined }),
          summary('closed', { lifecycle: 'closed', activity: undefined }),
        ]),
    })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['parked', 'live']))
    expect(s.of(LIST)[0].searchParams.get('lifecycle')).toBe('starting,active,parked,failed')
    await userEvent.click(within(region).getByRole('button', { name: 'Hide closed' }))
    await waitFor(() => expect(names(region)).toContain('closed'))
    expect(localStorage.getItem('hennery.hideClosed')).toBe('false')
    expect(s.of(LIST).at(-1)!.searchParams.get('lifecycle')).toBeNull()
  })

  it('a lifecycle filter shows that lifecycle only', async () => {
    at('/sessions')
    const s = server({
      list: () => page([summary('live'), summary('closed', { lifecycle: 'closed', activity: undefined })]),
    })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await userEvent.selectOptions(within(region).getByRole('combobox', { name: 'Lifecycle' }), 'closed')
    await waitFor(() => expect(names(region)).toEqual(['closed']))
    expect(s.of(LIST).at(-1)!.searchParams.get('lifecycle')).toBe('closed')
  })

  it('a search runs on the server across every lifecycle: only the hat applies (F-10)', async () => {
    localStorage.setItem('hennery.hat', 'hat-a')
    at('/sessions')
    const s = server({
      list: (params) =>
        params.get('q')
          ? page([
              summary('found-closed', { lifecycle: 'closed', activity: undefined }),
              summary('other-hat', { hat_id: 'hat-b' }),
            ])
          : page([summary('live')]),
    })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await userEvent.selectOptions(within(region).getByRole('combobox', { name: 'Lifecycle' }), 'active')
    await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'fix')
    await waitFor(() => expect(names(region)).toEqual(['found-closed']))
    const sent = s.of(LIST).at(-1)!.searchParams
    expect(sent.get('q')).toBe('fix')
    expect(sent.get('hat')).toBe('hat-a')
    expect(sent.get('lifecycle')).toBeNull()
    expect(within(region).getByRole('button', { name: 'Hide closed' })).toBeDisabled()
    expect(within(region).getByRole('combobox', { name: 'Lifecycle' })).toBeDisabled()
  })

  it('a search is at most 200 characters, the most the server takes', async () => {
    at('/sessions')
    const s = server({ list: () => page([summary('live')]) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    const search = within(region).getByRole('searchbox', { name: 'Search sessions' })
    expect(search).toHaveAttribute('maxlength', '200')
    await userEvent.click(search)
    await userEvent.paste('q'.repeat(201))
    expect(search).toHaveValue('q'.repeat(200))
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('q'.repeat(200)))
  })

  it('Hide closed is read back from storage', async () => {
    localStorage.setItem('hennery.hideClosed', 'false')
    at('/sessions')
    const s = server({ list: () => page([summary('live'), summary('closed', { lifecycle: 'closed', activity: undefined })]) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['live', 'closed']))
    expect(s.of(LIST)[0].searchParams.get('lifecycle')).toBeNull()
    expect(within(region).getByRole('button', { name: 'Hide closed' })).toHaveAttribute('aria-pressed', 'false')
  })

  it('loads the next page by its cursor', async () => {
    at('/sessions')
    const s = server({
      list: (params) => (params.get('cursor') === 'c1' ? page([summary('older', { last_event_at: '2026-10-01T10:00:00.000Z' })]) : page([summary('newer')], 'c1')),
    })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await userEvent.click(await within(region).findByRole('button', { name: 'Load more' }))
    await waitFor(() => expect(names(region)).toEqual(['newer', 'older']))
    expect(within(region).queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
  })
})

describe('rows', () => {
  it('show the host name, fetched once, and no refetch on events', async () => {
    at('/sessions')
    const s = server({ list: () => page([summary('a')]) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    expect(await within(region).findByText('laptop')).toBeInTheDocument()
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].event('session_upsert', summary('b', { last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:2'))
    await waitFor(() => expect(names(region)).toEqual(['b', 'a']))
    expect(s.of('/api/hosts')).toHaveLength(1)
    expect(s.of(LIST)).toHaveLength(1)
  })

  it('render a hostile title as text', async () => {
    at('/sessions')
    const s = server({ list: () => page([summary('a', { title: '<img src=x onerror=alert(1)>' })]) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    expect(await within(region).findByText('<img src=x onerror=alert(1)>')).toBeInTheDocument()
    expect(region.querySelector('img')).toBeNull()
  })
})

describe('the waiting count', () => {
  it('shows on a phone in the top bar', async () => {
    at('/sessions')
    const s = server({ list: () => page([summary('asks', { question_waits: true }), summary('calm')]) })
    const { container } = render(<App fetchImpl={s.fetch} />)
    await waitFor(() => expect(document.title).toBe('(1) hennery'))
    const bar = container.querySelector('.mtopbar') as HTMLElement
    expect(within(bar).getByRole('status', { name: '1 waiting on a question' })).toHaveTextContent('1')
  })


  it('is the tab title and the badge: the hat’s blocked or question-waiting sessions', async () => {
    localStorage.setItem('hennery.hat', 'hat-a')
    width(true)
    at('/sessions/x')
    const s = server({
      list: () =>
        page([
          summary('blocked', { activity: 'blocked' }),
          summary('asks', { question_waits: true }),
          summary('calm'),
          summary('elsewhere', { hat_id: 'hat-b', activity: 'blocked' }),
        ]),
    })
    render(<App fetchImpl={s.fetch} />)
    await waitFor(() => expect(document.title).toBe('(2) hennery'))
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    expect(within(rail).getByRole('status', { name: '2 waiting on a question' })).toHaveTextContent('2')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].event('session_upsert', summary('blocked', { activity: 'running' }), 'e1:2'))
    act(() => s.streams[0].event('session_upsert', summary('asks'), 'e1:3'))
    await waitFor(() => expect(document.title).toBe('hennery'))
    expect(within(rail).queryByRole('status', { name: /waiting on a question/ })).not.toBeInTheDocument()
  })

  // The fallback (pages with no `waiting`): the hat's rows, a search's
  // (none) and the closed filter's (one, calm).
  const hatsList = (params: URLSearchParams) =>
    params.get('q')
      ? page([])
      : params.get('lifecycle') === 'closed'
        ? page([summary('done', { lifecycle: 'closed', activity: undefined })])
        : page([summary('asks', { activity: 'blocked' }), summary('calm')])

  it('stays the hat’s, never the query’s, while a search or a lifecycle filter is set', async () => {
    at('/sessions')
    const s = server({ list: hatsList })
    const { container } = render(<App fetchImpl={s.fetch} />)
    const region = await list()
    const bar = container.querySelector('.mtopbar') as HTMLElement
    const counted = () => {
      expect(document.title).toBe('(1) hennery')
      expect(within(bar).getByRole('status', { name: '1 waiting on a question' })).toHaveTextContent('1')
    }
    await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
    counted()
    const search = within(region).getByRole('searchbox', { name: 'Search sessions' })
    await userEvent.type(search, 'zz')
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('zz'))
    await within(region).findByText('No matches')
    counted()
    await userEvent.clear(search)
    await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
    counted()
    await userEvent.selectOptions(within(region).getByRole('combobox', { name: 'Lifecycle' }), 'closed')
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('lifecycle')).toBe('closed'))
    await waitFor(() => expect(names(region)).toEqual(['done']))
    counted()
  })

  it('follows the stream while a search is set', async () => {
    at('/sessions')
    const s = server({ list: hatsList })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(document.title).toBe('(1) hennery'))
    await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'zz')
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('zz'))
    await within(region).findByText('No matches')
    await waitFor(() => expect(s.streams.filter((x) => !x.cancelled)).toHaveLength(1))
    const live = s.streams.filter((x) => !x.cancelled)[0]
    act(() => live.event('session_upsert', summary('asks', { activity: 'running', last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:5'))
    await waitFor(() => expect(document.title).toBe('hennery'))
    act(() => live.event('session_upsert', summary('calm', { question_waits: true, last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:6'))
    await waitFor(() => expect(document.title).toBe('(1) hennery'))
  })

  // The server's count: 3, whatever the rows say (one blocked), and the
  // same on every page of the hat, searched or filtered.
  const countedList = (params: URLSearchParams) =>
    params.get('q')
      ? page([], undefined, 3)
      : params.get('lifecycle') === 'closed'
        ? page([summary('done', { lifecycle: 'closed', activity: undefined })], undefined, 3)
        : page([summary('asks', { activity: 'blocked' }), summary('calm')], undefined, 3)

  it('is the server’s count, kept while a search or a lifecycle filter is set', async () => {
    at('/sessions')
    const s = server({ list: countedList })
    const { container } = render(<App fetchImpl={s.fetch} />)
    const region = await list()
    const bar = container.querySelector('.mtopbar') as HTMLElement
    const counted = () => {
      expect(document.title).toBe('(3) hennery')
      expect(within(bar).getByRole('status', { name: '3 waiting on a question' })).toHaveTextContent('3')
    }
    await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
    counted()
    const search = within(region).getByRole('searchbox', { name: 'Search sessions' })
    await userEvent.type(search, 'zz')
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('zz'))
    await within(region).findByText('No matches')
    counted()
    await userEvent.clear(search)
    await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
    counted()
    await userEvent.selectOptions(within(region).getByRole('combobox', { name: 'Lifecycle' }), 'closed')
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('lifecycle')).toBe('closed'))
    await waitFor(() => expect(names(region)).toEqual(['done']))
    counted()
  })

  it('is kept when the server refuses a search (a control character)', async () => {
    at('/sessions')
    const s = server({
      list: (params) =>
        params.get('q')?.includes('\t')
          ? json({ code: 'invalid', message: 'a search is at most 200 characters, with no control characters' }, 400)
          : countedList(params),
    })
    const { container } = render(<App fetchImpl={s.fetch} />)
    const region = await list()
    const bar = container.querySelector('.mtopbar') as HTMLElement
    await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
    expect(document.title).toBe('(3) hennery')
    await userEvent.click(within(region).getByRole('searchbox', { name: 'Search sessions' }))
    await userEvent.paste('a\tb')
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('a\tb'))
    // The refusal itself, not a page still loading (which also shows no rows).
    expect(await within(region).findByRole('alert')).toHaveTextContent(
      'a search is at most 200 characters, with no control characters',
    )
    expect(names(region)).toEqual([])
    expect(document.title).toBe('(3) hennery')
    expect(within(bar).getByRole('status', { name: '3 waiting on a question' })).toHaveTextContent('3')
  })

  it('follows waiting_changed, never an upsert, while a search is set', async () => {
    at('/sessions')
    const s = server({ list: countedList })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(document.title).toBe('(3) hennery'))
    await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'zz')
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('zz'))
    await within(region).findByText('No matches')
    await waitFor(() => expect(s.streams.filter((x) => !x.cancelled)).toHaveLength(1))
    const live = s.streams.filter((x) => !x.cancelled)[0]
    act(() => live.event('session_upsert', summary('asks', { activity: 'running', last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:5'))
    await new Promise((r) => setTimeout(r, 30))
    expect(document.title).toBe('(3) hennery')
    act(() => live.event('waiting_changed', { count: 2 }, 'e1:6'))
    await waitFor(() => expect(document.title).toBe('(2) hennery'))
    act(() => live.event('waiting_changed', { count: 0 }, 'e1:7'))
    await waitFor(() => expect(document.title).toBe('hennery'))
  })

  it('is the chosen hat’s: a new hat opens a new page and a new stream with it', async () => {
    localStorage.setItem('hennery.hat', 'hat-a')
    at('/sessions')
    const counts: Record<string, number> = { 'hat-a': 3, 'hat-b': 1 }
    const s = server({ list: (params) => page([], undefined, counts[params.get('hat') ?? ''] ?? 7) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(document.title).toBe('(3) hennery'))
    await waitFor(() => expect(s.streams).toHaveLength(1))
    await userEvent.click(await within(region).findByRole('button', { name: 'Home' }))
    await waitFor(() => expect(document.title).toBe('(1) hennery'))
    await userEvent.click(within(region).getByRole('button', { name: 'All hats' }))
    await waitFor(() => expect(document.title).toBe('(7) hennery'))
    await waitFor(() => expect(s.of(STREAM)).toHaveLength(3))
    // Never `hat=` empty: the server refuses it.
    expect(s.of(STREAM).map((u) => u.search)).toEqual(['?hat=hat-a', '?hat=hat-b', ''])
    expect(s.streams.slice(0, 2).every((x) => x.cancelled)).toBe(true)
  })
})

describe('selection (F-11)', () => {
  const rows = () =>
    page([
      summary('closed-newest', { lifecycle: 'closed', activity: undefined, last_event_at: '2026-10-02T12:00:00.000Z' }),
      summary('a', { last_event_at: '2026-10-02T11:00:00.000Z' }),
      summary('b', { hat_id: 'hat-b', last_event_at: '2026-10-02T10:00:00.000Z' }),
      summary('a-old', { last_event_at: '2026-10-02T09:00:00.000Z' }),
    ])

  it('on a desktop, /sessions opens the newest visible row, never a hidden one', async () => {
    width(true)
    at('/sessions')
    const s = server({ list: rows })
    render(<App fetchImpl={s.fetch} />)
    await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    await waitFor(() => expect(names(rail)).toContain('a'))
    const current = within(rail)
      .getAllByRole('link', { current: 'page' })
      .filter((l) => l.classList.contains('sess'))
    expect(current.map((l) => l.querySelector('.sess-name')?.textContent)).toEqual(['a'])
  })

  it('on a desktop, /sessions with no row to open says to pick one, not that the screen comes later', async () => {
    width(true)
    at('/sessions')
    const s = server()
    render(<App fetchImpl={s.fetch} />)
    expect(await screen.findByText('Pick a session from the list, or start a new one.')).toBeInTheDocument()
    expect(screen.queryByText('This screen arrives in a later part of the web UI.')).toBeNull()
    expect(location.pathname).toBe('/sessions')
  })

  it('on a phone, /sessions stays the list', async () => {
    at('/sessions')
    const s = server({ list: rows })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['a', 'b', 'a-old']))
    expect(location.pathname).toBe('/sessions')
  })

  it('honours a session outside the hat that the address names', async () => {
    localStorage.setItem('hennery.hat', 'hat-a')
    width(true)
    at('/sessions/b')
    const s = server({ list: rows })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['a', 'a-old']))
    expect(location.pathname).toBe('/sessions/b')
  })

  it('a click opens the session', async () => {
    at('/sessions')
    const s = server({ list: rows })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['a', 'b', 'a-old']))
    await userEvent.click(within(region).getAllByRole('link')[1])
    expect(location.pathname).toBe('/sessions/b')
  })

  /** The app on a desktop at `start`, with `hatBefore` chosen. */
  function openAt(start: string, hatBefore = '') {
    width(true)
    localStorage.setItem('hennery.hat', hatBefore)
    at(start)
    const s = server({ list: rows })
    render(<App fetchImpl={s.fetch} />)
    return s
  }

  it('switching hats clears a selection outside the new hat', async () => {
    openAt('/sessions/a')
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    await waitFor(() => expect(names(rail)).toContain('a'))
    await userEvent.click(await within(rail).findByRole('button', { name: 'Home' }))
    // Cleared, then the newest visible row of the new hat is opened.
    await waitFor(() => expect(location.pathname).toBe('/sessions/b'))
    expect(localStorage.getItem('hennery.hat')).toBe('hat-b')
  })

  it('switching hats clears a selection inside the new hat too', async () => {
    // Not the hat's newest: kept, it would stay on a-old.
    const s = openAt('/sessions/a-old')
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    await waitFor(() => expect(names(rail)).toContain('a-old'))
    const before = s.of(LIST).length
    await userEvent.click(await within(rail).findByRole('button', { name: 'Work' }))
    await waitFor(() => expect(s.of(LIST).length).toBeGreaterThan(before))
    await waitFor(() => expect(names(rail)).toEqual(['a', 'a-old']))
    await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
  })

  it('switching to every hat clears the selection', async () => {
    const s = openAt('/sessions/a-old', 'hat-a')
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    await waitFor(() => expect(names(rail)).toEqual(['a', 'a-old']))
    await userEvent.click(within(rail).getByRole('button', { name: 'All hats' }))
    await waitFor(() => expect(names(rail)).toEqual(['a', 'b', 'a-old']))
    await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
    expect(s.of(LIST).at(-1)!.searchParams.get('hat')).toBeNull()
  })

  it('switching hats clears a session the list does not hold', async () => {
    openAt('/sessions/unknown')
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    await waitFor(() => expect(names(rail)).toContain('a'))
    await userEvent.click(await within(rail).findByRole('button', { name: 'Work' }))
    await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
  })

  it('selects a session whose id needs encoding, through the address, and a hat switch clears it', async () => {
    width(true)
    localStorage.setItem('hennery.hat', '')
    at('/sessions/x%2Fy%20z')
    const odd = summary('x/y z', { title: 'Odd id', last_event_at: '2026-10-02T11:30:00.000Z' })
    const s = server({ list: () => page([odd, ...rows().sessions]) })
    render(<App fetchImpl={s.fetch} />)
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    await waitFor(() => expect(names(rail)).toContain('Odd id'))
    const current = within(rail)
      .getAllByRole('link', { current: 'page' })
      .filter((l) => l.classList.contains('sess'))
    expect(current.map((l) => l.querySelector('.sess-name')?.textContent)).toEqual(['Odd id'])
    expect(current[0]).toHaveAttribute('href', '/sessions/x%2Fy%20z')
    expect(location.pathname).toBe('/sessions/x%2Fy%20z')
    await userEvent.click(within(rail).getByRole('button', { name: 'Home' }))
    // Cleared, then the new hat's newest row is opened.
    await waitFor(() => expect(location.pathname).toBe('/sessions/b'))
    expect(s.of(LIST).at(-1)!.searchParams.get('hat')).toBe('hat-b')
  })

  it('choosing the hat already chosen keeps the selection', async () => {
    const s = openAt('/sessions/a-old', 'hat-a')
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    await waitFor(() => expect(names(rail)).toEqual(['a', 'a-old']))
    await userEvent.click(within(rail).getByRole('button', { name: 'Work' }))
    expect(location.pathname).toBe('/sessions/a-old')
    expect(s.of(LIST)).toHaveLength(1)
  })
})

describe('hats', () => {
  it('the chosen hat’s colour becomes the theme, a bad one the default', async () => {
    at('/sessions')
    const s = server({ list: () => page([summary('a')]) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await userEvent.click(await within(region).findByRole('button', { name: 'Work' }))
    await waitFor(() => expect(JSON.parse(localStorage.getItem(THEME_KEY)!)['--accent']).toBe('#112233'))
    expect(within(region).getByRole('button', { name: 'Work' })).toHaveAttribute('aria-pressed', 'true')
    await userEvent.click(within(region).getByRole('button', { name: 'Home' }))
    await waitFor(() => expect(localStorage.getItem(THEME_KEY)).toBeNull())
  })

  it('keeps the saved colours when the hats cannot be read', async () => {
    localStorage.setItem(THEME_KEY, JSON.stringify({ '--accent': '#445566', '--accent-2': '#334455' }))
    localStorage.setItem('hennery.hat', 'hat-a')
    at('/sessions')
    const s = server({ list: () => page([summary('a')]), hats: null })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['a']))
    await waitFor(() => expect(s.of('/api/hats')).toHaveLength(1))
    expect(localStorage.getItem(THEME_KEY)).not.toBeNull()
  })

  it('a remembered hat that is gone falls back to every hat', async () => {
    localStorage.setItem('hennery.hat', 'hat-gone')
    at('/sessions')
    const s = server({ list: () => page([summary('a')]) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(within(region).getByRole('button', { name: 'All hats' })).toHaveAttribute('aria-pressed', 'true'))
    expect(localStorage.getItem('hennery.hat')).toBe('')
  })
})

describe('hats and hosts, changed on their screens', () => {
  /** A server whose hats and hosts change when `change()` is called: a new
   *  hat, and the host renamed. */
  function changing() {
    const hats = [...HATS]
    // Whole hosts: the hosts screen renders them.
    const hosts = [host({ host_id: 'h1', name: 'laptop' })]
    const s = server({ list: () => page([summary('a')]), hats, hosts })
    const change = () => {
      hats.push(hat('hat-c', 'Clients', '#445566'))
      hosts[0] = host({ host_id: 'h1', name: 'build box' })
    }
    return { s, change }
  }

  /** The rail, once the session list's own hats and host names are read. */
  async function railRead() {
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    await within(rail).findByRole('button', { name: 'Work' })
    await within(rail).findByText('laptop')
    return rail
  }

  it.each([
    ['/hats', '/sessions'],
    ['/hosts', '/sessions'],
    ['/hats', '/hosts'],
    ['/hosts', '/hats'],
  ])('are read again on leaving %s (for %s)', async (from, to) => {
    width(true)
    at(from)
    const { s, change } = changing()
    render(<App fetchImpl={s.fetch} />)
    const rail = await railRead()
    change()
    act(() => navigate(to))
    expect(await within(rail).findByRole('button', { name: 'Clients' })).toBeInTheDocument()
    expect(await within(rail).findByText('build box')).toBeInTheDocument()
    expect(s.of(LIST)).toHaveLength(1)
  })

  it('are read again after a visit to /hosts from a session, and not before leaving it', async () => {
    width(true)
    at('/sessions/a')
    const { s, change } = changing()
    render(<App fetchImpl={s.fetch} />)
    const rail = await railRead()
    act(() => navigate('/hosts'))
    await screen.findByRole('heading', { name: 'laptop' })
    change()
    await new Promise((r) => setTimeout(r, 100))
    expect(within(rail).getByText('laptop')).toBeInTheDocument()
    act(() => navigate('/sessions/a'))
    expect(await within(rail).findByText('build box')).toBeInTheDocument()
    expect(within(rail).getByRole('button', { name: 'Clients' })).toBeInTheDocument()
  })

  it('are not read again between the session list and a session', async () => {
    width(true)
    at('/sessions/a')
    const { s, change } = changing()
    render(<App fetchImpl={s.fetch} />)
    const rail = await railRead()
    change()
    act(() => navigate('/sessions'))
    await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
    act(() => navigate('/sessions/b'))
    // The session view reads the hats for itself: the rail shows the
    // session list's own.
    await new Promise((r) => setTimeout(r, 100))
    expect(within(rail).queryByRole('button', { name: 'Clients' })).toBeNull()
    expect(within(rail).getByText('laptop')).toBeInTheDocument()
  })
})

describe('windowed rows', () => {
  it(`renders ${PAGE_ROWS} rows at a time, and fetches only past the rows held`, async () => {
    at('/sessions')
    const many = Array.from({ length: 120 }, (_, i) =>
      summary(`s${String(i).padStart(3, '0')}`, { last_event_at: new Date(Date.UTC(2026, 9, 2, 10, 0, 120 - i)).toISOString() }),
    )
    const s = server({ list: () => page(many) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toHaveLength(50))
    expect(names(region)[0]).toBe('s000')
    await userEvent.click(within(region).getByRole('button', { name: 'Load more' }))
    await waitFor(() => expect(names(region)).toHaveLength(100))
    await userEvent.click(within(region).getByRole('button', { name: 'Load more' }))
    await waitFor(() => expect(names(region)).toHaveLength(120))
    expect(within(region).queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
    expect(s.of(LIST)).toHaveLength(1)
  })

  it('opens a new query at one page of rows again', async () => {
    at('/sessions')
    const many = Array.from({ length: 120 }, (_, i) =>
      summary(`s${String(i).padStart(3, '0')}`, { last_event_at: new Date(Date.UTC(2026, 9, 2, 10, 0, 120 - i)).toISOString() }),
    )
    const s = server({ list: () => page(many) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toHaveLength(50))
    await userEvent.click(within(region).getByRole('button', { name: 'Load more' }))
    await waitFor(() => expect(names(region)).toHaveLength(100))
    await userEvent.click(within(region).getByRole('button', { name: 'Work' }))
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('hat')).toBe('hat-a'))
    await waitFor(() => expect(names(region)).toHaveLength(PAGE_ROWS))
    expect(names(region)[0]).toBe('s000')
  })

  it('fetches the next page only once every row held is shown', async () => {
    at('/sessions')
    const many = Array.from({ length: 120 }, (_, i) =>
      summary(`s${String(i).padStart(3, '0')}`, { last_event_at: new Date(Date.UTC(2026, 9, 2, 10, 0, 120 - i)).toISOString() }),
    )
    const older = summary('older', { last_event_at: '2026-10-01T10:00:00.000Z' })
    const s = server({ list: (params) => (params.get('cursor') === 'c1' ? page([older]) : page(many, 'c1')) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toHaveLength(50))
    const more = () => userEvent.click(within(region).getByRole('button', { name: 'Load more' }))
    await more()
    await waitFor(() => expect(names(region)).toHaveLength(100))
    await more()
    await waitFor(() => expect(names(region)).toHaveLength(120))
    expect(s.of(LIST)).toHaveLength(1)
    await more()
    await waitFor(() => expect(names(region)).toHaveLength(121))
    expect(s.of(LIST)).toHaveLength(2)
    expect(s.of(LIST)[1].searchParams.get('cursor')).toBe('c1')
    expect(within(region).queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
  })
})

describe('the list stream, as the list shows it', () => {
  async function open(rows: SessionSummary[], hatChosen = '') {
    localStorage.setItem('hennery.hat', hatChosen)
    at('/sessions')
    const s = server({ list: (params) => (params.get('q') ? page([summary('found', { title: 'fix it' })]) : page(rows)) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(s.streams).toHaveLength(1))
    await waitFor(() => expect(names(region)).toHaveLength(rows.length))
    return { s, region }
  }

  it('an upsert of a new session inserts its row in order', async () => {
    const { s, region } = await open([summary('a')])
    act(() => s.streams[0].event('session_upsert', summary('b', { last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:2'))
    await waitFor(() => expect(names(region)).toEqual(['b', 'a']))
  })

  it('an upsert updates a row in place', async () => {
    const { s, region } = await open([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })])
    act(() => s.streams[0].event('session_upsert', summary('b', { title: 'Renamed', last_event_at: '2026-10-02T09:00:00.000Z' }), 'e1:2'))
    await waitFor(() => expect(names(region)).toEqual(['a', 'Renamed']))
  })

  it('an upsert that closes a session takes its row out while Hide closed is on', async () => {
    const { s, region } = await open([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })])
    act(() => s.streams[0].event('session_upsert', summary('b', { lifecycle: 'closed', activity: undefined }), 'e1:2'))
    await waitFor(() => expect(names(region)).toEqual(['a']))
  })

  it('an upsert that moves a session to another hat takes its row out', async () => {
    const { s, region } = await open([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })], 'hat-a')
    act(() => s.streams[0].event('session_upsert', summary('b', { hat_id: 'hat-b' }), 'e1:2'))
    await waitFor(() => expect(names(region)).toEqual(['a']))
  })

  it('during a search an upsert updates a found row and never adds one', async () => {
    const { s, region } = await open([summary('a')])
    await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'fix')
    await waitFor(() => expect(names(region)).toEqual(['fix it']), { timeout: 3000 })
    await waitFor(() => expect(s.streams).toHaveLength(2))
    const live = s.streams[1]
    act(() => live.event('session_upsert', summary('new', { title: 'fix that', last_event_at: '2026-10-02T12:00:00.000Z' }), 'e1:2'))
    act(() => live.event('session_upsert', summary('found', { title: 'fix it now' }), 'e1:3'))
    await waitFor(() => expect(names(region)).toEqual(['fix it now']))
  })

  it('session_removed takes the row out', async () => {
    const { s, region } = await open([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })])
    act(() => s.streams[0].event('session_removed', { session_id: 'a' }, 'e1:2'))
    await waitFor(() => expect(names(region)).toEqual(['b']))
  })

  it('a resync fetches the first page again with the same query and replaces the rows', async () => {
    const pages = [page([summary('a')]), page([summary('z')])]
    let n = 0
    at('/sessions')
    const s = server({ list: () => pages[Math.min(n++, 1)] })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['a']))
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(names(region)).toEqual(['z']))
    const [first, again] = s.of(LIST)
    expect(again.search).toBe(first.search)
    await waitFor(() => expect(s.streams).toHaveLength(2))
  })

  it('says “Resynced” for a moment once a resync replaced the rows, and nothing before', async () => {
    const pages = [page([summary('a')]), page([summary('z')])]
    let n = 0
    at('/sessions')
    const s = server({ list: () => pages[Math.min(n++, 1)] })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['a']))
    await waitFor(() => expect(s.streams).toHaveLength(1))
    expect(within(region).queryByText('Resynced')).toBeNull()
    act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(names(region)).toEqual(['z']))
    // One status line: "Reconnecting…" before, "Resynced" after.
    expect(await within(region).findByRole('status')).toHaveTextContent('Resynced')
  })

  it('says it is reconnecting while the stream is down', async () => {
    const { s, region } = await open([summary('a')])
    act(() => s.streams[0].end())
    expect(await within(region).findByRole('status')).toHaveTextContent('Reconnecting…')
    expect(names(region)).toEqual(['a'])
  })
})
