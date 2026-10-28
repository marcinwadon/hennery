import { afterEach, describe, expect, it, vi } from 'vitest'
import { Client } from '../api/client'
import { json, routed } from '../test-stream'
import { BulkCloseController, CLOSE_TIMEOUT_MS, type BulkCloseTarget } from './bulkClose'

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const target = (id: string, lifecycle = 'active'): BulkCloseTarget => ({ id, label: `Session ${id}`, lifecycle })
const closed = (id: string) => json({ session_id: id, lifecycle: 'closed' }, 202)
const failure = (code: string, status = 409) => json({ code }, status)
const flush = async () => { for (let i = 0; i < 30; i++) await Promise.resolve() }
const states = (runner: BulkCloseController) => Object.fromEntries(runner.getSnapshot().results.map(result => [result.target.id, result.status]))
afterEach(() => vi.useRealTimers())

describe('explicit bulk Close', () => {
  it('freezes and deduplicates only supplied IDs, caps dispatch at three and keeps results in selection order', async () => {
    const requests = new Map<string, ReturnType<typeof deferred<Response>>>()
    const t = routed(call => { const d = deferred<Response>(); requests.set(call.path, d); return d.promise })
    const runner = new BulkCloseController(t.client)
    const selected = [target('a/b?c'), target('b'), target('c'), target('d'), target('b')]
    const run = runner.start(selected)
    selected.push(target('not-selected-at-confirmation'))
    await flush()
    expect(t.calls.map(call => call.path)).toEqual(['/api/sessions/a%2Fb%3Fc/close', '/api/sessions/b/close', '/api/sessions/c/close'])
    requests.get('/api/sessions/b/close')!.resolve(closed('b'))
    await flush()
    expect(t.calls).toHaveLength(4)
    requests.get('/api/sessions/d/close')!.resolve(closed('d'))
    requests.get('/api/sessions/c/close')!.resolve(closed('c'))
    requests.get('/api/sessions/a%2Fb%3Fc/close')!.resolve(closed('a/b?c'))
    await run
    expect(runner.getSnapshot().results.map(result => result.target.id)).toEqual(['a/b?c', 'b', 'c', 'd'])
    expect(Object.values(states(runner))).toEqual(['closed', 'closed', 'closed', 'closed'])
    expect(t.calls.every(call => call.method === 'POST')).toBe(true)
    expect(runner.getSnapshot().results[0].message).toContain('offline host may still run')
  })

  it('rejects a simultaneous submission and skips a known closed session without an HTTP request', async () => {
    const d = deferred<Response>(), t = routed(() => d.promise), runner = new BulkCloseController(t.client)
    const run = runner.start([target('already', 'closed'), target('a')])
    expect(await runner.start([target('other')])).toBe(false)
    await flush()
    expect(t.calls.map(call => call.path)).toEqual(['/api/sessions/a/close'])
    d.resolve(closed('a')); await run
    expect(states(runner)).toEqual({ already: 'already_closed', a: 'closed' })
  })

  it('retains per-ID partial outcomes, including missing and nonclosed/malformed success bodies', async () => {
    const t = routed(call => {
      if (call.path.includes('/start/')) return failure('starting')
      if (call.path.includes('/gone/')) return failure('not_found', 404)
      if (call.path.includes('/unknown/')) return failure('delivery_unknown', 503)
      if (call.path.includes('/resumed/')) return json({ session_id: 'resumed', lifecycle: 'active' }, 202)
      if (call.path.includes('/wrong/')) return closed('another-id')
      return closed('ok')
    })
    const runner = new BulkCloseController(t.client)
    await runner.start(['start', 'gone', 'unknown', 'resumed', 'wrong', 'ok'].map(id => target(id)))
    expect(states(runner)).toEqual({ start: 'failed', gone: 'unavailable', unknown: 'pending_unknown', resumed: 'pending_unknown', wrong: 'pending_unknown', ok: 'closed' })
    expect(runner.getSnapshot().results[0].message).toContain('still starting')
    expect(runner.getSnapshot().results[2].message).toContain('host is back')
  })

  it('stops unsent work after a 401 but lets already-sent requests settle', async () => {
    const d = deferred<Response>()
    const t = routed(call => call.path.includes('/a/') ? failure('unauthenticated', 401) : d.promise)
    const runner = new BulkCloseController(t.client)
    const run = runner.start(['a', 'b', 'c', 'd', 'e'].map(id => target(id)))
    await flush()
    expect(t.calls).toHaveLength(3)
    expect(states(runner)).toMatchObject({ a: 'failed', d: 'not_sent', e: 'not_sent' })
    d.resolve(json({ session_id: 'unexpected', lifecycle: 'closed' })); await run
    expect(t.calls).toHaveLength(3)
  })

  it('stops queued work on a network failure and never automatically retries an uncertain POST', async () => {
    const t = routed(call => call.path.includes('/a/') ? Promise.reject(new TypeError('Network lost')) : closed(call.path.split('/')[3]))
    const runner = new BulkCloseController(t.client)
    await runner.start(['a', 'b', 'c', 'd', 'e'].map(id => target(id)))
    expect(t.calls).toHaveLength(3)
    expect(states(runner)).toEqual({ a: 'pending_unknown', b: 'closed', c: 'closed', d: 'not_sent', e: 'not_sent' })
  })

  it('honours Retry-After without automatically sending anything when the cooldown ends', async () => {
    vi.useFakeTimers()
    const t = routed(call => call.method === 'GET' ? json({ session_id: call.path.split('/')[3], lifecycle: 'parked' })
      : new Response('{"code":"rate_limited"}', { status: 429, headers: { 'Content-Type': 'application/json', 'Retry-After': '8' } }))
    const runner = new BulkCloseController(t.client)
    await runner.start(['a', 'b', 'c', 'd'].map(id => target(id)))
    expect(states(runner).d).toBe('not_sent')
    expect(await runner.prepareRetry()).toBeNull()
    await vi.advanceTimersByTimeAsync(8_000)
    expect(t.calls).toHaveLength(3)
    const plan = await runner.prepareRetry()
    expect(plan?.targets.map(row => row.id)).toEqual(['a', 'b', 'c', 'd'])
    expect(t.calls.filter(call => call.method === 'POST')).toHaveLength(3)
  })

  it('aborts after 75 seconds (beyond host teardown) and marks only sent requests as unknown', async () => {
    vi.useFakeTimers()
    const fetch = vi.fn((_input: RequestInfo | URL, init?: RequestInit) => new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')), { once: true })
    }))
    const runner = new BulkCloseController(new Client({ fetch: fetch as typeof globalThis.fetch, navigate: vi.fn(), here: () => ({ pathname: '/sessions', search: '' }), stepUp: async () => {} }))
    const run = runner.start(['a', 'b', 'c', 'd'].map(id => target(id)))
    await vi.advanceTimersByTimeAsync(60_001)
    expect(runner.getSnapshot().busy).toBe(true)
    await vi.advanceTimersByTimeAsync(CLOSE_TIMEOUT_MS - 60_001)
    await run
    expect(states(runner)).toEqual({ a: 'pending_unknown', b: 'pending_unknown', c: 'pending_unknown', d: 'not_sent' })
    expect(fetch).toHaveBeenCalledTimes(3)
  })

  it('fences callbacks after stop and reactivation, even when the old fetch ignores abort', async () => {
    const d = deferred<Response>(), t = routed(call => call.path.includes('/old') ? d.promise : closed('new'))
    const runner = new BulkCloseController(t.client)
    const oldRun = runner.start(['old-a', 'old-b', 'old-c', 'old-d'].map(id => target(id)))
    await flush(); runner.stop()
    expect(states(runner)).toEqual({ 'old-a': 'pending_unknown', 'old-b': 'pending_unknown', 'old-c': 'pending_unknown', 'old-d': 'not_sent' })
    runner.activate(); await runner.start([target('new')])
    d.resolve(closed('old-a')); await oldRun
    expect(states(runner)).toEqual({ new: 'closed' })
    expect(t.calls).toHaveLength(4)
  })
})

