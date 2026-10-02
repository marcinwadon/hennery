// Test helpers for the stores: a `fetch` that answers by path and records
// every call with its headers, and SSE bodies a test writes to as it goes.
import { createElement, type ReactNode } from 'react'
import { vi } from 'vitest'
import { Client } from './api/client'
import { ClientContext } from './app-client'

export interface Call {
  method: string
  path: string
  headers: Record<string, string>
}

export function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

/** An SSE body that stays open until `end()`. */
export interface LiveStream {
  response: Response
  /** Write raw SSE text. */
  send(text: string): void
  /** One message: `event`, `data` (JSON unless a string), and an `id`. */
  event(event: string, data: unknown, id?: string): void
  end(): void
  cancelled: boolean
}

export function liveStream(): LiveStream {
  const encoder = new TextEncoder()
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const live: LiveStream = {
    cancelled: false,
    response: new Response(
      new ReadableStream<Uint8Array>({
        start(c) {
          controller = c
        },
        cancel() {
          live.cancelled = true
        },
      }),
      { status: 200, headers: { 'Content-Type': 'text/event-stream' } },
    ),
    send(text) {
      if (!live.cancelled) controller.enqueue(encoder.encode(text))
    },
    event(event, data, id) {
      const text = typeof data === 'string' ? data : JSON.stringify(data)
      live.send(`event: ${event}\ndata: ${text}\n${id === undefined ? '' : `id: ${id}\n`}\n`)
    },
    end() {
      if (!live.cancelled) controller.close()
    },
  }
  return live
}

/** A client over a `fetch` that `answer` serves; `calls` records each. */
export function routed(answer: (call: Call) => Response | Promise<Response>) {
  const calls: Call[] = []
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const call = {
      method: init?.method ?? 'GET',
      path: String(input),
      headers: { ...(init?.headers as Record<string, string>) },
    }
    calls.push(call)
    return answer(call)
  })
  const client = new Client({
    fetch: fetch as unknown as typeof globalThis.fetch,
    navigate: vi.fn(),
    here: () => ({ pathname: '/sessions', search: '' }),
    stepUp: async () => {},
  })
  const wrapper = ({ children }: { children: ReactNode }) => createElement(ClientContext.Provider, { value: client }, children)
  return { calls, fetch, client, wrapper }
}
