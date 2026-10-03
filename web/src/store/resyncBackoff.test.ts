// Resyncs one after another wait longer each time, in both stores
// (`ResyncBackoff`): a server that keeps sending a message the client
// refuses cannot make it refetch and reopen in a loop.
import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Item, ItemPage, SessionSummary, SummaryPage } from '../generated/view'
import { json, liveStream, routed, type LiveStream } from '../test-stream'
import type { ListFilters } from './sessionList'
import { SessionItemsController, useSessionItems } from './useSessionItems'
import { SessionListController, useSessionList } from './useSessionList'

const LIST = '/api/view/sessions'
const LIST_STREAM = '/api/stream/sessions'
const ITEMS = '/api/view/sessions/s1'
const ITEM_STREAM = '/api/stream/view/sessions/s1'
const CATALOG = '/api/sessions/s1/catalog'
// The real waits (1 s, 2 s, 4 s …, at most 30 s) and the real settle time.
const TIMING = { resyncedMs: 400 }

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

/** Let fetches, bodies and streams move on, with no time passing. */
async function flush() {
  for (let i = 0; i < 10; i++) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
  }
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
  await flush()
}

function summary(id: string): SessionSummary {
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
  }
}

function server(paths: { page: string; stream: string }, page: () => unknown) {
  const streams: LiveStream[] = []
  const t = routed((call) => {
    const url = new URL(call.path, 'http://h')
    if (url.pathname === paths.page) return json(page())
    if (url.pathname === CATALOG) return json({ session_id: 's1', config_options: [], commands: [] })
    if (url.pathname === paths.stream) {
      const live = liveStream()
      streams.push(live)
      return live.response
    }
    return json({ code: 'not_found', message: 'no' }, 404)
  })
  const of = (path: string) => t.calls.filter((c) => new URL(c.path, 'http://h').pathname === path)
  return { ...t, streams, of }
}

const listServer = () =>
  server({ page: LIST, stream: LIST_STREAM }, (): SummaryPage => ({ sessions: [summary('a')], epoch: 'e1', revision: 10, waiting: 1 }))

const BAD_COUNT = 'event: waiting_changed\ndata: {"count":-1}\n\n'

/** A resume burst: a good upsert first, then a count that does not parse. */
function badBurst(stream: LiveStream) {
  act(() => {
    stream.event('session_upsert', summary('b'), 'e1:11')
    stream.send(BAD_COUNT)
  })
}

describe('the session list’s resyncs', () => {
  it('refetch at once the first time, then wait 1 s, 2 s, 4 s … up to 30 s, even after a good message in the burst', async () => {
    const s = listServer()
    renderHook((f: ListFilters) => useSessionList(f, TIMING), { wrapper: s.wrapper, initialProps: { hideClosed: false } })
    await flush()
    expect(s.of(LIST)).toHaveLength(1)
    expect(s.streams).toHaveLength(1)

    badBurst(s.streams[0])
    await flush()
    expect(s.streams[0].cancelled).toBe(true)
    expect(s.of(LIST)).toHaveLength(2)
    expect(s.streams).toHaveLength(2)

    let pages = 2
    for (const wait of [1000, 2000, 4000, 8000, 16000, 30000, 30000]) {
      badBurst(s.streams[pages - 1])
      await flush()
      expect(s.streams[pages - 1].cancelled).toBe(true)
      await advance(wait - 1)
      expect(s.of(LIST)).toHaveLength(pages)
      expect(s.streams).toHaveLength(pages)
      await advance(1)
      pages++
      expect(s.of(LIST)).toHaveLength(pages)
      expect(s.streams).toHaveLength(pages)
    }
  })

  it('refetch at once again after the stream stayed open 30 s', async () => {
    const s = listServer()
    renderHook((f: ListFilters) => useSessionList(f, TIMING), { wrapper: s.wrapper, initialProps: { hideClosed: false } })
    await flush()
    badBurst(s.streams[0])
    await flush()
    expect(s.streams).toHaveLength(2)
    // Open 29 s: the next resync still follows this one.
    await advance(29_000)
    badBurst(s.streams[1])
    await advance(999)
    expect(s.streams).toHaveLength(2)
    await advance(1)
    expect(s.streams).toHaveLength(3)
    // Open 30 s: the next resync is a first one again.
    await advance(30_000)
    badBurst(s.streams[2])
    await flush()
    expect(s.of(LIST)).toHaveLength(4)
    expect(s.streams).toHaveLength(4)
  })

  it('a wait ends with the hook: nothing is fetched or opened after unmount', async () => {
    const s = listServer()
    const { unmount } = renderHook((f: ListFilters) => useSessionList(f, TIMING), {
      wrapper: s.wrapper,
      initialProps: { hideClosed: false },
    })
    await flush()
    badBurst(s.streams[0])
    await flush()
    badBurst(s.streams[1])
    await flush()
    // Waiting 1 s for the third page.
    unmount()
    await advance(60_000)
    expect(s.of(LIST)).toHaveLength(2)
    expect(s.streams).toHaveLength(2)
    expect(s.streams.every((live) => live.cancelled)).toBe(true)
    expect(vi.getTimerCount()).toBe(0)
  })

  it('a start after a stop begins a new run of resyncs: the first refetches at once', async () => {
    const s = listServer()
    const controller = new SessionListController(s.client, {}, TIMING)
    controller.start()
    await flush()
    badBurst(s.streams[0])
    await flush()
    badBurst(s.streams[1])
    await flush()
    // Waiting 1 s; hidden, then shown again.
    controller.stop()
    controller.start()
    await flush()
    expect(s.of(LIST)).toHaveLength(3)
    badBurst(s.streams[2])
    await flush()
    expect(s.of(LIST)).toHaveLength(4)
    expect(s.streams).toHaveLength(4)
    controller.stop()
  })

  it('a hat switch during a wait drops it: only the new hat’s page and stream come', async () => {
    const s = listServer()
    const { rerender } = renderHook((f: ListFilters) => useSessionList(f, TIMING), {
      wrapper: s.wrapper,
      initialProps: { hat: 'hat-a', hideClosed: false } as ListFilters,
    })
    await flush()
    badBurst(s.streams[0])
    await flush()
    badBurst(s.streams[1])
    await flush()
    rerender({ hat: 'hat-b', hideClosed: false })
    await flush()
    await advance(60_000)
    expect(s.of(LIST).map((c) => c.path)).toEqual([`${LIST}?hat=hat-a`, `${LIST}?hat=hat-a`, `${LIST}?hat=hat-b`])
    expect(s.of(LIST_STREAM).map((c) => c.path)).toEqual([`${LIST_STREAM}?hat=hat-a`, `${LIST_STREAM}?hat=hat-a`, `${LIST_STREAM}?hat=hat-b`])
    expect(s.streams.slice(0, 2).every((live) => live.cancelled)).toBe(true)
    expect(s.streams[2].cancelled).toBe(false)
  })
})

