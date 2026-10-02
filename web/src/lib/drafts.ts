// A draft per session (frontend spec §6.5, F-17), in `sessionStorage`: it
// survives a reload and a switch to another session, and is never shown,
// or sent, in any session but its own.

export function draftKey(sessionId: string): string {
  return `hennery.draft.${sessionId}`
}

export function loadDraft(sessionId: string): string {
  try {
    return sessionStorage.getItem(draftKey(sessionId)) ?? ''
  } catch {
    // Storage refused (private mode, quota): a draft only lives in the page.
    return ''
  }
}

/** Keep `text` as the session's draft; an empty one is removed. */
export function saveDraft(sessionId: string, text: string): void {
  try {
    if (text === '') sessionStorage.removeItem(draftKey(sessionId))
    else sessionStorage.setItem(draftKey(sessionId), text)
  } catch {
    // As above: the draft stays in the page only.
  }
}
