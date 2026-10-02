import { renderHook, waitFor } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import type { Item } from '../generated/view'
import { json, routed } from '../test-stream'
import { useAnswering } from './useAnswer'

const TS = '2026-10-02T10:00:00.000Z'
const msg = (id: string): Item => ({ id, version: 1, ts: TS, turn_id: 't1', kind: 'message', text: id }) as Item
const question = (id: string, patch: object = {}): Item =>
  ({
    id,
    version: 1,
    ts: TS,
    turn_id: 't1',
    kind: 'question',
    pending_id: id,
    question_kind: 'permission',
    request: { type: 'permission', options: [{ option_id: 'a', name: 'Allow', option_kind: 'allow_once' }] },
    answerable: true,
    state: 'open',
    answered: false,
    ...patch,
  }) as Item

const sent = (id: string) => question(id, { answered: true, answerable: false })

function hosts(list: unknown) {
  return routed((call) => (call.path === '/api/hosts' ? json(list) : json({ code: 'not_found', message: 'no' }, 404)))
}

type Props = { info?: { presumed_parked: boolean }; items: Item[]; loading: boolean; connected?: boolean }

function use(t: ReturnType<typeof routed>, initial: Props) {
  return renderHook((p: Props) => useAnswering('s1', p.info, p.items, p.loading, p.connected), { initialProps: initial, wrapper: t.wrapper })
}

describe('useAnswering', () => {
  it('keeps one book for the session', () => {
    const t = hosts([])
    const h = use(t, { items: [], loading: true })
    const first = h.result.current
    h.rerender({ items: [msg('a')], loading: false })
    expect(h.result.current).toBe(first)
  })

  it('says the host is away while the session is presumed parked', async () => {
    const t = hosts([])
    const h = use(t, { info: { presumed_parked: true }, items: [], loading: false, connected: true })
    await waitFor(() => expect(h.result.current.hostAway).toBe(true))
  })

  it('says the host is away when the view says it is not connected, and fetches no hosts itself', async () => {
    const t = hosts([{ host_id: 'h1', name: 'box', connected: true }])
    const info = { presumed_parked: false }
    const h = use(t, { info, items: [sent('q')], loading: false })
    expect(h.result.current.hostAway).toBe(false)
    h.rerender({ info, items: [sent('q')], loading: false, connected: false })
    await waitFor(() => expect(h.result.current.hostAway).toBe(true))
    expect(t.calls).toHaveLength(0)
  })

  it('a connected host, or one not known yet, is not away', async () => {
    const t = hosts([])
    const info = { presumed_parked: false }
    const h = use(t, { info, items: [sent('q')], loading: false, connected: true })
    await Promise.resolve()
    expect(h.result.current.hostAway).toBe(false)
    h.rerender({ info, items: [sent('q')], loading: false })
    await Promise.resolve()
    expect(h.result.current.hostAway).toBe(false)
  })

  it('marks fresh only a question opened at the tail after the first page', () => {
    const t = hosts([])
    const h = use(t, { items: [], loading: true })
    h.rerender({ items: [msg('a'), question('old')], loading: false })
    h.rerender({ items: [question('older'), msg('a'), question('old'), question('new')], loading: false })
    const book = h.result.current
    expect(book.claimFocus('old')).toBe(false)
    expect(book.claimFocus('older')).toBe(false)
    expect(book.claimFocus('new')).toBe(true)
    expect(book.claimFocus('new')).toBe(false)
  })

  it('takes nothing in while the first page loads', () => {
    const t = hosts([])
    const h = use(t, { items: [msg('a')], loading: true })
    h.rerender({ items: [msg('a'), question('q')], loading: true })
    h.rerender({ items: [msg('a'), question('q')], loading: false })
    expect(h.result.current.claimFocus('q')).toBe(false)
  })
})
