import { useCallback, useState } from 'react'

/**
 * Boolean state backed by localStorage. Reads the persisted value on init and
 * writes on every change. Tolerates JSON parse / storage access errors (e.g.
 * private-mode quota, disabled storage) by falling back to `def` and no-op
 * writes — the toggle still works in-memory for the session.
 */
export function usePersistentToggle(
  key: string,
  def: boolean,
): [boolean, (v: boolean) => void] {
  const [value, setValue] = useState<boolean>(() => {
    try {
      const raw = localStorage.getItem(key)
      if (raw === null) return def
      return JSON.parse(raw) === true
    } catch {
      return def
    }
  })

  const set = useCallback(
    (v: boolean) => {
      setValue(v)
      try {
        localStorage.setItem(key, JSON.stringify(v))
      } catch {
        // ignore storage errors — keep the in-memory value
      }
    },
    [key],
  )

  return [value, set]
}
