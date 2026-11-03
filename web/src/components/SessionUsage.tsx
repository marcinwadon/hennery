import type { SessionUsageState } from '../hooks/useSessionUsage'
import type { CacheRetention, ContextUsage, ReportedCost, TokenUsage, UsageTotals } from '../generated/protocol'
import { agentLabel } from '../lib/agent'
import { useId } from 'react'
import '../styles/usage.css'

export function tokenCount(n: number | null | undefined): string {
  return n == null ? '—' : new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 2 }).format(n)
}
function age(at: string): string {
  const seconds = Math.max(0, Math.floor((Date.now() - Date.parse(at)) / 1000))
  if (!Number.isFinite(seconds)) return 'at an unknown time'
  return seconds < 60 ? `${seconds}s ago` : seconds < 3600 ? `${Math.floor(seconds / 60)}m ago` : `${Math.floor(seconds / 3600)}h ago`
}
function percentage(context: ContextUsage | null | undefined) { return context && context.size > 0 ? Math.round(context.used / context.size * 100) : null }
function stale(context: ContextUsage | null | undefined) { return !!context && Date.now() - Date.parse(context.observed_at) > 120000 }
function costText(cost: ReportedCost | null): string {
  return cost ? new Intl.NumberFormat('en', { style: 'currency', currency: cost.currency }).format(cost.amount) : '—'
}
export function UsageMeter({ context, away, onClick }: { context?: ContextUsage | null; away: boolean; onClick: (trigger: HTMLButtonElement) => void }) {
  const percent = percentage(context)
  const tip = context ? `Context ${tokenCount(context.used)} / ${tokenCount(context.size)} (${percent}%). Last reported ${age(context.observed_at)}${away ? ' · host unreachable' : ''}` : 'Context not reported yet. Open Usage.'
  return <button className={'usage-meter-button' + (away || stale(context) ? ' is-stale' : '')} type="button" aria-label={tip} title={tip} onClick={e => onClick(e.currentTarget)}>
    <span className="usage-mini-track" aria-hidden="true"><span style={{ width: `${Math.min(100, percent ?? 0)}%` }} /></span>
    <span>{percent == null ? '—' : `${percent}%${stale(context) ? ` · ${age(context!.observed_at).replace(' ago', '')}` : ''}`}</span>
  </button>
}

