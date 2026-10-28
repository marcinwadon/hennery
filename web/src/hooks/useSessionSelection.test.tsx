import { act, renderHook } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { useSessionSelection } from './useSessionSelection'

describe('explicit checkbox selection', () => {
  it('retains only explicit checkbox IDs across ordinary list rerenders', () => {
    const { result, rerender } = renderHook(({ scope }) => useSessionSelection(scope), { initialProps: { scope: 'hat:a|host:all|status:all|q:' } })
    act(() => result.current.setChecked({ id: 'parent', label: 'Parent', lifecycle: 'active' }, true))
    rerender({ scope: 'hat:a|host:all|status:all|q:' })
    expect([...result.current.selectedIds]).toEqual(['parent'])
    expect(result.current.targets).toHaveLength(1)
  })

  it('snapshots selected labels, deduplicates events and refuses closed/unknown rows', () => {
    const { result } = renderHook(() => useSessionSelection('same'))
    const row = { id: 'a', label: 'Original', lifecycle: 'active' }
    act(() => { result.current.setChecked(row, true); result.current.setChecked(row, true); result.current.setChecked({ id: 'closed', label: 'Closed', lifecycle: 'closed' }, true); result.current.setChecked({ id: 'unknown', label: 'Unknown' }, true) })
    row.label = 'Mutated outside selection'
    expect(result.current.targets).toEqual([{ id: 'a', label: 'Original', lifecycle: 'active' }])
    act(() => result.current.setChecked(row, false))
    expect(result.current.targets).toEqual([])
  })

  it('clears immediately on scope change and ignores stale checkbox/clear handlers', () => {
    const { result, rerender } = renderHook(({ scope }) => useSessionSelection(scope), { initialProps: { scope: 'q:old' } })
    const stale = result.current
    act(() => stale.setChecked({ id: 'old', label: 'Old', lifecycle: 'active' }, true))
    rerender({ scope: 'q:new' })
    expect(result.current.targets).toEqual([])
    act(() => result.current.setChecked({ id: 'new', label: 'New', lifecycle: 'parked' }, true))
    act(() => { stale.setChecked({ id: 'old', label: 'Old', lifecycle: 'active' }, true); stale.clear() })
    expect([...result.current.selectedIds]).toEqual(['new'])
  })

  it('ignores stale handlers even after filters return to their previous values', () => {
    const { result, rerender } = renderHook(({ scope }) => useSessionSelection(scope), { initialProps: { scope: 'a' } })
    const stale = result.current
    rerender({ scope: 'b' }); rerender({ scope: 'a' })
    act(() => result.current.setChecked({ id: 'new', label: 'New', lifecycle: 'active' }, true))
    act(() => { stale.clear(); stale.setChecked({ id: 'old', label: 'Old', lifecycle: 'active' }, true) })
    expect([...result.current.selectedIds]).toEqual(['new'])
  })

  it('prunes only authoritative closed facts, retaining missing cursor rows and adding no new IDs', () => {
    const { result } = renderHook(() => useSessionSelection('same'))
    act(() => {
      result.current.setChecked({ id: 'a', label: 'A', lifecycle: 'active' }, true)
      result.current.setChecked({ id: 'b', label: 'B', lifecycle: 'active' }, true)
    })
    act(() => result.current.reconcile(new Map([['a', { lifecycle: 'closed' }], ['unselected', { lifecycle: 'active' }]])))
    expect([...result.current.selectedIds]).toEqual(['b'])
  })
})
