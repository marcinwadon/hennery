// Answering a question (ACP core §4.6, §9):
// `POST /api/sessions/{id}/pending/{pending_id}/answer`. 202 once the
// answer is queued, whatever the host's state; the verdict follows on the
// item stream. The route never asks for a step-up, so a 403 is an error
// here, never a dialog.
import type { AnswerRequest, AnswerResponse } from '../generated/protocol'
import type { Client } from './client'

export function answerPath(sessionId: string, pendingId: string): string {
  return `/api/sessions/${encodeURIComponent(sessionId)}/pending/${encodeURIComponent(pendingId)}/answer`
}

export function answerQuestion(client: Client, sessionId: string, pendingId: string, body: AnswerRequest): Promise<AnswerResponse> {
  return client.request<AnswerResponse>('POST', answerPath(sessionId, pendingId), body, { stepUp: false })
}
