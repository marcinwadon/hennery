import { useState } from 'react'
import { useClient } from '../app-client'
import { putSessionDefaults } from '../api/defaults'
import { ApiFailure } from '../api/errors'
import type { HatItem, HatSessionDefaults, HostCatalog } from '../generated/protocol'
import { fieldsOf, flatConfig, resolveDefaults, securityWarnings, sparseConfig, usableCatalog, supports, valueLabel } from '../lib/defaults'
import { useAccentStyle } from '../lib/theme'
import ProtocolControl from './ProtocolControl'
import ConfirmDialog from './ConfirmDialog'
import { ConfigControl } from './LaunchConfig'

export default function DefaultsEditor({ hat, record, catalog, onSaved, onReload }: {
  hat: HatItem; record: HatSessionDefaults; catalog: HostCatalog | null; onSaved: (record: HatSessionDefaults) => void; onReload: () => void
}) {
  const client = useClient(), [base, setBase] = useState(record), [edits, setEdits] = useState(record.config), [confirm, setConfirm] = useState(false)
  const [conflict, setConflict] = useState(false), [saved, setSaved] = useState(false)
  const accent = useAccentStyle(hat.colour), flat = flatConfig(edits)
  const fields = fieldsOf(catalog), known = new Map(fields.map(f => [f.key, f.option]))
  const keys = [...new Set([...fields.map(f => f.key), ...Object.keys(flat)])].filter(key => key !== 'protocol')
  const preview = catalog ? resolveDefaults(catalog, edits, {}) : null
  const effective = flatConfig(preview?.config ?? {})
  const dirty = JSON.stringify(edits) !== JSON.stringify(base.config)
  const replacements = Object.keys(flat).filter(key => flatConfig(base.config)[key] !== flat[key])
  const invalidReplacement = replacements.filter(key => key !== 'protocol').some(key => !usableCatalog(catalog) || !known.has(key) || !supports(known.get(key)!, flat[key]))
  const warnings = catalog ? [...new Set([...securityWarnings(catalog, edits), ...securityWarnings(catalog, preview?.config ?? {})])] : ['Permission settings are not verified without a fresh host settings preview.']
  return <section className="defaults-editor hn-accent-scope" style={accent} aria-label={`Defaults for ${hat.name}`}><h2><bdi>{hat.name}</bdi></h2><p className="hint">{record.agent} · Saved preferences. Reset removes only this hat’s override.</p>
    <ProtocolControl value={edits.protocol} inherit disabled={hat.purging} onChange={protocol => { setEdits(({ protocol: _old, ...other }) => ({ ...other, ...(protocol ? { protocol } : {}) })); setSaved(false) }} />
    <div className="config-fields">{keys.map(key => {
      const option = known.get(key), overridden = Object.hasOwn(flat, key)
      return <div className="config-field" key={key}>{option ? <ConfigControl option={option} value={overridden ? flat[key] : effective[key]} inherited={!overridden} disabled={!usableCatalog(catalog) || hat.purging} onChange={value => { setEdits(sparseConfig({ ...flat, [key]: value })); setSaved(false) }} /> : <p><bdi>{key}: {String(flat[key])}</bdi> · Stored value unavailable in this preview</p>}
        <span className={'config-source ' + (overridden ? 'override' : 'inherits')}>{overridden ? 'Hat override' : 'Inherits agent default'}</span>
        {overridden && <button type="button" className="btn btn-ghost" disabled={hat.purging} onClick={() => { const next = { ...flat }; delete next[key]; setEdits(sparseConfig(next)); setSaved(false) }}>Reset {option?.name ?? key} to inherited</button>}
        <p className="hint">Preview: <bdi>{valueLabel(catalog, key, effective[key])}</bdi> · {preview?.sources[key] ?? 'Agent default'}</p>
      </div>
    })}</div>
    {keys.length === 0 && <p>Everything inherits. Choose a fresh settings preview to add overrides.</p>}
    {preview?.notices.map(notice => <p role="status" key={notice}><bdi>{notice}</bdi></p>)}
    {warnings.map(warning => <p className="config-warning" key={warning}><bdi>{warning}</bdi></p>)}
    {invalidReplacement && <p role="alert">A chosen value is unavailable in this preview. Choose a fresh supported value or reset it before saving.</p>}
    {hat.purging && <p role="alert">This hat is being purged. Defaults cannot be changed.</p>}
    {conflict && <p role="alert">These defaults changed elsewhere. Your edits are retained. <button className="btn btn-secondary" type="button" onClick={onReload}>Reload and review</button></p>}
    {saved && <p role="status">Defaults saved.</p>}
    <div className="defaults-actions"><button className="btn btn-ghost" type="button" disabled={!dirty} onClick={() => { setEdits(base.config); setConflict(false) }}>Cancel changes</button><button className="btn btn-ghost" type="button" disabled={hat.purging || Object.keys(flat).length === 0} onClick={() => setEdits({})}>Reset all to inherited</button><button className="btn btn-primary" type="button" disabled={!dirty || hat.purging || conflict || invalidReplacement} onClick={() => setConfirm(true)}>Save defaults</button></div>
    {confirm && <ConfirmDialog title={`Save defaults for ${hat.name}`} confirm="Confirm save" action={async () => {
      try { const updated = await putSessionDefaults(client, hat.id, base.agent, { revision: base.revision, config: edits }); setBase(updated); setEdits(updated.config); onSaved(updated); setSaved(true) }
      catch (err) { if (err instanceof ApiFailure && err.code === 'defaults_changed') { setConflict(true); setConfirm(false) } throw err }
    }} onClose={() => setConfirm(false)}><p>Save these launcher preferences for <bdi>{hat.name}</bdi> / <bdi>{base.agent}</bdi>?</p><ul>{[...new Set([...Object.keys(flatConfig(base.config)), ...Object.keys(flat)])].filter(key => flatConfig(base.config)[key] !== flat[key]).map(key => <li key={key}><bdi>{known.get(key)?.name ?? (key === 'protocol' ? 'ACP protocol' : key)}: {String(flatConfig(base.config)[key] ?? 'Inherits')} → {String(flat[key] ?? 'Inherits')}</bdi></li>)}</ul>{warnings.map(warning => <p key={warning}>{warning}</p>)}<p>This affects later operator launches. Existing sessions keep their own settings.</p></ConfirmDialog>}
  </section>
}
