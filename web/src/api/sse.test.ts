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
  const open = (lastEventId?: string, onEvent?: (e: StreamEvent) => void) =>
    openStream(client, '/api/stream/x', {
      onEvent: onEvent ?? ((e) => events.push(e)),
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

  it('starts each wait at 1 s again once a connection opens', async () => {
    const t = setup([
      () => new Response(null, { status: 503 }),
      () => new Response(null, { status: 503 }),
      () => sse([]),
      () => new Response(null, { status: 503 }),
    ])
    const stream = t.open()
    await vi.waitFor(() => expect(t.sleeps).toHaveLength(4))
    expect(t.sleeps).toEqual([1000, 2000, 1000, 2000])
    stream.close()
  })

  it('ignores an id holding NUL, keeping the last good one', async () => {
    const t = setup([() => sse(['id: 1:2\ndata: a\n\nid: 1:3\u0000x\ndata: b\n\n'], true)])
    const stream = t.open()
    await vi.waitFor(() => expect(t.events).toHaveLength(2))
    expect(t.events[1].id).toBeUndefined()
    expect(stream.lastEventId()).toBe('1:2')
    stream.close()
  })

  it('ends a line at a lone CR, across chunks and at the end of the stream', async () => {
    const t = setup([() => sse(['event: item\rdata: a\r', '\rdata: b\r\r'])])
    const stream = t.open()
    await vi.waitFor(() => expect(t.events).toHaveLength(2))
    expect(t.events[0]).toEqual({ event: 'item', data: 'a', id: undefined })
    expect(t.events[1]).toEqual({ event: 'message', data: 'b', id: undefined })
    stream.close()
  })

  it('keeps the id of a block with no data, which dispatches nothing', async () => {
    const t = setup([() => sse(['id: 2:4\n\n', 'event: item\nid: 2:5\n\n'], true)])
    const stream = t.open('2:1')
    await vi.waitFor(() => expect(stream.lastEventId()).toBe('2:5'))
    expect(t.events).toHaveLength(0)
    stream.close()
  })

  it('an empty id clears it, so no Last-Event-ID is sent', async () => {
    const t = setup([() => sse(['id\n\n']), () => sse([], true)])
    const stream = t.open('2:1')
    await vi.waitFor(() => expect(t.sent).toHaveLength(2))
    expect(t.sent[1]['Last-Event-ID']).toBeUndefined()
    stream.close()
  })

  it('a handler that throws stops the burst, keeps the last good id and asks for a resync', async () => {
    const t = setup([
      () =>
        sse(
          [
            'event: item\ndata: a\nid: 1:6\n\n',
            'event: item\ndata: b\n\nevent: item\ndata: c\n\nevent: item\ndata: d\nid: 1:9\n\n',
          ],
          true,
        ),
      () => sse([], true),
    ])
    const seen: string[] = []
    const stream = t.open(undefined, (e) => {
      seen.push(e.data)
      if (e.data === 'b') throw new Error('boom')
    })
    await vi.waitFor(() => expect(t.onResync).toHaveBeenCalledTimes(1))
    expect(seen).toEqual(['a', 'b'])
    expect(stream.lastEventId()).toBe('1:6')
    // Not closed by the consumer: the reconnect resumes before the event that threw.
    await vi.waitFor(() => expect(t.sent).toHaveLength(2))
    expect(t.sent[1]['Last-Event-ID']).toBe('1:6')
    stream.close()
  })

  it('dispatches nothing more once closed from inside a handler', async () => {
    const t = setup([])
    const seen: string[] = []
    const client = new Client({
      fetch: (async () =>
        sse(['event: session_removed\ndata: {}\n\nevent: item\ndata: x\nid: 1:2\n\n'], true)) as unknown as typeof fetch,
      navigate: vi.fn(),
      here: () => ({ pathname: '/', search: '' }),
      stepUp: async () => {},
    })
    const stream: { s?: ReturnType<typeof openStream> } = {}
    stream.s = openStream(client, '/x', {
      lastEventId: '1:1',
      onEvent: (e) => {
        seen.push(e.event)
        if (e.event === 'session_removed') stream.s?.close()
      },
      onState: (s) => t.states.push(s),
    })
    await vi.waitFor(() => expect(t.states).toContain('closed'))
    await new Promise((r) => setTimeout(r, 10))
    expect(seen).toEqual(['session_removed'])
    expect(stream.s.lastEventId()).toBe('1:1')
  })

  it('reports closed at once when closed during a wait, and connects no more', async () => {
    const answers = [() => new Response(null, { status: 503 })]
    let calls = 0
    const states: StreamState[] = []
    let wake: () => void = () => {}
    const client = new Client({
      fetch: (async () => {
        calls++
        return answers.shift()?.() ?? sse([], true)
      }) as unknown as typeof fetch,
      navigate: vi.fn(),
      here: () => ({ pathname: '/', search: '' }),
      stepUp: async () => {},
    })
    const stream = openStream(client, '/x', {
      onEvent: () => {},
      onState: (s) => states.push(s),
      sleep: () => new Promise<void>((resolve) => (wake = resolve)),
    })
    await vi.waitFor(() => expect(states.at(-1)).toBe('reconnecting'))
    stream.close()
    expect(states.at(-1)).toBe('closed')
    wake()
    await new Promise((r) => setTimeout(r, 10))
    expect(calls).toBe(1)
    expect(states).toEqual(['connecting', 'reconnecting', 'closed'])
  })

  it('the default wait ends when the stream is closed', async () => {
    vi.useFakeTimers()
    try {
      const states: StreamState[] = []
      let calls = 0
      const client = new Client({
        fetch: (async () => {
          calls++
          return new Response(null, { status: 503 })
        }) as unknown as typeof fetch,
        navigate: vi.fn(),
        here: () => ({ pathname: '/', search: '' }),
        stepUp: async () => {},
      })
      const stream = openStream(client, '/x', { onEvent: () => {}, onState: (s) => states.push(s) })
      await vi.waitFor(() => expect(states.at(-1)).toBe('reconnecting'))
      expect(vi.getTimerCount()).toBe(1)
      stream.close()
      // At once: no timer is left to fire.
      expect(vi.getTimerCount()).toBe(0)
      expect(calls).toBe(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('cancels the body of an answer it does not read', async () => {
    const cancelled = vi.fn()
    const failing = (status: number) =>
      new Response(new ReadableStream({ cancel: cancelled }), { status })
    const t = setup([() => failing(503), () => failing(404)])
    t.open()
    await vi.waitFor(() => expect(t.onError).toHaveBeenCalledWith(404))
    await vi.waitFor(() => expect(cancelled).toHaveBeenCalledTimes(2))
  })

  it('removes its abort listener after each connection', async () => {
    const added: unknown[] = []
    const removed: unknown[] = []
    // The prototype that owns a signal's listener methods (jsdom's or Node's).
    let owner: object = new AbortController().signal
    while (!Object.prototype.hasOwnProperty.call(owner, 'addEventListener')) owner = Object.getPrototypeOf(owner)
    const target = owner as EventTarget
    const realAdd = target.addEventListener
    const realRemove = target.removeEventListener
    const addSpy = vi.spyOn(target, 'addEventListener').mockImplementation(function (
      this: EventTarget,
      type: string,
      listener: EventListenerOrEventListenerObject | null,
      opts?: boolean | AddEventListenerOptions,
    ) {
      if (type === 'abort') added.push(listener)
      return realAdd.call(this, type, listener, opts)
    })
    const removeSpy = vi.spyOn(target, 'removeEventListener').mockImplementation(function (
      this: EventTarget,
      type: string,
      listener: EventListenerOrEventListenerObject | null,
      opts?: boolean | EventListenerOptions,
    ) {
      if (type === 'abort') removed.push(listener)
      return realRemove.call(this, type, listener, opts)
    })
    try {
      const t = setup([() => sse(['data: a\n\n']), () => sse(['data: b\n\n']), () => sse(['data: c\n\n'])])
      const stream = t.open()
      await vi.waitFor(() => expect(t.sleeps).toHaveLength(3))
      stream.close()
      expect(added.length).toBeGreaterThanOrEqual(3)
      expect(added.filter((l) => !removed.includes(l))).toEqual([])
    } finally {
      addSpy.mockRestore()
      removeSpy.mockRestore()
    }
  })

  it('closed while connecting: reports nothing of the answer that comes after', async () => {
    let answer: (r: Response) => void = () => {}
    const states: StreamState[] = []
    const onError = vi.fn()
    const client = new Client({
      fetch: (() => new Promise<Response>((r) => (answer = r))) as unknown as typeof fetch,
      navigate: vi.fn(),
      here: () => ({ pathname: '/', search: '' }),
      stepUp: async () => {},
    })
    const stream = openStream(client, '/x', { onEvent: () => {}, onState: (s) => states.push(s), onError })
    await vi.waitFor(() => expect(states).toEqual(['connecting']))
    stream.close()
    answer(new Response(null, { status: 404 }))
    await new Promise((r) => setTimeout(r, 10))
    expect(states).toEqual(['connecting', 'closed'])
    expect(onError).not.toHaveBeenCalled()
  })
})
