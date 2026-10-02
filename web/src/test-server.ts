// A stub server for component tests: answers `fetch` by method and path,
// and records what was sent.
import { vi } from 'vitest'

export type Answer = Response | (() => Response | Promise<Response>)

export interface Sent {
  method: string
  path: string
  body: unknown
}

export function json(status: number, body?: unknown, headers: Record<string, string> = {}): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  })
}

/** `routes` maps `"METHOD /path"` to an answer, or to a list of answers
 *  given in turn (the last one repeats). */
export function stubServer(routes: Record<string, Answer | Answer[]>) {
  const sent: Sent[] = []
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const method = init?.method ?? 'GET'
    const path = String(input)
    const body = typeof init?.body === 'string' ? JSON.parse(init.body) : undefined
    sent.push({ method, path, body })
    const key = `${method} ${path}`
    const route = routes[key]
    if (route === undefined) return json(404, { code: 'not_found', message: key })
    const answer = Array.isArray(route) ? (route.length > 1 ? route.shift()! : route[0]) : route
    return typeof answer === 'function' ? answer() : answer.clone()
  })
  return { fetch: fetch as unknown as typeof globalThis.fetch, sent }
}

export const FULL = { mode: 'full', features: [] }
export const GATEWAY = { mode: 'gateway', features: [] }
