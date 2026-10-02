// One session's items, as a pure reducer (plan 4c decision 4): a first page,
// older pages prepended, and the item stream's upserts and removals.
//
// - An item is replaced in place by id, only by a version at least the
//   one held.
// - A new id joins its group (`turn_id`, or `start` before the first turn)
//   at the group's end when that group is loaded; it starts a new group at
//   the end when it is newer than every loaded group (no older groups left,
//   or its `ts` is at least the first loaded item's); otherwise it belongs
//   to a turn not loaded, and is ignored.
// - `turn_id` is an opaque key: never parsed.
// - Anything that is not an item is ignored: the reducer never throws.
import type { Item, ItemPage } from '../generated/view'

export interface ItemsState {
  items: Item[]
  /** Groups exist before the first loaded one. */
  older: boolean
  /** The first page's anchor: where the item stream resumes. */
  epoch: string
  revision: number
}

export type ItemsAction =
  /** The first page, or a resync's: replaces everything. */
  | { type: 'loaded'; page: ItemPage }
  /** An older page: its items go first, skipping ids already held. */
  | { type: 'prepended'; page: ItemPage }
  | { type: 'upsert'; item: Item }
  | { type: 'removed'; id: string }

export const EMPTY_ITEMS: ItemsState = { items: [], older: false, epoch: '', revision: 0 }

/** Whether `value` has what the store reads of an item. */
export function isItem(value: unknown): value is Item {
  if (!value || typeof value !== 'object') return false
  const v = value as Record<string, unknown>
  return (
    typeof v.id === 'string' &&
    typeof v.version === 'number' &&
    Number.isFinite(v.version) &&
    typeof v.ts === 'string' &&
    typeof v.kind === 'string' &&
    (v.turn_id === undefined || typeof v.turn_id === 'string')
  )
}

/** Whether `value` is a page the store can take. */
export function isItemPage(value: unknown): value is ItemPage {
  if (!value || typeof value !== 'object') return false
  const v = value as Record<string, unknown>
  return (
    Array.isArray(v.items) &&
    typeof v.older === 'boolean' &&
    typeof v.epoch === 'string' &&
    typeof v.revision === 'number'
  )
}

const groupOf = (item: Item): string => item.turn_id ?? 'start'

/** The items of `list` that are items, each id once (the first kept). */
function distinct(list: readonly unknown[], held: ReadonlySet<string> = new Set()): Item[] {
  const seen = new Set(held)
  const out: Item[] = []
  for (const value of list) {
    if (isItem(value) && !seen.has(value.id)) {
      seen.add(value.id)
      out.push(value)
    }
  }
  return out
}

/** `before_turn` for the next older page: the first loaded item's turn.
 *  Absent when nothing is older, or the first group is the session's start. */
export function beforeTurnOf(state: ItemsState): string | undefined {
  if (!state.older) return undefined
  return state.items[0]?.turn_id
}

/** Whether a new item of a group not loaded comes after every loaded one. */
function isNewer(state: ItemsState, item: Item): boolean {
  if (!state.older || state.items.length === 0) return true
  // An unreadable time compares false: not newer.
  return Date.parse(item.ts) >= Date.parse(state.items[0].ts)
}

export function itemsReducer(state: ItemsState, action: ItemsAction): ItemsState {
  switch (action.type) {
    case 'loaded': {
      if (!isItemPage(action.page)) return state
      const { page } = action
      return { items: distinct(page.items), older: page.older, epoch: page.epoch, revision: page.revision }
    }
    case 'prepended': {
      if (!isItemPage(action.page)) return state
      const held = new Set(state.items.map((item) => item.id))
      const before = distinct(action.page.items, held)
      return { ...state, items: [...before, ...state.items], older: action.page.older }
    }
    case 'upsert': {
      const { item } = action
      if (!isItem(item)) return state
      const at = state.items.findIndex((held) => held.id === item.id)
      if (at >= 0) {
        if (item.version < state.items[at].version) return state
        const items = state.items.slice()
        items[at] = item
        return { ...state, items }
      }
      const group = groupOf(item)
      let last = -1
      for (let i = 0; i < state.items.length; i++) if (groupOf(state.items[i]) === group) last = i
      if (last >= 0) {
        const items = state.items.slice()
        items.splice(last + 1, 0, item)
        return { ...state, items }
      }
      if (isNewer(state, item)) return { ...state, items: [...state.items, item] }
      // A turn not loaded (4a-i O-6): its page brings it.
      return state
    }
    case 'removed': {
      if (typeof action.id !== 'string') return state
      const items = state.items.filter((item) => item.id !== action.id)
      return items.length === state.items.length ? state : { ...state, items }
    }
    default:
      return state
  }
}
