// Server answers for the management screens' tests: one of each item, with
// every field set (an absent optional field makes for false passes), and
// overridable per test.
import type { HatItem, HostItem, McpConnectionItem, PurgePreview } from './generated/protocol'

export function host(over: Partial<HostItem> = {}): HostItem {
  return {
    host_id: 'host-1',
    name: 'laptop',
    platform: 'linux-x64',
    host_version: '0.1.0',
    capabilities: ['resolve_path'],
    default_hat_id: 'hat-a',
    workspace_roots: [],
    connected: true,
    created_at: '2026-10-01T10:00:00Z',
    last_seen_at: '2026-10-02T10:00:00Z',
    ...over,
  }
}

export function hat(over: Partial<HatItem> = {}): HatItem {
  return {
    id: 'hat-b',
    name: 'Work',
    colour: '#336699',
    created_at: '2026-10-01T10:00:00Z',
    default_for_new_hosts: false,
    purging: false,
    ...over,
  }
}

export function preview(over: Partial<PurgePreview> = {}): PurgePreview {
  return {
    hat_id: 'hat-b',
    purging: false,
    sessions: 3,
    running: [],
    rules: 2,
    recents: 1,
    unassigned: [],
    unassigned_count: 0,
    ...over,
  }
}

export function connection(over: Partial<McpConnectionItem> = {}): McpConnectionItem {
  return {
    id: 'conn-0000000000000001',
    slug: 'docs',
    label: 'Docs',
    url: 'https://mcp.example.com/mcp',
    hat_id: 'hat-a',
    cred_kind: 'static',
    static_header: 'Authorization',
    static_prefix: 'Bearer ',
    tool_allowlist: null,
    internal_network: false,
    status: 'not_connected',
    status_note: 'a note',
    account_label: 'someone',
    status_at: '2026-10-03T10:00:00Z',
    created_at: '2026-10-03T10:00:00Z',
    updated_at: '2026-10-03T10:00:00Z',
    has_credential: false,
    mounts: [],
    ...over,
  }
}
