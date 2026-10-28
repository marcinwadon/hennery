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
import * as markers from '../lib/status'

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
    operator_attention: patch.question_waits || patch.activity === 'blocked' ? 1 : 0,
    parent_handled_questions: 0,
    ...patch,
  }
}

/** A page; with no `waiting`, a server that does not count (the rows are
 *  counted instead: the fallback). */
const page = (sessions: SessionSummary[], next?: string, waiting?: number): SummaryPage =>
  ({ sessions, next_cursor: next, epoch: 'e1', revision: 1, ...(waiting === undefined ? {} : { waiting }) }) as SummaryPage

function hat(id: string, name: string, colour: string): HatItem {
  return { id, name, colour, created_at: '2026-10-01T00:00:00Z', default_for_new_hosts: false, purging: false, mcp_source_policy: 'legacy' as const, mcp_policy_revision: 0 }
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
      default: {
        // A selected session's view: an empty transcript that stays open
        // (a 404 there would mean the session is gone, and drop its row).
        if (/^\/api\/view\/sessions\/[^/]+$/.test(url.pathname)) return json({ items: [], older: false, epoch: 'e1', revision: 1 })
        if (url.pathname.startsWith('/api/stream/view/sessions/')) return liveStream().response
        return json({ code: 'not_found', message: 'no' }, 404)
      }
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
  // These legacy list assertions exercise the stable, unpartitioned view.
  localStorage.setItem('hennery.sort', 'started')
  localStorage.setItem('hennery.subgroup', 'none')
  sessionStorage.removeItem('hennery.expandedFamilies')
  sessionStorage.removeItem('hennery.collapsedProjects')
  document.title = 'hennery'
  width(false)
})

afterEach(() => {
  at('/')
  // @ts-expect-error jsdom has none; each test sets its own
  delete window.matchMedia
  vi.useRealTimers()
})

