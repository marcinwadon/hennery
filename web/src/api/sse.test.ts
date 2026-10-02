import { describe, expect, it, vi } from 'vitest'
import { Client } from './client'
import { openStream, type StreamEvent, type StreamState } from './sse'

/** A body that sends `chunks`, then ends (or stays open with `hold`). */
function body(chunks: string[], hold = false): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder()
  return new ReadableStream({
    start(controller) {
      for (const c of chunks) controller.enqueue(encoder.encode(c))
      if (!hold) controller.close()
    },
  })
}

function sse(chunks: string[], hold = false): Response {
  return new Response(body(chunks, hold), { status: 200, headers: { 'Content-Type': 'text/event-stream' } })
}

function setup(answers: (() => Response)[]) {
  const sent: Record<string, string>[] = []
  const navigate = vi.fn()
  const fetch = vi.fn(async (_path: RequestInfo | URL, init?: RequestInit) => {
    sent.push(init?.headers as Record<string, string>)
    const next = answers.shift()
    if (!next) return new Promise<Response>(() => {})
    return next()
  })
  const client = new Client({
    fetch: fetch as unknown as typeof globalThis.fetch,
    navigate,
    here: () => ({ pathname: '/sessions/s-1', search: '' }),
    stepUp: async () => {},
  })
  const events: StreamEvent[] = []
  const states: StreamState[] = []
  const sleeps: number[] = []
  const onResync = vi.fn()
  const onError = vi.fn()
  const open = (lastEventId?: string) =>
    openStream(client, '/api/stream/x', {
      onEvent: (e) => events.push(e),
      onState: (s) => states.push(s),
      onResync,
      onError,
      lastEventId,
      sleep: async (ms) => {
        sleeps.push(ms)
      },
    })
  return { open, sent, events, states, sleeps, onResync, onError, navigate }
}

describe('openStream', () => {
  it('parses events across chunks, with ids, multi-line data and comments', async () => {
    const t = setup([
      () => sse([': hello\n\nevent: item\nid: 1:5\ndata: {"a"', ':1}\n\n', 'data: one\ndata: two\r\n\r', '\n'], true),
    ])
    const stream = t.open()
    await vi.waitFor(() => expect(t.events).toHaveLength(2))
    expect(t.events[0]).toEqual({ event: 'item', data: '{"a":1}', id: '1:5' })
    expect(t.events[1]).toEqual({ event: 'message', data: 'one\ntwo', id: undefined })
    expect(stream.lastEventId()).toBe('1:5')
    expect(t.states).toEqual(['connecting', 'open'])
    stream.close()
  })

  it('reconnects with Last-Event-ID after the stream ends, saying so meanwhile', async () => {
    const t = setup([() => sse(['id: 1:7\nevent: item\ndata: x\n\n']), () => sse(['event: item\ndata: y\n\n'], true)])
    const stream = t.open()
    await vi.waitFor(() => expect(t.events).toHaveLength(2))
    expect(t.sent[0]['Last-Event-ID']).toBeUndefined()
    expect(t.sent[1]['Last-Event-ID']).toBe('1:7')
    expect(t.states).toEqual(['connecting', 'open', 'reconnecting', 'open'])
    stream.close()
  })

  it('resumes from a given id', async () => {
    const t = setup([() => sse([], true)])
    const stream = t.open('3:9')
    await vi.waitFor(() => expect(t.sent).toHaveLength(1))
    expect(t.sent[0]['Last-Event-ID']).toBe('3:9')
    stream.close()
  })

  it('hands resync_required to the consumer, with or without data', async () => {
    const t = setup([() => sse(['event: resync_required\n\nevent: resync_required\ndata: {}\n\n'], true)])
    const stream = t.open()
    await vi.waitFor(() => expect(t.onResync).toHaveBeenCalledTimes(2))
    expect(t.events).toHaveLength(0)
    stream.close()
  })

  it('backs off on server and network errors, doubling up to 30 s', async () => {
    const failures: (() => Response)[] = []
    for (let i = 0; i < 7; i++) failures.push(() => new Response(null, { status: 503 }))
    failures.push(() => {
      throw new TypeError('network down')
    })
    const t = setup(failures)
    const stream = t.open()
    await vi.waitFor(() => expect(t.sleeps).toHaveLength(8))
    expect(t.sleeps).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000])
    stream.close()
  })

  it('ends on a 401, sending the browser to sign in', async () => {
    const t = setup([
      () => new Response(JSON.stringify({ code: 'unauthenticated', message: 'm' }), { status: 401 }),
    ])
    t.open()
    await vi.waitFor(() => expect(t.states).toContain('closed'))
    expect(t.navigate).toHaveBeenCalledWith('/login?next=%2Fsessions%2Fs-1')
    expect(t.sent).toHaveLength(1)
  })

  it('ends on another 4xx, with the status', async () => {
    const t = setup([() => new Response(null, { status: 404 })])
    t.open()
    await vi.waitFor(() => expect(t.onError).toHaveBeenCalledWith(404))
    expect(t.states.at(-1)).toBe('closed')
    expect(t.sent).toHaveLength(1)
  })

  it('stops when closed', async () => {
    const t = setup([() => sse([], true)])
    const stream = t.open()
    await vi.waitFor(() => expect(t.states).toContain('open'))
    stream.close()
    await vi.waitFor(() => expect(t.states.at(-1)).toBe('closed'))
    expect(t.sent).toHaveLength(1)
  })
})
