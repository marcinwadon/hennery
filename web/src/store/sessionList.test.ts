import { describe, expect, it } from 'vitest'
import type { SessionSummary, SummaryPage } from '../generated/view'
import {
  EMPTY_LIST,
  carryWaiting,
  compareSummaries,
  isFirstPage,
  isShown,
  listReducer,
  serverQuery,
  shownOf,
  waitingCount,
  type ListFilters,
  type ListState,
} from './sessionList'

function summary(id: string, patch: Partial<SessionSummary> = {}): SessionSummary {
  return {
    session_id: id,
    host_id: 'h1',
    agent: 'claude',
    cwd: '/srv/work/app',
    hat_id: 'hat-a',
    lifecycle: 'active',
    presumed_parked: false,
    created_at: '2026-10-01T00:00:00.000Z',
    last_event_at: '2026-10-02T10:00:00.000Z',
    question_waits: false,
    ...patch,
  }
}

/** A page; with no `waiting`, a server that does not count (the rows are
 *  counted instead). */
function page(sessions: SessionSummary[], next?: string, revision = 10, waiting?: number): SummaryPage {
  return { sessions, next_cursor: next, epoch: 'e1', revision, ...(waiting === undefined ? {} : { waiting }) } as SummaryPage
}

const NONE: ListFilters = { hideClosed: false }
const ids = (list: SessionSummary[]) => list.map((s) => s.session_id)

function first(sessions: SessionSummary[], search = false): ListState {
  return listReducer(EMPTY_LIST, { type: 'first', page: page(sessions, 'c1'), search })
}

describe('serverQuery', () => {
  it('sends the hat, and Hide closed as every lifecycle but closed', () => {
    expect(serverQuery({ hat: 'hat-a', hideClosed: true })).toEqual({
      hat: 'hat-a',
      lifecycle: ['starting', 'active', 'parked', 'failed'],
    })
  })

  it('a lifecycle filter alone decides', () => {
    expect(serverQuery({ hideClosed: true, lifecycle: ['closed'] })).toEqual({ hat: undefined, lifecycle: ['closed'] })
  })

  it('a search sends q and the hat only', () => {
    expect(serverQuery({ hat: 'hat-a', hideClosed: true, lifecycle: ['active'], q: '  fix ' })).toEqual({
      q: 'fix',
      hat: 'hat-a',
    })
  })

  it('no hat sends none (the server refuses an empty one)', () => {
    expect(serverQuery({ hat: '', hideClosed: false })).toEqual({ hat: undefined })
    expect(serverQuery({ hat: null, hideClosed: false })).toEqual({ hat: undefined })
  })
})

describe('the shown list', () => {
  it('sorts by last_event_at, newest first, then by id', () => {
    const state = first([
      summary('b', { last_event_at: '2026-10-02T10:00:00.000Z' }),
      summary('c', { last_event_at: '2026-10-02T11:00:00.000Z' }),
      summary('a', { last_event_at: '2026-10-02T10:00:00.000Z' }),
    ])
    expect(ids(shownOf(state, NONE))).toEqual(['c', 'a', 'b'])
    expect(compareSummaries(summary('a'), summary('a'))).toBe(0)
  })

  it('filters by hat', () => {
    const state = first([summary('a'), summary('b', { hat_id: 'hat-b' })])
    expect(ids(shownOf(state, { hat: 'hat-b', hideClosed: false }))).toEqual(['b'])
  })

  it('Hide closed hides closed sessions and never parked ones', () => {
    const state = first([
      summary('a', { lifecycle: 'closed' }),
      summary('b', { lifecycle: 'parked' }),
      summary('c', { lifecycle: 'failed' }),
    ])
    expect(ids(shownOf(state, { hideClosed: true }))).toEqual(['b', 'c'])
    expect(ids(shownOf(state, { hideClosed: false }))).toEqual(['a', 'b', 'c'])
  })

  it('a lifecycle filter shows only its lifecycles', () => {
    const state = first([summary('a', { lifecycle: 'closed' }), summary('b', { lifecycle: 'parked' })])
    expect(ids(shownOf(state, { hideClosed: true, lifecycle: ['closed'] }))).toEqual(['a'])
  })

  it('a search shows what the server found, in the hat, whatever the lifecycle', () => {
    const state = first([summary('a', { lifecycle: 'closed' }), summary('b', { hat_id: 'hat-b' })], true)
    expect(ids(shownOf(state, { hideClosed: true, lifecycle: ['active'], q: 'x' }))).toEqual(['a', 'b'])
    expect(ids(shownOf(state, { hat: 'hat-a', hideClosed: true, q: 'x' }))).toEqual(['a'])
  })
})

