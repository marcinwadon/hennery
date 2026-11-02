import { act, renderHook, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { json, routed } from '../test-stream'
import { useSessionUsage } from './useSessionUsage'

function observation(id = 's', requested = 'inherit') {
  return json({ session_id: id, session: {}, family: {}, retention: { requested } })
}
afterEach(() => vi.restoreAllMocks())

describe('session usage observations', () => {
  it('an older read cannot undo a preference save and duplicate saves are coalesced', async () => {
    let finishRead!: (response: Response) => void
    let finishWrite!: (response: Response) => void
    const t = routed(call => new Promise<Response>(done => {
      if (call.method === 'GET') finishRead = done
      else finishWrite = done
    }))
    const { result } = renderHook(() => useSessionUsage('s'), { wrapper: t.wrapper })
    act(() => { void result.current.setRetention('1h'); void result.current.setRetention('5m') })
    expect(t.calls.filter(c => c.method === 'PATCH')).toHaveLength(1)
    await act(async () => finishWrite(observation('s', '1h')))
    await waitFor(() => expect(result.current.data?.retention.requested).toBe('1h'))
    await act(async () => finishRead(observation()))
    expect(result.current.data?.retention.requested).toBe('1h')
    expect(result.current.saving).toBe(false)
  })

  it('does not expose a response belonging to the previously selected session', async () => {
    const pending: Array<(response: Response) => void> = []
    const t = routed(() => new Promise<Response>(done => pending.push(done)))
    const { result, rerender } = renderHook(({ id }) => useSessionUsage(id), { wrapper: t.wrapper, initialProps: { id: 's' } })
    rerender({ id: 'other' })
    await act(async () => pending[1](observation('other')))
    await waitFor(() => expect(result.current.data?.session_id).toBe('other'))
    await act(async () => pending[0](observation()))
    expect(result.current.data?.session_id).toBe('other')
  })

  it('skips hidden tabs and retains the last observation after a refresh error', async () => {
    let hidden = true
    vi.spyOn(document, 'hidden', 'get').mockImplementation(() => hidden)
    const t = routed(() => t.calls.length === 1 ? observation() : json({ error: { code: 'offline', message: 'offline' } }, 503))
    const { result } = renderHook(() => useSessionUsage('s'), { wrapper: t.wrapper })
    expect(t.calls).toHaveLength(0)
    hidden = false
    act(() => document.dispatchEvent(new Event('visibilitychange')))
    await waitFor(() => expect(result.current.data?.session_id).toBe('s'))
    await act(async () => result.current.refresh())
    expect(result.current.data?.session_id).toBe('s')
    expect(result.current.error).toBeTruthy()
  })
})
