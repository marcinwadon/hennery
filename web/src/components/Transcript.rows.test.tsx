import { render } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { Item } from '../generated/view'
import Transcript from './Transcript'
import type { ItemEnv } from './items/types'

// Counts how often each message renders: the Markdown of a held item must
// not be parsed again when another item changes.
const renders = vi.hoisted(() => new Map<string, number>())
vi.mock('./items/AgentMessage', () => ({
  default: ({ item }: { item: { id: string; text: string } }) => {
    renders.set(item.id, (renders.get(item.id) ?? 0) + 1)
    return <p>{item.text}</p>
  },
}))

const env: ItemEnv = { sessionId: 's1', agent: 'Codex' }
const message = (id: string, text: string, version = 1) =>
  ({ id, version, ts: '2026-10-02T10:00:00.000Z', turn_id: 't1', kind: 'message', text }) as Item

describe('Transcript rows', () => {
  it('renders only the item that changed, or the one that came', () => {
    renders.clear()
    const a = message('a', 'first')
    const b = message('b', 'second')
    const { rerender } = render(<Transcript items={[a, b]} env={env} />)
    rerender(<Transcript items={[a, message('b', 'second, longer', 2)]} env={env} />)
    rerender(<Transcript items={[a, message('b', 'second, longer', 2), message('c', 'third')]} env={env} />)
    expect(renders.get('a')).toBe(1)
    expect(renders.get('c')).toBe(1)
    expect(renders.get('b')).toBeGreaterThanOrEqual(2)
  })

  it('renders every item again when the session’s environment changes', () => {
    renders.clear()
    const a = message('a', 'first')
    const { rerender } = render(<Transcript items={[a]} env={env} />)
    rerender(<Transcript items={[a]} env={{ ...env, agent: 'Claude' }} />)
    expect(renders.get('a')).toBe(2)
  })
})
