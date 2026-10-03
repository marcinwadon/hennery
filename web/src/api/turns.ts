// What the session view asks of a session (ACP core §9): a prompt, a
// cancel, a config switch, a resume (the footer's, and the composer's
// "Resume and send"), and the header menu's park, close and delete. Every id
// in a path is encoded: ids are server data, never trusted to be path-safe.
import type { Client } from './client'
import type {
  CancelResponse,
  ConfigValue,
  DeleteResult,
  LifecycleResponse,
  PromptResponse,
  SessionCatalog,
} from '../generated/protocol'

const enc = encodeURIComponent

/** One ACP content block of a prompt, in order: a run of text, or an image
 *  as base64 (ACP core §7). */
export type PromptBlock = { type: 'text'; text: string } | { type: 'image'; mimeType: string; data: string }

/** `POST /api/sessions/{id}/prompt` → 202 `{turn_id}`. */
export function prompt(client: Client, id: string, content: PromptBlock[]): Promise<PromptResponse> {
  return client.request<PromptResponse>('POST', `/api/sessions/${enc(id)}/prompt`, { content })
}

/** `POST /api/sessions/{id}/cancel` → 202 with the open turn's real outcome. */
export function cancel(client: Client, id: string): Promise<CancelResponse> {
  return client.request<CancelResponse>('POST', `/api/sessions/${enc(id)}/cancel`)
}

/** `POST /api/sessions/{id}/config` → 202 with the session's catalogue as the
 *  agent now reports it. */
export function setConfig(client: Client, id: string, configId: string, value: ConfigValue): Promise<SessionCatalog> {
  return client.request<SessionCatalog>('POST', `/api/sessions/${enc(id)}/config`, { config_id: configId, value })
}

/** `POST /api/sessions/{id}/resume` → 202 once the session has started again. */
export function resume(client: Client, id: string): Promise<LifecycleResponse> {
  return client.request<LifecycleResponse>('POST', `/api/sessions/${enc(id)}/resume`)
}

/** `POST /api/sessions/{id}/park` → 202 once the session is parked. */
export function park(client: Client, id: string): Promise<LifecycleResponse> {
  return client.request<LifecycleResponse>('POST', `/api/sessions/${enc(id)}/park`)
}

/** `POST /api/sessions/{id}/close` → 202 once the session is closed. */
export function close(client: Client, id: string): Promise<LifecycleResponse> {
  return client.request<LifecycleResponse>('POST', `/api/sessions/${enc(id)}/close`)
}

/** `DELETE /api/sessions/{id}` → 200 `DeleteResult` (a server from before
 *  it answers 204: `undefined`). Step-up: the client asks for it. */
export function deleteSession(client: Client, id: string): Promise<DeleteResult | undefined> {
  return client.request<DeleteResult | undefined>('DELETE', `/api/sessions/${enc(id)}`)
}
