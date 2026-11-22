import { describe, expect, it } from 'vitest'
import { resolveDefaults, securityWarnings } from './defaults'

import { catalog, select } from '../test-launch-catalog'
describe('catalog-aware defaults', () => {
  it('prunes only inherited nonsecurity axes for Opus to Sonnet, preserving baseline security axes and deliberate effort', () => {
    const result = resolveDefaults({ ...catalog, security: { known: true, axes: { mode: ['default'], sandbox: [true] } } }, { model: 'sonnet', axes: { effort: 'high' } })
    expect(result.config).toEqual({ model: 'sonnet', mode: 'default', axes: { effort: 'high', sandbox: true } })
    expect(result.sources.model).toBe('Hat override')
    expect(result.sources.effort).toBe('Hat override')
    expect(result.sources.sandbox).toBe('Agent default')
  })
  it.each(['stored', 'explicit'])('preserves deliberate false as %s even on a different model', source => {
    const result = resolveDefaults(catalog, source === 'stored' ? { model: 'sonnet', axes: { fast: false } } : { model: 'sonnet' }, source === 'explicit' ? { fast: false } : {})
    expect(result.config.axes?.fast).toBe(false)
    expect(result.sources.fast).toBe(source === 'stored' ? 'Hat override' : 'Your choice')
  })
  it('falls back visibly for unavailable nonsecurity overrides without changing the stored record', () => {
    const saved = { model: 'removed', axes: { effort: 'medium' } }
    const result = resolveDefaults(catalog, saved)
    expect(result.config.model).toBe('opus')
    expect(result.config.axes?.effort).toBe('low')
    expect(result.notices).toContain('Saved model “removed” is unavailable on this host; using opus.')
    expect(result.notices).toContain('Saved effort “medium” is unavailable on this host; using low.')
    expect(result.notices).toHaveLength(2)
    expect(result.blocked).toBe(false)
    expect(saved.model).toBe('removed')
  })
  it('falls back to unknown agent defaults for a missing nonsecurity axis, but always requests profile security axes', () => {
    const result = resolveDefaults({ ...catalog, config_options: [], defaults: {}, security: { known: true, axes: { sandbox: [true] } } }, { model: 'removed', mode: 'safe', axes: { fast: false, sandbox: true } })
    expect(result.config).toEqual({ mode: 'safe', axes: { sandbox: true } })
    expect(result.notices).toContain('Saved fast “false” is unavailable on this host; using agent default.')
    expect(result.sources.sandbox).toBe('Hat override')
    expect(result.sources.mode).toBe('Hat override')
    expect(result.blocked).toBe(false)
  })
  it('blocks invalid explicit choices instead of silently converting them to inheritance', () => {
    const result = resolveDefaults(catalog, {}, { model: 'removed', effort: 'medium' })
    expect(result.blocked).toBe(true)
    expect(result.notices).toHaveLength(2)
  })
  it('resets an individual choice to agent inheritance without reintroducing the hat override', () => {
    const result = resolveDefaults(catalog, { model: 'sonnet', mode: 'bypass' }, { model: null, mode: null })
    expect(result.config.model).toBe('opus')
    expect(result.config.mode).toBe('default')
    expect(result.sources.model).toBe('Agent default')
  })
  it('uses category identity, never friendly labels, for model/mode and profile security warnings', () => {
    const other = { ...catalog, config_options: [select('custom-model', ['opus', 'sonnet'], 'model'), select('permission', ['default', 'bypass'], 'mode')] }
    const result = resolveDefaults(other, { model: 'sonnet', mode: 'bypass' })
    expect(result.config).toEqual({ model: 'sonnet', mode: 'bypass' })
    expect(result.warnings).toHaveLength(1)
    expect(securityWarnings({ ...catalog, security: { known: false, axes: {} } }, {})).toHaveLength(1)
    expect(securityWarnings(catalog, { mode: 'default' })).toEqual([])
  })
  it('preserves baseline security axes when choosing another model', () => {
    const result = resolveDefaults({ ...catalog, security: { known: true, axes: { mode: ['default'], sandbox: [true] } } }, {}, { model: 'sonnet' })
    expect(result.config.axes).toEqual({ sandbox: true })
    expect(result.sources.model).toBe('Your choice')
  })
})
it('does not send unsupported baseline shapes and blocks unrepresentable security requirements', () => {
  const result = resolveDefaults({ ...catalog, config_options: [], security: { known: true, axes: { mode: ['default'] } } }, {})
  expect(result.config).toEqual({})
  expect(result.blocked).toBe(true)
  expect(result.notices.length).toBeGreaterThan(0)
})
it('preserves boolean false for opaque axis ids, including JavaScript object names', () => {
  const special = { ...catalog, config_options: [...catalog.config_options, { id: '__proto__', name: 'Custom', type: 'boolean', currentValue: true }] }
  const result = resolveDefaults(special, { axes: JSON.parse('{"__proto__":false}') })
  expect(Object.hasOwn(result.config.axes!, '__proto__')).toBe(true)
  expect(result.config.axes!['__proto__']).toBe(false)
})
it('accepts unknown baselines and separates defaults from the session current values', async () => {
  const { usableCatalog } = await import('./defaults')
  expect(usableCatalog(catalog)).toBe(true)
  expect(usableCatalog({ ...catalog, defaults: {} })).toBe(true)
  expect(usableCatalog({ ...catalog, defaults: { ...catalog.defaults, axes: { sandbox: false } }, security: { known: true, axes: { sandbox: [false] } } })).toBe(true)
  const off = { ...catalog, config_options: [{ id: 'sandbox', name: 'Sandbox', type: 'boolean', currentValue: false }], defaults: { axes: { sandbox: false } }, security: { known: true, axes: { sandbox: [false] } } }
  expect(usableCatalog(off)).toBe(true)
})
it.each(['stale', 'unknown'] as const)('refuses a %s pre-start catalog instead of promising its choices', async freshness => {
  const { usableCatalog } = await import('./defaults')
  expect(usableCatalog({ ...catalog, freshness })).toBe(false)
})
it('requires an adapter revision and a nonempty supported settings list', async () => {
  const { usableCatalog } = await import('./defaults')
  expect(usableCatalog({ ...catalog, adapter_revision: '' })).toBe(false)
  expect(usableCatalog({ ...catalog, config_options: [], defaults: {}, security: { known: false, axes: {} } })).toBe(false)
})
it('excludes incompatible model shapes and axis ids that collide with a wire role', async () => {
  const { fieldsOf, usableCatalog } = await import('./defaults')
  const malformed = { ...catalog, config_options: [
    { id: 'model', name: 'Model', category: 'model', type: 'boolean', currentValue: false },
    select('permission', ['default', 'bypass'], 'mode'),
    select('mode', ['low', 'high'], 'thought_level'),
  ] }
  expect(fieldsOf(malformed).map(field => [field.key, field.option.id])).toEqual([['mode', 'permission']])
  expect(usableCatalog(malformed)).toBe(false)
  expect(resolveDefaults(malformed, {}, { mode: 'bypass' }).config).toEqual({ mode: 'bypass' })
})

