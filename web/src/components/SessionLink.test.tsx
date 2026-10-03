// A session's address in the app (F-11, F-19): a push link or a reload
// opens `/sessions/<id>` as an explicit selection, its header fed by the
// list store, beside the list on a desktop and full screen on a phone.
import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import type { HatItem, SessionDetail } from '../generated/protocol'
import type { Item, SessionSummary, SummaryPage } from '../generated/view'
import { json, liveStream, type LiveStream } from '../test-stream'
import { DESKTOP } from './Shell'

const LIST = '/api/view/sessions'
const LIST_STREAM = '/api/stream/sessions'
const NARROW = '(max-width: 767px)'

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
    title: `Task ${id}`,
    ...patch,
  }
}

const hat = (id: string, name: string): HatItem => ({
  id,
  name,
  colour: '#112233',
  created_at: '2026-10-01T00:00:00Z',
  default_for_new_hosts: false,
  purging: false,
})

const ROWS = [
  summary('a', { last_event_at: '2026-10-02T11:00:00.000Z' }),
  summary('b', { hat_id: 'hat-b', last_event_at: '2026-10-02T10:00:00.000Z' }),
  summary('a-old', { last_event_at: '2026-10-02T09:00:00.000Z' }),
]

const listPage = (sessions: SessionSummary[]): SummaryPage => ({ sessions, epoch: 'e1', revision: 1, waiting: 0 })

function item(id: string, turn: string): Item {
  return { id, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: turn, kind: 'message', text: id } as Item
}

/** The list's page for a query, as the server answers it: the chosen hat's
 *  rows only. */
const hatPage = (params: URLSearchParams) => {
  const chosen = params.get('hat')
  return json(listPage(ROWS.filter((row) => chosen === null || row.hat_id === chosen)))
}

/** A session's detail as the server sends it: a summary's fields with
 *  `pending`, and no `question_waits` (the real detail has none). */
function detailOf(id: string): SessionDetail {
  const fields: Record<string, unknown> = { ...summary(id, { title: `Detail ${id}` }), pending: [] }
  delete fields.question_waits
  return fields as unknown as SessionDetail
}

/** A server with the list and, for any session id, a one-row transcript
 *  (`<id> says hello`), a detail titled `Detail <id>` and a catalogue. */
function server({ list = hatPage }: { list?: (params: URLSearchParams) => Response | Promise<Response> } = {}) {
  const calls: URL[] = []
  const listStreams: LiveStream[] = []
  const itemStreams: LiveStream[] = []
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = new URL(String(input), 'http://h')
    calls.push(url)
    const path = url.pathname
    if (path === '/api/capabilities') return json({ mode: 'full', features: [] })
    if (path === '/api/hats') return json([hat('hat-a', 'Work'), hat('hat-b', 'Home')])
    if (path === '/api/hosts') return json([{ host_id: 'h1', name: 'laptop' }])
    if (path === LIST) return list(url.searchParams)
    if (path === LIST_STREAM) {
      const live = liveStream()
      listStreams.push(live)
      return live.response
    }
    const view = /^\/api\/view\/sessions\/([^/]+)$/.exec(path)
    if (view) {
      const id = decodeURIComponent(view[1])
      return json({ items: [item(`${id} says hello`, 't1')], older: false, epoch: 'e1', revision: 1 })
    }
    if (/^\/api\/stream\/view\/sessions\/[^/]+$/.test(path)) {
      const live = liveStream()
      itemStreams.push(live)
      return live.response
    }
    const catalog = /^\/api\/sessions\/([^/]+)\/catalog$/.exec(path)
    if (catalog) return json({ session_id: decodeURIComponent(catalog[1]), config_options: [], commands: [] })
    const detail = /^\/api\/sessions\/([^/]+)$/.exec(path)
    if (detail) return json(detailOf(decodeURIComponent(detail[1])))
    return json({ code: 'not_found', message: 'no' }, 404)
  })
  const of = (path: string) => calls.filter((u) => u.pathname === path)
  return { fetch: fetch as unknown as typeof globalThis.fetch, calls, listStreams, itemStreams, of }
}

/** Both of the app's media queries answer as one width would. */
function width(desktop: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: query === DESKTOP ? desktop : query === NARROW ? !desktop : false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
}

function at(path: string) {
  history.replaceState(null, '', path)
}