describe('group headings and clock', () => {
  it('shows cross-hat pinned shortcuts, updates names, marks selection and removes deleted shortcuts', async () => {
    width(true)
    at('/sessions')
    const pin = summary('saved/id', { sidebar_pinned: true, lifecycle: 'closed', hat_id: 'hat-b', title: 'Saved session' })
    const s = server({ list: () => ({ ...page([summary('visible')]), sidebar_pins: [pin] }) })
    render(<App fetchImpl={s.fetch} />)
    const pins = await screen.findByRole('group', { name: 'Pinned sessions' })
    expect(within(pins).getByRole('link', { name: 'Saved session' })).toHaveAttribute('href', '/sessions/saved%2Fid')
    expect(names(await list())).toEqual(['visible'])
    act(() => s.streams[0].event('session_upsert', { ...pin, title: 'New name' }))
    await within(pins).findByRole('link', { name: 'New name' })
    act(() => navigate('/sessions/saved%2Fid'))
    await waitFor(() => expect(within(pins).getByRole('link', { name: 'New name' })).toHaveAttribute('aria-current', 'page'))
    act(() => s.streams[0].event('session_removed', { session_id: 'saved/id' }))
    await waitFor(() => expect(screen.queryByRole('group', { name: 'Pinned sessions' })).not.toBeInTheDocument())
  })
  it('groups dates under the actual hat instead of day headings', async () => {
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
    expect(headings).toEqual(['Work'])
  })

  it('retains the hat group across midnight while relative time advances without refetch', async () => {
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
    expect(headings()).toEqual(['Work'])
    expect(time()).toBe('29m ago')
    act(() => vi.advanceTimersByTime(2 * 60_000))
    expect(headings()).toEqual(['Work'])
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
    // A loaded machine may pause past the debounce mid-word: a search for
    // `f` can go first, with the same rows. Wait for the whole word's.
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('fix'))
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
      list: (params) => (params.get('cursor') === 'c1' ? page([summary('older', { created_at: '2026-09-30T10:00:00.000Z', last_event_at: '2026-10-01T10:00:00.000Z' })]) : page([summary('newer')], 'c1')),
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
    expect(await within(region).findByRole('cell', { name: 'laptop' })).toBeInTheDocument()
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
  it('shows on a phone in primary navigation', async () => {
    at('/sessions')
    const s = server({ list: () => page([summary('asks', { question_waits: true }), summary('calm')]) })
    const { container } = render(<App fetchImpl={s.fetch} />)
    await waitFor(() => expect(document.title).toBe('(1) hennery'))
    const bar = container.querySelector('.tabbar') as HTMLElement
    expect(within(bar).getByRole('status', { name: '1 waiting on you' })).toHaveTextContent('1')
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
    expect(within(rail).getByRole('status', { name: '2 waiting on you' })).toHaveTextContent('2')
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].event('session_upsert', summary('blocked', { activity: 'running' }), 'e1:2'))
    act(() => s.streams[0].event('session_upsert', summary('asks'), 'e1:3'))
    await waitFor(() => expect(document.title).toBe('hennery'))
    expect(within(rail).queryByRole('status', { name: /waiting on you/ })).not.toBeInTheDocument()
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
    const bar = container.querySelector('.tabbar') as HTMLElement
    const counted = () => {
      expect(document.title).toBe('(1) hennery')
      expect(within(bar).getByRole('status', { name: '1 waiting on you' })).toHaveTextContent('1')
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
    params.get('limit') === '100' ? page([summary('asks', { activity: 'blocked' }), summary('closed-one', { lifecycle: 'closed', operator_attention: 1 }), summary('closed-two', { lifecycle: 'closed', operator_attention: 1 })], undefined, 3) : params.get('q')
      ? page([], undefined, 3)
      : params.get('lifecycle') === 'closed'
        ? page([summary('done', { lifecycle: 'closed', activity: undefined })], undefined, 3)
        : page([summary('asks', { activity: 'blocked' }), summary('calm')], undefined, 3)

  it('includes discovered closed attention and keeps its cards available across overview filters', async () => {
    at('/sessions')
    const s = server({ list: countedList })
    const { container } = render(<App fetchImpl={s.fetch} />)
    const region = await list()
    const bar = container.querySelector('.tabbar') as HTMLElement
    const counted = () => {
      expect(document.title).toBe('(3) hennery')
      expect(within(bar).getByRole('status', { name: '3 waiting on you' })).toHaveTextContent('3')
    }
    await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
    await waitFor(counted)
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
    const bar = container.querySelector('.tabbar') as HTMLElement
    await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
    await waitFor(() => expect(document.title).toBe('(3) hennery'))
    await userEvent.click(within(region).getByRole('searchbox', { name: 'Search sessions' }))
    await userEvent.paste('a\tb')
    await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('a\tb'))
    // The refusal itself, not a page still loading (which also shows no rows).
    expect(await within(region).findByRole('alert')).toHaveTextContent(
      'a search is at most 200 characters, with no control characters',
    )
    expect(names(region)).toEqual([])
    expect(document.title).toBe('(3) hennery')
    expect(within(bar).getByRole('status', { name: '3 waiting on you' })).toHaveTextContent('3')
  })

  it('counts the cards available after an upsert and honours an authoritative zero while searching', async () => {
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
    expect(document.title).toBe('(2) hennery')
    act(() => live.event('waiting_changed', { count: 2 }, 'e1:6'))
    await waitFor(() => expect(document.title).toBe('(2) hennery'))
    act(() => live.event('waiting_changed', { count: 0 }, 'e1:7'))
    await waitFor(() => expect(document.title).toBe('hennery'))
  })

  it('is the chosen hat’s: a new hat opens a new page and a new stream with it', async () => {
    localStorage.setItem('hennery.hat', 'hat-a')
    at('/sessions')
    const counts: Record<string, number> = { 'hat-a': 3, 'hat-b': 1 }
    const s = server({ list: (params) => {
      const hat = params.get('hat'), count = counts[hat ?? ''] ?? 7
      return page(params.get('limit') === '100' ? Array.from({ length: count }, (_, i) => summary(`attention-${hat ?? 'all'}-${i}`, { hat_id: hat ?? (i < 3 ? 'hat-a' : 'hat-b'), operator_attention: 1 })) : [], undefined, count)
    } })
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

describe('route selection and overview', () => {
  const rows = () => page([summary('a'), summary('b', { hat_id: 'hat-b' }), summary('a-old', { created_at: '2026-09-30T00:00:00Z', last_event_at: '2026-10-01T00:00:00Z' }), summary('closed', { lifecycle: 'closed' })])
  it.each([false, true])('keeps /sessions as an overview at desktop=%s', async desktop => {
    width(desktop); at('/sessions'); const s = server({ list: rows }); render(<App fetchImpl={s.fetch} />)
    const region = await list(); await waitFor(() => expect(names(region)).toEqual(['b', 'a', 'a-old']))
    expect(location.pathname).toBe('/sessions')
    expect(s.calls.filter(u => u.pathname.startsWith('/api/stream/view/'))).toHaveLength(0)
  })
  it('shows an empty overview with a new-session link', async () => {
    at('/sessions'); render(<App fetchImpl={server().fetch} />)
    const region = await list(); expect(await within(region).findByText('No sessions', { exact: true })).toBeInTheDocument()
    expect(within(region).getByRole('link', { name: 'New session' })).toHaveAttribute('href', '/new')
  })
  it('opens the selected row through its encoded native link', async () => {
    at('/sessions'); render(<App fetchImpl={server({ list: () => page([summary('x/y z', { title: 'Odd id' })]) }).fetch} />)
    const region = await list(); const link = await within(region).findByRole('link', { name: /Odd id/ })
    expect(link).toHaveAttribute('href', '/sessions/x%2Fy%20z'); await userEvent.click(link)
    expect(location.pathname).toBe('/sessions/x%2Fy%20z')
  })
  it.each(['/sessions/b', '/sessions/unknown', '/sessions/x%2Fy%20z'])('honours %s outside the selected hat without a side list', async path => {
    localStorage.setItem('hennery.hat', 'hat-a'); at(path); const s = server({ list: rows }); render(<App fetchImpl={s.fetch} />)
    const rail = await screen.findByRole('complementary', { name: 'Views' }); await within(rail).findByRole('link', { name: 'Work' })
    await waitFor(() => expect(s.of(LIST).filter(url => !url.searchParams.has('q'))).toHaveLength(1)); expect(location.pathname).toBe(path)
    // Direct links outside the baseline may resolve their visit by exact id;
    // they never mount another list/stream or change the selected browse hat.
    expect(s.of(LIST).filter(url => url.searchParams.has('q')).every(url => url.searchParams.get('q') === decodeURIComponent(path.slice('/sessions/'.length)))).toBe(true)
    expect(screen.queryByRole('region', { name: 'Session list' })).toBeNull()
  })
  it.each([['', 'Home', 'hat-b'], ['', 'Work', 'hat-a'], ['hat-a', 'All hats', '']])('hat switch %s → %s clears selection and returns to overview', async (before, name, next) => {
    localStorage.setItem('hennery.hat', before); at('/sessions/a-old'); render(<App fetchImpl={server({ list: rows }).fetch} />)
    const rail = await screen.findByRole('complementary', { name: 'Views' }); await userEvent.click(await within(rail).findByRole('link', { name }))
    await waitFor(() => expect(location.pathname).toBe('/sessions')); expect(localStorage.getItem('hennery.hat')).toBe(next)
    await list()
  })
  it('the current rail hat opens its scoped overview without auto-selecting a session', async () => {
    localStorage.setItem('hennery.hat', 'hat-a'); at('/sessions/a-old?pending=older'); const s = server({ list: rows }); render(<App fetchImpl={s.fetch} />)
    const rail = await screen.findByRole('complementary', { name: 'Views' }); await userEvent.click(await within(rail).findByRole('link', { name: 'Work' }))
    expect(location.pathname + location.search).toBe('/sessions?hat=hat-a'); expect(localStorage.getItem('hennery.hat')).toBe('hat-a')
  })
})

describe('hats', () => {
  it('the chosen hat’s colour becomes the theme, a bad one the default', async () => {
    at('/sessions')
    const s = server({ list: () => page([summary('a')]) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await userEvent.click(await within(region).findByRole('button', { name: 'Work' }))
    await waitFor(() => expect(JSON.parse(localStorage.getItem(THEME_KEY)!).source).toBe('#112233'))
    expect(within(region).getByRole('button', { name: 'Work' })).toHaveAttribute('aria-pressed', 'true')
    await userEvent.click(within(region).getByRole('button', { name: 'Home' }))
    await waitFor(() => expect(JSON.parse(localStorage.getItem(THEME_KEY)!).source).toBe('#64748b'))
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
    await within(rail).findByRole('link', { name: 'Work' })
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
    expect(await within(rail).findByRole('link', { name: 'Clients' })).toBeInTheDocument()
    act(() => navigate('/sessions'))
    expect(await within(await list()).findByRole('cell', { name: 'build box' })).toBeInTheDocument()
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
    expect(s.of('/api/hosts')).toHaveLength(3)
    act(() => navigate('/sessions/a'))
    act(() => navigate('/sessions'))
    expect(await within(await list()).findByRole('cell', { name: 'build box' })).toBeInTheDocument()
    expect(within(rail).getByRole('link', { name: 'Clients' })).toBeInTheDocument()
  })

  it('are not read again between the session list and a session', async () => {
    width(true)
    at('/sessions/a')
    const { s, change } = changing()
    render(<App fetchImpl={s.fetch} />)
    const rail = await railRead()
    change()
    act(() => navigate('/sessions'))
    await waitFor(() => expect(location.pathname).toBe('/sessions'))
    act(() => navigate('/sessions/b'))
    // The session view reads the hats for itself: the rail shows the
    // session list's own.
    await new Promise((r) => setTimeout(r, 100))
    expect(within(rail).queryByRole('link', { name: 'Clients' })).toBeNull()
    act(() => navigate('/sessions'))
    expect(await within(await list()).findByRole('cell', { name: 'laptop' })).toBeInTheDocument()
  })
})

describe('windowed rows', () => {
  it(`renders ${PAGE_ROWS} rows at a time, and fetches only past the rows held`, async () => {
    at('/sessions')
    const many = Array.from({ length: 120 }, (_, i) =>
      summary(`s${String(i).padStart(3, '0')}`, { created_at: new Date(Date.UTC(2026, 9, 1, 10, 0, 120 - i)).toISOString(), last_event_at: new Date(Date.UTC(2026, 9, 2, 10, 0, 120 - i)).toISOString() }),
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
      summary(`s${String(i).padStart(3, '0')}`, { created_at: new Date(Date.UTC(2026, 9, 1, 10, 0, 120 - i)).toISOString(), last_event_at: new Date(Date.UTC(2026, 9, 2, 10, 0, 120 - i)).toISOString() }),
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
      summary(`s${String(i).padStart(3, '0')}`, { created_at: new Date(Date.UTC(2026, 9, 1, 10, 0, 120 - i)).toISOString(), last_event_at: new Date(Date.UTC(2026, 9, 2, 10, 0, 120 - i)).toISOString() }),
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
  async function open(rows: SessionSummary[], hatChosen = '', visibleCount = rows.filter(row => row.lifecycle !== 'closed').length) {
    localStorage.setItem('hennery.hat', hatChosen)
    at('/sessions')
    const s = server({ list: (params) => (params.get('q') ? page([summary('found', { title: 'fix it' })]) : page(rows)) })
    render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(s.streams).toHaveLength(1))
    await waitFor(() => expect(names(region)).toHaveLength(visibleCount))
    return { s, region }
  }

  it('an upsert of a new session inserts its row in order', async () => {
    const { s, region } = await open([summary('a')])
    act(() => s.streams[0].event('session_upsert', summary('b', { last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:2'))
    await waitFor(() => expect(names(region)).toEqual(['b', 'a']))
  })

  it('an upsert updates a row in place', async () => {
    const b = summary('b', { created_at: '2026-09-30T00:00:00.000Z', last_event_at: '2026-10-02T09:00:00.000Z' })
    const { s, region } = await open([summary('a'), b])
    act(() => s.streams[0].event('session_upsert', { ...b, title: 'Renamed' }, 'e1:2'))
    await waitFor(() => expect(names(region)).toEqual(['a', 'Renamed']))
  })

  it('keeps order, DOM identity and untouched row renders stable through streaming activity', async () => {
    const parent = summary('parent', { title: 'Coordinator', created_at: '2026-10-01T09:00:00Z', family: { revision: 1, children: [{ session_id: 'child', agent: 'claude' }, { session_id: 'child', agent: 'claude' }, { session_id: 'closed', agent: 'claude' }], child_counts: { total: 2, active: 1, waiting: 0 }, attention: 0 } })
    const child = summary('child', { title: 'Worker', created_at: '2026-10-01T11:00:00Z', family: { revision: 1, parent: { session_id: 'parent', agent: 'claude' }, children: [], child_counts: { total: 0, active: 0, waiting: 0 }, attention: 0 } })
    const quiet = summary('quiet', { title: 'Untouched', created_at: '2026-10-01T10:00:00Z' })
    const closed = { ...child, session_id: 'closed', title: 'Closed worker', lifecycle: 'closed' }
    const { s, region } = await open([parent, child, quiet, closed], '', 2)
    await userEvent.click(within(region).getByRole('button', { name: /↳ 1 child/ }))
    const links = () => [...region.querySelectorAll('[data-session-link]')]
    await waitFor(() => expect(names(region)).toEqual(['Untouched', 'Coordinator', 'Worker']))
    const before = links()
    const renders = vi.spyOn(markers, 'statusOf')
    try {
      for (let n = 0; n < 40; n++) {
        const activity = n % 2 ? 'running' : 'idle'
        act(() => s.streams[0].event('session_upsert', { ...child, activity, last_event_at: new Date(Date.UTC(2026, 9, 2, 11, 0, n)).toISOString() }, `e1:${n + 2}`))
        await waitFor(() => expect(within(before[2].closest('tr')!).getByRole('img', { name: activity === 'running' ? 'Running' : 'Idle' })).toBeInTheDocument())
        expect(names(region)).toEqual(['Untouched', 'Coordinator', 'Worker'])
        links().forEach((link, index) => expect(link).toBe(before[index]))
      }
      expect(renders.mock.calls.filter(([s]) => 'session_id' in s && s.session_id === 'quiet')).toHaveLength(0)
      expect(within(region).getByText('↳ 1 child')).toBeInTheDocument()
      expect(within(region).queryByText(/^(Parent:|Child:)/)).not.toBeInTheDocument()
      await userEvent.click(within(region).getByRole('button', { name: 'Filters' }))
      await userEvent.click(screen.getByRole('button', { name: 'Hide closed' }))
      await waitFor(() => expect(within(region).getByText('↳ 2 children')).toBeInTheDocument())
      expect(region.querySelectorAll('[data-session-id="closed"]')).toHaveLength(1)
    } finally { renders.mockRestore() }
  })

  it('collapses 100 children, previews at most five with running first, and remembers expansion', async () => {
    localStorage.setItem('hennery.sort', 'attention')
    const parent = summary('parent', { title: 'Coordinator', family: { revision: 1, children: [], child_counts: { total: 100, active: 1, waiting: 0 }, attention: 0 } })
    const children = Array.from({ length: 100 }, (_, i) => summary(`child-${i}`, { title: `Worker ${i}`, activity: i === 0 ? 'running' : 'idle', created_at: new Date(Date.UTC(2026, 9, 2, 0, 0, i)).toISOString(), family: { revision: 1, parent: { session_id: 'parent', agent: 'claude' }, children: [], child_counts: { total: 0, active: 0, waiting: 0 }, attention: 0 } }))
    const { region } = await open([parent, ...children], '', 1)
    const chip = within(region).getByRole('button', { name: '↳ 100 children' })
    expect(chip).toHaveAttribute('aria-description', '1 running · 99 idle')
    expect(chip).toHaveAttribute('aria-expanded', 'false')
    await userEvent.click(chip)
    expect(names(region)).toEqual(['Coordinator', 'Worker 0', 'Worker 99', 'Worker 98', 'Worker 97', 'Worker 96'])
    expect(region.querySelectorAll('.sess-child')).toHaveLength(5)
    expect(within(region).getByRole('link', { name: 'Show all 100 children' })).toHaveAttribute('href', '/sessions/parent?family=1')
    act(() => navigate('/more')); act(() => navigate('/sessions'))
    const returned = await list()
    await waitFor(() => expect(within(returned).getByRole('button', { name: '↳ 100 children' })).toHaveAttribute('aria-expanded', 'true'))
    expect(returned.querySelectorAll('.sess-child')).toHaveLength(5)
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

it('counts status-only operator attention while excluding parent-handled questions, with truthful badge words', async () => {
  at('/sessions')
  const s = server({ list: () => page([
    summary('failed', { lifecycle: 'failed', question_waits: false, operator_attention: 1 }),
    summary('handled', { question_waits: true, operator_attention: 0, parent_handled_questions: 1 }),
  ]) })
  const { container } = render(<App fetchImpl={s.fetch} />)
  await waitFor(() => expect(document.title).toBe('(1) hennery'))
  const bar = container.querySelector('.tabbar') as HTMLElement
  expect(within(bar).getByRole('status', { name: '1 waiting on you' })).toHaveTextContent('1')
})

it('keeps the shared attention count reachable on a phone session while its tab bar is hidden', async () => {
  at('/sessions/failed')
  const s = server({ list: () => page([summary('failed', { lifecycle: 'failed', operator_attention: 1 })], undefined, 1) })
  const { container } = render(<App fetchImpl={s.fetch} />)
  await waitFor(() => expect(document.title).toBe('(1) hennery'))
  const bar = container.querySelector('.session-scope-status') as HTMLElement
  expect(within(bar).getByRole('status', { name: '1 waiting on you' })).toHaveTextContent('1')
  expect(within(bar).getByRole('link')).toHaveAttribute('href', '/needs-you')
  await waitFor(() => expect(s.streams).toHaveLength(1))
  act(() => s.streams[0].event('waiting_changed', { count: 0 }))
  await waitFor(() => expect(container.querySelector('.session-scope-status')).toBeNull())
})

describe('two-level overview preferences', () => {
  it('defaults invalid stored choices to Hat / Project / Needs you first and persists all choices', async () => {
    for (const key of ['hennery.group', 'hennery.subgroup', 'hennery.sort']) localStorage.setItem(key, 'invalid')
    width(true)
    const s = server({ list: () => page([summary('one')]) })
    at('/sessions')
    const app = render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['one']))
    for (const name of ['Hat', 'Project', 'Needs you first']) expect(within(region).getByRole('radio', { name })).toBeChecked()
    for (const name of ['Host', 'Nothing', 'Started']) await userEvent.click(within(region).getByRole('radio', { name }))
    expect(localStorage.getItem('hennery.group')).toBe('host')
    expect(localStorage.getItem('hennery.subgroup')).toBe('none')
    expect(localStorage.getItem('hennery.sort')).toBe('started')
    app.unmount()
    render(<App fetchImpl={s.fetch} />)
    for (const name of ['Host', 'Nothing', 'Started']) expect(await screen.findByRole('radio', { name })).toBeChecked()
  })
  it('collapses an entire project family and reports waiting sessions, full path and hosts', async () => {
    localStorage.setItem('hennery.subgroup', 'project')
    const parent = Object.assign(summary('parent', { title: 'parent', cwd: '/srv/repo/src' }), { project_root: '/srv/repo' })
    const child = Object.assign(summary('child', { title: 'child', host_id: 'h2', cwd: '/scratch/worktree', operator_attention: 3, family: { revision: 1, parent: { session_id: 'parent', agent: 'claude' }, children: [], child_counts: { total: 0, active: 0, waiting: 0 }, attention: 0 } }), { project_root: '/different' })
    const s = server({ list: () => page([parent, child]), hosts: [...HOSTS, { host_id: 'h2', name: 'worker' } as HostItem] })
    at('/sessions'); render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(region.querySelectorAll('.project-toggle')).toHaveLength(1))
    expect(region.querySelector('.grp-path')).toHaveAttribute('title', '/srv/repo')
    expect(region.querySelector('.grp-host')).toHaveTextContent('laptop, worker')
    expect(within(region).getByLabelText('1 session needs you', { exact: true })).toBeInTheDocument()
    await userEvent.click(within(region).getByRole('button', { name: '↳ 1 child' }))
    expect(names(region)).toEqual(['parent', 'child'])
    const toggle = region.querySelector<HTMLButtonElement>('.project-toggle')!
    await userEvent.click(toggle)
    expect(toggle).toHaveAttribute('aria-expanded', 'false')
    expect(names(region)).toEqual([])
    await userEvent.click(toggle)
    expect(names(region)).toEqual(['parent', 'child'])
  })
  it('offers all three sorts and changes family order without splitting children', async () => {
    width(true)
    const parent = summary('parent', { created_at: '2026-10-01T00:00:00Z' })
    const worker = summary('worker', { operator_attention: 1, last_event_at: '2026-10-03T00:00:00Z', family: { revision: 1, parent: { session_id: 'parent', agent: 'claude' }, children: [], child_counts: { total: 0, active: 0, waiting: 0 }, attention: 0 } })
    const recent = summary('recent', { created_at: '2026-10-02T00:00:00Z', last_event_at: '2026-10-04T00:00:00Z' })
    const s = server({ list: () => page([parent, recent, worker]) })
    at('/sessions'); render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['recent', 'parent']))
    await userEvent.click(within(region).getByRole('radio', { name: 'Needs you first' }))
    expect(names(region)).toEqual(['parent', 'recent'])
    await userEvent.click(within(region).getByRole('radio', { name: 'Last activity' }))
    expect(names(region)).toEqual(['recent', 'parent'])
    expect(s.of(LIST)).toHaveLength(1)
    expect(s.of(STREAM)).toHaveLength(1)
  })
  it('keeps preference changes working when browser storage writes are denied', async () => {
    width(true)
    const s = server({ list: () => page([summary('one')]) })
    at('/sessions'); render(<App fetchImpl={s.fetch} />)
    const region = await list()
    await waitFor(() => expect(names(region)).toEqual(['one']))
    const writes = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('denied') })
    try {
      for (const name of ['Host', 'Project', 'Last activity']) {
        await userEvent.click(within(region).getByRole('radio', { name }))
        expect(within(region).getByRole('radio', { name })).toBeChecked()
      }
    } finally { writes.mockRestore() }
  })
})

it('keeps a held family in its parent project when status or host filters match only children', async () => {
  width(true)
  localStorage.setItem('hennery.subgroup', 'project')
  const parent = Object.assign(summary('parent', { title: 'Coordinator', cwd: '/srv/repo/src' }), { project_root: '/srv/repo' })
  const worker = (id: string, title: string, host_id: string) => Object.assign(summary(id, { title, host_id, cwd: `/scratch/${id}`, operator_attention: 1, family: { revision: 1, parent: { session_id: 'parent', agent: 'claude' }, children: [], child_counts: { total: 0, active: 0, waiting: 0 }, attention: 0 } }), { project_root: `/other/${id}` })
  const s = server({ list: () => page([parent, worker('child-a', 'Worker A', 'h2'), worker('child-b', 'Worker B', 'h3')]), hosts: [...HOSTS, { host_id: 'h2', name: 'worker-a' } as HostItem, { host_id: 'h3', name: 'worker-b' } as HostItem] })
  at('/sessions'); render(<App fetchImpl={s.fetch} />)
  const region = await list()
  await waitFor(() => expect(names(region)).toEqual(['Coordinator']))
  const status = within(region).getByRole('group', { name: 'Status' })
  await userEvent.click(within(status).getByRole('button', { name: 'Needs you 2' }))
  expect(region.querySelector('.overview-counter')).toHaveTextContent('2 sessions')
  expect(region.querySelectorAll('.project-toggle')).toHaveLength(1)
  expect(region.querySelector('.grp-path')).toHaveAttribute('title', '/srv/repo')
  await userEvent.click(within(region).getByRole('button', { name: '↳ 2 children' }))
  expect(names(region)).toEqual(['Coordinator', 'Worker B', 'Worker A'])
  await userEvent.selectOptions(within(region).getByRole('combobox', { name: 'Host' }), 'h2')
  expect(region.querySelector('.overview-counter')).toHaveTextContent('1 sessions')
  expect(region.querySelectorAll('.project-toggle')).toHaveLength(1)
  expect(region.querySelector('.grp-path')).toHaveAttribute('title', '/srv/repo')
  expect(names(region)).toEqual(['Coordinator', 'Worker A'])
})

it('does not spend the row budget on collapsed projects', async () => {
  width(true)
  localStorage.setItem('hennery.subgroup', 'project')
  const rows = [...Array.from({ length: PAGE_ROWS + 1 }, (_, n) => summary(`z-${n}`, { cwd: '/srv/first' })), summary('a', { title: 'a', cwd: '/srv/second' })]
  const s = server({ list: () => page(rows) })
  at('/sessions'); render(<App fetchImpl={s.fetch} />)
  const region = await list()
  await waitFor(() => expect(names(region)).toHaveLength(PAGE_ROWS))
  await userEvent.click(region.querySelector<HTMLButtonElement>('.project-toggle')!)
  expect(names(region)).toEqual(['a'])
  expect(within(region).queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
})

it('remembers collapsed projects across overview remounts', async () => {
  localStorage.setItem('hennery.subgroup', 'project')
  const s = server({ list: () => page([summary('a')]) })
  at('/sessions'); const app = render(<App fetchImpl={s.fetch} />)
  const region = await list()
  await waitFor(() => expect(names(region)).toEqual(['a']))
  await userEvent.click(region.querySelector<HTMLButtonElement>('.project-toggle')!)
  app.unmount(); render(<App fetchImpl={s.fetch} />)
  await waitFor(() => expect(screen.getByRole('region', { name: 'Session list' }).querySelector('.project-toggle')).toHaveAttribute('aria-expanded', 'false'))
})

it('prunes collapsed project ids after loading and when their last session is removed', async () => {
  localStorage.setItem('hennery.subgroup', 'project')
  const current = JSON.stringify(['hat', 'hat-a', '/srv/work/a']), stale = JSON.stringify(['hat', 'hat-a', '/obsolete'])
  sessionStorage.setItem('hennery.collapsedProjects', JSON.stringify([current, stale]))
  const s = server({ list: () => page([summary('a', { title: 'a' }), summary('b', { title: 'b' })]) })
  at('/sessions'); render(<App fetchImpl={s.fetch} />)
  const region = await list()
  await waitFor(() => expect(names(region)).toEqual(['b']))
  await waitFor(() => expect(JSON.parse(sessionStorage.getItem('hennery.collapsedProjects')!)).toEqual([current]))
  act(() => s.streams[0].event('session_removed', { session_id: 'a' }, 'e1:2'))
  await waitFor(() => expect(JSON.parse(sessionStorage.getItem('hennery.collapsedProjects')!)).toEqual([]))
  expect(names(region)).toEqual(['b'])
})

it('previews only direct children while keeping grandchildren in the same project family', async () => {
  const family = (parent?: string) => ({ revision: 1, parent: parent ? { session_id: parent, agent: 'claude' } : undefined, children: [], child_counts: { total: 1, active: 1, waiting: 0 }, attention: 0 })
  const rows = [summary('parent', { title: 'Parent', family: family() }), summary('child', { title: 'Worker', family: family('parent') }), summary('grandchild', { title: 'Nested worker', family: { ...family('child'), child_counts: { total: 0, active: 0, waiting: 0 } } })]
  const s = server({ list: () => page(rows) })
  at('/sessions'); render(<App fetchImpl={s.fetch} />)
  const region = await list()
  await waitFor(() => expect(names(region)).toEqual(['Parent']))
  await userEvent.click(within(region).getByRole('button', { name: '↳ 1 child' }))
  expect(names(region)).toEqual(['Parent', 'Worker'])
  expect(within(region).getByRole('link', { name: 'Show all 1 child' })).toHaveAttribute('href', '/sessions/parent?family=1')
})

it('shows running and waiting group summaries, repository labels and one visual column header', async () => {
  width(true)
  localStorage.setItem('hennery.subgroup', 'project')
  const rows = [summary('a', { cwd: '/scratch/a', project_root: '/store/sub1.git', activity: 'running' }), summary('b', { cwd: '/scratch/b', project_root: '/store/sub2.git', operator_attention: 1 })]
  const s = server({ list: () => page(rows) })
  at('/sessions'); render(<App fetchImpl={s.fetch} />)
  const region = await list()
  await waitFor(() => expect(names(region)).toHaveLength(2))
  expect(region.querySelector('.grp-count')).toHaveTextContent('1 need you · 1 running · 2 total')
  expect([...region.querySelectorAll('.project-name')].map(el => el.textContent)).toEqual(['sub2', 'sub1'])
  expect([...region.querySelectorAll('.project-summary')].map(el => el.textContent)).toEqual(['1 session', '1 running · 1 session'])
  expect(region.querySelectorAll('.session-columns')).toHaveLength(1)
})

it('scopes Recent, list and attention counts together, restores All hats, and prunes stream deletions', async () => {
  width(true); at('/sessions')
  const a = summary('019a0180-1234-7000-8000-000000000001', { title: 'Work visit', operator_attention: 1 }), b = summary('019a0180-1234-7000-8000-000000000002', { title: 'Home visit', hat_id: 'hat-b', operator_attention: 1 })
  const s = server({ list: params => page([a, b].filter(row => !params.has('hat') || row.hat_id === params.get('hat'))) })
  render(<App fetchImpl={s.fetch} />)
  await list()
  await act(async () => navigate('/sessions/019a0180-1234-7000-8000-000000000001'))
  const recent = await screen.findByRole('region', { name: 'Recent sessions' })
  await waitFor(() => expect(within(recent).getAllByRole('link')).toHaveLength(1))
  await act(async () => navigate('/sessions/019a0180-1234-7000-8000-000000000002'))
  await waitFor(() => expect(within(recent).getAllByRole('link')).toHaveLength(2))
  expect(within(recent).getAllByRole('link')[0]).toHaveAttribute('aria-current', 'page')
  expect(screen.getByRole('navigation', { name: 'Main navigation' }).querySelectorAll('[aria-current="page"]')).toHaveLength(1)
  expect(within(recent).getByRole('heading', { name: 'Recent sessions', level: 2 })).toBeInTheDocument()
  expect([...recent.querySelectorAll('kbd')].every(key => key.textContent?.trim())).toBe(true)
  await act(async () => navigate('/sessions'))
  const hatSwitch = screen.getByRole('group', { name: 'Hat' })
  await userEvent.click(within(hatSwitch).getByRole('button', { name: 'Work' }))
  await waitFor(() => expect(names(screen.getByRole('region', { name: 'Session list' }))).toEqual(['Work visit']))
  expect(within(recent).getAllByRole('link')).toHaveLength(1)
  expect(within(recent).getByRole('link')).toHaveTextContent('Work visit')
  await waitFor(() => expect(document.title).toBe('(1) hennery'))
  await userEvent.click(within(hatSwitch).getByRole('button', { name: 'All hats' }))
  await waitFor(() => expect(within(recent).getAllByRole('link')).toHaveLength(2))
  await waitFor(() => expect(document.title).toBe('(2) hennery'))
  act(() => s.streams.at(-1)!.send('event: session_removed\nid: e1:2\ndata: {"session_id":"019a0180-1234-7000-8000-000000000002"}\n\n'))
  await waitFor(() => expect(within(recent).getAllByRole('link')).toHaveLength(1))
  expect(localStorage.getItem('hennery.visitedSessions')).not.toContain('019a0180-1234-7000-8000-000000000002')
})

it('keeps Recent slots and numbered shortcuts stable across clicks, switching back and streamed updates', async () => {
  width(true); at('/sessions')
  const rows = Array.from({ length: 6 }, (_, n) => summary(`019a0180-1234-7000-8000-${String(n).padStart(12, '0')}`, { title: `Visit ${n}` }))
  const ids = rows.map(row => row.session_id)
  localStorage.setItem('hennery.visitedSessions', JSON.stringify(ids))
  const s = server({ list: () => page(rows) })
  render(<App fetchImpl={s.fetch} />)
  const recent = await screen.findByRole('region', { name: 'Recent sessions' })
  const links = () => within(recent).getAllByRole('link')
  const order = () => links().map(link => link.getAttribute('href'))
  await waitFor(() => expect(links()).toHaveLength(6))
  const before = order(), hints = links().map(link => link.querySelector('kbd')?.textContent)
  await userEvent.click(links()[2])
  await waitFor(() => expect(location.pathname).toBe(`/sessions/${ids[2]}`))
  expect(order()).toEqual(before)
  expect(links()[2]).toHaveAttribute('aria-current', 'page')
  expect(links().map(link => link.querySelector('kbd')?.textContent)).toEqual(hints)
  await userEvent.keyboard('{Alt>}2{/Alt}')
  await waitFor(() => expect(location.pathname).toBe(`/sessions/${ids[1]}`))
  await userEvent.keyboard('{Alt>}2{/Alt}')
  expect(location.pathname).toBe(`/sessions/${ids[1]}`)
  await userEvent.keyboard('`')
  await waitFor(() => expect(location.pathname).toBe(`/sessions/${ids[2]}`))
  act(() => s.streams[0].event('session_upsert', { ...rows[4], title: 'Renamed visit', activity: 'running', last_event_at: '2099-01-01T00:00:00Z' }, 'e1:2'))
  await waitFor(() => expect(links()[4]).toHaveTextContent('Renamed visit'))
  expect(order()).toEqual(before)
  expect(links().map(link => link.querySelector('kbd')?.textContent)).toEqual(hints)
})

it('removes closed visits live, fills Recent slots and updates numbered shortcuts, Back and palette Recent', async () => {
  width(true); at('/sessions')
  const rows = Array.from({ length: 8 }, (_, n) => summary(`019a0180-1234-7000-8000-${String(n).padStart(12, '0')}`, { title: `Visit ${n}` }))
  rows[0] = { ...rows[0], lifecycle: 'closed' }
  const ids = rows.map(row => row.session_id)
  localStorage.setItem('hennery.visitedSessions', JSON.stringify(ids))
  const s = server({ list: () => page(rows) })
  render(<App fetchImpl={s.fetch} />)
  const recent = await screen.findByRole('region', { name: 'Recent sessions' })
  const order = () => within(recent).getAllByRole('link').map(link => link.getAttribute('href'))
  await waitFor(() => expect(order()).toEqual(ids.slice(1, 7).map(id => `/sessions/${id}`)))
  await userEvent.click(within(recent).getAllByRole('link')[0])
  await userEvent.keyboard('{Alt>}2{/Alt}')
  await waitFor(() => expect(location.pathname).toBe(`/sessions/${ids[2]}`))
  act(() => s.streams[0].event('session_upsert', { ...rows[1], lifecycle: 'closed', activity: undefined, last_event_at: '2099-01-01T00:00:00Z' }, 'e1:2'))
  await waitFor(() => expect(order()).toEqual(ids.slice(2).map(id => `/sessions/${id}`)))
  await userEvent.keyboard('`')
  expect(location.pathname).toBe(`/sessions/${ids[2]}`)
  await userEvent.keyboard('{Alt>}2{/Alt}')
  await waitFor(() => expect(location.pathname).toBe(`/sessions/${ids[3]}`))
  await userEvent.keyboard('{Control>}k{/Control}')
  const palette = await screen.findByRole('dialog', { name: 'Command palette' })
  const visits = within(palette).getByRole('group', { name: 'Recent' })
  expect(within(visits).queryByRole('option', { name: /^Visit [01] / })).not.toBeInTheDocument()
  expect(within(visits).getByRole('option', { name: /^Visit 2 / })).toBeInTheDocument()
})

it('does not repeat history id lookups for search keystrokes or hat switches', async () => {
  width(true); at('/sessions')
  const id = '019a0180-1234-7000-8000-000000000001'
  localStorage.setItem('hennery.visitedSessions', JSON.stringify([id]))
  const s = server({ list: params => params.get('q') === id ? page([summary(id, { hat_id: 'hat-b' })]) : page([]) })
  render(<App fetchImpl={s.fetch} />)
  await list()
  const lookups = () => s.of(LIST).filter(url => url.searchParams.get('q') === id).length
  await waitFor(() => expect(lookups()).toBeGreaterThan(0))
  await userEvent.type(screen.getByRole('searchbox'), 'parser')
  await waitFor(() => expect(s.of(LIST).some(url => url.searchParams.get('q') === 'parser')).toBe(true))
  await userEvent.click(within(screen.getByRole('group', { name: 'Hat' })).getByRole('button', { name: 'Work' }))
  await waitFor(() => expect(s.of(LIST).some(url => url.searchParams.get('hat') === 'hat-a')).toBe(true))
  expect(lookups()).toBe(1)
})

it.each([true, false])('never restores an outside-hat deleted Recent after All hats, live removal=%s', async liveRemoval => {
  width(true); at('/sessions')
  const id = '019a0180-1234-7000-8000-000000000002'
  const home = summary(id, { title: 'Home visit', hat_id: 'hat-b' })
  let removed = false
  const s = server({ list: params => page(!removed && (!params.has('hat') || params.get('hat') === home.hat_id) ? [home] : []) })
  render(<App fetchImpl={s.fetch} />)
  await list()
  await act(async () => navigate(`/sessions/${id}`))
  const recent = await screen.findByRole('region', { name: 'Recent sessions' })
  await waitFor(() => expect(within(recent).getAllByRole('link')).toHaveLength(1))
  await act(async () => navigate('/sessions'))
  const hats = screen.getByRole('group', { name: 'Hat' })
  await userEvent.click(within(hats).getByRole('button', { name: 'Work' }))
  await waitFor(() => expect(s.of(STREAM).at(-1)?.searchParams.get('hat')).toBe('hat-a'))
  removed = true
  if (liveRemoval) {
    act(() => s.streams.at(-1)!.event('session_removed', { session_id: id }))
    await waitFor(() => expect(localStorage.getItem('hennery.visitedSessions')).not.toContain(id))
  }
  await userEvent.click(within(hats).getByRole('button', { name: 'All hats' }))
  await waitFor(() => expect(s.of(STREAM).at(-1)?.searchParams.has('hat')).toBe(false))
  await waitFor(() => expect(within(recent).queryAllByRole('link')).toHaveLength(0))
  await waitFor(() => expect(localStorage.getItem('hennery.visitedSessions')).not.toContain(id))
})
