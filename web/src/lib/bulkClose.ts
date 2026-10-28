import type { Client, RequestOptions } from '../api/client'
import { ApiFailure, Unauthenticated } from '../api/errors'
import { close } from '../api/turns'
import { closeRefusal } from '../components/sessionWords'

export const CLOSE_CONCURRENCY = 3
// The host teardown deadline is 60s. Aborting this wait cannot undo a Close.
export const CLOSE_TIMEOUT_MS = 75_000
const READ_TIMEOUT_MS = 15_000

export interface BulkCloseTarget {
  readonly id: string
  readonly label: string
  readonly lifecycle?: string
}
export type CloseStatus = 'queued' | 'closing' | 'checking' | 'closed' | 'already_closed' | 'pending_unknown' | 'failed' | 'unavailable' | 'not_sent'
export interface CloseResult {
  readonly target: BulkCloseTarget
  readonly status: CloseStatus
  readonly message: string
}
/** Returned by prepareRetry; only that exact plan can be confirmed. */
export interface CloseRetryPlan { readonly targets: readonly BulkCloseTarget[] }
export interface BulkCloseSnapshot {
  readonly busy: boolean
  readonly results: readonly CloseResult[]
  readonly retryPlan: CloseRetryPlan | null
  /** A 429 stops unsent work; expiry never automatically retries anything. */
  readonly retryAt: number
}
interface Operation {
  readonly abort: AbortController
  stopped: boolean
  readonly beforeRead: Map<string, CloseResult>
}
const CLOSED = 'Closed in Hennery. An offline host may still run it until it reconnects.'
const UNKNOWN = 'The close outcome is unknown. Check status before choosing a new close.'
const ALREADY_CLOSED = 'Already closed in Hennery.'
const unavailable = (target: BulkCloseTarget): CloseResult => ({ target, status: 'unavailable', message: 'This session is no longer available.' })
const retryable = (result: CloseResult) => result.status === 'failed' || result.status === 'not_sent'

/** Copy only explicit targets: no list, family, filter or cursor expansion. */
export function closeTargets(targets: readonly BulkCloseTarget[]): readonly BulkCloseTarget[] {
  const unique = new Map<string, BulkCloseTarget>()
  for (const target of targets) if (!unique.has(target.id)) unique.set(target.id, Object.freeze({ id: target.id, label: target.label, lifecycle: target.lifecycle }))
  return Object.freeze([...unique.values()])
}

/** Window-local orchestration, not a durable job or a cross-tab mutation lock.
 * start/confirmRetry are called only after the UI confirms their exact targets.
 * Lifecycle closed proves collector state, never remote process termination.
 */
export class BulkCloseController {
  private snapshot: BulkCloseSnapshot = { busy: false, results: [], retryPlan: null, retryAt: 0 }
  private readonly listeners = new Set<() => void>()
  private operation: Operation | null = null
  private active = true
  private readonly observed = new Map<string, { lifecycle: string; generation: number }>()

  constructor(private readonly client: Client) {}

