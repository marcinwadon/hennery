// A test server for the session screen with its composer: any session id,
// its page, stream, detail and catalogue, the hosts, an undelivered turn
// and its images, the composer's and the cards' POSTs, recorded with their
// bodies.
import type { SessionCatalog, SessionDetail } from '../generated/protocol'
import type { Item, TurnContent } from '../generated/view'
import { json, liveStream, routed, type LiveStream } from '../test-stream'

export function message(id: string, turn: string, text = id): Item {
  return { id, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: turn, kind: 'message', text } as Item
}

export function catalogOf(id: string, model = 'a'): SessionCatalog {
  return {
    session_id: id,
    config_options: [
      {
        id: 'model',
        name: 'Model',
        category: 'model',
        type: 'select',
        currentValue: model,
        options: ['a', 'b', 'c'].map((v) => ({ value: v, name: v.toUpperCase() })),
      },
    ],
    commands: [],
  } as SessionCatalog
}

function detail(id: string, patch: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session_id: id,
    host_id: 'h1',
    agent: 'claude',
    cwd: '/srv/work/project',
    hat_id: 'hat1',
    lifecycle: 'active',
    activity: 'idle',
    presumed_parked: false,
    created_at: '2026-10-02T09:00:00.000Z',
    last_event_at: '2026-10-02T10:00:00.000Z',
    pending: [],
    ...patch,
  } as SessionDetail
}

export interface Opts {
  /** The items of a session's first page; none by default. */
  items?: (id: string) => Item[]
  /** `GET /api/hosts`. */
  hosts?: () => Response
  /** `GET …/turns/{turn}`. */
  turn?: () => Response | Promise<Response>
  /** `GET /api/attachments/{sha256}`. */
  attachment?: () => Response
  /** `POST …/config`. */
  config?: (id: string) => Response
  /** `GET /api/view/sessions/{id}`, before `items`. */
  page?: (id: string) => Response | undefined
  /** What a session's detail holds besides an idle, active session. */
  detail?: Partial<SessionDetail>
  /** `POST …/pending/{pending_id}/answer`; 202 by default. */
  answer?: () => Response
}

export function sessionServer(opts: Opts = {}) {
  const streams: LiveStream[] = []
  const t = routed((call) => {
    const path = new URL(call.path, 'http://h').pathname
    let m: RegExpMatchArray | null
    if (path === '/api/hosts') return opts.hosts?.() ?? json([{ host_id: 'h1', name: 'build-box', capabilities: ['images'] }])
    if (path.startsWith('/api/attachments/')) return opts.attachment?.() ?? json({ code: 'not_found', message: 'no' }, 404)
    if ((m = path.match(/^\/api\/view\/sessions\/([^/]+)\/turns\/[^/]+$/))) {
      return opts.turn?.() ?? json({ code: 'not_found', message: 'no' }, 404)
    }
    if ((m = path.match(/^\/api\/view\/sessions\/([^/]+)$/))) {
      const id = decodeURIComponent(m[1])
      return opts.page?.(id) ?? json({ items: opts.items?.(id) ?? [], older: false, epoch: 'e1', revision: 1 })
    }
    if (path.startsWith('/api/stream/view/sessions/')) {
      const live = liveStream()
      streams.push(live)
      return live.response
    }
    if ((m = path.match(/^\/api\/sessions\/[^/]+\/pending\/([^/]+)\/answer$/))) {
      return opts.answer?.() ?? json({ pending_id: decodeURIComponent(m[1]), request_id: 'r1' }, 202)
    }
    if ((m = path.match(/^\/api\/sessions\/([^/]+)\/catalog$/))) return json(catalogOf(decodeURIComponent(m[1])))
    if ((m = path.match(/^\/api\/sessions\/([^/]+)\/prompt$/))) return json({ turn_id: 'new' }, 202)
    if ((m = path.match(/^\/api\/sessions\/([^/]+)\/config$/))) {
      const id = decodeURIComponent(m[1])
      return opts.config?.(id) ?? json(catalogOf(id), 202)
    }
    if ((m = path.match(/^\/api\/sessions\/([^/]+)$/))) return json(detail(decodeURIComponent(m[1]), opts.detail))
    return json({ code: 'not_found', message: 'no' }, 404)
  })
  /** The POSTs to paths ending in `suffix`, with their bodies. */
  const posted = (suffix: string) =>
    t.fetch.mock.calls
      .filter((c) => c[1]?.method === 'POST' && String(c[0]).endsWith(suffix))
      .map((c) => ({ path: String(c[0]), body: JSON.parse(String(c[1]?.body)) as unknown }))
  const of = (path: string) => t.calls.filter((c) => new URL(c.path, 'http://h').pathname === path)
  return { ...t, streams, posted, of }
}

export const FAST = { retryMs: () => 5, resyncedMs: 300 }

/** A turn the host never started, with a picture. */
export const UNDELIVERED: TurnContent = {
  turn_id: 't9',
  content: [
    { type: 'text', text: 'look at' },
    { type: 'image', mimeType: 'image/png', sha256: 'ab/c', size: 4 },
    { type: 'text', text: 'and fix it' },
  ],
}
