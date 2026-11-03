import { useCallback, useEffect, useRef, useState } from 'react'
import { useClient } from '../app-client'
import { messageOf } from '../api/errors'
import type { CacheRetention, SessionUsage } from '../generated/protocol'

/** A separate bounded read also observes child usage without another SSE stream.
 * Poll only the selected, visible session; retain the last observation on errors. */
export function useSessionUsage(id: string) {
  const client = useClient()
  const [data, setData] = useState<SessionUsage | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const current = useRef({ id, generation: 0, controller: null as AbortController | null, busy: false, writing: false })
  const path = `/api/sessions/${encodeURIComponent(id)}`
  const read = useCallback(async () => {
    const state = current.current
    if (state.busy || document.hidden) return
    state.busy = true
    const generation = ++state.generation
    const controller = new AbortController()
    state.controller = controller
    try {
      const next = await client.request<SessionUsage>('GET', `${path}/usage`, undefined, { signal: controller.signal })
      if (current.current !== state || generation !== state.generation) return
      if (!next || next.session_id !== id || !next.session || !next.family || !next.retention) throw new Error('Usage is not available from this collector.')
      setData(next); setError(null)
    } catch (e) {
      if (!controller.signal.aborted && current.current === state && generation === state.generation) setError(messageOf(e))
    } finally { if (generation === state.generation) state.busy = false }
  }, [client, id, path])
  useEffect(() => {
    const state = { id, generation: 0, controller: null as AbortController | null, busy: false, writing: false }
    current.current = state
    setData(null); setError(null); setSaving(false)
    void read()
    const interval = window.setInterval(() => void read(), 5000)
    const visible = () => { if (!document.hidden) void read() }
    document.addEventListener('visibilitychange', visible)
    return () => { state.generation++; state.controller?.abort(); clearInterval(interval); document.removeEventListener('visibilitychange', visible) }
  }, [id, read])
  const setRetention = useCallback(async (requested: CacheRetention) => {
    const state = current.current
    if (state.writing || state.id !== id) return
    state.writing = true
    const generation = ++state.generation
    state.controller?.abort()
    state.busy = true
    setSaving(true); setError(null)
    try {
      const next = await client.request<SessionUsage>('PATCH', `${path}/cache-retention`, { requested })
      if (current.current === state && generation === state.generation) setData(next)
    } catch (e) { if (current.current === state && generation === state.generation) setError(messageOf(e)) }
    finally { if (current.current === state && generation === state.generation) { state.busy = false; state.writing = false; setSaving(false) } }
  }, [client, id, path])
  return { data, error, saving, setRetention, refresh: read }
}
export type SessionUsageState = ReturnType<typeof useSessionUsage>
