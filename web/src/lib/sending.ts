// A prompt being sent, per session (frontend spec §6.5): held outside React,
// as the draft's images are (lib/attachments.ts). A switch away and back
// before the answer mounts a new composer: it finds the send here, stays
// read-only until it ends, and takes its outcome (a 202 clears the draft).

const bySession = new Map<string, Promise<unknown>>()

/** The send in flight for `sessionId`, if any. */
export function sendingFor<T>(sessionId: string): Promise<T> | undefined {
  return bySession.get(sessionId) as Promise<T> | undefined
}

/** Run `work` as the send of `sessionId`: it is in flight until it ends,
 *  and is gone from here before its outcome is handed on. `work` must not
 *  reject: its outcome is a value. */
export function track<T>(sessionId: string, work: () => Promise<T>): Promise<T> {
  let done!: (outcome: T) => void
  const pending = new Promise<T>((resolve) => (done = resolve))
  bySession.set(sessionId, pending)
  void work().then((outcome) => {
    if (bySession.get(sessionId) === pending) bySession.delete(sessionId)
    done(outcome)
  })
  return pending
}

/** Forget every send (tests). */
export function forgetAllSends(): void {
  bySession.clear()
}