describe('listReducer', () => {
  it('an upsert of a new session inserts it', () => {
    let state = first([summary('a')])
    state = listReducer(state, { type: 'upsert', summary: summary('n', { last_event_at: '2026-10-02T12:00:00.000Z' }) })
    expect(ids(shownOf(state, NONE))).toEqual(['n', 'a'])
  })

  it('an upsert updates a session in place, and moves it by its time', () => {
    let state = first([summary('a', { last_event_at: '2026-10-02T09:00:00.000Z' }), summary('b')])
    state = listReducer(state, {
      type: 'upsert',
      summary: summary('a', { title: 'new', last_event_at: '2026-10-02T12:00:00.000Z' }),
    })
    const shown = shownOf(state, NONE)
    expect(ids(shown)).toEqual(['a', 'b'])
    expect(shown[0].title).toBe('new')
  })

  it('an upsert out of the filter takes the row off the shown list, and keeps it in all', () => {
    let state = first([summary('a'), summary('b')])
    state = listReducer(state, { type: 'upsert', summary: summary('a', { lifecycle: 'closed' }) })
    expect(ids(shownOf(state, { hideClosed: true }))).toEqual(['b'])
    expect(state.all.get('a')?.lifecycle).toBe('closed')
  })

  it('while searching, an upsert updates a found row and never adds one', () => {
    let state = first([summary('a')], true)
    state = listReducer(state, { type: 'upsert', summary: summary('a', { title: 't2' }) })
    state = listReducer(state, { type: 'upsert', summary: summary('n') })
    const shown = shownOf(state, { hideClosed: false, q: 'x' })
    expect(ids(shown)).toEqual(['a'])
    expect(shown[0].title).toBe('t2')
    // The header still sees it.
    expect(state.all.has('n')).toBe(true)
  })

  it('session_removed takes a session out of all, the shown list and the search', () => {
    let state = first([summary('a'), summary('b')], true)
    state = listReducer(state, { type: 'removed', id: 'a' })
    expect(state.all.has('a')).toBe(false)
    expect(ids(shownOf(state, { hideClosed: false, q: 'x' }))).toEqual(['b'])
    expect(listReducer(state, { type: 'removed', id: 'zzz' })).toBe(state)
  })

  it('a session_removed sent twice is a keyed delete: the second changes nothing', () => {
    let state = first([summary('a'), summary('b')])
    state = listReducer(state, { type: 'removed', id: 'a' })
    expect(listReducer(state, { type: 'removed', id: 'a' })).toBe(state)
    expect([...state.all.keys()]).toEqual(['b'])
  })

  it('a further page adds rows and keeps the first page’s anchor', () => {
    let state = first([summary('a')])
    state = listReducer(state, {
      type: 'more',
      page: page([summary('b', { last_event_at: '2026-10-01T00:00:00.000Z' })], undefined, 99),
    })
    expect(ids(shownOf(state, NONE))).toEqual(['a', 'b'])
    expect(state.revision).toBe(10)
    expect(state.nextCursor).toBeUndefined()
  })

  it('a further page never puts back an older row than the stream sent', () => {
    let state = first([summary('x')])
    state = listReducer(state, { type: 'upsert', summary: summary('a', { title: 'live', last_event_at: '2026-10-02T12:00:00.000Z' }) })
    state = listReducer(state, { type: 'more', page: page([summary('a', { title: 'old' })]) })
    expect(state.all.get('a')?.title).toBe('live')
  })

  it('a search’s further page adds to what it found', () => {
    let state = first([summary('a')], true)
    state = listReducer(state, { type: 'more', page: page([summary('b')]) })
    expect(ids(shownOf(state, { hideClosed: false, q: 'x' }))).toEqual(['a', 'b'])
  })

  it('a first page replaces everything', () => {
    let state = first([summary('a'), summary('b')])
    state = listReducer(state, { type: 'first', page: page([summary('c')], undefined, 50), search: false })
    expect([...state.all.keys()]).toEqual(['c'])
    expect(state.revision).toBe(50)
  })

  it('never crashes on a malformed summary or page', () => {
    const state = first([summary('a')])
    for (const bad of [null, 1, {}, { session_id: 'x' }, { ...summary('x'), question_waits: 'yes' }]) {
      expect(listReducer(state, { type: 'upsert', summary: bad as SessionSummary })).toBe(state)
    }
    expect(listReducer(state, { type: 'more', page: { sessions: 3 } as unknown as SummaryPage })).toBe(state)
    expect(listReducer(state, { type: 'first', page: null as unknown as SummaryPage, search: false })).toBe(state)
    expect(listReducer(state, { type: 'removed', id: 4 as unknown as string })).toBe(state)
  })
})

