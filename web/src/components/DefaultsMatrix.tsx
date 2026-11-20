import { useState } from 'react'
import type { ConfigValue, HatItem, HatSessionDefaults, HostCatalog, SessionConfig } from '../generated/protocol'
import { fieldsOf, flatConfig, resolveDefaults, securityWarnings, sparseConfig, usableCatalog, valueLabel } from '../lib/defaults'
import { putSessionDefaults } from '../api/defaults'
import { useClient } from '../app-client'
import ProtocolControl from './ProtocolControl'
import ConfirmDialog from './ConfirmDialog'
import { ConfigControl } from './LaunchConfig'

export default function DefaultsMatrix({ hats, records, catalog, onSaved }: {
  hats: HatItem[]; records: Record<string, HatSessionDefaults>; catalog: HostCatalog | null; onSaved: (record: HatSessionDefaults) => void
}) {
  const client = useClient()
  const [change, setChange] = useState<{ hat: HatItem; record: HatSessionDefaults; config: SessionConfig; setting: string } | null>(null)
  const fields = fieldsOf(catalog)
  function propose(hat: HatItem, key: string, setting: string, value?: ConfigValue) {
    const record = records[hat.id]
    if (!record || hat.purging) return
    const next = { ...flatConfig(record.config) }
    if (value === undefined) delete next[key]; else next[key] = value
    setChange({ hat, record, config: sparseConfig(next), setting })
  }
  return <>
    <div className="defaults-matrix-scroll"><table className="defaults-matrix"><caption>Click a hat cell to override. Reset follows the agent default again.</caption>
      <thead><tr><th scope="col">Setting</th><th scope="col">Agent default</th>{hats.map(hat => <th scope="col" key={hat.id}><bdi>{hat.name}</bdi></th>)}</tr></thead>
      <tbody><tr><th scope="row">ACP protocol</th><td>v1<span className="config-source">default</span></td>{hats.map(hat => <td key={hat.id}><ProtocolControl value={records[hat.id]?.config.protocol} inherit disabled={!records[hat.id] || hat.purging} onChange={value => propose(hat, 'protocol', 'ACP protocol', value)} /></td>)}</tr>{fields.map(({ key, option }) => <tr key={key}><th scope="row">{option.name}</th><td><bdi>{valueLabel(catalog, key, flatConfig(catalog?.defaults ?? {})[key])}</bdi><span className="config-source">base</span></td>{hats.map(hat => {
        const stored = records[hat.id], result = stored && catalog ? resolveDefaults(catalog, stored.config, {}) : null
        const overridden = !!stored && Object.hasOwn(flatConfig(stored.config), key)
        return <td key={hat.id} className={overridden ? 'matrix-override' : 'matrix-inherits'}><div role="group" aria-label={`${option.name} for ${hat.name}`}>
          <ConfigControl option={option} value={result ? flatConfig(result.config)[key] : undefined} inherited={!overridden} disabled={!stored || !usableCatalog(catalog) || hat.purging} onChange={value => propose(hat, key, option.name, value)} />
          {overridden ? <button type="button" className="matrix-reset" disabled={hat.purging} onClick={() => propose(hat, key, option.name)}>× reset</button> : <button type="button" className="matrix-pin" disabled={!result || hat.purging || !usableCatalog(catalog) || !Object.hasOwn(flatConfig(result.config), key)} onClick={() => { if (result) propose(hat, key, option.name, flatConfig(result.config)[key]) }}>inherits</button>}
          {overridden && result?.sources[key] !== 'Hat override' && <span className="config-source">Stored override unavailable</span>}
        </div></td>
      })}</tr>)}{fields.length === 0 && <tr><td colSpan={hats.length + 2}>A connected host is needed to preview agent settings. Saved overrides remain available below.</td></tr>}</tbody>
    </table></div>
    <section className="defaults-previews" aria-label="New session preview"><h2>Preview · a new session starts with</h2><div>{hats.map(hat => {
      const record = records[hat.id], result = catalog && record ? resolveDefaults(catalog, record.config, {}) : null
      return <article key={hat.id}><h3><bdi>{hat.name}</bdi></h3>{result ? <>{fields.filter(({ key }) => Object.hasOwn(flatConfig(result.config), key)).map(({ key }) => <p key={key}><bdi>{valueLabel(catalog, key, flatConfig(result.config)[key])}</bdi><span> · {result.sources[key] === 'Hat override' ? hat.name : 'default'}</span></p>)}{result.notices.map(notice => <p role="status" key={notice}><bdi>{notice}</bdi></p>)}</> : <p>Preview unavailable</p>}</article>
    })}</div></section>
    {change && <ConfirmDialog title={`Save defaults for ${change.hat.name}`} confirm="Confirm save" action={async () => onSaved(await putSessionDefaults(client, change.hat.id, change.record.agent, { revision: change.record.revision, config: change.config }))} onClose={() => setChange(null)}>
      <p>Save <bdi>{change.setting}</bdi> for <bdi>{change.hat.name}</bdi> / <bdi>{change.record.agent}</bdi>?</p>
      {catalog && securityWarnings(catalog, change.config).map(warning => <p key={warning}><bdi>{warning}</bdi></p>)}
      <p>Later operator launches use this value. Existing sessions keep their own settings.</p>
    </ConfirmDialog>}
  </>
}
