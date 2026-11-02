import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import UsagePanel, { UsageMeter } from './SessionUsage'
import type { SessionUsage } from '../generated/protocol'
import type { SessionUsageState } from '../hooks/useSessionUsage'

function fixture(): SessionUsage {
  const tokens = { input_tokens: 10, output_tokens: 5, cache_read_tokens: 80, cache_write_tokens: 5, total_tokens: 100 }
  const total = { tokens, models: [{ model: 'claude-test', tokens }], turns: 2, reported_turns: 1, since: '2026-11-02T10:00:00Z', cost: { amount: 2.31, currency: 'USD', observed_at: '2026-11-02T10:00:00Z', partial: true }, partial: true }
  return {
    session_id: 's', source: 'claude', context: { used: 124000, size: 200000, observed_at: new Date().toISOString() },
    last_turn: { turn_id: 't', outcome: 'completed', observed_at: new Date().toISOString(), usage: { source: 'claude', tokens, models: [] } },
    session: total, family: { totals: total, sessions: 3, reporting_sessions: 2, cost_reporting_sessions: 1, truncated: false },
    retention: { supported: true, requested: 'inherit', attached_request: 'inherit' },
  }
}
function state(data: SessionUsage | null): SessionUsageState {
  return { data, error: null, saving: false, setRetention: vi.fn(async () => {}), refresh: vi.fn(async () => {}) }
}
describe('observed session usage', () => {
  it('shows actual cache token share and labels incomplete accounting and cost', async () => {
    render(<UsagePanel state={state(fixture())} away={false} />)
    expect(screen.getByRole('meter', { name: 'Context used' })).toHaveAttribute('aria-valuenow', '124000')
    expect(screen.getByText('84% of input tokens came from cache')).toBeInTheDocument()
    expect(screen.getByText('2 of 3 sessions reporting tokens. 1 reporting cost.')).toBeInTheDocument()
    expect(within(screen.getByRole('region', { name: 'This session' })).getByText('≥ 100')).toBeInTheDocument()
    expect(screen.getByText('Reported by Claude, not your bill. Coverage unconfirmed.')).toBeInTheDocument()
    await userEvent.click(screen.getByText('By model'))
    expect(screen.getByText('Full accounting can include native subagents, compaction and helper calls, so these rows can exceed the main-loop total.')).toBeInTheDocument()
  })
  it('preserves missing cache counters and never invents an exact cache share', () => {
    const u = fixture()
    u.source = 'codex'; u.retention.supported = false; u.session.cost = null
    u.last_turn!.usage!.tokens.cache_write_tokens = null
    render(<UsagePanel state={state(u)} away={true} />)
    expect(screen.getByText('Some cache token counts were not reported.')).toBeInTheDocument()
    expect(screen.queryByText(/% of input tokens came from cache$/)).not.toBeInTheDocument()
    expect(screen.getByText('Not available for Codex on this host.')).toBeInTheDocument()
    expect(screen.queryByRole('radio')).not.toBeInTheDocument()
    expect(screen.getByText(/host unreachable$/)).toBeInTheDocument()
  })
  it('shows an unknown cancelled turn separately from known zero usage', () => {
    const u = fixture(); u.last_turn!.outcome = 'cancelled'; u.last_turn!.usage = null
    render(<UsagePanel state={state(u)} away={false} />)
    expect(screen.getByText(/^No token counts were reported for this turn/)).toBeInTheDocument()
    expect(screen.getByRole('region', { name: 'Last turn' })).toHaveTextContent('cancelled')
  })
  it('only requests retention on an explicit choice and explains deferred application', async () => {
    const u = fixture(); u.retention.requested = '1h'
    const s = state(u)
    render(<UsagePanel state={s} away={false} />)
    expect(s.setRetention).not.toHaveBeenCalled()
    expect(screen.getByText(/^Requested 1 hour for the main conversation/)).toHaveTextContent('Takes effect on the next resume.')
    expect(screen.getByText(/^Requested 1 hour for the main conversation/)).toHaveTextContent('Claude does not confirm the effective TTL.')
    await userEvent.click(screen.getByRole('radio', { name: '5 min' }))
    expect(s.setRetention).toHaveBeenCalledWith('5m')
  })
  it('meter opens usage and does not infer cache expiry from context age', async () => {
    const open = vi.fn()
    render(<UsageMeter context={fixture().context} away={true} onClick={open} />)
    await userEvent.click(screen.getByRole('button', { name: /^Context 124K/ }))
    expect(open).toHaveBeenCalledTimes(1)
    expect(screen.getByText('62%')).toBeInTheDocument()
    expect(screen.queryByText(/^Cache expires/i)).not.toBeInTheDocument()
  })
})
