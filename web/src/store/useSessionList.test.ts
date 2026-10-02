import { act, renderHook, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { SessionSummary, SummaryPage } from '../generated/view'
import { json, liveStream, routed, type LiveStream } from '../test-stream'
import type { ListFilters } from './sessionList'
import { useSessionList } from './useSessionList'

const LIST = '/api/view/sessions'
const STREAM = '/api/stream/sessions'
const FAST = { retryMs: () => 5, resyncedMs: 400 }

function summary(id: string, patch: Partial<SessionSummary> = {}): SessionSummary {
  return {
    session_id: id,
    host_id: 'h1',
    agent: 'claude',
    cwd: '/srv/work/app',
    hat_id: 'hat-a',
    lifecycle: 'active',
    presumed_parked: false,
    created_at: '2026-10-01T00:00:00.000Z',
    last_event_at: '2026-10-02T10:00:00.000Z',
    question_waits: false,
    ...patch,
  }
}

/** A page; with no `waiting`, a server that does not count (the rows are
 *  counted instead). */
function page(sessions: SessionSummary[], revision: number, next?: string, waiting?: number): SummaryPage {
  return { sessions, next_cursor: next, epoch: 'e1', revision, ...(waiting === undefined ? {} : { waiting }) } as SummaryPage
}

/** First pages served in turn (the last repeats), further pages by cursor,
 *  and a new live stream per connection. */
function server(firsts: SummaryPage[], more: Record<string, SummaryPage> = {}) {
  const streams: LiveStream[] = []
  const t = routed((call) => {
    const url = new URL(call.path, 'http://h')
    if (url.pathname === LIST) {
      const cursor = url.searchParams.get('cursor')
      if (cursor !== null) return json(more[cursor])
      return json(firsts.length > 1 ? firsts.shift()! : firsts[0])
    }
    if (url.pathname === STREAM) {
      const live = liveStream()
      streams.push(live)
      return live.response
    }
    return json({ code: 'not_found', message: 'no' }, 404)
  })
  const of = (path: string) => t.calls.filter((c) => new URL(c.path, 'http://h').pathname === path)
  return { ...t, streams, of }
}

const ids = (list: SessionSummary[]) => list.map((s) => s.session_id)

function render(s: ReturnType<typeof server>, filters: ListFilters) {
  return renderHook((f: ListFilters) => useSessionList(f, FAST), { wrapper: s.wrapper, initialProps: filters })
}

describe('useSessionList', () => {
  it('opens: the first page for the query, then the stream from its anchor', async () => {
    const s = server([page([summary('a'), summary('b', { lifecycle: 'closed' })], 10, 'c1')])
    const { result } = render(s, { hat: 'hat-a', hideClosed: true })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    expect(s.of(LIST)[0].path).toBe(`${LIST}?hat=hat-a&lifecycle=starting%2Cactive%2Cparked%2Cfailed`)
    expect(s.of(STREAM)[0].path).toBe(`${STREAM}?hat=hat-a`)
    expect(s.of(STREAM)[0].headers['Last-Event-ID']).toBe('e1:10')
    expect(ids(result.current.shown)).toEqual(['a'])
    expect(result.current.all.size).toBe(2)
    expect(result.current.hasMore).toBe(true)
  })

  it('upserts insert, update, and move a session off the shown list when it leaves the filter', async () => {
    const s = server([page([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })], 10)])
    const { result } = render(s, { hideClosed: true })
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].event('session_upsert', summary('n', { last_event_at: '2026-10-02T12:00:00.000Z' }), 'e1:11'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['n', 'a', 'b']))
    act(() => s.streams[0].event('session_upsert', summary('b', { title: 'T', last_event_at: '2026-10-02T13:00:00.000Z' }), 'e1:12'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['b', 'n', 'a']))
    expect(result.current.shown[0].title).toBe('T')
    act(() => s.streams[0].event('session_upsert', summary('n', { lifecycle: 'closed' }), 'e1:13'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['b', 'a']))
    expect(result.current.all.has('n')).toBe(true)
  })

  it('during a search an upsert never inserts a row', async () => {
    const s = server([page([summary('a')], 10)])
    const { result } = render(s, { hideClosed: true, q: 'fix' })
    await waitFor(() => expect(s.streams).toHaveLength(1))
    expect(s.of(LIST)[0].path).toBe(`${LIST}?q=fix`)
    expect(result.current.searching).toBe(true)
    act(() => {
      s.streams[0].event('session_upsert', summary('n'))
      s.streams[0].event('session_upsert', summary('a', { title: 'found' }), 'e1:11')
    })
    await waitFor(() => expect(result.current.shown[0].title).toBe('found'))
    expect(ids(result.current.shown)).toEqual(['a'])
  })

  it('session_removed takes the session away', async () => {
    const s = server([page([summary('a'), summary('b')], 10)])
    const { result } = render(s, { hideClosed: false })
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].event('session_removed', { session_id: 'a' }, 'e1:11'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['b']))
    expect(result.current.all.has('a')).toBe(false)
  })

  it('on resync_required: closes at once, refetches the first page with the current query, replaces and reopens', async () => {
    const s = server([page([summary('a')], 10, 'c1'), page([summary('z')], 40)], { c1: page([summary('b', { last_event_at: '2026-10-01T00:00:00.000Z' })], 15) })
    const { result } = render(s, { hat: 'hat-a', hideClosed: false })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    await act(() => result.current.loadMore())
    expect(ids(result.current.shown)).toEqual(['a', 'b'])
    act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['z']))
    expect(result.current.resynced).toBe(true)
    await waitFor(() => expect(s.streams).toHaveLength(2))
    expect(s.streams[0].cancelled).toBe(true)
    expect(s.of(LIST).map((c) => c.path)).toEqual([`${LIST}?hat=hat-a`, `${LIST}?cursor=c1&hat=hat-a`, `${LIST}?hat=hat-a`])
    expect(s.of(STREAM).map((c) => c.headers['Last-Event-ID'])).toEqual(['e1:10', 'e1:40'])
    await waitFor(() => expect(result.current.resynced).toBe(false))
  })

  it.each([
    ['session_upsert', 'nope'],
    ['session_upsert', '{"session_id":"a"}'],
    ['session_removed', '{}'],
    ['waiting_changed', 'nope'],
    ['waiting_changed', '{}'],
    ['waiting_changed', '{"count":-1}'],
    ['waiting_changed', '{"count":1.5}'],
    ['waiting_changed', '{"count":"2"}'],
  ])('a malformed %s (%s) resyncs', async (event, data) => {
    const s = server([page([summary('a')], 10), page([summary('y')], 30)])
    const { result } = render(s, { hideClosed: false })
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => s.streams[0].send(`event: ${event}\ndata: ${data}\n\n`))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['y']))
    await waitFor(() => expect(s.streams).toHaveLength(2))
    expect(s.of(STREAM)[1].headers['Last-Event-ID']).toBe('e1:30')
  })

  it('a further page never moves the stream’s anchor', async () => {
    const s = server([page([summary('a')], 10, 'c1')], { c1: page([summary('b')], 77) })
    const { result } = render(s, { hideClosed: false })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    await act(() => result.current.loadMore())
    expect(result.current.hasMore).toBe(false)
    act(() => s.streams[0].end())
    await waitFor(() => expect(s.streams).toHaveLength(2), { timeout: 3000 })
    expect(s.of(STREAM)[1].headers['Last-Event-ID']).toBe('e1:10')
  })

  it('no event ever fetches (F-4)', async () => {
    const s = server([page([summary('a'), summary('b')], 10)])
    const { result } = render(s, { hideClosed: true })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    const before = s.fetch.mock.calls.length
    act(() => {
      s.streams[0].event('session_upsert', summary('n'))
      s.streams[0].event('session_upsert', summary('a', { lifecycle: 'closed' }))
      s.streams[0].event('session_upsert', summary('b', { activity: 'blocked' }))
      s.streams[0].event('session_removed', { session_id: 'n' })
      s.streams[0].event('something_new', { x: 1 }, 'e1:20')
    })
    await waitFor(() => expect(result.current.counts.waiting).toBe(1))
    act(() => {
      // The server may send a session_removed twice (a delete racing its read).
      s.streams[0].event('session_removed', { session_id: 'n' })
      s.streams[0].event('session_removed', { session_id: 'a' })
      s.streams[0].event('session_removed', { session_id: 'a' }, 'e1:21')
      s.streams[0].event('waiting_changed', { count: 6 }, 'e1:22')
    })
    await waitFor(() => expect(result.current.counts.waiting).toBe(6))
    expect(result.current.all.has('a')).toBe(false)
    await new Promise((r) => setTimeout(r, 30))
    expect(s.fetch.mock.calls.length).toBe(before)
    expect(s.streams).toHaveLength(1)
    expect(result.current.stream).toBe('open')
  })

  it('the stream is opened with the hat, encoded, and with none for every hat', async () => {
    for (const [hat, path] of [
      ['h/1 x', `${STREAM}?hat=h%2F1+x`],
      ['', STREAM],
      [null, STREAM],
    ] as const) {
      const s = server([page([summary('a')], 10)])
      const { result, unmount } = render(s, { hat, hideClosed: false })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      expect(s.of(STREAM)[0].path).toBe(path)
      unmount()
    }
  })

  it('holds the server’s count from the first page, and waiting_changed sets it, with no fetch', async () => {
    // One blocked row, but the server counts 4: the server's number wins.
    const s = server([page([summary('a', { activity: 'blocked' })], 10, undefined, 4)])
    const { result } = render(s, { hat: 'hat-a', hideClosed: false })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    expect(result.current.counts.waiting).toBe(4)
    const before = s.fetch.mock.calls.length
    act(() => s.streams[0].event('session_upsert', summary('a', { activity: 'running' }), 'e1:11'))
    await new Promise((r) => setTimeout(r, 20))
    expect(result.current.counts.waiting).toBe(4)
    act(() => s.streams[0].event('waiting_changed', { count: 1 }, 'e1:12'))
    await waitFor(() => expect(result.current.counts.waiting).toBe(1))
    act(() => s.streams[0].event('waiting_changed', { count: 0 }, 'e1:13'))
    await waitFor(() => expect(result.current.counts.waiting).toBe(0))
    expect(s.fetch.mock.calls.length).toBe(before)
  })

  it('a further page never changes the server’s count', async () => {
    const s = server([page([summary('a')], 10, 'c1', 2)], { c1: page([summary('b', { question_waits: true })], 15, undefined, 9) })
    const { result } = render(s, { hideClosed: false })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    act(() => s.streams[0].event('waiting_changed', { count: 3 }, 'e1:11'))
    await waitFor(() => expect(result.current.counts.waiting).toBe(3))
    await act(() => result.current.loadMore())
    expect(ids(result.current.shown)).toEqual(['a', 'b'])
    expect(result.current.counts.waiting).toBe(3)
  })

  it('a resync takes the count from the new first page', async () => {
    const s = server([
      page([summary('a')], 10, undefined, 5),
      page([summary('z')], 40, undefined, 1),
      page([summary('y', { activity: 'blocked' }), summary('x', { activity: 'blocked' })], 50),
    ])
    const { result } = render(s, { hideClosed: false })
    await waitFor(() => expect(result.current.counts.waiting).toBe(5))
    act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['z']))
    expect(result.current.counts.waiting).toBe(1)
    await waitFor(() => expect(s.streams).toHaveLength(2))
    // A first page with no count: the rows are counted, never the old count.
    act(() => s.streams[1].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['x', 'y']))
    expect(result.current.counts.waiting).toBe(2)
  })

  it('a first page whose count is not a count is fetched again', async () => {
    const answers = [() => json({ ...page([summary('a')], 10), waiting: -1 }), () => json(page([summary('b')], 20, undefined, 2))]
    const streams: LiveStream[] = []
    const t = routed((call) => {
      if (call.path.startsWith(STREAM)) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      return answers.shift()!()
    })
    const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['b']))
    expect(result.current.counts.waiting).toBe(2)
    expect(streams).toHaveLength(1)
  })

  it('counts the hat’s sessions waiting on a question', async () => {
    const s = server([
      page(
        [
          summary('a', { activity: 'blocked' }),
          summary('b', { question_waits: true }),
          summary('c', { activity: 'blocked', hat_id: 'hat-b' }),
        ],
        10,
      ),
    ])
    const { result } = render(s, { hat: 'hat-a', hideClosed: false })
    await waitFor(() => expect(result.current.counts.waiting).toBe(2))
  })

  it('a new query starts over; the same query in a new object does not', async () => {
    const s = server([page([summary('a')], 10), page([summary('b', { hat_id: 'hat-b' })], 20)])
    const { result, rerender } = render(s, { hat: 'hat-a', hideClosed: false })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    rerender({ hat: 'hat-a', hideClosed: false })
    await new Promise((r) => setTimeout(r, 20))
    expect(s.of(LIST)).toHaveLength(1)
    rerender({ hat: 'hat-b', hideClosed: false })
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['b']))
    expect(s.of(LIST)[1].path).toBe(`${LIST}?hat=hat-b`)
    expect(s.streams[0].cancelled).toBe(true)
    await waitFor(() => expect(s.of(STREAM)[1]?.headers['Last-Event-ID']).toBe('e1:20'))
    // The new stream counts the new hat's sessions.
    expect(s.of(STREAM).map((c) => c.path)).toEqual([`${STREAM}?hat=hat-a`, `${STREAM}?hat=hat-b`])
  })

  it('a further page that comes after a resync is dropped', async () => {
    let release: (r: Response) => void = () => {}
    const firsts = [page([summary('a')], 10, 'c1'), page([summary('z')], 20)]
    const streams: LiveStream[] = []
    const t = routed((call) => {
      if (call.path.includes('cursor=')) return new Promise<Response>((r) => (release = r))
      if (call.path.startsWith(STREAM)) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      return json(firsts.length > 1 ? firsts.shift()! : firsts[0])
    })
    const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(streams).toHaveLength(1))
    let more: Promise<void> = Promise.resolve()
    act(() => {
      more = result.current.loadMore()
    })
    act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['z']))
    await act(async () => {
      release(json(page([summary('b')], 15)))
      await more
    })
    expect(ids(result.current.shown)).toEqual(['z'])
    expect(result.current.loadingMore).toBe(false)
  })

  it('after a resync dropped a further page, the next page can still be loaded', async () => {
    let release: (r: Response) => void = () => {}
    const firsts = [page([summary('a')], 10, 'c1'), page([summary('z')], 20, 'c2')]
    const streams: LiveStream[] = []
    const t = routed((call) => {
      if (call.path.includes('cursor=c1')) return new Promise<Response>((r) => (release = r))
      if (call.path.includes('cursor=c2')) return json(page([summary('y', { last_event_at: '2026-10-02T09:00:00.000Z' })], 25))
      if (call.path.startsWith(STREAM)) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      return json(firsts.length > 1 ? firsts.shift()! : firsts[0])
    })
    const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(streams).toHaveLength(1))
    let more: Promise<void> = Promise.resolve()
    act(() => {
      more = result.current.loadMore()
    })
    act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['z']))
    await act(async () => {
      release(json(page([summary('b')], 15)))
      await more
    })
    await act(() => result.current.loadMore())
    expect(ids(result.current.shown)).toEqual(['z', 'y'])
    expect(t.calls.filter((c) => c.path.includes('cursor=c2'))).toHaveLength(1)
  })

  it('a refused first page shows its error and is not fetched again', async () => {
    const t = routed(() => json({ code: 'invalid', message: 'bad' }, 400))
    const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(result.current.error).toBe('bad'))
    expect(result.current.stream).toBe('closed')
    await new Promise((r) => setTimeout(r, 30))
    expect(t.calls).toHaveLength(1)
  })

  it('a refusal from the stream ends it with an error', async () => {
    const t = routed((call) =>
      call.path.startsWith(STREAM) ? json({ code: 'x', message: 'x' }, 403) : json(page([summary('a')], 10)),
    )
    const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(result.current.error).not.toBeNull())
    expect(result.current.stream).toBe('closed')
  })

  it('a failed or unreadable first page is fetched again, the wait starting short each time', async () => {
    const retryMs = vi.fn(() => 5)
    const answers = [
      () => json({ code: 'x', message: 'x' }, 503),
      () => json(page([summary('a')], 10)),
      () => json({ sessions: 'no' }),
      () => json(page([summary('b')], 20)),
    ]
    const streams: LiveStream[] = []
    const t = routed((call) => {
      if (call.path.startsWith(STREAM)) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      return answers.shift()!()
    })
    const { result } = renderHook(() => useSessionList({ hideClosed: false }, { ...FAST, retryMs }), { wrapper: t.wrapper })
    await waitFor(() => expect(streams).toHaveLength(1))
    act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(ids(result.current.shown)).toEqual(['b']))
    expect(retryMs.mock.calls).toEqual([[0], [0]])
  })

  it('keeps the hat’s waiting count through a search and while the hat’s list loads again', async () => {
    let held: ((r: Response) => void) | null = null
    let unfilteredPages = 0
    const t = routed((call) => {
      if (call.path.startsWith(STREAM)) return liveStream().response
      if (call.path.includes('q=')) return json(page([], 30))
      unfilteredPages++
      const first = page([summary('asks', { activity: 'blocked' }), summary('calm')], 10)
      return unfilteredPages === 1 ? json(first) : new Promise<Response>((r) => (held = r))
    })
    const { result, rerender } = renderHook((f: ListFilters) => useSessionList(f, FAST), {
      wrapper: t.wrapper,
      initialProps: { hideClosed: false } as ListFilters,
    })
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.counts.waiting).toBe(1)
    rerender({ hideClosed: false, q: 'zz' })
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.shown).toEqual([])
    expect(result.current.counts.waiting).toBe(1)
    rerender({ hideClosed: false })
    await waitFor(() => expect(held).not.toBeNull())
    expect(result.current.loading).toBe(true)
    expect(result.current.counts.waiting).toBe(1)
    await act(async () => held!(json(page([summary('asks'), summary('calm')], 40))))
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.counts.waiting).toBe(0)
  })

  // The server's count: rows that disagree with it show which one is read.
  it('the server’s count holds through a search and while the hat’s list loads again', async () => {
    let held: ((r: Response) => void) | null = null
    let unfilteredPages = 0
    const t = routed((call) => {
      if (call.path.startsWith(STREAM)) return liveStream().response
      // The search finds nothing; the hat's count went up to 5 meanwhile.
      if (call.path.includes('q=')) return json(page([], 30, undefined, 5))
      unfilteredPages++
      const first = page([summary('asks', { activity: 'blocked' }), summary('calm')], 10, undefined, 4)
      return unfilteredPages === 1 ? json(first) : new Promise<Response>((r) => (held = r))
    })
    const { result, rerender } = renderHook((f: ListFilters) => useSessionList(f, FAST), {
      wrapper: t.wrapper,
      initialProps: { hat: 'hat-a', hideClosed: false } as ListFilters,
    })
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.counts.waiting).toBe(4)
    rerender({ hat: 'hat-a', hideClosed: false, q: 'zz' })
    expect(result.current.loading).toBe(true)
    expect(result.current.counts.waiting).toBe(4)
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.counts.waiting).toBe(5)
    rerender({ hat: 'hat-a', hideClosed: false })
    await waitFor(() => expect(held).not.toBeNull())
    expect(result.current.loading).toBe(true)
    expect(result.current.counts.waiting).toBe(5)
    await act(async () => held!(json(page([summary('asks', { activity: 'blocked' })], 40, undefined, 0))))
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.counts.waiting).toBe(0)
  })

  it('the server’s count of one hat is never shown for another while its list loads', async () => {
    let held: ((r: Response) => void) | null = null
    const t = routed((call) => {
      if (call.path.startsWith(STREAM)) return liveStream().response
      if (call.path.includes('hat=hat-b')) return new Promise<Response>((r) => (held = r))
      return json(page([summary('asks', { activity: 'blocked' })], 10, undefined, 4))
    })
    const { result, rerender } = renderHook((f: ListFilters) => useSessionList(f, FAST), {
      wrapper: t.wrapper,
      initialProps: { hat: 'hat-a', hideClosed: false } as ListFilters,
    })
    await waitFor(() => expect(result.current.counts.waiting).toBe(4))
    rerender({ hat: 'hat-b', hideClosed: false })
    await waitFor(() => expect(held).not.toBeNull())
    expect(result.current.loading).toBe(true)
    expect(result.current.counts.waiting).toBe(0)
    await act(async () => held!(json(page([], 20, undefined, 2))))
    await waitFor(() => expect(result.current.counts.waiting).toBe(2))
    rerender({ hat: 'hat-a', hideClosed: false })
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.counts.waiting).toBe(4)
  })
})
