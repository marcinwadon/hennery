import type { SessionCatalog } from '../generated/protocol'
import type { SessionConfigState } from '../hooks/useSessionConfig'
import { configOptions, type ConfigOption } from '../lib/catalog'
import SessionConfig from './SessionConfig'

function actualValue(option: ConfigOption): string {
  return option.kind === 'boolean' ? (option.current ? 'on' : 'off')
    : option.choices.find(choice => choice.value === option.current)?.name || String(option.current)
}

export default function SessionSettings({ catalog, config }: { catalog: SessionCatalog | null; config: SessionConfigState }) {
  const options = configOptions(catalog)
  return <section className="session-settings" aria-label="Session settings" tabIndex={-1}>
    <h3>Session settings</h3>
    <div className="composer-controls" role="group" aria-label="Session settings">
      {options.map(option => <SessionConfig key={option.id} option={option} value={Object.hasOwn(config.values, option.id) ? config.values[option.id] : option.current} busy={config.pending} onPick={value => config.pick(option, value)} />)}
    </div>
    <p className="config-readback">Agent reports: <bdi>{catalog === null ? 'Loading settings…' : options.map(option => `${option.name}: ${actualValue(option)}`).join(' · ') || 'No settings reported'}</bdi></p>
    {config.pending && <p className="config-pending" role="status">Applying session settings…</p>}
    {config.error && <p className="form-error" role="alert"><bdi>{config.error}</bdi></p>}
  </section>
}
