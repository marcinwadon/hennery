import { act, renderHook } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { useNow } from './useNow'

afterEach(() => {
  vi.useRealTimers()
})

describe('useNow', () => {
  it('reads the time again at each interval, with nothing else rendering', () => {
    vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
    vi.setSystemTime(new Date(2026, 9, 2, 23, 59))
    const { result } = renderHook(() => useNow(60_000))
    const start = result.current
    expect(start).toBe(new Date(2026, 9, 2, 23, 59).getTime())
    act(() => vi.advanceTimersByTime(59_999))
    expect(result.current).toBe(start)
    act(() => vi.advanceTimersByTime(1))
    expect(result.current).toBe(start + 60_000)
  })

  it('reads the time again when the page’s visibility changes, before the interval', () => {
    vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
    vi.setSystemTime(new Date(2026, 9, 2, 23, 0))
    const { result } = renderHook(() => useNow(60_000))
    // The browser held the timers back while the tab slept: only the
    // clock moved.
    vi.setSystemTime(new Date(2026, 9, 3, 7, 0))
    expect(result.current).toBe(new Date(2026, 9, 2, 23, 0).getTime())
    act(() => void document.dispatchEvent(new Event('visibilitychange')))
    expect(result.current).toBe(new Date(2026, 9, 3, 7, 0).getTime())
  })

  it('stops reading once unmounted', () => {
    vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
    const remove = vi.spyOn(document, 'removeEventListener')
    const { unmount } = renderHook(() => useNow(60_000))
    unmount()
    expect(vi.getTimerCount()).toBe(0)
    expect(remove).toHaveBeenCalledWith('visibilitychange', expect.any(Function))
    remove.mockRestore()
  })
})