const main = () => document.querySelector('main.main') as HTMLElement
/** The signed-in frame, once the capabilities came. */
const rail = () => screen.findByRole('complementary', { name: 'Views' })
/** The session's header title. */
const title = () => within(main()).getByRole('heading', { level: 1 })
/** Waits for the frame, then for the header's title to read `text`. */
async function titled(text: string) {
  await rail()
  await waitFor(() => expect(title()).toHaveTextContent(text))
}

beforeEach(() => {
  localStorage.clear()
  width(true)
})

afterEach(() => {
  at('/')
  // @ts-expect-error jsdom has none; each test sets its own
  delete window.matchMedia
})

describe('the session’s header, from the list (F-4)', () => {
  it('follows the list stream’s upserts, and fetches no detail when the list holds the session', async () => {
    at('/sessions/a')
    const s = server()
    render(<App fetchImpl={s.fetch} />)
    await titled('Task a')
    await within(main()).findByText('a says hello')
    await waitFor(() => expect(s.listStreams).toHaveLength(1))
    act(() =>
      s.listStreams[0].event('session_upsert', summary('a', { title: 'Task a, renamed', activity: 'running', last_event_at: '2026-10-02T11:30:00.000Z' }), 'e1:2'),
    )
    await titled('Task a, renamed')
    expect(within(main()).getByText('Running')).toBeInTheDocument()
    // The view opened before the list's first page came: it waited for it.
    expect(s.of('/api/sessions/a')).toHaveLength(0)
  })

  it('a session the view finds removed leaves the list at once', async () => {
    at('/sessions/a')
    const s = server()
    render(<App fetchImpl={s.fetch} />)
    await titled('Task a')
    const list = await screen.findByRole('complementary', { name: 'Views' })
    expect(within(list).getByText('Task a', { exact: true })).toBeInTheDocument()
    await waitFor(() => expect(s.itemStreams).toHaveLength(1))
    // The item stream says so; the list stream has not yet.
    act(() => s.itemStreams[0].event('session_removed', { session_id: 'a' }))
    expect(await within(main()).findByRole('heading', { name: 'This session was deleted' })).toBeInTheDocument()
    await waitFor(() => expect(within(list).queryByText('Task a', { exact: true })).toBeNull())
    expect(within(list).getByText('Task a-old', { exact: true })).toBeInTheDocument()
  })

  it('fetches the detail only for a session the list does not hold', async () => {
    at('/sessions/elsewhere')
    const s = server()
    render(<App fetchImpl={s.fetch} />)
    await titled('Detail elsewhere')
    expect(within(main()).getByText('elsewhere says hello')).toBeInTheDocument()
    expect(s.of('/api/sessions/elsewhere')).toHaveLength(1)
    expect(s.of('/api/sessions/a')).toHaveLength(0)
  })

  it('keeps the header while the list starts over for a search', async () => {
    at('/sessions/a')
    // The search's page never comes.
    const s = server({ list: (params) => (params.get('q') ? new Promise<Response>(() => {}) : json(listPage(ROWS))) })
    render(<App fetchImpl={s.fetch} />)
    await titled('Task a')
    const views = await rail()
    await userEvent.type(within(views).getByRole('searchbox', { name: 'Search sessions' }), 'zzz')
    await waitFor(() => expect(s.of(LIST).some((u) => u.searchParams.get('q') === 'zzz')).toBe(true), { timeout: 3000 })
    expect(title()).toHaveTextContent('Task a')
    expect(s.of('/api/sessions/a')).toHaveLength(0)
  })

  it('fetches the detail when the list cannot be read', async () => {
    at('/sessions/a')
    const s = server({ list: () => json({ code: 'internal', message: 'down' }, 500) })
    render(<App fetchImpl={s.fetch} />)
    await titled('Detail a')
    expect(s.of('/api/sessions/a')).toHaveLength(1)
  })
})

