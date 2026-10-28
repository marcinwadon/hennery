// Delivery metadata has no SSE delta. Only explicit lifecycle/resync
// and hat boundaries trigger a read; starting → active also reads the host's source
// acknowledgement. A catalog snapshot also marks an attach when list updates
// coalesce parked/starting/active into one render. Text chunks and focus never
// trigger a read.
import { useCallback, useEffect, useRef, useState } from 'react'
import { useClient } from '../app-client'
import { sessionDetail } from '../api/view'
import { messageOf } from '../api/errors'
import type { SessionDetail } from '../generated/protocol'
import { runningLifecycle } from '../lib/mcpDelivery'

interface ReadState { id: string; detail: SessionDetail | null; error: string | null }

export function useMcpDelivery(id: string, lifecycle: string | undefined, resync: number, catalogChanges = 0, hatId?: string) {
  const client = useClient()
  const [state, setState] = useState<ReadState>({ id, detail: null, error: null })
  const request = useRef<(() => void) | null>(null)
  const previous = useRef<{ id: string; lifecycle: string | undefined; resync: number; catalogChanges: number; hatId?: string } | null>(null)
  const refresh = useCallback(() => request.current?.(), [])

  useEffect(() => {
    let live = true
    let wanted = 0
    let busy = false
    setState({ id, detail: null, error: null })
    const run = async () => {
      if (busy) return
      busy = true
      while (live) {
        const generation = wanted
        try {
          const detail = await sessionDetail(client, id)
          if (live && generation === wanted) setState({ id, detail, error: null })
        } catch (err) {
          if (live && generation === wanted) setState({ id, detail: null, error: messageOf(err) })
        }
        if (generation === wanted) break
      }
      busy = false
    }
    request.current = () => { wanted++; void run() }
    request.current()
    return () => { live = false; request.current = null }
  }, [client, id])

  useEffect(() => {
    const before = previous.current
    const previousHat = before?.id === id ? before.hatId ?? state.detail?.hat_id : undefined
    previous.current = { id, lifecycle, resync, catalogChanges, hatId: hatId ?? previousHat }
    if (!before || before.id !== id) return
    const resumed = before.lifecycle !== undefined && !runningLifecycle(before.lifecycle) && runningLifecycle(lifecycle)
    const confirmed = before.lifecycle === 'starting' && lifecycle === 'active'
    const catalogChanged = catalogChanges !== before.catalogChanges
    const hatChanged = hatId !== undefined && hatId !== previousHat
    if (resync !== before.resync || resumed || confirmed || catalogChanged || hatChanged) refresh()
  }, [id, lifecycle, resync, catalogChanges, hatId, state.detail?.hat_id, refresh])

  return {
    detail: state.id === id ? state.detail : null,
    error: state.id === id ? state.error : null,
    refresh,
  }
}