describe('manual retry and reconciliation', () => {
  it('excludes a ready candidate changed by SSE while another retry read is pending', async () => {
    const b = deferred<Response>()
    const t = routed(call => call.method === 'POST' ? failure('starting') : call.path.endsWith('/a') ? json({ session_id: 'a', lifecycle: 'parked' }) : b.promise)
    const runner = new BulkCloseController(t.client)
    await runner.start([target('a'), target('b')])
    const preparing = runner.prepareRetry(); await flush()
    runner.reconcile(new Map([['a', { lifecycle: 'active' }]]))
    b.resolve(json({ session_id: 'b', lifecycle: 'parked' }))
    const plan = await preparing
    expect(plan?.targets.map(row => row.id) ?? []).not.toContain('a')
    if (plan) await runner.confirmRetry(plan)
    expect(t.calls.filter(call => call.method === 'POST' && call.path.includes('/a/'))).toHaveLength(1)
  })

  it('refuses a stale parked read which started before an authoritative active SSE', async () => {
    const a = deferred<Response>()
    const t = routed(call => call.method === 'POST' ? failure('starting') : a.promise)
    const runner = new BulkCloseController(t.client)
    await runner.start([target('a')])
    const preparing = runner.prepareRetry(); await flush()
    runner.reconcile(new Map([['a', { lifecycle: 'active' }]]))
    a.resolve(json({ session_id: 'a', lifecycle: 'parked' }))
    expect(await preparing).toBeNull()
    expect(runner.getSnapshot().results[0].target.lifecycle).toBe('active')
    expect(t.calls.filter(call => call.method === 'POST')).toHaveLength(1)
  })

  it('reads only failed/unsent IDs and requires confirmation of the fresh plan before a new POST', async () => {
    let attempt = 0
    const t = routed(call => {
      const id = call.path.split('/')[3]
      if (call.method === 'GET') return json({ session_id: id, lifecycle: id === 'failed-but-now-closed' ? 'closed' : 'parked' })
      if (id === 'success') return closed(id)
      if (id === 'pending') return failure('delivery_unknown', 503)
      if (id === 'missing') return failure('not_found', 404)
      return attempt++ < 2 ? failure('starting') : closed(id)
    })
    const runner = new BulkCloseController(t.client)
    await runner.start(['failed', 'failed-but-now-closed', 'success', 'pending', 'missing'].map(id => target(id)))
    const plan = (await runner.prepareRetry())!
    expect(t.calls.filter(call => call.method === 'GET').map(call => call.path)).toEqual(['/api/sessions/failed', '/api/sessions/failed-but-now-closed'])
    expect(plan.targets).toEqual([target('failed', 'parked')])
    expect(t.calls.filter(call => call.method === 'POST')).toHaveLength(5)
    expect(await runner.confirmRetry({ targets: plan.targets })).toBe(false)
    expect(await runner.confirmRetry(plan)).toBe(true)
    expect(t.calls.filter(call => call.method === 'POST').map(call => call.path)).toEqual([
      '/api/sessions/failed/close', '/api/sessions/failed-but-now-closed/close', '/api/sessions/success/close', '/api/sessions/pending/close', '/api/sessions/missing/close', '/api/sessions/failed/close',
    ])
    expect(await runner.confirmRetry(plan)).toBe(false)
  })

  it('checks unknown outcomes using GET only, and a resumed run never becomes a retry candidate', async () => {
    let lifecycle = 'active'
    const t = routed(call => call.method === 'GET' ? json({ session_id: 'a', lifecycle }) : failure('delivery_unknown', 503))
    const runner = new BulkCloseController(t.client)
    await runner.start([target('a')]); await runner.checkUnknown()
    expect(states(runner).a).toBe('pending_unknown')
    expect(runner.getSnapshot().results[0].message).toContain('current run')
    expect(await runner.prepareRetry()).toBeNull()
    lifecycle = 'closed'; await runner.checkUnknown()
    expect(states(runner).a).toBe('closed')
    expect(t.calls.map(call => call.method)).toEqual(['POST', 'GET', 'GET'])
  })

  it('does not convert a failed status read on an unknown close into a retryable failure', async () => {
    const t = routed(call => failure(call.method === 'GET' ? 'busy' : 'delivery_unknown', call.method === 'GET' ? 409 : 503))
    const runner = new BulkCloseController(t.client)
    await runner.start([target('a')]); await runner.checkUnknown()
    expect(states(runner).a).toBe('pending_unknown')
    expect(await runner.prepareRetry()).toBeNull()
    expect(t.calls.map(call => call.method)).toEqual(['POST', 'GET'])
  })

  it('treats missing fresh reads as unavailable and malformed fresh reads as failed, without sending Close', async () => {
    const t = routed(call => call.method === 'POST' ? failure('starting') : call.path.endsWith('/gone') ? failure('not_found', 404) : json({ session_id: 'wrong', lifecycle: 'active' }))
    const runner = new BulkCloseController(t.client)
    await runner.start([target('gone'), target('bad')]); expect(await runner.prepareRetry()).toBeNull()
    expect(states(runner)).toEqual({ gone: 'unavailable', bad: 'failed' })
    expect(t.calls.filter(call => call.method === 'POST')).toHaveLength(2)
  })

  it('invalidates a prepared retry when a stream reports changed lifecycle', async () => {
    const t = routed(call => call.method === 'POST' ? failure('starting') : json({ session_id: 'a', lifecycle: 'parked' }))
    const runner = new BulkCloseController(t.client)
    await runner.start([target('a')]); const plan = (await runner.prepareRetry())!
    runner.reconcile(new Map([['a', { lifecycle: 'active' }]]))
    expect(await runner.confirmRetry(plan)).toBe(false)
    expect(t.calls.filter(call => call.method === 'POST')).toHaveLength(1)
  })

  it('never infers deletion from missing cursor rows and resolves unknown outcomes from authoritative closed summaries', async () => {
    const t = routed(() => failure('delivery_unknown', 503)), runner = new BulkCloseController(t.client)
    await runner.start([target('a')])
    runner.reconcile(new Map([['unselected', { lifecycle: 'closed' }]]))
    expect(states(runner)).toEqual({ a: 'pending_unknown' })
    runner.reconcile(new Map([['a', { lifecycle: 'closed' }]]))
    expect(states(runner)).toEqual({ a: 'closed' })
    expect(t.calls).toHaveLength(1)
  })

  it('does not let an old GET overwrite an SSE close or include it in a retry plan', async () => {
    const d = deferred<Response>(), t = routed(call => call.method === 'GET' ? d.promise : failure('starting'))
    const runner = new BulkCloseController(t.client)
    await runner.start([target('a')]); const prepare = runner.prepareRetry(); await flush()
    runner.reconcile(new Map([['a', { lifecycle: 'closed' }]]))
    d.resolve(json({ session_id: 'a', lifecycle: 'active' }))
    expect(await prepare).toBeNull()
    expect(states(runner).a).toBe('closed')
  })

  it('finishes in-flight reads after a 429 without leaving checking states or a confirmable partial plan', async () => {
    const d = deferred<Response>()
    const t = routed(call => call.method === 'POST' ? failure('starting') : call.path.endsWith('/a') ? failure('rate_limited', 429) : d.promise)
    const runner = new BulkCloseController(t.client)
    await runner.start(['a', 'b', 'c', 'd'].map(id => target(id)))
    const prepare = runner.prepareRetry(); await flush()
    d.resolve(json({ session_id: 'b', lifecycle: 'parked' }))
    expect(await prepare).toBeNull()
    expect(runner.getSnapshot().results.every(result => result.status !== 'checking')).toBe(true)
    expect(runner.getSnapshot().busy).toBe(false)
  })

  it('keeps a streamed close despite a late lost HTTP response', async () => {
    const d = deferred<Response>(), t = routed(() => d.promise), runner = new BulkCloseController(t.client)
    const run = runner.start([target('a')]); await flush()
    runner.reconcile(new Map([['a', { lifecycle: 'closed' }]]))
    d.reject(new TypeError('Response lost')); await run
    expect(states(runner).a).toBe('closed')
  })

  it('keeps a streamed close while a stale unknown-status GET returns a resumed lifecycle', async () => {
    const d = deferred<Response>(), t = routed(call => call.method === 'GET' ? d.promise : failure('delivery_unknown', 503))
    const runner = new BulkCloseController(t.client)
    await runner.start([target('a')]); const check = runner.checkUnknown(); await flush()
    runner.reconcile(new Map([['a', { lifecycle: 'closed' }]]))
    d.resolve(json({ session_id: 'a', lifecycle: 'active' })); await check
    expect(states(runner).a).toBe('closed')
    expect(t.calls.map(call => call.method)).toEqual(['POST', 'GET'])
  })

  it('does not revive a failed result when stopped after SSE resolves an in-flight read', async () => {
    const d = deferred<Response>(), t = routed(call => call.method === 'GET' ? d.promise : failure('starting'))
    const runner = new BulkCloseController(t.client)
    await runner.start([target('a')]); const prepare = runner.prepareRetry(); await flush()
    runner.reconcile(new Map([['a', { lifecycle: 'closed' }]])); runner.stop()
    expect(states(runner).a).toBe('closed')
    runner.activate(); await runner.start([target('b', 'closed')])
    d.resolve(json({ session_id: 'a', lifecycle: 'active' }))
    expect(await prepare).toBeNull()
    expect(states(runner)).toEqual({ b: 'already_closed' })
  })

  it('invalidates old retry plans when a new explicitly confirmed batch replaces their results', async () => {
    const t = routed(call => call.method === 'POST' ? failure('starting') : json({ session_id: 'a', lifecycle: 'active' }))
    const runner = new BulkCloseController(t.client)
    await runner.start([target('a')]); const plan = (await runner.prepareRetry())!
    await runner.start([target('b', 'closed')])
    expect(await runner.confirmRetry(plan)).toBe(false)
    expect(states(runner)).toEqual({ b: 'already_closed' })
    expect(t.calls.map(call => call.method)).toEqual(['POST', 'GET'])
  })
})