describe('a session’s link (F-11, F-19)', () => {
  it('shows a session outside the chosen hat', async () => {
    localStorage.setItem('hennery.hat', 'hat-a')
    at('/sessions/b')
    const s = server()
    render(<App fetchImpl={s.fetch} />)
    // The hat's list does not hold it: its header is the detail's, as a
    // push link's is.
    await titled('Detail b')
    expect(await within(main()).findByText('b says hello')).toBeInTheDocument()
    const views = await rail()
    await waitFor(() => expect(within(views).queryAllByRole('link').filter((l) => l.classList.contains('sess'))).toHaveLength(2))
    expect(within(views).queryByText('Task b')).toBeNull()
    expect(location.pathname).toBe('/sessions/b')
    expect(s.of('/api/sessions/b')).toHaveLength(1)
    expect(s.of(LIST).every((u) => u.searchParams.get('hat') === 'hat-a')).toBe(true)
  })

  it('shows a session the loaded list does not hold', async () => {
    at('/sessions/not-in-the-list')
    const s = server()
    render(<App fetchImpl={s.fetch} />)
    await rail()
    expect(await within(main()).findByText('not-in-the-list says hello')).toBeInTheDocument()
    await waitFor(() => expect(s.listStreams).toHaveLength(1))
    expect(location.pathname).toBe('/sessions/not-in-the-list')
  })

  it('is never replaced by the desktop’s auto-select, even when a newer session comes', async () => {
    at('/sessions/a-old')
    const s = server()
    render(<App fetchImpl={s.fetch} />)
    await titled('Task a-old')
    await waitFor(() => expect(s.listStreams).toHaveLength(1))
    act(() => s.listStreams[0].event('session_upsert', summary('newest', { last_event_at: '2026-10-02T12:00:00.000Z' }), 'e1:2'))
    const views = await rail()
    await within(views).findByText('Task newest')
    expect(location.pathname).toBe('/sessions/a-old')
    expect(title()).toHaveTextContent('Task a-old')
  })

  it('is restored by a reload', async () => {
    at('/sessions/a')
    const s = server()
    const first = render(<App fetchImpl={s.fetch} />)
    await userEvent.click(await within(await rail()).findByText('Task a-old'))
    await within(main()).findByText('a-old says hello')
    first.unmount()
    // A reload: a new app at the same address.
    render(<App fetchImpl={server().fetch} />)
    await rail()
    expect(await within(main()).findByText('a-old says hello')).toBeInTheDocument()
    expect(location.pathname).toBe('/sessions/a-old')
  })

  it('shows the list in the rail and the session beside it on a desktop', async () => {
    at('/sessions/a')
    const s = server()
    render(<App fetchImpl={s.fetch} />)
    expect(await within(await rail()).findByRole('region', { name: 'Session list' })).toBeInTheDocument()
    expect(await within(main()).findByText('a says hello')).toBeInTheDocument()
    expect(within(main()).queryByRole('region', { name: 'Session list' })).toBeNull()
  })

  it('on a phone shows the session alone, with a way back to the list', async () => {
    width(false)
    at('/sessions/b')
    const s = server()
    render(<App fetchImpl={s.fetch} />)
    await rail()
    expect(await within(main()).findByText('b says hello')).toBeInTheDocument()
    // A phone's first page: 8 groups.
    expect(s.of('/api/view/sessions/b')[0].searchParams.get('limit')).toBe('8')
    expect(screen.queryByRole('region', { name: 'Session list' })).toBeNull()
    const back = within(main()).getByRole('link', { name: 'Back to sessions' })
    expect(back).toHaveAttribute('href', '/sessions')
    await userEvent.click(back)
    expect(location.pathname).toBe('/sessions')
    expect(await screen.findByRole('region', { name: 'Session list' })).toBeInTheDocument()
    // A phone stays on the list: no auto-select.
    expect(location.pathname).toBe('/sessions')
  })

  // jsdom applies no media query: the rules themselves. The way back is
  // hidden by default and shown under 768 px by a later rule.
  it('has its way back shown under 768 px', () => {
    const css = readFileSync(join(process.cwd(), 'src/index.css'), 'utf8').replace(/\s+/g, ' ')
    const hidden = css.indexOf('.back-btn { display:none; }')
    expect(hidden).toBeGreaterThan(-1)
    const mobile = /@media \(max-width:767px\) \{((?:[^{}]*\{[^}]*\})*)[^{}]*\}/g
    const shownAfter = [...css.matchAll(mobile)].some(
      (m) => (m.index ?? 0) > hidden && /\.back-btn \{[^}]*display:flex/.test(m[1]),
    )
    expect(shownAfter).toBe(true)
  })
})
