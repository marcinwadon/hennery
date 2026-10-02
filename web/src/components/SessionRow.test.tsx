import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import type { SessionSummary } from '../generated/view'
import SessionRow, { rowText } from './SessionRow'

function summary(patch: Partial<SessionSummary> = {}): SessionSummary {
  return {
    session_id: 's1',
    host_id: 'h1',
    agent: 'claude',
    cwd: '/srv/work/app',
    hat_id: 'hat-a',
    lifecycle: 'active',
    activity: 'idle',
    presumed_parked: false,
    created_at: '2026-10-01T00:00:00.000Z',
    last_event_at: '2026-10-02T10:00:00.000Z',
    question_waits: false,
    ...patch,
  }
}

describe('rowText', () => {
  it('titles a row by its title, else its directory, else its id', () => {
    expect(rowText(summary({ title: 'Fix the parser' })).title).toBe('Fix the parser')
    expect(rowText(summary()).title).toBe('app')
    expect(rowText(summary({ cwd: '' })).title).toBe('s1')
  })

  it('puts the branch on line two, else the directory name', () => {
    expect(rowText(summary({ title: 'Fix', git_branch: 'fix/parser' })).where).toBe('fix/parser')
    expect(rowText(summary({ title: 'Fix' })).where).toBe('app')
  })

  it('never repeats the title on line two', () => {
    expect(rowText(summary()).where).toBe('')
    expect(rowText(summary({ title: 'main', git_branch: 'main' })).where).toBe('')
  })
})

describe('SessionRow', () => {
  it('links to the session, and shows agent, title, branch and host', () => {
    render(<SessionRow session={summary({ session_id: 'a b/c', title: 'Fix', git_branch: 'fix/x' })} selected hostName="laptop" />)
    const link = screen.getByRole('link')
    expect(link).toHaveAttribute('href', '/sessions/a%20b%2Fc')
    expect(link).toHaveAttribute('aria-current', 'page')
    expect(within(link).getByText('Claude')).toBeInTheDocument()
    expect(within(link).getByText('fix/x')).toBeInTheDocument()
    expect(within(link).getByText('laptop')).toBeInTheDocument()
  })

  it('shows the agent it is given, not a fixed one', () => {
    render(<SessionRow session={summary({ agent: 'codex' })} selected={false} />)
    expect(screen.getByText('Codex')).toBeInTheDocument()
    expect(screen.queryByText('Claude')).not.toBeInTheDocument()
  })

  it('renders a hostile title as text', () => {
    const { container } = render(<SessionRow session={summary({ title: '<img src=x onerror=alert(1)>' })} selected={false} />)
    expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument()
    expect(container.querySelector('img')).toBeNull()
  })

  it('marks a session waiting on a question with words', () => {
    render(<SessionRow session={summary({ question_waits: true })} selected={false} />)
    expect(screen.getByRole('img', { name: 'Waiting on a question' })).toHaveClass('led-attn')
    expect(screen.getByText('Waiting on a question')).toBeInTheDocument()
  })

  it('animates a running session', () => {
    render(<SessionRow session={summary({ activity: 'running' })} selected={false} />)
    expect(screen.getByRole('img', { name: 'Running' })).toHaveClass('led-run')
  })

  it('offers Resume on a parked row, inside its link', () => {
    render(<SessionRow session={summary({ lifecycle: 'parked', activity: undefined })} selected={false} />)
    expect(within(screen.getByRole('link')).getByText('Resume')).toBeInTheDocument()
  })

  it('says a presumed-parked host is offline', () => {
    render(<SessionRow session={summary({ lifecycle: 'parked', activity: undefined, presumed_parked: true })} selected={false} />)
    expect(screen.getByRole('img', { name: 'Host offline' })).toBeInTheDocument()
    expect(screen.getByText('Resume')).toBeInTheDocument()
  })

  it('shows a failure reason on hover and on a tap, without opening the session', async () => {
    render(
      <SessionRow session={summary({ lifecycle: 'failed', activity: undefined, failure_reason: 'agent_not_logged_in' })} selected={false} />,
    )
    expect(screen.getByRole('img', { name: 'Failed' })).toHaveAttribute('title', 'Failed: agent_not_logged_in')
    expect(screen.queryByText('agent_not_logged_in')).not.toBeInTheDocument()
    const why = screen.getByRole('button', { name: 'Why it failed' })
    expect(screen.getByRole('link')).not.toContainElement(why)
    await userEvent.click(why)
    expect(screen.getByText('agent_not_logged_in')).toBeInTheDocument()
    expect(why).toHaveAttribute('aria-expanded', 'true')
  })

  it('shows no reason control on a session that did not fail', () => {
    render(<SessionRow session={summary({ failure_reason: 'left over' })} selected={false} />)
    expect(screen.queryByRole('button')).not.toBeInTheDocument()
    expect(screen.getByRole('img', { name: 'Idle' })).toHaveAttribute('title', 'Idle')
  })
})
