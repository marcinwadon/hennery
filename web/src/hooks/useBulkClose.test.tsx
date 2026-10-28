import { StrictMode, type ReactNode } from 'react'
import { act, renderHook } from '@testing-library/react'
import { expect, it } from 'vitest'
import { json, routed } from '../test-stream'
import { useBulkClose } from './useBulkClose'

it('survives the StrictMode effect replay and publishes results through the hook', async () => {
  const t = routed(() => json({ session_id: 'a', lifecycle: 'closed' }, 202))
  const wrapper = ({ children }: { children: ReactNode }) => <StrictMode><t.wrapper>{children}</t.wrapper></StrictMode>
  const { result } = renderHook(() => useBulkClose(), { wrapper })
  await act(async () => { await result.current.start([{ id: 'a', label: 'A', lifecycle: 'active' }]) })
  expect(result.current.busy).toBe(false)
  expect(result.current.results[0].status).toBe('closed')
  expect(t.calls).toHaveLength(1)
})

it('prevents queue dispatch after the owning list unmounts', async () => {
  let resolve!: (response: Response) => void
  const pending = new Promise<Response>(yes => { resolve = yes })
  const t = routed(() => pending)
  const { result, unmount } = renderHook(() => useBulkClose(), { wrapper: t.wrapper })
  let run!: Promise<boolean>
  act(() => { run = result.current.start(['a', 'b', 'c', 'd', 'e'].map(id => ({ id, label: id, lifecycle: 'active' }))) })
  await act(async () => { for (let i = 0; i < 5; i++) await Promise.resolve() })
  expect(t.calls).toHaveLength(3)
  unmount()
  await act(async () => { resolve(json({ session_id: 'a', lifecycle: 'closed' })); await run })
  expect(t.calls).toHaveLength(3)
})
