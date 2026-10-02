// A session's catalogue, read defensively (ACP core §7, §9). The collector
// stores the adapter's ACP objects as they came (`SessionConfigOption`,
// `AvailableCommand`) and never reads them, so here they are `unknown`: an
// entry that does not have the shape is skipped, never a crash.
//
// The shapes read (agent-client-protocol, as serialized):
// - a select: `{id, name, description?, category?, type: "select",
//   currentValue: string, options}`, where `options` is a flat list of
//   `{value, name, description?}` or a list of groups `{group, name, options}`;
// - a boolean: `{id, name, description?, category?, type: "boolean",
//   currentValue: boolean}`;
// - a command: `{name, description?, input?: {hint}}`.
import type { SessionCatalog } from '../generated/protocol'

export interface Choice {
  value: string
  name: string
  description?: string
  /** The name of the group it is listed under, if the adapter grouped them. */
  group?: string
}

interface OptionBase {
  id: string
  name: string
  description?: string
  category?: string
}

export type ConfigOption =
  | (OptionBase & { kind: 'select'; current: string; choices: Choice[] })
  | (OptionBase & { kind: 'boolean'; current: boolean })

export interface Command {
  name: string
  description?: string
  /** What the command takes after its name, as the adapter hints it. */
  hint?: string
}

type Obj = Record<string, unknown>

function isObj(value: unknown): value is Obj {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function str(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined
}

function choice(value: unknown, group?: string): Choice | null {
  if (!isObj(value) || typeof value.value !== 'string') return null
  return {
    value: value.value,
    name: str(value.name) || value.value,
    description: str(value.description),
    ...(group === undefined ? {} : { group }),
  }
}

function choices(list: unknown): Choice[] {
  if (!Array.isArray(list)) return []
  const out: Choice[] = []
  for (const entry of list) {
    if (isObj(entry) && Array.isArray(entry.options)) {
      const group = str(entry.name) || str(entry.group) || ''
      for (const inner of entry.options) {
        const c = choice(inner, group)
        if (c) out.push(c)
      }
    } else {
      const c = choice(entry)
      if (c) out.push(c)
    }
  }
  return out
}

/** One config option, or null when it is not one this build can show. */
export function parseOption(value: unknown): ConfigOption | null {
  if (!isObj(value) || typeof value.id !== 'string' || value.id === '') return null
  const base: OptionBase = {
    id: value.id,
    name: str(value.name) || value.id,
    description: str(value.description),
    category: str(value.category),
  }
  if (value.type === 'boolean' && typeof value.currentValue === 'boolean') {
    return { ...base, kind: 'boolean', current: value.currentValue }
  }
  if (value.type === 'select' && typeof value.currentValue === 'string') {
    const list = choices(value.options)
    // A select with nothing to pick is noise; adapters do report empty ones.
    if (list.length === 0) return null
    return { ...base, kind: 'select', current: value.currentValue, choices: list }
  }
  return null
}

/** The option that plays `category`'s part, as the host decides it: the
 *  first of that category, else one with the conventional id that no other
 *  category claims (no category, or a custom `_`-prefixed one). */
function axis(options: ConfigOption[], category: string): ConfigOption | undefined {
  const uncategorized = (o: ConfigOption) => o.category === undefined || o.category.startsWith('_')
  return options.find((o) => o.category === category) ?? options.find((o) => uncategorized(o) && o.id === category)
}

/** The switchers to show: the model, the mode, then every other option by
 *  id. The order is stable whatever order the adapter reports them in. */
export function configOptions(catalog: SessionCatalog | null | undefined): ConfigOption[] {
  const raw = Array.isArray(catalog?.config_options) ? catalog.config_options : []
  const seen = new Set<string>()
  const options: ConfigOption[] = []
  for (const entry of raw) {
    const option = parseOption(entry)
    if (option && !seen.has(option.id)) {
      seen.add(option.id)
      options.push(option)
    }
  }
  const model = axis(options, 'model')
  const mode = axis(options, 'mode')
  const rest = options
    .filter((o) => o !== model && o !== mode)
    .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
  return [...(model ? [model] : []), ...(mode && mode !== model ? [mode] : []), ...rest]
}

/** The slash commands, each name once, in the adapter's order. */
export function commands(catalog: SessionCatalog | null | undefined): Command[] {
  const raw = Array.isArray(catalog?.commands) ? catalog.commands : []
  const seen = new Set<string>()
  const out: Command[] = []
  for (const entry of raw) {
    if (!isObj(entry) || typeof entry.name !== 'string' || entry.name === '' || seen.has(entry.name)) continue
    seen.add(entry.name)
    out.push({
      name: entry.name,
      description: str(entry.description),
      hint: isObj(entry.input) ? str(entry.input.hint) : undefined,
    })
  }
  return out
}

/** The commands a `/query` lists: names that start with it first, then
 *  names that contain it, each in the adapter's order. */
export function matchCommands(list: Command[], query: string): Command[] {
  const q = query.toLowerCase()
  const starts = list.filter((c) => c.name.toLowerCase().startsWith(q))
  const contains = list.filter((c) => !c.name.toLowerCase().startsWith(q) && c.name.toLowerCase().includes(q))
  return [...starts, ...contains]
}
