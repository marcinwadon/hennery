import { describe, expect, it, vi } from 'vitest'
import { Client, type ClientDeps } from './client'
import { ApiFailure, StepUpCancelled, Unauthenticated } from './errors'

function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  })
}

const STEP_UP = { code: 'step_up_required', message: 'confirm' }

/** A client over a stub server that answers each request with the next of
 *  `answers`, recording what was sent. */
function stub(answers: Response[], deps: Partial<ClientDeps> = {}) {
  const sent: { method: string; path: string; body?: string; headers: Record<string, string> }[] = []
  const fetch = vi.fn(async (path: RequestInfo | URL, init?: RequestInit) => {
    sent.push({
      method: init?.method ?? 'GET',
      path: String(path),
      body: init?.body as string | undefined,
      headers: init?.headers as Record<string, string>,
    })
    const next = answers.shift()
    if (!next) throw new Error('no answer left')
    return next
  })
  const navigate = vi.fn()
  const stepUp = vi.fn(async () => {})
  const client = new Client({
    fetch: fetch as unknown as typeof globalThis.fetch,
    navigate,
    here: () => ({ pathname: '/hosts', search: '?x=1' }),
    stepUp,
    ...deps,
  })
  return { client, sent, navigate, stepUp, fetch }
}

describe('request', () => {
  it('sends JSON with the cookie, and reads JSON back', async () => {
    const { client, sent } = stub([json(200, { a: 1 })])
    await expect(client.request('POST', '/api/x', { b: 2 })).resolves.toEqual({ a: 1 })
    expect(sent[0]).toMatchObject({ method: 'POST', path: '/api/x', body: '{"b":2}' })
    expect(sent[0].headers['Content-Type']).toBe('application/json')
  })

  it('sends no body and no content type without a body, and reads 204 as nothing', async () => {
    const { client, sent } = stub([new Response(null, { status: 204 })])
    await expect(client.request('POST', '/api/auth/logout')).resolves.toBeUndefined()
    expect(sent[0].body).toBeUndefined()
    expect(sent[0].headers['Content-Type']).toBeUndefined()
  })

  it('leaves fetch in its default cors mode, so a POST carries Origin', async () => {
    const { client, fetch } = stub([json(200, {})])
    await client.request('POST', '/api/x', {})
    const init = fetch.mock.calls[0][1] as RequestInit
    expect(init.mode).toBeUndefined()
    expect(init.credentials).toBe('same-origin')
  })

  it('answers a refusal with its code and a plain message', async () => {
    const { client } = stub([json(409, { code: 'host_offline', message: 'server text' })])
    const err = await client.request('POST', '/api/x').catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiFailure)
    expect(err).toMatchObject({ status: 409, code: 'host_offline', message: 'The host is offline.' })
  })

  it('shows the server’s message for a code it does not know', async () => {
    const { client } = stub([json(409, { code: 'brand_new', message: 'server text' })])
    await expect(client.request('POST', '/api/x')).rejects.toMatchObject({ code: 'brand_new', message: 'server text' })
  })

  it('reads Retry-After on a 429', async () => {
    const { client } = stub([json(429, { code: 'rate_limited', message: 'm' }, { 'Retry-After': '42' })])
    await expect(client.request('POST', '/api/auth/login', {})).rejects.toMatchObject({ retryAfter: 42 })
  })
})

describe('401', () => {
  it('sends the browser to sign in, returning to the path and query, never the fragment', async () => {
    const { client, navigate } = stub([json(401, { code: 'unauthenticated', message: 'sign in' })])
    await expect(client.request('GET', '/api/hosts')).rejects.toBeInstanceOf(Unauthenticated)
    expect(navigate).toHaveBeenCalledWith('/login?next=%2Fhosts%3Fx%3D1')
  })

  it('is only an error on the sign-in screens', async () => {
    for (const pathname of ['/login', '/setup']) {
      const { client, navigate } = stub([json(401, { code: 'unauthenticated', message: 'm' })], {
        here: () => ({ pathname, search: '' }),
      })
      await expect(client.request('GET', '/api/capabilities')).rejects.toBeInstanceOf(ApiFailure)
      expect(navigate).not.toHaveBeenCalled()
    }
  })

  it('is an error, not a redirect, for any other 401 code', async () => {
    const { client, navigate } = stub([json(401, { code: 'invalid_password', message: 'm' })])
    await expect(client.request('POST', '/api/auth/login', {})).rejects.toMatchObject({ code: 'invalid_password' })
    expect(navigate).not.toHaveBeenCalled()
  })
})

describe('403 step_up_required', () => {
  it('steps up, then sends the same request once more', async () => {
    const { client, sent, stepUp } = stub([json(403, STEP_UP), json(201, { id: 'p' })])
    await expect(client.request('POST', '/api/hosts/pairing-codes', { a: 1 })).resolves.toEqual({ id: 'p' })
    expect(stepUp).toHaveBeenCalledTimes(1)
    expect(sent).toHaveLength(2)
    expect(sent[1]).toMatchObject({ method: 'POST', path: '/api/hosts/pairing-codes', body: '{"a":1}' })
  })

  it('never sends it a third time, nor opens a second dialog', async () => {
    const { client, sent, stepUp } = stub([json(403, STEP_UP), json(403, STEP_UP)])
    await expect(client.request('DELETE', '/api/hosts/h-1')).rejects.toMatchObject({ code: 'step_up_required' })
    expect(stepUp).toHaveBeenCalledTimes(1)
    expect(sent).toHaveLength(2)
  })

  it('sends nothing again when the dialog is dismissed', async () => {
    const { client, sent } = stub([json(403, STEP_UP)], {
      stepUp: async () => {
        throw new StepUpCancelled()
      },
    })
    await expect(client.request('DELETE', '/api/hosts/h-1')).rejects.toBeInstanceOf(StepUpCancelled)
    expect(sent).toHaveLength(1)
  })

  it('opens one dialog for requests refused together', async () => {
    let release!: () => void
    const stepUp = vi.fn(() => new Promise<void>((resolve) => (release = resolve)))
    const { client, sent } = stub([json(403, STEP_UP), json(403, STEP_UP), json(204, undefined), json(204, undefined)], {
      stepUp,
    })
    const both = Promise.all([client.request('DELETE', '/api/a'), client.request('DELETE', '/api/b')])
    await vi.waitFor(() => expect(stepUp).toHaveBeenCalled())
    await new Promise((r) => setTimeout(r, 0))
    release()
    await both
    expect(stepUp).toHaveBeenCalledTimes(1)
    expect(sent).toHaveLength(4)
  })

  it('is a plain error on the step-up routes themselves', async () => {
    const { client, stepUp, sent } = stub([json(403, STEP_UP)])
    await expect(
      client.request('POST', '/api/auth/step-up/password', { password: 'x' }, { stepUp: false }),
    ).rejects.toMatchObject({ code: 'step_up_required' })
    expect(stepUp).not.toHaveBeenCalled()
    expect(sent).toHaveLength(1)
  })

  it('is not opened by another 403', async () => {
    const { client, stepUp } = stub([json(403, { code: 'origin_mismatch', message: 'm' })])
    await expect(client.request('POST', '/api/x')).rejects.toMatchObject({ code: 'origin_mismatch' })
    expect(stepUp).not.toHaveBeenCalled()
  })
})
