// Like an in-flight send, a config request outlives navigation away from its
// session. Keep only pending operations, never drafts or durable preferences.
import type { Client } from '../api/client'
import { setConfig } from '../api/turns'
import type { ConfigValue, SessionCatalog } from '../generated/protocol'
import { configOptions, type ConfigOption } from './catalog'
import { configRefusal } from '../components/composerWords'

type Result = { catalog?: SessionCatalog; error: string | null }
export interface ConfigChange {
  values: Record<string, ConfigValue>
  result: Promise<Result>
  valid: boolean
}
const pending = new Map<string, ConfigChange>()

export function configuringFor(id: string): ConfigChange | undefined { return pending.get(id) }

export function changeConfig(client: Client, id: string, option: ConfigOption, value: ConfigValue): ConfigChange {
  const held = pending.get(id)
  if (held) return held
  const change: ConfigChange = { values: { [option.id]: value }, result: Promise.resolve({ error: null }), valid: true }
  // Reserve synchronously, before either a second pick or submit can run.
  pending.set(id, change)
  change.result = apply().finally(() => { if (pending.get(id) === change) pending.delete(id) })
  return change

  async function apply(): Promise<Result> {
    try {
      const catalog = await setConfig(client, id, option.id, value)
      if (catalog.session_id !== id) throw new Error('The agent returned settings for another session.')
      const actual = configOptions(catalog).find(candidate => candidate.id === option.id)
      const error = actual?.current === value ? null : actual
        ? `${option.name}: the agent kept ${String(actual.current)} instead of ${String(value)}. Review its current setting before sending.`
        : `${option.name}: the agent did not confirm this setting. Review the current settings before sending.`
      return { catalog, error }
    } catch (err) {
      return { error: `${option.name}: ${configRefusal(err)}` }
    }
  }
}

export function forgetAllConfigChanges(): void {
  for (const change of pending.values()) change.valid = false
  pending.clear()
}
