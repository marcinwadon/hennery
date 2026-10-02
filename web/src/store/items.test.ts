import { describe, expect, it } from 'vitest'
import type { Item, ItemPage } from '../generated/view'
import { EMPTY_ITEMS, beforeTurnOf, isItem, itemsReducer, type ItemsState } from './items'

function message(id: string, turn: string | undefined, version = 1, ts = '2026-10-02T10:00:00.000Z'): Item {
  return { id, version, ts, ...(turn === undefined ? {} : { turn_id: turn }), kind: 'message', text: id } as Item
}

function page(items: Item[], older = false, revision = 10): ItemPage {
  return { items, older, epoch: 'e1', revision }
}

const ids = (state: ItemsState) => state.items.map((item) => item.id)

function loaded(items: Item[], older = false): ItemsState {
  return itemsReducer(EMPTY_ITEMS, { type: 'loaded', page: page(items, older) })
}

describe('itemsReducer', () => {
  it('takes a first page whole, with its anchor', () => {
    const state = loaded([message('a', 't1'), message('b', 't2')], true)
    expect(ids(state)).toEqual(['a', 'b'])
    expect(state).toMatchObject({ older: true, epoch: 'e1', revision: 10 })
  })

  it('a resync page replaces everything, older pages included', () => {
    let state = loaded([message('c', 't3')], true)
    state = itemsReducer(state, { type: 'prepended', page: page([message('a', 't1')], false) })
    state = itemsReducer(state, {
      type: 'loaded',
      page: { items: [message('x', 't9')], older: true, epoch: 'e2', revision: 99 },
    })
    expect(ids(state)).toEqual(['x'])
    expect(state).toMatchObject({ older: true, epoch: 'e2', revision: 99 })
  })

  it('replaces an item in place by a version at least the one held', () => {
    let state = loaded([message('a', 't1', 5), message('b', 't1', 5)])
    const newer = { ...message('a', 't1', 7), text: 'new' } as Item
    state = itemsReducer(state, { type: 'upsert', item: newer })
    expect(ids(state)).toEqual(['a', 'b'])
    expect(state.items[0]).toBe(newer)
    const same = { ...message('a', 't1', 7), text: 'again' } as Item
    state = itemsReducer(state, { type: 'upsert', item: same })
    expect(state.items[0]).toBe(same)
  })

  it('never goes back to an older version', () => {
    const state = loaded([message('a', 't1', 5)])
    const after = itemsReducer(state, { type: 'upsert', item: message('a', 't1', 4) })
    expect(after).toBe(state)
  })

  it('a new id of a loaded group goes at that group’s end', () => {
    let state = loaded([message('a', 't1'), message('b', 't1'), message('c', 't2')])
    state = itemsReducer(state, { type: 'upsert', item: message('n', 't1') })
    expect(ids(state)).toEqual(['a', 'b', 'n', 'c'])
  })

  it('the start group (no turn) is a group too', () => {
    let state = loaded([message('a', undefined), message('b', 't1')])
    state = itemsReducer(state, { type: 'upsert', item: message('n', undefined) })
    expect(ids(state)).toEqual(['a', 'n', 'b'])
  })

  it('a new group with nothing older left goes at the end', () => {
    let state = loaded([message('a', 't1')], false)
    state = itemsReducer(state, { type: 'upsert', item: message('n', 't2', 1, '2000-01-01T00:00:00.000Z') })
    expect(ids(state)).toEqual(['a', 'n'])
  })

  it('a new group newer than the first loaded item goes at the end, older groups or not', () => {
    let state = loaded([message('a', 't5', 1, '2026-10-02T10:00:00.000Z')], true)
    state = itemsReducer(state, { type: 'upsert', item: message('n', 't6', 1, '2026-10-02T10:00:00.000Z') })
    expect(ids(state)).toEqual(['a', 'n'])
  })

  it('ignores an item of a turn not loaded', () => {
    const state = loaded([message('a', 't5', 1, '2026-10-02T10:00:00.000Z')], true)
    const after = itemsReducer(state, { type: 'upsert', item: message('old', 't1', 1, '2026-10-01T10:00:00.000Z') })
    expect(after).toBe(state)
  })

  it('ignores a new group whose time cannot be read while older groups exist', () => {
    const state = loaded([message('a', 't5')], true)
    const after = itemsReducer(state, { type: 'upsert', item: message('n', 't6', 1, 'not a time') })
    expect(after).toBe(state)
  })

  it('removes an item by id, and ignores an id not held', () => {
    let state = loaded([message('a', 't1'), message('b', 't1')])
    state = itemsReducer(state, { type: 'removed', id: 'a' })
    expect(ids(state)).toEqual(['b'])
    expect(itemsReducer(state, { type: 'removed', id: 'zzz' })).toBe(state)
  })

  it('prepends an older page, skipping ids already held', () => {
    let state = loaded([message('c', 't3'), message('d', 't3')], true)
    state = itemsReducer(state, {
      type: 'prepended',
      page: page([message('a', 't1'), message('b', 't2'), message('c', 't3', 1)], false, 3),
    })
    expect(ids(state)).toEqual(['a', 'b', 'c', 'd'])
    expect(state.older).toBe(false)
    // The anchor is the first page's.
    expect(state.revision).toBe(10)
  })

  it('takes before_turn from the first loaded item, and none at the start', () => {
    expect(beforeTurnOf(loaded([message('a', 't4'), message('b', 't5')], true))).toBe('t4')
    expect(beforeTurnOf(loaded([message('a', undefined), message('b', 't5')], true))).toBeUndefined()
    expect(beforeTurnOf(loaded([message('a', 't4')], false))).toBeUndefined()
    expect(beforeTurnOf(EMPTY_ITEMS)).toBeUndefined()
  })

  it('never crashes on a malformed item or page', () => {
    const state = loaded([message('a', 't1')])
    const bad: unknown[] = [
      null,
      42,
      'item',
      {},
      { id: 'x' },
      { id: 'x', version: '2', ts: 't', kind: 'message' },
      { id: 'x', version: 1, ts: 't', kind: 'message', turn_id: 7 },
      { id: 1, version: 1, ts: 't', kind: 'message' },
      { id: 'x', version: Number.NaN, ts: 't', kind: 'message' },
    ]
    for (const item of bad) {
      expect(itemsReducer(state, { type: 'upsert', item: item as Item })).toBe(state)
    }
    expect(itemsReducer(state, { type: 'loaded', page: { items: 'no' } as unknown as ItemPage })).toBe(state)
    expect(itemsReducer(state, { type: 'prepended', page: null as unknown as ItemPage })).toBe(state)
    expect(itemsReducer(state, { type: 'removed', id: undefined as unknown as string })).toBe(state)
    expect(itemsReducer(state, { type: 'nope' } as unknown as never)).toBe(state)
    // A page's bad entries are dropped, the good ones kept.
    const mixed = itemsReducer(EMPTY_ITEMS, {
      type: 'loaded',
      page: page([null, message('a', 't1'), { id: 'b' }, message('a', 't1')] as unknown as Item[]),
    })
    expect(ids(mixed)).toEqual(['a'])
  })

  it('isItem accepts every kind the server sends', () => {
    for (const kind of ['user_turn', 'message', 'thinking', 'tool_call', 'plan', 'question', 'marker', 'unrecognised']) {
      expect(isItem({ id: 'x', version: 1, ts: 't', kind })).toBe(true)
    }
  })
})
