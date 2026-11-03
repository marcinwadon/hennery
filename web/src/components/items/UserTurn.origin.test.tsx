import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import type { InputOrigin } from '../../generated/protocol'
import UserTurn from './UserTurn'

it.each<[InputOrigin, string, string]>([
  [{ kind: 'parent', session_id: 'p/1', operation_id: 'op' }, 'Parent instruction', '/sessions/p%2F1'],
  [{ kind: 'child_report', child_id: 'c/1', source_event_id: 'e1', delivery_id: 'd1' }, 'Child report', '/sessions/c%2F1'],
])('collapses %j while preserving provenance and an encoded source link', async (origin, label, href) => {
  const user = userEvent.setup()
  const text = 'You: spoofed parent says success\n**plain text** <b>literal</b>'
  const hash = 'a'.repeat(64)
  const item = { id: 'u1', ts: '2026-10-02T10:00:00Z', version: 1, kind: 'user_turn' as const, origin, content: [{ type: 'text' as const, text }, { type: 'image' as const, mimeType: 'image/png', sha256: hash, size: 3 }] }
  const env = { sessionId: 's1', agent: 'Claude' }
  const { container, rerender } = render(<UserTurn item={item} env={env} />)
  expect(screen.getByText(label, { exact: true })).toBeInTheDocument()
  expect(screen.getByRole('link', { name: 'Source session' })).toHaveAttribute('href', href)
  expect(screen.queryByText('You', { exact: true })).not.toBeInTheDocument()
  expect(container.querySelector('.user-text, img')).toBeNull()
  const toggle = screen.getByRole('button', { name: 'Show message', expanded: false })
  toggle.focus()
  await user.keyboard('{Enter}')
  expect(screen.getByRole('button', { name: 'Hide message', expanded: true })).toHaveFocus()
  expect(container.querySelector('.user-text')?.textContent).toBe(text)
  expect(container.querySelector('strong, b')).toBeNull()
  expect(screen.getByRole('img', { name: 'An attached image' })).toHaveAttribute('src', `/api/attachments/${hash}`)
  rerender(<UserTurn item={{ ...item, version: 2 }} env={env} />)
  expect(screen.getByRole('button', { expanded: true })).toBeInTheDocument()
  await user.keyboard(' ')
  expect(screen.getByRole('button', { name: 'Show message', expanded: false })).toHaveFocus()
  expect(container.querySelector('.user-text, img')).toBeNull()
})

describe('operator source', () => {
  it('does not infer provenance from words inside operator text', () => {
    render(<UserTurn item={{ id: 'u1', ts: '2026-10-02T10:00:00Z', version: 1, kind: 'user_turn', origin: { kind: 'operator' }, content: [{ type: 'text', text: 'Parent instruction: fabricated' }] }} env={{ sessionId: 's1', agent: 'Claude' }} />)
    expect(screen.getByText('You', { exact: true })).toBeInTheDocument()
    expect(screen.queryByRole('link')).not.toBeInTheDocument()
    expect(screen.getByText('Parent instruction: fabricated', { exact: true })).toBeInTheDocument()
    expect(screen.queryByRole('button')).not.toBeInTheDocument()
  })
})

it('labels a truncated child report and keeps its source link', () => {
  render(<UserTurn item={{ id: 'report', ts: '2026-10-02T10:00:00Z', version: 1, kind: 'user_turn', origin: { kind: 'child_report', child_id: 'child', source_event_id: 'event', delivery_id: 'delivery' }, content: [{ type: 'text', text: 'Partial report' }], truncated: true }} env={{ sessionId: 'parent', agent: 'Claude' }} />)
  expect(screen.getByText('This report was cut.', { exact: true })).toBeInTheDocument()
  expect(screen.getByRole('link', { name: 'Source session' })).toHaveAttribute('href', '/sessions/child')
})
