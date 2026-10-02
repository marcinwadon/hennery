import { describe, it, expect, vi, afterEach } from 'vitest'
import { act, renderHook } from '@testing-library/react'
import { useMediaQuery } from './useMediaQuery'

/**
 * Minimal matchMedia stub: lets a test set the initial match and later emit a
 * `change` event to simulate the viewport crossing the breakpoint.
 */
function stubMatchMedia(initial: boolean) {
  let listener: (() => void) | null = null
  const mql = {
    matches: initial,
    addEventListener: (_t: string, cb: () => void) => {
      listener = cb
    },
    removeEventListener: () => {
      listener = null
    },
  }
  window.matchMedia = vi.fn().mockReturnValue(mql) as unknown as typeof window.matchMedia
  return {
    set(v: boolean) {
      mql.matches = v
      listener?.()
    },
  }
}

afterEach(() => {
  // @ts-expect-error allow clearing the stub between tests
  delete window.matchMedia
  vi.restoreAllMocks()
})

describe('useMediaQuery', () => {
  it('reads the initial match synchronously', () => {
    stubMatchMedia(true)
    const { result } = renderHook(() => useMediaQuery('(min-width: 768px)'))
    expect(result.current).toBe(true)
  })

  it('updates when the query starts/stops matching', () => {
    const ctl = stubMatchMedia(false)
    const { result } = renderHook(() => useMediaQuery('(min-width: 768px)'))
    expect(result.current).toBe(false)
    act(() => ctl.set(true))
    expect(result.current).toBe(true)
  })

  it('falls back to false when matchMedia is unavailable', () => {
    const { result } = renderHook(() => useMediaQuery('(min-width: 768px)'))
    expect(result.current).toBe(false)
  })
})
