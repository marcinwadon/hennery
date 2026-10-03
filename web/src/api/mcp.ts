// The MCP gateway's connection routes (gateway spec §9, plan 8a), typed.
// Which of them need a fresh step-up is the server's to say: the client
// opens the dialog on 403 `step_up_required` and retries once.
import type {
  CreateMcpConnectionRequest,
  McpConnectionItem,
  UpdateMcpConnectionRequest,
} from '../generated/protocol'
import type { Client } from './client'

const id = (value: string) => encodeURIComponent(value)

export function connections(client: Client): Promise<McpConnectionItem[]> {
  return client.request('GET', '/api/mcp/connections')
}

/** Step-up. */
export function createConnection(client: Client, connection: CreateMcpConnectionRequest): Promise<McpConnectionItem> {
  return client.request('POST', '/api/mcp/connections', connection)
}

/** Step-up when the change names `url`, `cred_kind`, `internal_network`,
 *  `static_header` or `static_prefix`, even with the stored value: send
 *  only what changed. */
export function updateConnection(
  client: Client,
  connectionId: string,
  change: UpdateMcpConnectionRequest,
): Promise<McpConnectionItem> {
  return client.request('PATCH', `/api/mcp/connections/${id(connectionId)}`, change)
}

/** Step-up. Its mounts and token go with it. */
export function deleteConnection(client: Client, connectionId: string): Promise<void> {
  return client.request('DELETE', `/api/mcp/connections/${id(connectionId)}`)
}

/** The whole set of hosts, replacing the one before. */
export function replaceMounts(client: Client, connectionId: string, hostIds: string[]): Promise<McpConnectionItem> {
  return client.request('PUT', `/api/mcp/connections/${id(connectionId)}/mounts`, { host_ids: hostIds })
}

/** A static token, write-only: 204, nothing comes back. Step-up. */
export function setToken(client: Client, connectionId: string, token: string): Promise<void> {
  return client.request('PUT', `/api/mcp/connections/${id(connectionId)}/credential`, { token })
}