  getSnapshot = (): BulkCloseSnapshot => this.snapshot
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener) } }
  activate = () => { this.active = true }

  /** On unmount, stop unsent work and fence every old callback, including
   * fetch implementations which ignore AbortSignal. Sent closes may finish.
   */
  stop = () => {
    this.active = false
    const op = this.operation
    this.operation = null
    op?.abort.abort()
    this.update({ busy: false, retryPlan: null, results: this.snapshot.results.map(result => {
      if (result.status === 'queued') return { ...result, status: 'not_sent', message: 'Not sent: you left the session list.' }
      if (result.status === 'closing') return { ...result, status: 'pending_unknown', message: UNKNOWN }
      return result.status === 'checking' ? op?.beforeRead.get(result.target.id) ?? result : result
    }) })
  }

  start = async (targets: readonly BulkCloseTarget[]): Promise<boolean> => {
    if (!this.canBegin()) return false
    const frozen = closeTargets(targets)
    if (!frozen.length) return false
    const op = this.begin()
    this.update({ results: frozen.map(target => target.lifecycle === 'closed'
      ? { target, status: 'already_closed', message: ALREADY_CLOSED }
      : { target, status: 'queued', message: 'Waiting to close.' }) })
    await this.send(op, frozen)
    this.finish(op)
    return true
  }

  /** GET only. Successes, unavailable IDs and uncertain closes are excluded.
   * A fresh state is advisory: the Close endpoint has no compare-and-set.
   */
  prepareRetry = async (): Promise<CloseRetryPlan | null> => {
    if (!this.canBegin()) return null
    const candidates = this.snapshot.results.filter(retryable)
    if (!candidates.length) return null
    const op = this.begin()
    const ready = new Map<string, { target: BulkCloseTarget; generation: number }>()
    await this.pool(op, candidates.map(result => result.target), async target => {
      const previous = this.result(target.id)!
      if (!retryable(previous)) return
      const fresh = await this.read(op, previous)
      if (!this.current(op) || !fresh) return
      if (fresh.lifecycle === 'closed') this.set({ target: fresh, status: 'already_closed', message: ALREADY_CLOSED })
      else if (!op.stopped) {
        ready.set(target.id, { target: fresh, generation: this.observed.get(target.id)?.generation ?? 0 })
        this.set({ target: fresh, status: previous.status, message: 'State refreshed. Confirm a new close of this session’s current run.' })
      }
    })
    if (!this.current(op)) return null
    const targets = closeTargets(candidates.flatMap(result => {
      const candidate = ready.get(result.target.id)
      return candidate && candidate.generation === (this.observed.get(result.target.id)?.generation ?? 0) && retryable(this.result(result.target.id)!) ? [candidate.target] : []
    }))
    const plan = !op.stopped && targets.length ? Object.freeze({ targets }) : null
    this.finish(op)
    this.update({ retryPlan: plan })
    return plan
  }

  confirmRetry = async (plan: CloseRetryPlan): Promise<boolean> => {
    if (!this.canBegin() || plan !== this.snapshot.retryPlan || !plan.targets.length) return false
    const op = this.begin()
    for (const target of plan.targets) this.set({ target, status: 'queued', message: 'Waiting to close.' })
    await this.send(op, plan.targets)
    this.finish(op)
    return true
  }

  /** Checking an unknown result never promotes it into automatic retry. A
   * nonclosed run might have been explicitly resumed by another operator.
   */
  checkUnknown = async (): Promise<boolean> => {
    if (!this.canBegin()) return false
    const candidates = this.snapshot.results.filter(result => result.status === 'pending_unknown')
    if (!candidates.length) return false
    const op = this.begin()
    await this.pool(op, candidates.map(result => result.target), async target => {
      const previous = this.result(target.id)!
      if (previous.status !== 'pending_unknown') return
      const fresh = await this.read(op, previous)
      if (!this.current(op) || !fresh) return
      this.set({ target: fresh, status: fresh.lifecycle === 'closed' ? 'closed' : 'pending_unknown', message: fresh.lifecycle === 'closed'
        ? CLOSED : `Currently ${fresh.lifecycle}. Select it again to confirm a new close of its current run.` })
    })
    this.finish(op)
    return true
  }

  /** Pass authoritative SSE summaries only. Missing cursor-page IDs say
   * nothing about deletion. A resync must not manufacture an outcome.
   */
  reconcile = (summaries: ReadonlyMap<string, { lifecycle: string }>) => {
    for (const result of this.snapshot.results) {
      const fresh = summaries.get(result.target.id)
      if (!fresh) continue
      this.observed.set(result.target.id, { lifecycle: fresh.lifecycle, generation: (this.observed.get(result.target.id)?.generation ?? 0) + 1 })
      const prepared = this.snapshot.retryPlan?.targets.find(target => target.id === result.target.id)
      if (prepared && prepared.lifecycle !== fresh.lifecycle) this.update({ retryPlan: null })
      if (fresh.lifecycle !== 'closed') {
        this.set({ ...result, target: { ...result.target, lifecycle: fresh.lifecycle } })
        continue
      }
      if (['pending_unknown', 'closing', 'checking', 'queued', 'failed', 'not_sent'].includes(result.status)) {
        this.set({ target: { ...result.target, lifecycle: 'closed' }, status: result.status === 'queued' || retryable(result) ? 'already_closed' : 'closed', message: CLOSED })
      }
    }
  }

  private canBegin() { return this.active && this.operation === null && Date.now() >= this.snapshot.retryAt }
  private begin(): Operation {
    const op = { abort: new AbortController(), stopped: false, beforeRead: new Map<string, CloseResult>() }
    this.operation = op
    this.update({ busy: true, retryPlan: null })
    return op
  }
  private current(op: Operation) { return this.active && this.operation === op }
  private finish(op: Operation) { if (this.current(op)) { this.operation = null; this.update({ busy: false }) } }
  private update(patch: Partial<BulkCloseSnapshot>) {
    this.snapshot = Object.freeze({ ...this.snapshot, ...patch })
    for (const listener of this.listeners) listener()
  }
  private result(id: string) { return this.snapshot.results.find(result => result.target.id === id) }
  private set(result: CloseResult) {
    this.update({ results: Object.freeze(this.snapshot.results.map(before => before.target.id === result.target.id ? Object.freeze(result) : before)) })
  }
  private options(op: Operation, timeoutMs: number): RequestOptions {
    return { signal: op.abort.signal, timeoutMs, beforeSend: async () => {
      if (!this.current(op) || op.stopped) throw new DOMException('The operation stopped.', 'AbortError')
    } }
  }
  private async pool(op: Operation, targets: readonly BulkCloseTarget[], work: (target: BulkCloseTarget) => Promise<void>) {
    let next = 0
    await Promise.all(Array.from({ length: Math.min(CLOSE_CONCURRENCY, targets.length) }, async () => {
      while (this.current(op) && !op.stopped && next < targets.length) await work(targets[next++])
    }))
  }
  private stopQueue(op: Operation, message: string) {
    op.stopped = true
    for (const result of this.snapshot.results) if (result.status === 'queued') this.set({ ...result, status: 'not_sent', message })
  }
  private interruption(op: Operation, err: unknown) {
    if (err instanceof Unauthenticated || err instanceof ApiFailure && err.status === 401) this.stopQueue(op, 'Not sent: sign in again.')
    else if (err instanceof ApiFailure && err.status === 429) {
      this.update({ retryAt: Math.max(this.snapshot.retryAt, Date.now() + (err.retryAfter ?? 60) * 1000) })
      this.stopQueue(op, 'Not sent: rate limited. Retry remaining after the cooldown.')
    } else if (!(err instanceof ApiFailure)) this.stopQueue(op, 'Not sent: the connection was interrupted. Check the earlier outcomes first.')
  }
  private async send(op: Operation, targets: readonly BulkCloseTarget[]) {
    await this.pool(op, targets, async target => {
      if (this.result(target.id)?.status !== 'queued') return
      this.set({ target, status: 'closing', message: 'Closing…' })
      try {
        const response = await close(this.client, target.id, this.options(op, CLOSE_TIMEOUT_MS))
        if (!this.current(op)) return
        const closed = response?.session_id === target.id && response.lifecycle === 'closed'
        if (!closed && this.result(target.id)?.status === 'closed') return
        this.set({ target, status: closed ? 'closed' : 'pending_unknown', message: closed ? CLOSED : UNKNOWN })
      } catch (err) {
        if (!this.current(op)) return
        this.interruption(op, err)
        if (this.result(target.id)?.status === 'closed') return
        if (err instanceof ApiFailure && err.status === 404) this.set(unavailable(target))
        else {
          const unknown = !(err instanceof ApiFailure || err instanceof Unauthenticated) || err instanceof ApiFailure && err.status >= 500
          this.set({ target, status: unknown ? 'pending_unknown' : 'failed', message: unknown
            ? err instanceof ApiFailure && err.code === 'delivery_unknown' ? closeRefusal(err) : UNKNOWN
            : closeRefusal(err) })
        }
      }
    })
  }
  private async read(op: Operation, previous: CloseResult): Promise<BulkCloseTarget | null> {
    const target = previous.target
    const generation = this.observed.get(target.id)?.generation ?? 0
    op.beforeRead.set(target.id, previous)
    this.set({ target, status: 'checking', message: 'Checking current state…' })
    try {
      const response = await this.client.request<{ session_id: string; lifecycle: string }>('GET', `/api/sessions/${encodeURIComponent(target.id)}`, undefined, this.options(op, READ_TIMEOUT_MS))
      if (!this.current(op)) return null
      if (this.result(target.id)?.status === 'closed') return null
      const latest = this.observed.get(target.id)
      if ((latest?.generation ?? 0) !== generation) {
        this.set({ ...previous, target: { ...target, lifecycle: latest!.lifecycle }, message: 'State changed while checking. Refresh before retrying.' })
        return null
      }
      if (!response || response.session_id !== target.id || !['starting', 'active', 'parked', 'closed', 'failed'].includes(response.lifecycle)) throw new Error('The current session state could not be read.')
      const fresh = Object.freeze({ ...target, lifecycle: response.lifecycle })
      this.set({ ...previous, target: fresh })
      return fresh
    } catch (err) {
      if (!this.current(op)) return null
      this.interruption(op, err)
      if (this.result(target.id)?.status === 'closed') return null
      this.set(err instanceof ApiFailure && err.status === 404 ? unavailable(target) : { ...previous, message: `Status check failed: ${closeRefusal(err)}` })
      return null
    } finally { op.beforeRead.delete(target.id) }
  }
}