function message(id: string): Item {
  return { id, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't1', kind: 'message', text: id } as Item
}

const itemServer = () => server({ page: ITEMS, stream: ITEM_STREAM }, (): ItemPage => ({ items: [message('a')], older: false, epoch: 'e1', revision: 10 }))

/** A resume burst: a good item first, then one that does not parse. */
function badItems(stream: LiveStream) {
  act(() => {
    stream.event('item', message('b'), 'e1:11')
    stream.send('event: item\ndata: nope\n\n')
  })
}

describe('a session’s resyncs', () => {
  it('refetch at once the first time, then wait 1 s, 2 s, 4 s, even after a good item in the burst', async () => {
    const s = itemServer()
    renderHook(() => useSessionItems('s1', TIMING), { wrapper: s.wrapper })
    await flush()
    expect(s.of(ITEMS)).toHaveLength(1)
    badItems(s.streams[0])
    await flush()
    expect(s.streams[0].cancelled).toBe(true)
    expect(s.of(ITEMS)).toHaveLength(2)
    expect(s.streams).toHaveLength(2)

    let pages = 2
    for (const wait of [1000, 2000, 4000]) {
      badItems(s.streams[pages - 1])
      await flush()
      // Closed at once: the stream is not read through the wait.
      expect(s.streams[pages - 1].cancelled).toBe(true)
      await advance(wait - 1)
      expect(s.of(ITEMS)).toHaveLength(pages)
      expect(s.streams).toHaveLength(pages)
      await advance(1)
      pages++
      expect(s.of(ITEMS)).toHaveLength(pages)
      expect(s.streams).toHaveLength(pages)
    }
  })

  it('refetch at once again after the stream stayed open 30 s', async () => {
    const s = itemServer()
    renderHook(() => useSessionItems('s1', TIMING), { wrapper: s.wrapper })
    await flush()
    badItems(s.streams[0])
    await flush()
    await advance(30_000)
    badItems(s.streams[1])
    await flush()
    expect(s.of(ITEMS)).toHaveLength(3)
    expect(s.streams).toHaveLength(3)
  })

  it('a start after a stop begins a new run of resyncs: the first refetches at once', async () => {
    const s = itemServer()
    const controller = new SessionItemsController(s.client, 's1', () => undefined, TIMING)
    controller.start()
    await flush()
    badItems(s.streams[0])
    await flush()
    badItems(s.streams[1])
    await flush()
    controller.stop()
    controller.start()
    await flush()
    expect(s.of(ITEMS)).toHaveLength(3)
    badItems(s.streams[2])
    await flush()
    expect(s.of(ITEMS)).toHaveLength(4)
    expect(s.streams).toHaveLength(4)
    controller.stop()
  })

  it('a wait ends with the hook: nothing is fetched or opened after unmount', async () => {
    const s = itemServer()
    const { unmount } = renderHook(() => useSessionItems('s1', TIMING), { wrapper: s.wrapper })
    await flush()
    badItems(s.streams[0])
    await flush()
    badItems(s.streams[1])
    await flush()
    unmount()
    await advance(60_000)
    expect(s.of(ITEMS)).toHaveLength(2)
    expect(s.streams).toHaveLength(2)
    expect(s.streams.every((live) => live.cancelled)).toBe(true)
    expect(vi.getTimerCount()).toBe(0)
  })
})
