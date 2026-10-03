// Server-sent events over `fetch` (client view spec §4.1, §6): `EventSource`
// can neither see a 401 nor send `Last-Event-ID` on a reconnect of its own
// making, and both are needed.
//
// - A lost stream shows "Reconnecting…" and comes back with `Last-Event-ID`
//   set to the last id seen, so the server sends only what changed since.
// - `resync_required` asks the consumer to refetch its first page. The
//   server ends the stream after it: the consumer closes this stream at
//   once, inside `onResync`, and opens a new one from the new page's anchor
//   (close and reopen; the helper has no way to reset its id).
// - A handler that throws is a resync too: the helper stops reading that
//   connection, keeps the id of the last event handled whole, and calls
//   `onResync`. If the consumer does not close, the reconnect resumes from
//   that id, so the event that threw is sent again, never lost. Consumers
//   are not meant to throw: they parse in try/catch and resync on a
//   malformed message themselves.
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
  /** The server can no longer fill the gap, or a handler threw: close this
   *  stream, refetch the first page and open a new one. */
  onResync?: () => void
  onState?: (state: StreamState) => void
  /** The stream ended for good (a 4xx other than 401). */
  onError?: (status: number) => void
  /** `Last-Event-ID` for the first connection, when resuming. */
  lastEventId?: string
  /** Waits between attempts; tests replace it. */
  sleep?: (ms: number, signal: AbortSignal) => Promise<void>
}

export interface Stream {
  close(): void
  lastEventId(): string | undefined
}

const FIRST_DELAY_MS = 1000
const MAX_DELAY_MS = 30000

/** `setTimeout` as a promise that also settles when `signal` aborts, so a
 *  closed stream leaves no timer behind. */
function abortableSleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise<void>((resolve) => {
    if (signal.aborted) return resolve()
    const done = () => {
      clearTimeout(timer)
      signal.removeEventListener('abort', done)
      resolve()
    }
    const timer = setTimeout(done, ms)
    signal.addEventListener('abort', done, { once: true })
  })
}

export function openStream(client: Client, path: string, options: StreamOptions): Stream {
  const abort = new AbortController()
  const sleep = options.sleep ?? abortableSleep
  let lastId = options.lastEventId === '' ? undefined : options.lastEventId
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
        // Closed while connecting: nothing of this answer is reported.
        if (abort.signal.aborted) {
          void response.body?.cancel().catch(() => {})
          break
        }
        if (response.ok && response.body) {
          set('open')
          delay = FIRST_DELAY_MS
          const threw = await read(response.body, abort.signal, (event) => {
            if (event.event === 'resync_required') {
              options.onResync?.()
              return
            }
            if (event.data !== undefined) options.onEvent(event as StreamEvent)
          }, (id) => {
            lastId = id
          })
          if (threw && !abort.signal.aborted) options.onResync?.()
        } else {
          // Nobody reads it: give the connection back.
          void response.body?.cancel().catch(() => {})
          if (status >= 400 && status < 500) {
            set('closed')
            options.onError?.(status)
            return
          }
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
      await sleep(delay, abort.signal)
      delay = Math.min(delay * 2, MAX_DELAY_MS)
    }
    set('closed')
  }
  void run()

  return {
    close: () => {
      abort.abort()
      set('closed')
    },
    lastEventId: () => lastId,
  }
}

interface Block {
  event: string
  data?: string
  id?: string
}

/** Read `body` to its end, calling `dispatch` per event (the SSE format:
 *  `event:`, `data:` lines joined by newlines, `id:`; `:` comments; a line
 *  ends at CRLF, LF or a lone CR). An event with no data dispatches nothing,
 *  as the format says, except `resync_required`, which needs none; its `id`
 *  still counts. `commit` gets each block's id once its handler returned
 *  (an empty id clears it). Whether a handler threw: the reading stops
 *  there, and no later id is committed. */
async function read(
  body: ReadableStream<Uint8Array>,
  signal: AbortSignal,
  dispatch: (event: Block) => void,
  commit: (id: string | undefined) => void,
): Promise<boolean> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  const cancel = () => void reader.cancel().catch(() => {})
  signal.addEventListener('abort', cancel, { once: true })
  let buffer = ''
  let event = ''
  let data: string[] = []
  let id: string | undefined
  let hasId = false

  /** One line; `false` once a handler threw. */
  const line = (text: string): boolean => {
    if (text === '') {
      const block: Block = { event: event || 'message', id }
      if (data.length > 0) block.data = data.join('\n')
      const takesId = hasId
      const blockId = id
      event = ''
      data = []
      id = undefined
      hasId = false
      if (signal.aborted) return true
      if (block.data !== undefined || block.event === 'resync_required') {
        try {
          dispatch(block)
        } catch {
          return false
        }
      }
      if (takesId) commit(blockId === '' ? undefined : blockId)
      return true
    }
    if (text.startsWith(':')) return true
    const colon = text.indexOf(':')
    const field = colon < 0 ? text : text.slice(0, colon)
    let value = colon < 0 ? '' : text.slice(colon + 1)
    if (value.startsWith(' ')) value = value.slice(1)
    if (field === 'event') event = value
    else if (field === 'data') data.push(value)
    else if (field === 'id' && !value.includes('\0')) {
      id = value
      hasId = true
    }
    return true
  }

  /** Every whole line in the buffer; at the end, a trailing CR ends one. */
  const lines = (atEnd: boolean): boolean => {
    let newline: number
    while ((newline = buffer.search(/\r\n|\r|\n/)) >= 0) {
      // A `\r` that ends the chunk may be the first half of `\r\n`.
      if (!atEnd && newline === buffer.length - 1 && buffer[newline] === '\r') break
      const text = buffer.slice(0, newline)
      buffer = buffer.slice(newline + (buffer.startsWith('\r\n', newline) ? 2 : 1))
      if (!line(text)) return false
    }
    return true
  }

  try {
    for (;;) {
      const { value, done } = await reader.read()
      if (done) {
        buffer += decoder.decode()
        // An event not ended by an empty line is dropped, as the format says.
        return !lines(true)
      }
      buffer += decoder.decode(value, { stream: true })
      if (!lines(false)) {
        cancel()
        return true
      }
    }
  } finally {
    signal.removeEventListener('abort', cancel)
  }
}
