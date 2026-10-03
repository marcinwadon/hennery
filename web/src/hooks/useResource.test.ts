import { act, renderHook, waitFor } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { useResource } from './useResource'

/** A load that answers only when told to. */
function deferred() {
  const answers: Array<(value: string) => void> = []
  const load = () => new Promise<string>((resolve) => answers.push(resolve))
  return { load, answer: (value: string) => answers.at(-1)!(value) }
}

describe('one server read', () => {
  it('drops what it read for the old key as the key changes', async () => {
    const server = deferred()
    const { result, rerender } = renderHook(({ id }) => useResource(server.load, [id]), { initialProps: { id: 'a' } })
    act(() => server.answer('rules of a'))
    await waitFor(() => expect(result.current.data).toBe('rules of a'))
    rerender({ id: 'b' })
    expect(result.current.data).toBeNull()
    act(() => server.answer('rules of b'))
    await waitFor(() => expect(result.current.data).toBe('rules of b'))
  })

  it('keeps what it shows while it reads the same key again', async () => {
    const server = deferred()
    const { result, rerender } = renderHook(({ id }) => useResource(server.load, [id]), { initialProps: { id: 'a' } })
    act(() => server.answer('first'))
    await waitFor(() => expect(result.current.data).toBe('first'))
    act(() => result.current.reload())
    rerender({ id: 'a' })
    expect(result.current.data).toBe('first')
    act(() => server.answer('second'))
    await waitFor(() => expect(result.current.data).toBe('second'))
  })

  it('takes a change as a function of what it holds', async () => {
    const server = deferred()
    const { result } = renderHook(() => useResource(server.load))
    act(() => server.answer('a'))
    await waitFor(() => expect(result.current.data).toBe('a'))
    act(() => {
      result.current.set((prev) => `${prev}b`)
      result.current.set((prev) => `${prev}c`)
    })
    expect(result.current.data).toBe('abc')
  })
})
