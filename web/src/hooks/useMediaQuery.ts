import { useEffect, useState } from 'react'

/**
 * Tracks whether a CSS media query currently matches. Reads the initial value
 * synchronously (so the first render is correct, not a flash of the wrong
 * layout) and subscribes to changes. SSR/no-`matchMedia` environments fall
 * back to `false`.
 *
 * Used to gate desktop-only behavior (e.g. auto-selecting a session) so it
 * doesn't fire on phones, where it would immediately jump into the detail view.
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState<boolean>(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return false
    return window.matchMedia(query).matches
  })

  useEffect(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return
    const mql = window.matchMedia(query)
    const onChange = () => setMatches(mql.matches)
    // Sync in case the query changed between render and effect.
    onChange()
    mql.addEventListener('change', onChange)
    return () => mql.removeEventListener('change', onChange)
  }, [query])

  return matches
}
