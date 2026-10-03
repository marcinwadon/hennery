import { render, screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import StepList from './StepList'

describe('StepList', () => {
  it('renders nothing for no steps', () => {
    const { container } = render(<StepList entries={[]} />)
    expect(container.firstChild).toBeNull()
  })

  it('shows each step and marks the current and the finished ones', () => {
    const { container } = render(
      <StepList
        entries={[
          { content: 'Read the spec', status: 'completed' },
          { content: 'Write the test', status: 'in_progress' },
          { content: 'Ship it', status: 'pending' },
        ]}
      />,
    )
    const list = within(container.querySelector('ol') as HTMLElement)
    expect(list.getByText('Ship it')).toBeTruthy()
    expect(list.getByText('Write the test').closest('li')?.className).toBe('on')
    expect(list.getByText('Read the spec').closest('li')?.className).toBe('done')
    expect(list.getByText('Ship it').closest('li')?.className).toBe('')
  })

  it('counts finished steps over all', () => {
    render(<StepList entries={[{ content: 'a', status: 'completed' }, { content: 'b', status: 'pending' }]} />)
    expect(screen.getByText(/^1\s*\/\s*2$/)).toBeTruthy()
  })

  it('names the step in progress in the summary', () => {
    const { container } = render(
      <StepList entries={[{ content: 'a', status: 'completed' }, { content: 'building the thing', status: 'in_progress' }]} />,
    )
    expect(within(container.querySelector('summary') as HTMLElement).getByText('building the thing')).toBeTruthy()
  })

  it('still shows a step the agent sent without text', () => {
    const { container } = render(<StepList entries={[{ content: '' }]} />)
    expect(within(container.querySelector('ol') as HTMLElement).getByText('Untitled step')).toBeTruthy()
  })

  it('is open while work is in progress and closed once all is done', () => {
    const { container: running } = render(<StepList entries={[{ content: 'a', status: 'in_progress' }]} />)
    expect(running.querySelector('details')?.hasAttribute('open')).toBe(true)
    const { container: finished } = render(<StepList entries={[{ content: 'a', status: 'completed' }]} />)
    expect(finished.querySelector('details')?.hasAttribute('open')).toBe(false)
  })

  it('stays closed when the caller says so', () => {
    const { container } = render(<StepList entries={[{ content: 'a', status: 'in_progress' }]} open={false} />)
    expect(container.querySelector('details')?.hasAttribute('open')).toBe(false)
  })

  it('says when steps were cut', () => {
    render(<StepList entries={[{ content: 'a' }]} truncated />)
    expect(screen.getByText(/cut or left out\.$/)).toBeTruthy()
  })
})
