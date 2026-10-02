// One server read for a screen: loads on mount and when `key` changes, can
// be loaded again, and can be replaced by what a change answered (the
// server's own copy, never a local guess), as a function of what it holds
// so that two changes landing together both stay. A new key drops what the
// old one read; reading the same key again keeps it on screen meanwhile. A
// 401 has sent the browser to sign in already, so it is no error here.
import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from 'react'
import { Unauthenticated, messageOf } from '../api/errors'

export interface Resource<T> {
  data: T | null
  error: string | null
  reload: () => void
  set: Dispatch<SetStateAction<T | null>>
}

export function useResource<T>(load: () => Promise<T>, key: unknown[] = []): Resource<T> {
  const [data, setData] = useState<T | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [attempt, setAttempt] = useState(0)
  const lastKey = useRef(key)

  useEffect(() => {
    let live = true
    const before = lastKey.current
    lastKey.current = key
    if (before.length !== key.length || before.some((k, i) => !Object.is(k, key[i]))) setData(null)
    setError(null)
    load().then(
      (value) => live && setData(value),
      (err) => {
        if (live && !(err instanceof Unauthenticated)) setError(messageOf(err))
      },
    )
    return () => {
      live = false
    }
    // `load` is a new closure each render; `key` names what it depends on.
  }, [attempt, ...key])

  const reload = useCallback(() => setAttempt((n) => n + 1), [])
  return { data, error, reload, set: setData }
}
