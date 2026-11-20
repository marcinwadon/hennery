import { fireEvent, render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import BackgroundTasks, { BackgroundCount } from './BackgroundTasks'

it('omits an absent or empty group, retains completed work until group reset', () => {
  const { rerender } = render(<BackgroundCount />)
  expect(screen.queryByText('Background tasks')).not.toBeInTheDocument()
  rerender(<BackgroundCount value={{ active: 0, done: 0, all: 0 }} />)
  expect(screen.queryByText('Background tasks')).not.toBeInTheDocument()
  rerender(<BackgroundCount value={{ active: 0, done: 5, all: 5 }} />)
  expect(screen.getByText('0 / 5 / 5')).toBeInTheDocument()
})

it('exposes the exact task state and marks offline counts as last known', () => {
  render(<BackgroundTasks stale value={{ active: 1, done: 1, all: 2, items: [{ id: 'one', title: 'Integration tests', status: 'paused', kind: 'terminal' }] }} />)
  expect(screen.getByText('· last known')).toBeInTheDocument()
  fireEvent.click(screen.getByText('Background tasks'))
  expect(screen.getByText('Integration tests')).toBeVisible()
  expect(screen.getByText('Paused')).toBeVisible()
})