describe('the server’s waiting count', () => {
  const blocked = [summary('a', { activity: 'blocked' })]

  it('a first page holds its waiting; a later first page replaces it, or clears it when it has none', () => {
    let state = listReducer(EMPTY_LIST, { type: 'first', page: page(blocked, 'c1', 10, 4), search: false })
    expect(state.waiting).toBe(4)
    state = listReducer(state, { type: 'first', page: page(blocked, undefined, 20, 0), search: true })
    expect(state.waiting).toBe(0)
    state = listReducer(state, { type: 'first', page: page(blocked, undefined, 30), search: false })
    expect(state.waiting).toBeUndefined()
  })

  it('a further page never changes it, whatever it carries', () => {
    let state = listReducer(EMPTY_LIST, { type: 'first', page: page(blocked, 'c1', 10, 2), search: false })
    state = listReducer(state, { type: 'waiting', count: 3 })
    state = listReducer(state, { type: 'more', page: page([summary('b', { question_waits: true })], undefined, 15, 9) })
    expect(state.waiting).toBe(3)
    expect(state.all.has('b')).toBe(true)
  })

  it('waiting_changed sets it; a count that is not a whole number from 0 changes nothing', () => {
    let state = listReducer(EMPTY_LIST, { type: 'first', page: page(blocked, undefined, 10, 2), search: false })
    state = listReducer(state, { type: 'waiting', count: 0 })
    expect(state.waiting).toBe(0)
    for (const bad of [-1, 1.5, Number.NaN, '3', null]) {
      expect(listReducer(state, { type: 'waiting', count: bad as number })).toBe(state)
    }
    expect(listReducer(state, { type: 'waiting', count: 0 })).toBe(state)
  })

  it('a first page whose waiting is not a count is unreadable; a further page’s is never read', () => {
    for (const bad of [-1, 2.5, '1', null]) {
      const bogus = { ...page(blocked), waiting: bad } as unknown as SummaryPage
      expect(isFirstPage(bogus)).toBe(false)
      expect(listReducer(EMPTY_LIST, { type: 'first', page: bogus, search: false })).toBe(EMPTY_LIST)
    }
    expect(isFirstPage(page(blocked))).toBe(true)
    expect(isFirstPage(page(blocked, undefined, 10, 0))).toBe(true)
    const state = first([summary('x')])
    const more = listReducer(state, { type: 'more', page: { ...page(blocked), waiting: -1 } as unknown as SummaryPage })
    expect(more.all.has('a')).toBe(true)
  })

  it('waitingCount is the server’s count when it sent one, else the loaded rows’', () => {
    const rows = [summary('a', { activity: 'blocked' }), summary('b', { question_waits: true })]
    const counted = listReducer(EMPTY_LIST, { type: 'first', page: page(rows, undefined, 10, 7), search: false })
    expect(waitingCount(counted, 'hat-a')).toBe(7)
    expect(waitingCount(listReducer(counted, { type: 'waiting', count: 0 }), 'hat-a')).toBe(0)
    expect(waitingCount(first(rows), 'hat-a')).toBe(2)
  })
})

describe('waitingCount', () => {
  it('counts the hat’s sessions that are blocked or have a question open', () => {
    const state = first([
      summary('a', { activity: 'blocked' }),
      summary('b', { question_waits: true, lifecycle: 'closed' }),
      summary('c', { activity: 'running' }),
      summary('d', { activity: 'blocked', hat_id: 'hat-b' }),
    ])
    expect(waitingCount(state, 'hat-a')).toBe(2)
    expect(waitingCount(state, null)).toBe(3)
    expect(isShown(state, summary('d', { hat_id: 'hat-b' }), { hat: 'hat-a', hideClosed: false })).toBe(false)
  })
})

describe('carryWaiting', () => {
  it('keeps the ids held and applies every summary received since', () => {
    const held = new Set(['kept', 'answered', 'moved'])
    const since = first([
      summary('answered', { activity: 'running' }),
      summary('moved', { activity: 'blocked', hat_id: 'hat-b' }),
      summary('asks', { question_waits: true }),
      summary('calm'),
    ])
    expect([...carryWaiting(held, since, 'hat-a')].sort()).toEqual(['asks', 'kept'])
    expect([...held].sort()).toEqual(['answered', 'kept', 'moved'])
  })
})
