// What the MCP screen decides without the server (gateway spec §2, §7): the
// words for a connection's state, the form a connection is edited in, and
// what a change sends. The server checks every field again; these only keep
// a request to what the operator changed.
import type {
  CreateMcpConnectionRequest,
  McpConnectionItem,
  McpCredKind,
  UpdateMcpConnectionRequest,
} from '../generated/protocol'

/** A connection's state in words (gateway spec §7). `not_connected` is a
 *  connection no request has gone through yet (static and `none` ones are
 *  never probed; live traffic sets their state). `error` never asks for a
 *  reconnect: a click will not fix an outage. A state a newer collector
 *  adds reads as unknown. */
export function statusLabel(status: string): string {
  switch (status) {
    case 'not_connected':
      return 'Not used yet'
    case 'ok':
      return 'Working'
    case 'needs_auth':
      return 'Needs sign-in again'
    case 'error':
      return 'Is failing'
    default:
      return 'Unknown state'
  }
}

/** How a connection authenticates, in words; a kind a newer collector adds
 *  is named as it is. */
export function kindLabel(kind: string): string {
  switch (kind) {
    case 'none':
      return 'None'
    case 'static':
      return 'Token'
    case 'oauth_dcr':
    case 'oauth_client':
      return 'OAuth'
    default:
      return kind
  }
}

/** The longest vendor text shown (the status note): the rest is cut. */
export const NOTE_LIMIT = 300

/** `text` cut to `NOTE_LIMIT` characters (code points), with an ellipsis
 *  when it was longer. */
export function capped(text: string): string {
  const chars = [...text]
  return chars.length <= NOTE_LIMIT ? text : chars.slice(0, NOTE_LIMIT).join('') + '…'
}

/** What the connection form holds. */
export interface ConnectionForm {
  label: string
  url: string
  credKind: McpCredKind
  header: string
  prefix: string
  /** Every tool (`tool_allowlist: null`); else `tools`, which may be none. */
  everyTool: boolean
  /** Tool names, one per line or comma-separated. */
  tools: string
  internalNetwork: boolean
}

export const NEW_FORM: ConnectionForm = {
  label: '',
  url: '',
  credKind: 'static',
  header: 'Authorization',
  prefix: 'Bearer ',
  everyTool: true,
  tools: '',
  internalNetwork: false,
}

export function formOf(item: McpConnectionItem): ConnectionForm {
  return {
    label: item.label,
    url: item.url,
    credKind: item.cred_kind,
    header: item.static_header,
    prefix: item.static_prefix,
    everyTool: item.tool_allowlist === null,
    tools: (item.tool_allowlist ?? []).join('\n'),
    internalNetwork: item.internal_network,
  }
}

/** The tool names `text` lists: split on lines and commas, trimmed, blanks
 *  and repeats dropped, the order kept. */
export function toolList(text: string): string[] {
  const seen = new Set<string>()
  for (const name of text.split(/[\n,]/)) {
    const trimmed = name.trim()
    if (trimmed !== '') seen.add(trimmed)
  }
  return [...seen]
}

function allowlistOf(form: ConnectionForm): string[] | null {
  return form.everyTool ? null : toolList(form.tools)
}

export function createRequest(form: ConnectionForm, slug: string, hatId: string): CreateMcpConnectionRequest {
  const request: CreateMcpConnectionRequest = {
    slug: slug.trim(),
    label: form.label.trim(),
    url: form.url.trim(),
    hat_id: hatId,
    cred_kind: form.credKind,
    tool_allowlist: allowlistOf(form),
    internal_network: form.internalNetwork,
  }
  if (form.credKind === 'static') {
    request.static_header = form.header.trim()
    request.static_prefix = form.prefix
  }
  return request
}

const sameList = (a: string[] | null, b: string[] | null) =>
  a === null || b === null ? a === b : a.length === b.length && a.every((x, i) => x === b[i])

/** Only what `form` changed of `item`: naming a field the server steps up
 *  for, even unchanged, would ask for a step-up a label edit does not
 *  need. The header and prefix count only for a token connection. */
export function changeOf(item: McpConnectionItem, form: ConnectionForm): UpdateMcpConnectionRequest {
  const change: UpdateMcpConnectionRequest = {}
  const label = form.label.trim()
  if (label !== item.label) change.label = label
  const url = form.url.trim()
  if (url !== item.url) change.url = url
  if (form.credKind !== item.cred_kind) change.cred_kind = form.credKind
  if (form.credKind === 'static') {
    const header = form.header.trim()
    if (header !== item.static_header) change.static_header = header
    if (form.prefix !== item.static_prefix) change.static_prefix = form.prefix
  }
  const allowlist = allowlistOf(form)
  if (!sameList(allowlist, item.tool_allowlist)) change.tool_allowlist = allowlist
  if (form.internalNetwork !== item.internal_network) change.internal_network = form.internalNetwork
  return change
}

/** `scheme://host[:port]` of `url`, or `null` when it does not parse. */
export function originOf(url: string): string | null {
  try {
    const origin = new URL(url).origin
    return origin === 'null' ? null : origin
  } catch {
    return null
  }
}

/** Whether the server deletes the stored token for `change`: another kind,
 *  or a URL of another origin (gateway spec §4.6). A new URL that does not
 *  parse deletes nothing: the server refuses the change whole. A stored one
 *  that does not parse cannot be compared, and counts as another origin. */
export function dropsToken(item: McpConnectionItem, change: UpdateMcpConnectionRequest): boolean {
  if (change.cred_kind !== undefined && change.cred_kind !== item.cred_kind) return true
  if (change.url === undefined) return false
  const before = originOf(item.url)
  if (before === null) return true
  const after = originOf(change.url)
  return after !== null && after !== before
}

/** The mounts with one host ticked or not, sorted: the set a save sends
 *  is the connection's own, so a host this screen does not list keeps its
 *  mount. */
export function mountsWith(mounts: string[], hostId: string, on: boolean): string[] {
  const set = new Set(mounts)
  if (on) set.add(hostId)
  else set.delete(hostId)
  return [...set].sort()
}

/** The name agents see the connection as. */
export const serverName = (slug: string) => `hennery-${slug}`
