import { act, renderHook, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { SessionCatalog } from '../generated/protocol'
import type { Item, ItemPage } from '../generated/view'
import { json, liveStream, routed, type Call, type LiveStream } from '../test-stream'
import { useSessionItems } from './useSessionItems'

// A session id that needs encoding in a path.
const ID = 's/1 x'
const PAGE_PATH = '/api/view/sessions/s%2F1%20x'
const STREAM_PATH = '/api/stream/view/sessions/s%2F1%20x'
const CATALOG_PATH = '/api/sessions/s%2F1%20x/catalog'

function message(id: string, turn: string | undefined, version = 1, ts = '2026-10-02T10:00:00.000Z'): Item {
  return { id, version, ts, ...(turn === undefined ? {} : { turn_id: turn }), kind: 'message', text: id } as Item
}

function page(items: Item[], revision: number, older = false): ItemPage {
  return { items, older, epoch: 'e1', revision }
}

function catalog(model: string): SessionCatalog {
  return { session_id: ID, config_options: [], commands: [], model }
}

const FAST = { retryMs: () => 5, resyncedMs: 400 }

/** A server for one session: pages served in turn (the last repeats), a
 *  catalogue, and a new live stream per connection. */
function server(pages: (ItemPage | number)[], options: { older?: Record<string, ItemPage | number> } = {}) {
  const streams: LiveStream[] = []
  const catalogs: SessionCatalog[] = [catalog('m1')]
  const answer = (value: ItemPage | number) => (typeof value === 'number' ? json({ code: 'x', message: 'x' }, value) : json(value))
  const t = routed((call: Call) => {
    const url = new URL(call.path, 'http://h')
    if (url.pathname === PAGE_PATH) {
      const before = url.searchParams.get('before_turn')
      if (before !== null) return answer(options.older?.[before] ?? 500)
      return answer(pages.length > 1 ? pages.shift()! : pages[0])
    }
    if (url.pathname === CATALOG_PATH) return json(catalogs.length > 1 ? catalogs.shift()! : catalogs[0])
    if (url.pathname === STREAM_PATH) {
      const live = liveStream()
      streams.push(live)
      return live.response
    }
    return json({ code: 'not_found', message: 'no' }, 404)
  })
  const of = (path: string) => t.calls.filter((c) => new URL(c.path, 'http://h').pathname === path)
  return { ...t, streams, catalogs, of }
}

afterEach(() => {
  // @ts-expect-error clear the stub between tests
  delete window.matchMedia
})

describe('useSessionItems', () => {
  it('opens: the first page, the catalogue, then the stream from the page’s anchor', async () => {
    const s = server([page([message('a', 't1')], 10)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    expect(result.current.loading).toBe(true)
    await waitFor(() => expect(result.current.stream).toBe('open'))
    expect(result.current.items.map((i) => i.id)).toEqual(['a'])
    expect(result.current.loading).toBe(false)
    expect(result.current.catalog?.model).toBe('m1')
    expect(s.of(PAGE_PATH)[0].path).toBe(PAGE_PATH)
    expect(s.of(STREAM_PATH)).toHaveLength(1)
    expect(s.of(STREAM_PATH)[0].headers['Last-Event-ID']).toBe('e1:10')
    expect(s.of(STREAM_PATH)[0].path).toBe(STREAM_PATH)
  })

  it('asks for 8 groups under 768 px', async () => {
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: query === '(max-width: 767px)',
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia
    const s = server([page([message('a', 't1')], 10)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    expect(s.of(PAGE_PATH)[0].path).toBe(`${PAGE_PATH}?limit=8`)
  })

  it('applies upserts and removals from the stream', async () => {
    const s = server([page([message('a', 't1', 1)], 10)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(s.streams).toHaveLength(1))
    act(() => {
      s.streams[0].event('item', { ...message('a', 't1', 2), text: 'changed' })
      s.streams[0].event('item', message('b', 't1', 3), 'e1:12')
    })
    await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['a', 'b']))
    expect((result.current.items[0] as Item & { text: string }).text).toBe('changed')
    act(() => s.streams[0].event('item_removed', { id: 'a' }, 'e1:13'))
    await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['b']))
  })

  it('says reconnecting while the stream is lost', async () => {
    const s = server([page([], 10)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    act(() => {
      s.streams[0].event('item', message('a', 't1'), 'e1:11')
      s.streams[0].end()
    })
    await waitFor(() => expect(result.current.stream).toBe('reconnecting'))
    await waitFor(() => expect(s.streams).toHaveLength(2), { timeout: 3000 })
    expect(s.of(STREAM_PATH)[1].headers['Last-Event-ID']).toBe('e1:11')
    await waitFor(() => expect(result.current.stream).toBe('open'))
    // No page was fetched again.
    expect(s.of(PAGE_PATH)).toHaveLength(1)
  })

  it('on resync_required: closes at once, refetches, replaces the store and reopens from the new anchor', async () => {
    const s = server([page([message('c', 't3')], 10, true), page([message('x', 't9')], 20, true)], {
      older: { t3: page([message('a', 't1')], 10, false) },
    })
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    expect(result.current.loads).toBe(1)
    await act(() => result.current.loadOlder())
    expect(s.of(PAGE_PATH)[1].path).toBe(`${PAGE_PATH}?before_turn=t3`)
    expect(result.current.items.map((i) => i.id)).toEqual(['a', 'c'])
    // An older page is not a first page.
    expect(result.current.loads).toBe(1)
    act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']))
    // The items were replaced: the view's window goes back to the tail.
    expect(result.current.loads).toBe(2)
    expect(result.current.older).toBe(true)
    expect(result.current.resynced).toBe(true)
    await waitFor(() => expect(result.current.stream).toBe('open'))
    // The old stream was closed, never reconnected with the old anchor.
    expect(s.streams[0].cancelled).toBe(true)
    expect(s.of(STREAM_PATH).map((c) => c.headers['Last-Event-ID'])).toEqual(['e1:10', 'e1:20'])
    // The catalogue too: a gap may have hidden a change of it.
    expect(s.of(CATALOG_PATH)).toHaveLength(2)
    await waitFor(() => expect(result.current.resynced).toBe(false))
  })

  it.each([
    ['item', 'not json'],
    ['item', '{"id":"a"}'],
    ['item_removed', '{"nope":1}'],
    ['catalog_changed', '{"model":"x"}'],
  ])('a malformed %s message (%s) resyncs', async (event, data) => {
    const s = server([page([message('a', 't1')], 10), page([message('y', 't2')], 30)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    act(() => s.streams[0].send(`event: ${event}\ndata: ${data}\nid: e1:15\n\n`))
    await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['y']))
    await waitFor(() => expect(s.streams).toHaveLength(2))
    expect(s.of(STREAM_PATH)[1].headers['Last-Event-ID']).toBe('e1:30')
    expect(s.streams[0].cancelled).toBe(true)
  })

  it('on session_removed: marks the session removed and closes the stream', async () => {
    const s = server([page([message('a', 't1')], 10)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    act(() => {
      s.streams[0].event('session_removed', { session_id: ID })
      s.streams[0].event('item', message('late', 't1'), 'e1:99')
    })
    await waitFor(() => expect(result.current.removed).toBe(true))
    expect(result.current.stream).toBe('closed')
    expect(s.streams[0].cancelled).toBe(true)
    expect(result.current.items.map((i) => i.id)).toEqual(['a'])
    await new Promise((r) => setTimeout(r, 30))
    expect(s.of(STREAM_PATH)).toHaveLength(1)
  })

  it('a 404 from the page marks the session removed and opens no stream', async () => {
    const s = server([404])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.removed).toBe(true))
    expect(result.current.loading).toBe(false)
    expect(result.current.stream).toBe('closed')
    expect(s.of(STREAM_PATH)).toHaveLength(0)
  })

  it('a 404 from the stream marks the session removed', async () => {
    const t = routed((call) => {
      if (call.path.startsWith('/api/stream/')) return json({ code: 'not_found', message: 'no' }, 404)
      if (call.path.endsWith('/catalog')) return json(catalog('m1'))
      return json(page([message('a', 't1')], 10))
    })
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(result.current.removed).toBe(true))
    expect(result.current.stream).toBe('closed')
  })

  it('another refusal from the stream ends it with an error, not removed', async () => {
    const t = routed((call) => {
      if (call.path.startsWith('/api/stream/')) return json({ code: 'invalid', message: 'no' }, 400)
      if (call.path.endsWith('/catalog')) return json(catalog('m1'))
      return json(page([], 10))
    })
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(result.current.error).not.toBeNull())
    expect(result.current.removed).toBe(false)
    expect(result.current.stream).toBe('closed')
  })

  it('on catalog_changed: the catalogue is replaced, with no fetch', async () => {
    const s = server([page([], 10)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.catalog?.model).toBe('m1'))
    const fetches = s.fetch.mock.calls.length
    act(() => s.streams[0].event('catalog_changed', catalog('m2')))
    await waitFor(() => expect(result.current.catalog?.model).toBe('m2'))
    expect(s.fetch.mock.calls.length).toBe(fetches)
  })

  it('a catalogue fetched before a catalog_changed never replaces it', async () => {
    let release: (r: Response) => void = () => {}
    const streams: LiveStream[] = []
    const t = routed((call) => {
      if (call.path.endsWith('/catalog')) return new Promise<Response>((r) => (release = r))
      if (call.path.startsWith('/api/stream/')) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      return json(page([], 10))
    })
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(streams).toHaveLength(1))
    act(() => streams[0].event('catalog_changed', catalog('new')))
    await waitFor(() => expect(result.current.catalog?.model).toBe('new'))
    await act(async () => release(json(catalog('old'))))
    expect(result.current.catalog?.model).toBe('new')
  })

  it('setCatalog (a config answer) replaces the catalogue', async () => {
    const s = server([page([], 10)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.catalog?.model).toBe('m1'))
    act(() => result.current.setCatalog(catalog('m3')))
    expect(result.current.catalog?.model).toBe('m3')
  })

  it('loadOlder does nothing when nothing is older', async () => {
    const s = server([page([message('a', 't1')], 10, false)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    await act(() => result.current.loadOlder())
    expect(s.of(PAGE_PATH)).toHaveLength(1)
  })

  it('an older page that comes after a resync is dropped', async () => {
    let release: (r: Response) => void = () => {}
    const pages = [page([message('c', 't3')], 10, true), page([message('x', 't9')], 20, false)]
    const streams: LiveStream[] = []
    const t = routed((call) => {
      if (call.path.includes('before_turn')) return new Promise<Response>((r) => (release = r))
      if (call.path.endsWith('/catalog')) return json(catalog('m1'))
      if (call.path.startsWith('/api/stream/')) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      return json(pages.length > 1 ? pages.shift()! : pages[0])
    })
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(streams).toHaveLength(1))
    let older: Promise<void> = Promise.resolve()
    act(() => {
      older = result.current.loadOlder()
    })
    act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']))
    await act(async () => {
      release(json(page([message('a', 't1')], 10, false)))
      await older
    })
    expect(result.current.items.map((i) => i.id)).toEqual(['x'])
    expect(result.current.loadingOlder).toBe(false)
  })

  it('fetches a failed first page again, then opens', async () => {
    const s = server([503, page([message('a', 't1')], 10)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    expect(s.of(PAGE_PATH)).toHaveLength(2)
    expect(result.current.error).toBeNull()
  })

  it('closes the stream on unmount', async () => {
    const s = server([page([], 10)])
    const { result, unmount } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    unmount()
    await waitFor(() => expect(s.streams[0].cancelled).toBe(true))
  })

  it('a refused first page shows its error and is not fetched again', async () => {
    const s = server([400, page([], 10)])
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.error).not.toBeNull())
    expect(result.current.stream).toBe('closed')
    expect(result.current.removed).toBe(false)
    await new Promise((r) => setTimeout(r, 30))
    expect(s.of(PAGE_PATH)).toHaveLength(1)
  })

  it('a first page that is not a page is fetched again', async () => {
    let n = 0
    const t = routed((call) => {
      if (call.path.endsWith('/catalog')) return json(catalog('m1'))
      if (call.path.startsWith('/api/stream/')) return liveStream().response
      return json(n++ === 0 ? { nope: true } : page([message('a', 't1')], 10))
    })
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
    await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['a']))
    expect(n).toBe(2)
  })

  it('the wait before a refetch starts short again after a page comes', async () => {
    const retryMs = vi.fn(() => 5)
    const s = server([503, page([message('a', 't1')], 10), 503, page([message('b', 't1')], 20)])
    const { result } = renderHook(() => useSessionItems(ID, { ...FAST, retryMs }), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['b']))
    expect(retryMs.mock.calls).toEqual([[0], [0]])
  })

  it('an older page answered 404 marks the session removed', async () => {
    const s = server([page([message('c', 't3')], 10, true)], { older: { t3: 404 } })
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    await act(() => result.current.loadOlder())
    expect(result.current.removed).toBe(true)
    expect(s.streams[0].cancelled).toBe(true)
  })

  it('an older page answered 400 (the turn is gone) resyncs', async () => {
    const s = server([page([message('c', 't3')], 10, true), page([message('x', 't9')], 20)], { older: { t3: 400 } })
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    await act(() => result.current.loadOlder())
    await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']))
    expect(result.current.error).toBeNull()
  })

  /** A server whose first page comes at once and whose later first pages
   *  and older pages are held until the test answers them. */
  function heldServer(first: ItemPage) {
    const pages: ((r: Response) => void)[] = []
    const older: ((r: Response) => void)[] = []
    const streams: LiveStream[] = []
    let firstPages = 0
    const t = routed((call) => {
      if (call.path.includes('before_turn')) return new Promise<Response>((r) => older.push(r))
      if (call.path.endsWith('/catalog')) return json(catalog('m1'))
      if (call.path.startsWith('/api/stream/')) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      firstPages++
      return firstPages === 1 ? json(first) : new Promise<Response>((r) => pages.push(r))
    })
    const live = () => streams.filter((s) => !s.cancelled)
    return { ...t, pages, older, streams, live, firstPages: () => firstPages }
  }

  const settle = () => new Promise((r) => setTimeout(r, 50))

  it('a resync while an older page is answered 400 leaves one stream open, and none after unmount', async () => {
    const s = heldServer(page([message('c', 't3')], 10, true))
    const { result, unmount } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    let older: Promise<void> = Promise.resolve()
    act(() => {
      older = result.current.loadOlder()
    })
    await waitFor(() => expect(s.older).toHaveLength(1))
    act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(s.pages).toHaveLength(1))
    // The 400 belongs to the run the resync replaced: it must not resync again.
    await act(async () => {
      s.older[0](json({ code: 'invalid', message: 'gone' }, 400))
      await older
    })
    await act(settle)
    await act(async () => {
      for (const answer of s.pages.splice(0)) answer(json(page([message('x', 't9')], 20, true)))
    })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    await act(settle)
    expect(s.firstPages()).toBe(2)
    expect(s.streams).toHaveLength(2)
    expect(s.live()).toHaveLength(1)
    expect(result.current.items.map((i) => i.id)).toEqual(['x'])
    expect(result.current.loadingOlder).toBe(false)
    unmount()
    await waitFor(() => expect(s.live()).toHaveLength(0))
  })

  it('an older page answered 400 after session_removed opens nothing', async () => {
    const s = heldServer(page([message('c', 't3')], 10, true))
    const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    let older: Promise<void> = Promise.resolve()
    act(() => {
      older = result.current.loadOlder()
    })
    await waitFor(() => expect(s.older).toHaveLength(1))
    act(() => s.streams[0].event('session_removed', { session_id: ID }))
    await waitFor(() => expect(result.current.removed).toBe(true))
    await act(async () => {
      s.older[0](json({ code: 'invalid', message: 'gone' }, 400))
      await older
    })
    await act(settle)
    expect(s.firstPages()).toBe(1)
    expect(s.streams).toHaveLength(1)
    expect(s.live()).toHaveLength(0)
    expect(result.current.stream).toBe('closed')
    expect(result.current.loadingOlder).toBe(false)
  })

  it('a resync drops a refetch already waiting to run', async () => {
    // A resync's first page fails (503) and waits to be fetched again; an
    // older page answered 400 then resyncs. The wait is dropped: one fetch.
    let n = 0
    const streams: LiveStream[] = []
    const t = routed((call) => {
      if (call.path.includes('before_turn')) return json({ code: 'invalid', message: 'gone' }, 400)
      if (call.path.endsWith('/catalog')) return json(catalog('m1'))
      if (call.path.startsWith('/api/stream/')) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      n++
      if (n === 1) return json(page([message('c', 't3')], 10, true))
      if (n === 2) return json({ code: 'x', message: 'x' }, 503)
      return json(page([message('x', 't9')], 20, true))
    })
    const { result } = renderHook(() => useSessionItems(ID, { ...FAST, retryMs: () => 1000 }), { wrapper: t.wrapper })
    await waitFor(() => expect(result.current.stream).toBe('open'))
    act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
    await waitFor(() => expect(n).toBe(2))
    await waitFor(() => expect(result.current.error).not.toBeNull())
    await act(() => result.current.loadOlder())
    // This resync follows the first with no stream opened between: it waits
    // `retryMs(0)` too (`ResyncBackoff`), after the dropped refetch's wait.
    await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']), { timeout: 3000 })
    // Past the dropped refetch's wait: it never fetches.
    await act(() => new Promise((r) => setTimeout(r, 1200)))
    expect(n).toBe(3)
  })
})