function Breakdown({ tokens }: { tokens: TokenUsage }) {
  const input = tokens.input_tokens + (tokens.cache_read_tokens ?? 0) + (tokens.cache_write_tokens ?? 0)
  const known = tokens.cache_read_tokens != null && tokens.cache_write_tokens != null
  const cells = [
    ['Input', tokens.input_tokens, 'input', 'Not read from cache'],
    ['Cache read', tokens.cache_read_tokens, 'read', 'Input served from cache'],
    ['Cache write', tokens.cache_write_tokens, 'write', 'Input written to cache'],
    ['Output', tokens.output_tokens, 'output', 'Generated tokens'],
  ] as const
  return <>
    {known && input > 0 && <div className="usage-input-bar" aria-hidden="true">{cells.slice(0, 3).map(([name, value, cls]) => <span key={name} className={cls} style={{ width: `${(value ?? 0) / input * 100}%` }} />)}</div>}
    <dl className="usage-token-grid">{cells.map(([name, value, cls, hint]) => <div key={name}><dt><span className={'usage-dot ' + cls} />{name}</dt><dd title={value == null ? 'Not reported by this runtime' : `${value.toLocaleString()} · ${hint}`}>{tokenCount(value)}</dd></div>)}</dl>
    {known && input > 0 ? <p className="usage-note">{Math.round(tokens.cache_read_tokens! / input * 100)}% of input tokens came from cache</p> : <p className="usage-note">Some cache token counts were not reported.</p>}
  </>
}
function Totals({ title, total, source, family, subtitle }: { title: string; total: UsageTotals; source: string; family?: boolean; subtitle?: string }) {
  return <section className="usage-total" aria-label={title}>
    <div className="usage-row"><h4>{title}</h4><span className="usage-note">{subtitle ?? (total.since ? `since ${new Date(total.since).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : 'No data yet')}</span></div>
    <dl className="usage-totals-grid">
      <dt>Tokens</dt><dd><span className="usage-number">{total.tokens && total.partial ? '≥ ' : ''}{tokenCount(total.tokens?.total_tokens)}</span><span className="usage-note">Main loop · {total.reported_turns} of {total.turns} completed turns</span></dd>
      <dt>Cost</dt><dd><span className="usage-number">{family && total.cost ? '≥ ' : ''}{costText(total.cost)}</span><span className="usage-note">{total.cost ? `Reported by ${family ? 'runtimes' : source}, not your bill. Coverage unconfirmed.` : `${source} has not reported cost.`}</span></dd>
    </dl>
    {total.partial && <p className="usage-coverage">Token totals include reported turns only. Earlier or interrupted turns may be missing.</p>}
    {!family && total.models.length > 0 && <details className="usage-models"><summary>By model</summary>
      <dl>{total.models.map(m => <div key={m.model}><dt><bdi title={m.model}>{m.model}</bdi></dt><dd>{tokenCount(m.tokens.total_tokens)}</dd></div>)}</dl>
      <p className="usage-note">{source === 'Claude' ? 'Full accounting can include native subagents, compaction and helper calls, so these rows can exceed the main-loop total.' : 'Model attribution is reported by the runtime; earlier model switches may not be split out.'}</p>
    </details>}
  </section>
}

export default function UsagePanel({ state, away, compact, compacting, onCompact }: {
  state: SessionUsageState; away: boolean; compact?: boolean; compacting?: boolean; onCompact?: () => void
}) {
  const radioName = useId()
  const u = state.data
  if (!u) return <section className="usage-panel" tabIndex={-1}><p className="usage-note">{state.error ? 'Usage is unavailable. It will be retried automatically.' : 'Waiting for usage observations…'}</p></section>
  const source = agentLabel(u.source)
  const context = u.context
  const percent = percentage(context)
  const options: [CacheRetention, string][] = [['inherit', 'Inherit'], ['5m', '5 min'], ['1h', '1 hour']]
  return <section className="usage-panel" tabIndex={-1}>
    <p className="usage-source">Reported by <bdi>{source}</bdi></p>
    {state.error && <p className="usage-coverage" role="alert">{state.error} Showing the last observations.</p>}
    <div className="usage-context">
      <div className="usage-row"><h4>Context</h4><span className="usage-number">{tokenCount(context?.used)} / {tokenCount(context?.size)}</span>{percent != null && <span className="usage-note">{percent}%</span>}</div>
      <div className={'usage-context-track' + (away || stale(context) ? ' is-stale' : '')} role="meter" aria-label="Context used" aria-valuemin={0} aria-valuemax={context?.size ?? 1} aria-valuenow={context ? Math.min(context.used, context.size) : undefined} aria-valuetext={context ? `${context.used} of ${context.size} tokens, reported ${age(context.observed_at)}` : 'Not reported'}>
        <span style={{ width: `${Math.min(100, percent ?? 0)}%` }} />
      </div>
      <div className="usage-row"><span className={'usage-note' + (away ? ' usage-coverage' : '')}>{context ? `Last reported ${age(context.observed_at)}${away ? ' · host unreachable' : ''}` : 'Not reported yet. Runtime window sizes may be estimates.'}</span>
        {compact && <button type="button" className="btn btn-sm" disabled={!onCompact || compacting} title="Runs /compact. Costs tokens; earlier history is replaced by a summary." onClick={onCompact}>{compacting ? 'Compacting…' : 'Compact'}</button>}
      </div>
    </div>
    <section className="usage-last-turn" aria-label="Last turn">
      <h4>Last turn{u.last_turn ? ` · ${new Date(u.last_turn.observed_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}${u.last_turn.outcome === 'completed' ? '' : ` · ${u.last_turn.outcome}`}` : ''}</h4>
      {u.last_turn?.usage ? <Breakdown tokens={u.last_turn.usage.tokens} /> : <p className="usage-note">{u.last_turn ? 'No token counts were reported for this turn. It is missing from the token totals.' : 'Appears when the first turn completes.'}</p>}
    </section>
    <Totals title="This session" total={u.session} source={source} />
    <Totals title="Family" total={u.family.totals} source="Family" family subtitle={`${u.family.truncated ? 'At least ' : ''}${u.family.sessions} ${u.family.sessions === 1 ? 'session' : 'sessions'}`} />
    <p className={u.family.totals.partial ? 'usage-coverage' : 'usage-note'}>{u.family.reporting_sessions} of {u.family.sessions} sessions reporting tokens. {u.family.cost_reporting_sessions} reporting cost.{u.family.truncated ? ' This large family is partially summarized.' : ''}</p>
    <fieldset className="usage-retention" disabled={state.saving}><legend>Prompt cache retention</legend>
      {u.retention.supported ? <><div className="usage-retention-options">{options.map(([value, label]) => <label key={value}><input type="radio" name={radioName} value={value} checked={u.retention.requested === value} onChange={() => void state.setRetention(value)} /><span>{label}</span></label>)}</div>
        <p className="usage-note">{u.retention.requested === 'inherit' ? 'Uses your Claude settings. Applies to the main conversation.' : `Requested ${u.retention.requested === '1h' ? '1 hour' : '5 min'} for the main conversation.`}
          {u.retention.requested !== u.retention.attached_request ? ' Takes effect on the next resume.' : ' This request was passed at the last attachment.'}
          {' Claude does not confirm the effective TTL.'}{u.retention.requested === '1h' ? ' One-hour cache writes cost more.' : ''}</p></> : <p className="usage-note">Not available for {source} on this host.</p>}
      {!u.retention.supported && u.retention.requested !== 'inherit' && <button type="button" className="btn btn-sm" onClick={() => void state.setRetention('inherit')}>Use runtime default</button>}
    </fieldset>
  </section>
}
