// Server-sent events over `fetch` (client view spec §4.1, §6): `EventSource`
// can neither see a 401 nor send `Last-Event-ID` on a reconnect of its own
// making, and both are needed.
//
// - A lost stream shows "Reconnecting…" and comes back with `Last-Event-ID`
//   set to the last id seen, so the server sends only what changed since.
// - `resync_required` asks the consumer to refetch its first page.
// - 401: to the login screen (the client's rule), and the stream ends.
// - Any other 4xx: the stream ends with an error; a 5xx or a network error
//   is retried, waiting 1 s, then twice as long each time, up to 30 s.
import type { Client } from './client'

export type StreamState = 'connecting' | 'open' | 'reconnecting' | 'closed'

export interface StreamEvent {
  event: string
  data: string
  id?: string
}

export interface StreamOptions {
  onEvent: (event: StreamEvent) => void
  /** The server can no longer fill the gap: refetch the first page. */
  onResync?: () => void
  onState?: (state: StreamState) => void
  /** The stream ended for good (a 4xx other than 401). */
  onError?: (status: number) => void
  /** `Last-Event-ID` for the first connection, when resuming. */
  lastEventId?: string
  /** Waits between attempts; tests replace it. */
  sleep?: (ms: number) => Promise<void>
}

export interface Stream {
  close(): void
  lastEventId(): string | undefined
}

const FIRST_DELAY_MS = 1000
const MAX_DELAY_MS = 30000

export function openStream(client: Client, path: string, options: StreamOptions): Stream {
  const abort = new AbortController()
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)))
  let lastId = options.lastEventId
  let state: StreamState | undefined
  const set = (next: StreamState) => {
    if (next !== state) {
      state = next
      options.onState?.(next)
    }
  }

  const run = async () => {
    let delay = FIRST_DELAY_MS
    set('connecting')
    while (!abort.signal.aborted) {
      let status = 0
      try {
        const headers: Record<string, string> = { Accept: 'text/event-stream' }
        if (lastId !== undefined) headers['Last-Event-ID'] = lastId
        const response = await client.open(path, headers, abort.signal)
        status = response.status
        if (response.ok && response.body) {
          set('open')
          delay = FIRST_DELAY_MS
          await read(response.body, abort.signal, (event) => {
            if (event.id !== undefined) lastId = event.id
            if (event.event === 'resync_required') options.onResync?.()
            else options.onEvent(event)
          })
        } else if (status >= 400 && status < 500) {
          set('closed')
          options.onError?.(status)
          return
        }
      } catch (err) {
        // A 401 (the client navigated away) or `close()`: the stream ends.
        if (abort.signal.aborted || (err instanceof Error && err.name === 'Unauthenticated')) {
          set('closed')
          return
        }
      }
      if (abort.signal.aborted) break
      set('reconnecting')
      await sleep(delay)
      delay = Math.min(delay * 2, MAX_DELAY_MS)
    }
    set('closed')
  }
  void run()

  return {
    close: () => abort.abort(),
    lastEventId: () => lastId,
  }
}

/** Read `body` to its end, calling `dispatch` per event (the SSE format:
 *  `event:`, `data:` lines joined by newlines, `id:`; `:` comments). An
 *  event with no data is dropped, as the format says, except
 *  `resync_required`, which needs none. */
async function read(
  body: ReadableStream<Uint8Array>,
  signal: AbortSignal,
  dispatch: (event: StreamEvent) => void,
): Promise<void> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  signal.addEventListener('abort', () => void reader.cancel().catch(() => {}), { once: true })
  let buffer = ''
  let event = ''
  let data: string[] = []
  let id: string | undefined
  for (;;) {
    const { value, done } = await reader.read()
    if (done || signal.aborted) return
    buffer += decoder.decode(value, { stream: true })
    let newline: number
    while ((newline = buffer.search(/\r\n|\r|\n/)) >= 0) {
      // A `\r` that ends the chunk may be the first half of `\r\n`.
      if (newline === buffer.length - 1 && buffer[newline] === '\r') break
      const line = buffer.slice(0, newline)
      buffer = buffer.slice(newline + (buffer.startsWith('\r\n', newline) ? 2 : 1))
      if (line === '') {
        if (data.length > 0 || event === 'resync_required') dispatch({ event: event || 'message', data: data.join('\n'), id })
        event = ''
        data = []
        id = undefined
        continue
      }
      if (line.startsWith(':')) continue
      const colon = line.indexOf(':')
      const field = colon < 0 ? line : line.slice(0, colon)
      let value = colon < 0 ? '' : line.slice(colon + 1)
      if (value.startsWith(' ')) value = value.slice(1)
      if (field === 'event') event = value
      else if (field === 'data') data.push(value)
      else if (field === 'id' && !value.includes('\0')) id = value
    }
  }
}
