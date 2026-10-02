import { fireEvent, render, screen } from '@testing-library/react'
import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Item } from '../generated/view'
import ItemBoundary from './ItemBoundary'
import Transcript from './Transcript'
import type { ItemEnv } from './items/types'

const env: ItemEnv = { sessionId: 's1', agent: 'Claude' }
const TS = '2026-10-02T10:00:00.000Z'

function message(id: string, text: string, version = 1): Item {
  return { id, version, ts: TS, turn_id: 't1', kind: 'message', text } as Item
}

// A plan whose entries are not a list: the store's check (id, version, ts,
// kind) lets it through, and the step list throws on it.
function brokenPlan(id: string, version: number): Item {
  return { id, version, ts: TS, turn_id: 't1', kind: 'plan', entries: null } as unknown as Item
}

let quiet: ReturnType<typeof vi.spyOn>
beforeEach(() => {
  // React reports a caught render error on the console.
  quiet = vi.spyOn(console, 'error').mockImplementation(() => {})
})
afterEach(() => quiet.mockRestore())

describe('Transcript', () => {
  it('says when there is nothing yet', () => {
    render(<Transcript items={[]} env={env} />)
    expect(screen.getByText('Nothing in this session yet.')).toBeInTheDocument()
  })

  it('renders the items in the order given', () => {
    const { container } = render(<Transcript items={[message('a', 'first'), message('b', 'second')]} env={env} />)
    expect(Array.from(container.querySelectorAll('.bubble')).map((b) => b.textContent)).toEqual(['first', 'second'])
  })

  it('keeps an item that throws to itself: the rest still renders', () => {
    render(<Transcript items={[message('a', 'before'), brokenPlan('p', 2), message('b', 'after')]} env={env} />)
    expect(screen.getByText('Could not render this item')).toBeInTheDocument()
    expect(screen.getByText('before')).toBeInTheDocument()
    expect(screen.getByText('after')).toBeInTheDocument()
  })

  it('keeps a possible fabrication’s banner when its tool call fails to render', () => {
    // A title that is not text: the tool call's renderer throws on it.
    const tool = { id: 'tc', version: 1, ts: TS, turn_id: 't1', kind: 'tool_call', tool_call_id: 'x', title: { not: 'text' }, fabricated: '<b>made</b> up' } as unknown as Item
    const { container } = render(<Transcript items={[tool]} env={env} />)
    expect(screen.getByText('Could not render this item')).toBeInTheDocument()
    const alert = screen.getByRole('alert')
    expect(alert).toHaveTextContent('Possible fabricated tool call')
    expect(alert).toHaveTextContent('<b>made</b> up')
    expect(container.querySelector('.fab-warn b')).toBeNull()
  })

  it('keeps the banner whatever throws inside the boundary', () => {
    const Throws = () => {
      throw new Error('broken')
    }
    render(
      <ItemBoundary version={1} fabricated="">
        <Throws />
      </ItemBoundary>,
    )
    expect(screen.getByText('Could not render this item')).toBeInTheDocument()
    expect(screen.getByRole('alert')).toHaveTextContent('Possible fabricated tool call')
  })

  it('shows no banner for a failed item that is not a tool call, whatever it carries', () => {
    const plan = { ...brokenPlan('p', 2), fabricated: 'not a tool call' } as unknown as Item
    render(<Transcript items={[brokenPlan('q', 2), plan]} env={env} />)
    expect(screen.getAllByText('Could not render this item')).toHaveLength(2)
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('renders the item again when a new version of it comes', () => {
    const { rerender } = render(<Transcript items={[brokenPlan('p', 2)]} env={env} />)
    expect(screen.getByText('Could not render this item')).toBeInTheDocument()
    const fixed = { id: 'p', version: 3, ts: TS, turn_id: 't1', kind: 'plan', entries: [{ content: 'step one' }] } as Item
    rerender(<Transcript items={[fixed]} env={env} />)
    expect(screen.queryByText('Could not render this item')).toBeNull()
    expect(screen.getAllByText('step one').length).toBeGreaterThan(0)
  })

  it('keeps a failed item failed while its version stays', () => {
    const { rerender } = render(<Transcript items={[brokenPlan('p', 2)]} env={env} />)
    rerender(<Transcript items={[brokenPlan('p', 2), message('b', 'later')]} env={env} />)
    expect(screen.getByText('Could not render this item')).toBeInTheDocument()
  })

  it('keeps an item’s state (an open tool call) across a new version of it', () => {
    const tool = (version: number, status: string) =>
      ({ id: 'tc', version, ts: TS, turn_id: 't1', kind: 'tool_call', tool_call_id: 'x', title: 'Run', status, output: 'out' }) as Item
    const { container, rerender } = render(<Transcript items={[tool(1, 'in_progress')]} env={env} />)
    fireEvent.click(screen.getByRole('button'))
    rerender(<Transcript items={[tool(2, 'completed')]} env={env} />)
    expect(screen.getByText('Done')).toBeInTheDocument()
    expect(container.querySelector('.tool-output')).not.toBeNull()
  })

  it('keeps an item’s state when items come in before it', () => {
    const tool = { id: 'tc', version: 1, ts: TS, turn_id: 't2', kind: 'tool_call', tool_call_id: 'x', title: 'Run', output: 'out' } as Item
    const { container, rerender } = render(<Transcript items={[tool]} env={env} />)
    fireEvent.click(screen.getByRole('button'))
    rerender(<Transcript items={[message('a', 'earlier'), tool]} env={env} />)
    expect(container.querySelector('.tool-output')).not.toBeNull()
  })

  // The golden fixtures: what the collector's fold makes of two recorded
  // sessions, one per pinned adapter.
  const FIXTURES = join(process.cwd(), '../crates/hennery-view/tests/fixtures')
  const files = readdirSync(FIXTURES).filter((f) => f.endsWith('.items.json'))

  it('has golden fixtures to render', () => {
    expect(files.length).toBeGreaterThanOrEqual(2)
  })

  it.each(files)('renders every item of %s', (file) => {
    const items = JSON.parse(readFileSync(join(FIXTURES, file), 'utf8')) as Item[]
    const { container } = render(<Transcript items={items} env={env} />)
    expect(screen.queryByText('Could not render this item')).toBeNull()
    expect(container.querySelector('.unrec')).toBeNull()
    expect(quiet).not.toHaveBeenCalled()
  })
})
