import { useEffect, useRef, useState } from 'react'
import { useClient } from '../app-client'
import type { SessionDetail } from '../generated/protocol'
import { messageOf } from '../api/errors'

/** A navigation shortcut, separate from the automatic parking preference. */
export default function PinSessionAction({ id, pinned, disabled }: { id: string; pinned: boolean; disabled: boolean }) {
  const client = useClient()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const button = useRef<HTMLButtonElement>(null)
  const wasBusy = useRef(false)
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  useEffect(() => {
    if (wasBusy.current && !busy && document.activeElement === document.body) button.current?.focus()
    wasBusy.current = busy
  }, [busy])
  const toggle = async () => {
    if (busy) return
    setBusy(true); setError(null)
    try {
      await client.request<SessionDetail>('PATCH', `/api/sessions/${encodeURIComponent(id)}`, { sidebar_pinned: !pinned })
    } catch (err) {
      if (mounted.current) setError(`Could not ${pinned ? 'unpin' : 'pin'} this session. ${messageOf(err)}`)
    } finally { if (mounted.current) setBusy(false) }
  }
  return <>
    <button ref={button} type="button" role="switch" aria-label="Pin to sidebar" aria-checked={pinned} disabled={disabled || busy} onClick={() => void toggle()}><span>Pin to sidebar</span><span className="session-toggle-state" aria-hidden="true">{pinned ? '✓ On' : 'Off'}</span></button>
    {error && <p className="form-error" role="alert"><bdi>{error}</bdi></p>}
  </>
}
