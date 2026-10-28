// What the session view asks of a session (ACP core §9): a prompt, a
// cancel, a config switch, a resume (the footer's, and the composer's
// "Resume and send"), and the header menu's park, close and delete. Every id
// in a path is encoded: ids are server data, never trusted to be path-safe.
import type { Client, RequestOptions } from './client'
import type {
  CancelResponse,
  ConfigValue,
  DeleteResult,
  LifecycleResponse,
  PromptResponse,
  QueuedPromptResponse,
  QueuedPrompt,
  SessionCatalog,
} from '../generated/protocol'

const enc = encodeURIComponent

/** One ACP content block of a prompt, in order: a run of text, or an image
 *  as base64 (ACP core §7). */
export type PromptBlock = { type: 'text'; text: string } | { type: 'image'; mimeType: string; data: string }

/** `POST /api/sessions/{id}/prompt` → 202 `{turn_id}`. */
export function prompt(client: Client, id: string, content: PromptBlock[], requestKey?: string): Promise<PromptResponse | QueuedPromptResponse> {
  return client.request<PromptResponse | QueuedPromptResponse>('POST', `/api/sessions/${enc(id)}/prompt`, { content, ...(requestKey === undefined ? {} : { request_key: requestKey }) })
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
export function close(client: Client, id: string, options?: RequestOptions): Promise<LifecycleResponse> {
  return client.request<LifecycleResponse>('POST', `/api/sessions/${enc(id)}/close`, undefined, options)
}

/** `DELETE /api/sessions/{id}` → 200 `DeleteResult` (a server from before
 *  it answers 204: `undefined`). Step-up: the client asks for it. */
export function deleteSession(client: Client, id: string): Promise<DeleteResult | undefined> {
  return client.request<DeleteResult | undefined>('DELETE', `/api/sessions/${enc(id)}`)
}

/** Durable collector queue; acceptance does not mean the agent has read it. */
export function promptQueue(client: Client, id: string): Promise<QueuedPrompt[]> {
  return client.request('GET', `/api/sessions/${enc(id)}/queue`)
}
export function editQueuedPrompt(client: Client, id: string, queueId: string, text: string): Promise<QueuedPrompt[]> {
  return client.request('PATCH', `/api/sessions/${enc(id)}/queue/${enc(queueId)}`, { text })
}
export function removeQueuedPrompt(client: Client, id: string, queueId: string): Promise<QueuedPrompt[]> {
  return client.request('DELETE', `/api/sessions/${enc(id)}/queue/${enc(queueId)}`)
}
