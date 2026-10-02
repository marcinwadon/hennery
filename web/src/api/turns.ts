// What the composer asks of a session (ACP core §9): a prompt, a cancel, a
// config switch, and the resume behind "Resume and send". Every id in a path
// is encoded: ids are server data, never trusted to be path-safe.
import type { Client } from './client'
import type {
  CancelResponse,
  ConfigValue,
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
