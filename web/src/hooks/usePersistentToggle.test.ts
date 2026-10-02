import { describe, it, expect, beforeEach } from 'vitest'
import { act, renderHook } from '@testing-library/react'
import { usePersistentToggle } from './usePersistentToggle'

describe('usePersistentToggle', () => {
  beforeEach(() => localStorage.clear())

  it('uses the default when nothing is persisted', () => {
    const { result } = renderHook(() => usePersistentToggle('hennery.k', true))
    expect(result.current[0]).toBe(true)
  })

  it('reads a previously persisted value on init', () => {
    localStorage.setItem('hennery.k', JSON.stringify(true))
    const { result } = renderHook(() => usePersistentToggle('hennery.k', false))
    expect(result.current[0]).toBe(true)
  })

  it('persists to localStorage on change', () => {
    const { result } = renderHook(() => usePersistentToggle('hennery.k', false))
    act(() => result.current[1](true))
    expect(result.current[0]).toBe(true)
    expect(localStorage.getItem('hennery.k')).toBe('true')
  })
})