it('keeps every offered mode for unknown-security runtimes, including modes added later', () => {
  const live = { ...catalog, agent: 'codex', security: { known: false, axes: {} }, config_options: [select('model', ['opus', 'sonnet']), select('mode', ['default', 'agent-full-access'])] }
  const result = resolveDefaults(live, { mode: 'agent-full-access' })
  expect(result.config.mode).toBe('agent-full-access')
  expect(result.sources.mode).toBe('Hat override')
  expect(result.blocked).toBe(false)
})

it('keeps host protocol preference outside adapter axes', async () => {
  const { flatConfig, sparseConfig, resolveDefaults } = await import('./defaults')
  expect(sparseConfig({ protocol: 'v2-v1', fast: false })).toEqual({ protocol: 'v2-v1', axes: { fast: false } })
  expect(flatConfig({ protocol: 'v2' })).toEqual({ protocol: 'v2' })
  const host = { agent: 'custom', adapter_revision: 'one', freshness: 'fresh', defaults: {}, config_options: [], security: { known: true, axes: {} } } as const
  const result = resolveDefaults({ ...host, config_options: [] }, { protocol: 'v2' }, { protocol: 'v2-v1' })
  expect(result.config).toEqual({ protocol: 'v2-v1' })
  expect(result.blocked).toBe(false)
})
