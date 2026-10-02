import { describe, expect, it } from 'vitest'
import type { SessionCatalog } from '../generated/protocol'
import { commands, configOptions, matchCommands, parseOption } from './catalog'

function cat(config_options: unknown[], cmds: unknown[] = []): SessionCatalog {
  return { session_id: 's1', config_options, commands: cmds } as SessionCatalog
}

const select = (id: string, category: string | undefined, current = 'a', extra: object = {}) => ({
  id,
  name: id.toUpperCase(),
  ...(category === undefined ? {} : { category }),
  type: 'select',
  currentValue: current,
  options: [
    { value: 'a', name: 'A' },
    { value: 'b', name: 'B' },
  ],
  ...extra,
})

describe('parseOption', () => {
  it('reads a select, flat or grouped, and a boolean', () => {
    expect(parseOption(select('model', 'model'))).toMatchObject({ kind: 'select', id: 'model', current: 'a' })
    const grouped = parseOption({
      id: 'model',
      name: 'Model',
      type: 'select',
      currentValue: 'x',
      options: [{ group: 'g1', name: 'Fast', options: [{ value: 'x', name: 'X' }] }],
    })
    expect(grouped).toMatchObject({ choices: [{ value: 'x', name: 'X', group: 'Fast' }] })
    expect(parseOption({ id: 'think', name: 'Think', type: 'boolean', currentValue: true })).toMatchObject({
      kind: 'boolean',
      current: true,
    })
  })

  it('skips what it cannot show, never throwing', () => {
    for (const bad of [null, 'x', [], {}, { id: '' }, { id: 'a', type: 'select', currentValue: 1 }, { id: 'a', type: 'slider' }]) {
      expect(parseOption(bad)).toBeNull()
    }
    expect(parseOption(select('m', undefined, 'a', { options: [] }))).toBeNull()
  })
})

describe('configOptions', () => {
  it('orders the model, then the mode, then the rest by id, whatever the adapter’s order', () => {
    const list = configOptions(
      cat([select('zeta', undefined), select('mode', 'mode'), select('alpha', 'other'), select('model', 'model')]),
    )
    expect(list.map((o) => o.id)).toEqual(['model', 'mode', 'alpha', 'zeta'])
  })

  it('finds the model and mode by category first, else by a conventional id', () => {
    const byCategory = configOptions(cat([select('b-thing', 'mode'), select('a-thing', 'model'), select('mode', undefined)]))
    expect(byCategory.map((o) => o.id)).toEqual(['a-thing', 'b-thing', 'mode'])
  })

  it('takes each id once, and nothing from a catalogue that is not there', () => {
    expect(configOptions(cat([select('x', undefined), select('x', undefined, 'b')])).map((o) => o.current)).toEqual(['a'])
    expect(configOptions(null)).toEqual([])
    expect(configOptions(cat('nope' as unknown as unknown[]))).toEqual([])
  })
})

describe('commands', () => {
  it('lists each command once, in the adapter’s order, with its hint', () => {
    const list = commands(
      cat([], [{ name: 'review', description: 'Review' }, { name: 'init', input: { hint: 'path' } }, { name: 'review' }, { nope: 1 }]),
    )
    expect(list).toEqual([
      { name: 'review', description: 'Review', hint: undefined },
      { name: 'init', description: undefined, hint: 'path' },
    ])
  })

  it('matchCommands lists names that start with the query first, then those that contain it', () => {
    const list = commands(cat([], [{ name: 'compact' }, { name: 'pr-comments' }, { name: 'cost' }, { name: 'clear' }]))
    expect(matchCommands(list, 'co').map((c) => c.name)).toEqual(['compact', 'cost', 'pr-comments'])
    expect(matchCommands(list, '').map((c) => c.name)).toEqual(['compact', 'pr-comments', 'cost', 'clear'])
  })
})
