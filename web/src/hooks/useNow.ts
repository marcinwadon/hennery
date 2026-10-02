import { useEffect, useState } from 'react'

/**
 * The time now, read again every `everyMs` and whenever the page's
 * visibility changes (a tab brought back after a night asleep, whose timers
 * the browser held back). What depends on the clock alone, a day heading or
 * "5m ago", then moves on an idle page too, with no other render to wait
 * for (F-8: a "Today" heading must not lie the morning after).
 */
export function useNow(everyMs: number): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const read = () => setNow(Date.now())
    const timer = setInterval(read, everyMs)
    // Any change: a tab that becomes hidden reads once more, harmlessly.
    document.addEventListener('visibilitychange', read)
    return () => {
      clearInterval(timer)
      document.removeEventListener('visibilitychange', read)
    }
  }, [everyMs])
  return now
}
