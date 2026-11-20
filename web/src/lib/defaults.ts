import type { ConfigValue, HostCatalog, SessionConfig } from '../generated/protocol'
import { protocolValue } from './protocol'
import { catalogFields, configOptions, type ConfigOption } from './catalog'

export type Picks = Record<string, ConfigValue | null>
export type Source = 'Agent default' | 'Hat override' | 'Your choice'
export interface ResolvedDefaults {
  config: SessionConfig
  sources: Record<string, Source>
  notices: string[]
  blocked: boolean
  warnings: string[]
}
export function fieldsOf(catalog: Pick<HostCatalog, 'config_options'> | null) {
  return catalogFields(configOptions(catalog ? { session_id: '', commands: [], config_options: catalog.config_options } : null))
}
export function securityKeys(catalog: Pick<HostCatalog, 'security'>): Set<string> {
  return new Set(['mode', ...Object.keys(catalog.security.axes)])
}
export function flatConfig(config: SessionConfig): Record<string, ConfigValue> {
  return { ...(config.axes ?? {}), ...(config.protocol ? { protocol: config.protocol } : {}), ...(config.model == null ? {} : { model: config.model }), ...(config.mode == null ? {} : { mode: config.mode }) }
}
export function sparseConfig(flat: Record<string, ConfigValue>): SessionConfig {
  const { model, mode, protocol, ...axes } = flat
  return { ...(protocolValue(protocol) ? { protocol: protocolValue(protocol) } : {}), ...(typeof model === 'string' ? { model } : {}), ...(typeof mode === 'string' ? { mode } : {}), ...(Object.keys(axes).length ? { axes } : {}) }
}
export function supports(option: ConfigOption, value: ConfigValue): boolean {
  return option.kind === 'boolean' ? typeof value === 'boolean' : typeof value === 'string' && option.choices.some(choice => choice.value === value)
}
export function usableCatalog(catalog: HostCatalog | null): catalog is HostCatalog {
  if (catalog === null || !['fresh', 'cached'].includes(catalog.freshness) || catalog.adapter_revision === '') return false
  const fields = fieldsOf(catalog), defaults = flatConfig(catalog.defaults)
  const required = new Set(['model', 'mode'].filter(key => Object.hasOwn(defaults, key)))
  return fields.length > 0 && [...required].every(key => {
    const option = fields.find(field => field.key === key)?.option
    return option !== undefined && supports(option, defaults[key])
  })
}
export function securityWarnings(catalog: HostCatalog, config: SessionConfig): string[] {
  if (!catalog.security.known) return ['This agent’s permission baseline is unknown. Review its access before saving or starting.']
  const flat = flatConfig(config)
  return Object.entries(catalog.security.axes).filter(([key, allowed]) => Object.hasOwn(flat, key) && !allowed.includes(flat[key])).map(([key]) =>
    `${fieldsOf(catalog).find(f => f.key === key)?.option.name ?? key} is outside the agent’s permission baseline. The agent may act with fewer prompts.`)
}
/** Only inherited, nonsecurity probe axes are pruned on a model change. */
export function resolveDefaults(catalog: HostCatalog, stored: SessionConfig, picks: Picks = {}): ResolvedDefaults {
  const fields = fieldsOf(catalog), offered = new Map(fields.map(field => [field.key, field.option]))
  const baseline = flatConfig(catalog.defaults), saved = flatConfig(stored)
  const flat: Record<string, ConfigValue> = Object.create(null), sources: Record<string, Source> = Object.create(null), notices: string[] = []
  let blocked = false
  const security = securityKeys(catalog)
  for (const [key, value] of Object.entries(baseline)) {
    if (key === 'protocol' && protocolValue(value)) { flat[key] = value; sources[key] = 'Agent default'; continue }
    const option = offered.get(key)
    if (option && supports(option, value)) { flat[key] = value; sources[key] = 'Agent default' }
    else {
      notices.push(`The agent default for ${key} cannot be represented by this catalog. Review this setting before starting.`)
      if (security.has(key)) blocked = true
    }
  }
  // Model is resolved first, independently of catalog ordering.
  const keys = [...new Set(['model', ...Object.keys(saved), ...Object.keys(picks)])]
  const overlay = (key: string) => {
    if (key === 'protocol') {
      const value = Object.hasOwn(picks, key) ? picks[key] : saved[key]
      if (protocolValue(value)) { flat[key] = value as string; sources[key] = Object.hasOwn(picks, key) ? 'Your choice' : 'Hat override' }
      return
    }
    const explicit = Object.hasOwn(picks, key), value = explicit ? picks[key] : saved[key]
    if (value === undefined || value === null) return
    const option = offered.get(key)
    if (!option || !supports(option, value)) {
      const strict = security.has(key)
      const fallback = Object.hasOwn(flat, key) ? valueLabel(catalog, key, flat[key]) : 'agent default'
      notices.push(`${explicit ? 'Chosen' : 'Saved'} ${option?.name ?? key} “${String(value)}” is unavailable on this host; ${explicit ? 'choose another value before starting.' : strict ? 'it will still be requested and must be confirmed before the first prompt.' : `using ${fallback}.`}`)
      if (explicit) blocked = true
      else if (strict) { flat[key] = value; sources[key] = 'Hat override' }
      return
    }
    flat[key] = value
    sources[key] = explicit ? 'Your choice' : 'Hat override'
  }
  overlay('model')
  if (flat.model !== baseline.model) {
    for (const key of Object.keys(catalog.defaults.axes ?? {})) {
      if (!security.has(key)) { delete flat[key]; delete sources[key] }
    }
  }
  for (const key of keys) if (key !== 'model') overlay(key)
  const config = sparseConfig(flat)
  return { config, sources, notices, blocked, warnings: securityWarnings(catalog, config) }
}
/** Use adapter display labels in the UI; exact ids remain in requests. */
export function valueLabel(catalog: Pick<HostCatalog, 'config_options'> | null, key: string, value: ConfigValue | undefined): string {
  if (value === undefined) return 'Agent default'
  if (key === 'protocol') return value === 'v2-v1' ? 'v2 / v1' : String(value)
  if (typeof value === 'boolean') return value ? 'On' : 'Off'
  const option = fieldsOf(catalog).find(field => field.key === key)?.option
  return option?.kind === 'select' ? option.choices.find(choice => choice.value === value)?.name ?? value : value
}
