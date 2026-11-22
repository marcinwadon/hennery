import { render as renderView, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import type { SessionSummary } from '../generated/view'
import SessionRow, { rowText } from './SessionRow'
import { json, routed } from '../test-stream'
const rowServer = routed(() => json({ code: 'not_found', message: 'no' }, 404))
const render = (ui: React.ReactNode) => renderView(ui, { wrapper: rowServer.wrapper })

/** The list's clock, 15 minutes after the fixture's last event. */
const NOW = Date.parse('2026-10-02T10:15:00.000Z')

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
    operator_attention: patch.question_waits || patch.activity === 'blocked' ? 1 : 0,
    parent_handled_questions: 0,
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
  it('shows unseen independently of status and removes the dot and weight after reading', () => {
    const row = summary({ title: 'A result', unseen: { event_id: 10 } })
    const view = render(<SessionRow now={NOW} session={row} selected={false} />)
    expect(screen.getByRole('img', { name: 'Unseen' })).toBeVisible()
    expect(view.container.querySelector('.sess')).toHaveClass('session-unseen')
    expect(screen.getByRole('img', { name: 'Idle' })).toBeVisible()
    view.rerender(<SessionRow now={NOW} session={{ ...row, unseen: undefined, operator_attention: 1 }} selected={false} />)
    expect(screen.queryByRole('img', { name: 'Unseen' })).not.toBeInTheDocument()
    expect(view.container.querySelector('.sess')).not.toHaveClass('session-unseen')
    expect(screen.getByRole('img', { name: 'Waiting on you' })).toBeVisible()
  })
  it('reads its relative time from the list’s clock, not from the time it renders', () => {
    const { container, rerender } = render(<SessionRow now={NOW} session={summary()} selected={false} />)
    expect(container.querySelector('.sess-time')?.textContent).toBe('15m ago')
    rerender(<SessionRow now={NOW + 60 * 60_000} session={summary()} selected={false} />)
    expect(container.querySelector('.sess-time')?.textContent).toBe('1h ago')
  })

  it('links to the session, and shows agent, title, branch and host', () => {
    render(<SessionRow now={NOW} session={summary({ session_id: 'a b/c', title: 'Fix', git_branch: 'fix/x' })} selected hostName="laptop" />)
    const link = screen.getByRole('link')
    expect(link).toHaveAttribute('href', '/sessions/a%20b%2Fc')
    expect(link).toHaveAttribute('aria-current', 'page')
    expect(within(link).getByText('Claude')).toBeInTheDocument()
    expect(within(link).getByText('fix/x')).toBeInTheDocument()
    expect(within(link).getByText('laptop')).toBeInTheDocument()
  })

  it('shows the agent it is given, not a fixed one', () => {
    render(<SessionRow now={NOW} session={summary({ agent: 'codex' })} selected={false} />)
    expect(screen.getByText('Codex')).toBeInTheDocument()
    expect(screen.queryByText('Claude')).not.toBeInTheDocument()
  })

  it('renders a hostile title as text', () => {
    const { container } = render(<SessionRow now={NOW} session={summary({ title: '<img src=x onerror=alert(1)>' })} selected={false} />)
    expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument()
    expect(container.querySelector('img')).toBeNull()
  })

  it('marks a session waiting on a question with words', () => {
    render(<SessionRow now={NOW} session={summary({ question_waits: true })} selected={false} />)
    expect(screen.getByRole('img', { name: 'Waiting on you' })).toHaveClass('led-attn')
    expect(screen.getByText('Waiting on you')).toBeInTheDocument()
  })

  it('animates a running session', () => {
    render(<SessionRow now={NOW} session={summary({ activity: 'running' })} selected={false} />)
    expect(screen.getByRole('img', { name: 'Running' })).toHaveClass('led-run')
  })

  it('offers Resume on a parked row, inside its link', () => {
    render(<SessionRow now={NOW} session={summary({ lifecycle: 'parked', activity: undefined })} selected={false} />)
    expect(within(screen.getByRole('link')).getByText('Resume')).toBeInTheDocument()
  })

  it('says a presumed-parked host is offline', () => {
    render(<SessionRow now={NOW} session={summary({ lifecycle: 'parked', activity: undefined, presumed_parked: true })} selected={false} />)
    expect(screen.getByRole('img', { name: 'Host offline' })).toBeInTheDocument()
    expect(screen.getByText('Resume')).toBeInTheDocument()
  })

  it('shows a failure reason on hover and on a tap, without opening the session', async () => {
    render(
      <SessionRow now={NOW} session={summary({ lifecycle: 'failed', activity: undefined, failure_reason: 'agent_not_logged_in' })} selected={false} />,
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
    render(<SessionRow now={NOW} session={summary({ failure_reason: 'left over' })} selected={false} />)
    expect(screen.queryByRole('button', { name: 'Why it failed' })).not.toBeInTheDocument()
    expect(screen.getByRole('img', { name: 'Idle' })).toHaveAttribute('title', 'Idle')
  })
})

it('uses a compact child count and an indented clickable child without family link lines', () => {
  const family = { revision: 1, parent: { session_id: 'p/1', title: 'Coordinator', agent: 'claude' }, children: [{ session_id: 'c/1', title: 'Worker', agent: 'codex' }], child_counts: { total: 1, active: 1, waiting: 0 }, attention: 0 }
  const { container } = render(<SessionRow now={NOW} session={summary({ family })} selected={false} childrenCount={1} indent />)
  expect(screen.getAllByRole('link')).toHaveLength(1)
  expect(screen.getByRole('button', { name: '↳ 1 child' })).toHaveClass('sess-children')
  expect(container.querySelector('.sess-child .sess-branch')).toHaveTextContent('↳')
  expect(screen.queryByText(/^(Parent:|Child:)/)).not.toBeInTheDocument()
})

it('shortens model ids and retains complete host/model tooltips', () => {
  render(<table><tbody><SessionRow now={NOW} session={summary({ model: 'claude-haiku-4-5-20251001' })} selected={false} hostName="a-very-long-development-hostname" table /></tbody></table>)
  expect(screen.getByTitle('claude-haiku-4-5-20251001')).toHaveTextContent('Claude · Haiku 4.5')
  expect(screen.getByTitle('a-very-long-development-hostname')).toHaveClass('sess-cell-truncate')
})

it('keeps the background counter beside the status when attention takes priority', () => {
  const row = summary({ background: { active: 2, done: 3, all: 5, stale: false, items: [] } })
  const view = render(<SessionRow now={NOW} session={row} selected={false} />)
  expect(screen.getByRole('img', { name: 'Background' })).toBeVisible()
  expect(screen.getByText('2 / 3 / 5')).toBeVisible()
  view.rerender(<SessionRow now={NOW} session={{ ...row, operator_attention: 1 }} selected={false} />)
  expect(screen.getByRole('img', { name: 'Waiting on you' })).toBeVisible()
  expect(screen.getByText('2 / 3 / 5')).toBeVisible()
})
