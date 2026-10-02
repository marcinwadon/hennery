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
 *  and is gone from here before its outcome is handed on. A `work` that
 *  rejects ends too, with `onError`'s outcome: a send held for ever would
 *  leave every composer of the session read-only until a reload. */
export function track<T>(sessionId: string, work: () => Promise<T>, onError: (err: unknown) => T): Promise<T> {
  let done!: (outcome: T) => void
  const pending = new Promise<T>((resolve) => (done = resolve))
  bySession.set(sessionId, pending)
  const end = (outcome: T) => {
    if (bySession.get(sessionId) === pending) bySession.delete(sessionId)
    done(outcome)
  }
  void work().then(end, (err: unknown) => end(onError(err)))
  return pending
}

/** Let go of every send: a sign-out's, and the tests'. A send let go of
 *  still ends, and its composer, if still shown, takes its outcome. */
export function forgetAllSends(): void {
  bySession.clear()
}
