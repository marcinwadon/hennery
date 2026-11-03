import type { ItemOf } from './types'

const failures = {
  unsupported: 'This host does not support startup context.',
  unavailable: 'The plug-in was unavailable or returned invalid context.',
  timed_out: 'The plug-in did not respond within two seconds.',
  revoked: 'The plug-in grant or session scope changed before delivery.',
}

export default function PluginContext({ item }: { item: ItemOf<'plugin_context'> }) {
  const { startup } = item
  return (
    <aside aria-label="Plug-in context" className="unrec fade-in">
      <p><strong>Plug-in context — untrusted reference data</strong></p>
      {startup.status !== 'delivered' && <p role="status">Plug-in context: {failures[startup.status]} Your prompt proceeds without it.</p>}
      {startup.block?.context.entries.map((entry, i) => (
        <div key={i}>
          <pre className="user-text">{entry.text}</pre>
          <ul aria-label="Source provenance">
            {entry.provenance.map((source, j) => <li key={j}>
              {source.kind === 'unverified' || source.origin.kind === 'plugin_context'
                ? 'Unverified source'
                : <><span>{source.origin.kind === 'operator' ? 'Operator source' : source.origin.kind === 'external_client' ? 'External client source' : source.origin.kind === 'parent' ? 'Parent source' : 'Child report source'}</span><pre>{JSON.stringify(source)}</pre></>}
            </li>)}
          </ul>
        </div>
      ))}
      {startup.block?.context.truncated && <p className="item-note">Some context was redacted or cut.</p>}
    </aside>
  )
}
