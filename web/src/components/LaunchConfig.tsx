import { useEffect, useId, useRef, useState } from 'react'
import type { ConfigValue, SessionConfig } from '../generated/protocol'
import type { ConfigOption } from '../lib/catalog'
import { fieldsOf, flatConfig, resolveDefaults, sparseConfig, type Picks, valueLabel } from '../lib/defaults'
import type { LaunchContext } from './CatalogContext'
import ProtocolControl from './ProtocolControl'
import { protocolValue } from '../lib/protocol'
import ComposerSettings, { SettingsSummary } from './ComposerSettings'
import { modelLabel } from '../lib/agent'

export function ConfigControl({ option, value, inherited, disabled, onChange }: { option: ConfigOption; value: ConfigValue | undefined; inherited: boolean; disabled?: boolean; onChange: (value: ConfigValue) => void }) {
  const id = useId()
  if (option.kind === 'boolean') return <label className="config-boolean" htmlFor={id}><input id={id} type="checkbox" checked={value === true} disabled={disabled} onChange={e => onChange(e.target.checked)} />{option.name}{value === undefined ? ' · agent default' : inherited ? ' · inherited' : ''}</label>
  const known = typeof value === 'string' && option.choices.some(c => c.value === value)
  return <div className="config-select"><label htmlFor={id}>{option.name}</label><select id={id} disabled={disabled} value={typeof value === 'string' ? value : ''} onChange={e => onChange(e.target.value)}>
    {!known && <option value={typeof value === 'string' ? value : ''}>{value === undefined ? 'Agent default' : `${String(value)} · unavailable`}</option>}
    {option.choices.map(choice => <option key={choice.value} value={choice.value}>{choice.name}</option>)}
  </select></div>
}
export default function LaunchConfig({ context, picks, onPicks, expanded, onExpanded }: {
  context: LaunchContext; picks: Picks; onPicks: (picks: Picks) => void; expanded: boolean; onExpanded: (value: boolean) => void
}) {
  const trigger = useRef<HTMLButtonElement>(null)
  const age = useCatalogAge(context.catalog?.saved_at)
  const result = context.catalog ? resolveDefaults(context.catalog, context.defaults?.config ?? {}, picks) : null
  const effective = flatConfig(result?.config ?? {})
  const requested = Object.keys(picks).filter(key => picks[key] !== null)
  const summaryFields = fieldsOf(context.catalog).filter(({ key, option }) => Object.hasOwn(effective, key) && (['model', 'mode', 'effort', 'reasoning_effort'].includes(key) || ['model', 'mode', 'thought_level'].includes(option.category ?? '')))
  const requestedProtocol = protocolValue(picks.protocol === null ? undefined : picks.protocol ?? context.defaults?.config.protocol)
  const summary = [...summaryFields.map(({ key }) => modelLabel(valueLabel(context.catalog, key, effective[key]))), ...(requestedProtocol ? [`ACP ${requestedProtocol === 'v2-v1' ? 'v2 / v1' : requestedProtocol}`] : [])].join(' · ')
  const warning = !!context.error || context.state === 'unavailable' || !!result?.blocked || !!result?.notices.length || !!result?.warnings.length
  const mode = summaryFields.find(({ key, option }) => key === 'mode' || option.category === 'mode')
  return <section className="launch-config" aria-label="Session settings"><SettingsSummary trigger={trigger} summary={summary || 'Agent defaults'} expanded={expanded} open={() => onExpanded(true)} warning={warning} launch />
    <div className="catalog-age"><span>{age}</span> {context.refresh && <button type="button" className="btn btn-ghost" disabled={context.refreshing} onClick={context.refresh}>{context.refreshing ? 'Refreshing…' : 'Refresh'}</button>}</div>
    {expanded && <ComposerSettings trigger={trigger} close={() => onExpanded(false)}>
    <ProtocolControl value={requestedProtocol} onChange={value => onPicks({ ...picks, protocol: value ?? null })} />
    {context.error && <p role="alert">{context.error}</p>}
    {!context.catalog ? <p role="status">Agent defaults are unknown. Hat overrides and your choices will still be requested and checked after starting.</p> : <>
      <p className="config-readback">Requested for this start: <bdi>{summary || 'Agent defaults'}</bdi></p>
      {fieldsOf(context.catalog).length < (context.catalog?.config_options.length ?? 0) && <p className="hint">Some reported settings cannot be represented here. Review them in the agent.</p>}
      {expanded && <div className="config-fields">{fieldsOf(context.catalog).map(({ key, option }) => <div className="config-field" key={key}><ConfigControl option={option} value={effective[key]} inherited={!requested.includes(key)} onChange={value => onPicks({ ...picks, [key]: value })} /><span className="config-source">{result?.sources[key] ?? 'Agent default'}</span>{(requested.includes(key) || result?.sources[key] === 'Hat override') && <button type="button" className="btn btn-ghost" onClick={() => onPicks({ ...picks, [key]: null })}>Use agent default for {option.name}</button>}</div>)}</div>}
      {!!result?.warnings.length && <p className="config-warning"><span aria-hidden="true">⚠ </span>{mode ? `${valueLabel(context.catalog, mode.key, effective[mode.key])}: the agent won't ask before acting.` : 'These settings give the agent more permission to act.'}</p>}
    </>}
    </ComposerSettings>}
    {result?.notices.map(notice => <p role={result.blocked ? 'alert' : 'status'} key={notice}><bdi>{notice}</bdi></p>)}
  </section>
}
export function requestFor(context: LaunchContext, picks: Picks): SessionConfig | null {
  if (!context.catalog) {
    const flat: Record<string, ConfigValue> = Object.assign(Object.create(null), flatConfig(context.defaults?.config ?? {}))
    for (const [key, value] of Object.entries(picks)) {
      if (value === null) delete flat[key]
      else flat[key] = value
    }
    return sparseConfig(flat)
  }
  const result = resolveDefaults(context.catalog, context.defaults?.config ?? {}, picks)
  return result.blocked ? null : result.config
}

export function useCatalogAge(savedAt: number | undefined): string {
  const [, tick] = useState(0)
  useEffect(() => {
    if (savedAt === undefined) return
    const timer = window.setInterval(() => tick(value => value + 1), 60_000)
    return () => window.clearInterval(timer)
  }, [savedAt])
  return catalogAge(savedAt)
}

export function catalogAge(savedAt: number | undefined): string {
  if (savedAt === undefined) return ''
  const seconds = Math.max(0, Math.floor(Date.now() / 1000 - savedAt))
  return seconds < 60 ? 'saved just now' : seconds < 3600 ? `saved ${Math.floor(seconds / 60)} min ago` : seconds < 86400 ? `saved ${Math.floor(seconds / 3600)} h ago` : `saved ${Math.floor(seconds / 86400)} d ago`
}
