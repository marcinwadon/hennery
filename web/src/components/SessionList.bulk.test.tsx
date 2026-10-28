import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, expect, it, vi } from 'vitest'
import App from '../App'
import type { SessionSummary } from '../generated/view'
import { host, hat } from '../test-fixtures'
import { json, liveStream, type LiveStream } from '../test-stream'

const summary = (id: string, patch: Partial<SessionSummary> = {}): SessionSummary => ({ session_id: id, title: id, title_source: 'operator', host_id: 'h1', hat_id: 'hat-a', agent: 'claude', cwd: '/project', lifecycle: 'active', activity: 'idle', presumed_parked: false, question_waits: false, operator_attention: 0, parent_handled_questions: 0, created_at: '2026-10-01T00:00:00Z', last_event_at: '2026-10-02T00:00:00Z', ...patch })
function fixture(initial = [summary('A'), summary('B'), summary('C')]) {
  let rows = initial
  const streams: LiveStream[] = [], calls: { method: string; path: string }[] = []
  let close = (id: string) => json({ session_id: id, lifecycle: 'closed' }, 202)
  let read: (id: string) => Response | Promise<Response> = id => json(summary(id))
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), 'http://test'), path = url.pathname, method = init?.method ?? 'GET'
    calls.push({ method, path })
    if (path === '/api/capabilities') return json({ mode: 'full', features: [] })
    if (path === '/api/hosts') return json([host({ host_id: 'h1', name: 'Local' }), host({ host_id: 'h2', name: 'Other' })])
    if (path === '/api/hats') return json([hat({ id: 'hat-a', name: 'Work' }), hat({ id: 'hat-b', name: 'Home' })])
    if (path === '/api/view/sessions') return json({ sessions: rows, epoch: 'list', revision: 1, waiting: 0 })
    if (path === '/api/stream/sessions') { const stream = liveStream(); streams.push(stream); return stream.response }
    const id = decodeURIComponent(path.split('/')[3] ?? '')
    if (path.endsWith('/close')) return close(id)
    if (path === `/api/sessions/${encodeURIComponent(id)}`) return read(id)
    return json({ code: 'not_found' }, 404)
  })
  return { fetch: fetch as unknown as typeof globalThis.fetch, streams, calls, rows: (next: SessionSummary[]) => { rows = next }, close: (answer: typeof close) => { close = answer }, read: (answer: typeof read) => { read = answer } }
}
async function open(f = fixture()) {
  render(<App fetchImpl={f.fetch} />)
  await userEvent.click(await screen.findByRole('button', { name: 'Select sessions' }))
  return f
}
const checked = (name: string) => screen.getByRole('checkbox', { name: `Select ${name}` })
beforeEach(() => {
  localStorage.clear(); sessionStorage.clear(); history.replaceState(null, '', '/sessions')
  localStorage.setItem('hennery.subgroup', 'none'); localStorage.setItem('hennery.sort', 'started')
  window.matchMedia = ((query: string) => ({ matches: query.includes('min-width: 768'), media: query, addEventListener: () => {}, removeEventListener: () => {} })) as unknown as typeof matchMedia
})

async function unknownSelected() {
  const f = fixture(); f.close(() => json({ code: 'delivery_unknown' }, 503))
  await open(f); await userEvent.click(checked('C'))
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Close sessions' }))
  await waitFor(() => expect(screen.getByRole('region', { name: 'Close results' })).toHaveTextContent('Confirmation pending'))
  return f
}

it('requires a fresh state check and new confirmation before Close selected can close an unknown result again', async () => {
  const f = await unknownSelected(); f.read(id => json(summary(id, { lifecycle: 'parked' })))
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  const dialog = await screen.findByRole('dialog')
  expect(f.calls.filter(call => call.path === '/api/sessions/C')).toHaveLength(1)
  expect(within(dialog).getByRole('list', { name: 'Sessions to close' })).toHaveTextContent('parked')
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(1)
  await userEvent.click(within(dialog).getByRole('button', { name: 'Close sessions' }))
  await waitFor(() => expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(2))
})

it.each(['closed', 'missing', 'error'])('sends no second Close for unknown selection when GET is %s', async outcome => {
  const f = await unknownSelected()
  f.read(id => outcome === 'closed' ? json(summary(id, { lifecycle: 'closed' })) : json({ code: outcome === 'missing' ? 'not_found' : 'unavailable' }, outcome === 'missing' ? 404 : 503))
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  await screen.findByRole('list', { name: 'Selection check results' })
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  expect(f.calls.filter(call => call.path === '/api/sessions/C')).toHaveLength(1)
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(1)
})

it('cannot confirm or dispatch another Close while the unknown selection GET is pending', async () => {
  const f = await unknownSelected()
  let resolve!: (response: Response) => void
  f.read(() => new Promise<Response>(done => { resolve = done }))
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  expect(screen.getByRole('button', { name: 'Close selected' })).toBeDisabled()
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(1)
  expect(f.calls.filter(call => call.path === '/api/sessions/C')).toHaveLength(1)
  await act(async () => resolve(json(summary('C', { lifecycle: 'closed' }))))
  await screen.findByRole('list', { name: 'Selection check results' })
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(1)
})

it('does not forget an unknown ID when another selected batch replaces the results', async () => {
  const f = await unknownSelected()
  await userEvent.click(checked('C')); await userEvent.click(checked('B'))
  f.close(id => json({ session_id: id, lifecycle: 'closed' }, 202))
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Close sessions' }))
  await waitFor(() => expect(screen.getByRole('group', { name: 'Selected session actions' })).toHaveTextContent('0 selected'))
  await userEvent.click(checked('C'))
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  await screen.findByRole('dialog')
  expect(f.calls.filter(call => call.path === '/api/sessions/C')).toHaveLength(1)
  expect(f.calls.filter(call => call.path === '/api/sessions/C/close')).toHaveLength(1)
})

it('rejects an unknown selection GET overtaken by SSE instead of confirming the older run', async () => {
  const f = await unknownSelected()
  let resolve!: (response: Response) => void
  f.read(() => new Promise<Response>(done => { resolve = done }))
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  act(() => f.streams[0].event('session_upsert', summary('C', { activity: 'running', last_event_at: '2026-10-03T00:00:00Z' }), 'list:2'))
  await act(async () => resolve(json(summary('C', { lifecycle: 'parked' }))))
  expect(await screen.findByRole('list', { name: 'Selection check results' })).toHaveTextContent('State changed')
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(1)
})

it('invalidates the new unknown confirmation when a newer SSE arrives after its GET', async () => {
  const f = await unknownSelected(); f.read(id => json(summary(id, { lifecycle: 'parked' })))
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  const dialog = await screen.findByRole('dialog')
  expect(dialog).toHaveTextContent('parked')
  act(() => f.streams[0].event('session_upsert', summary('C', { activity: 'running', last_event_at: '2026-10-03T00:00:00Z' }), 'list:2'))
  const confirm = within(dialog).getByRole('button', { name: 'Close sessions' })
  await waitFor(() => expect(confirm).toBeDisabled()); await userEvent.click(confirm)
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(1)
})

it('checkboxes are outside links; exact confirmation/Cancel/focus never closes an unchecked row', async () => {
  const f = await open()
  expect(checked('A').closest('a')).toBeNull()
  await userEvent.click(checked('A')); await userEvent.click(checked('C'))
  expect(location.pathname).toBe('/sessions')
  expect(screen.queryAllByRole('button', { name: 'Session actions' })).toHaveLength(0)
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  const dialog = await screen.findByRole('dialog', { name: 'Close 2 selected sessions?' })
  expect(within(dialog).getByRole('list', { name: 'Sessions to close' }).children).toHaveLength(2)
  expect(within(dialog).getByRole('button', { name: 'Cancel' })).toHaveFocus()
  await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
  expect(screen.getByRole('button', { name: 'Close selected' })).toHaveFocus()
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(0)
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Close sessions' }))
  await waitFor(() => expect(within(screen.getByRole('region', { name: 'Close results' })).getByRole('status')).toHaveTextContent('2 / 2'))
  expect(f.calls.filter(call => call.method === 'POST').map(call => call.path)).toEqual(['/api/sessions/A/close', '/api/sessions/C/close'])
  expect(f.calls.some(call => call.method === 'DELETE')).toBe(false)
  expect(screen.getByRole('group', { name: 'Selected session actions' })).toHaveFocus()
})

it.each(['search', 'host', 'status', 'lifecycle', 'hideClosed', 'hat'])('clears selection on %s scope change', async kind => {
  await open(); await userEvent.click(checked('A'))
  if (kind === 'search') await userEvent.type(screen.getByRole('searchbox', { name: 'Search sessions' }), 'x')
  if (kind === 'host') await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Host' }), 'h2')
  if (kind === 'status') await userEvent.click(screen.getByRole('button', { name: /^Running / }))
  if (kind === 'lifecycle') await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Lifecycle' }), 'parked')
  if (kind === 'hideClosed') await userEvent.click(screen.getByRole('button', { name: 'Hide closed' }))
  if (kind === 'hat') await userEvent.click(screen.getByRole('button', { name: 'Home' }))
  expect(screen.getByRole('group', { name: 'Selected session actions' })).toHaveTextContent('0 selected')
  expect(screen.getByRole('button', { name: 'Close selected' })).toBeDisabled()
})

it('keeps selected IDs across sorting, grouping, collapsed projects and Load more; no new IDs become checked', async () => {
  const f = await open(fixture(Array.from({ length: 55 }, (_, index) => summary(`S${index}`, { created_at: new Date(2026, 9, 1, 0, index).toISOString() }))))
  await userEvent.click(checked('S54'))
  await userEvent.click(screen.getByRole('radio', { name: 'Last activity' }))
  await userEvent.click(screen.getByRole('radio', { name: 'Project' }))
  await userEvent.click(screen.getByRole('button', { name: /project.*55/ }))
  expect(screen.getByRole('region', { name: 'Selected sessions' })).toHaveTextContent('1 selected outside current rows')
  await userEvent.click(screen.getByRole('button', { name: /project.*55/ }))
  await userEvent.click(screen.getByRole('button', { name: 'Load more' }))
  expect(screen.getAllByRole('checkbox').filter(box => (box as HTMLInputElement).checked)).toHaveLength(1)
  act(() => f.streams[0].event('session_upsert', summary('New SSE row'), 'list:2'))
  expect(screen.getByRole('group', { name: 'Selected session actions' })).toHaveTextContent('1 selected')
})

it('retains selected missing resync rows, checks fresh detail, and never infers deletion from pagination', async () => {
  const f = await open(); await userEvent.click(checked('B'))
  f.rows([summary('A')])
  act(() => f.streams[0].event('resync_required', {}, 'list:2'))
  await waitFor(() => expect(screen.queryByRole('checkbox', { name: 'Select B' })).not.toBeInTheDocument())
  expect(screen.getByRole('region', { name: 'Selected sessions' })).toHaveTextContent('1 selected outside current rows')
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  expect(await screen.findByRole('dialog')).toHaveTextContent('B')
  expect(f.calls).toContainEqual({ method: 'GET', path: '/api/sessions/B' })
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(0)
  await userEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }))
  expect(screen.getByRole('button', { name: 'Close selected' })).toHaveFocus()
})

it('retains per-ID results and retries only a fresh failed ID after a second confirmation', async () => {
  const f = fixture(); f.close(id => id === 'B' ? json({ code: 'starting' }, 409) : id === 'C' ? json({ code: 'delivery_unknown' }, 503) : json({ session_id: id, lifecycle: 'closed' }, 202))
  await open(f)
  for (const id of ['A', 'B', 'C']) await userEvent.click(checked(id))
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Close sessions' }))
  const results = await screen.findByRole('region', { name: 'Close results' })
  await waitFor(() => expect(results).toHaveTextContent('Confirmation pending'))
  await userEvent.click(within(results).getByRole('button', { name: 'Check status' }))
  await waitFor(() => expect(results).toHaveTextContent('current run'))
  await userEvent.click(within(results).getByRole('button', { name: 'Review retry' }))
  const retry = await screen.findByRole('dialog', { name: 'Close 1 selected session?' })
  expect(within(retry).getByRole('list', { name: 'Sessions to close' })).toHaveTextContent('B')
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(3)
  f.close(id => json({ session_id: id, lifecycle: 'closed' }, 202))
  await userEvent.click(within(retry).getByRole('button', { name: 'Close sessions' }))
  await waitFor(() => expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(4))
  expect(f.calls.filter(call => call.method === 'POST').at(-1)?.path).toBe('/api/sessions/B/close')
})

it('does not let a historical success deselect an explicitly reselected resumed session', async () => {
  const f = await open()
  await userEvent.click(checked('A')); await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Close sessions' }))
  await waitFor(() => expect(screen.getByRole('group', { name: 'Selected session actions' })).toHaveTextContent('0 selected'))
  act(() => f.streams[0].event('session_upsert', summary('A', { activity: 'running', last_event_at: '2026-10-03T00:00:00Z' }), 'list:2'))
  await userEvent.click(checked('A'))
  act(() => f.streams[0].event('session_upsert', summary('A', { activity: 'idle', last_event_at: '2026-10-03T00:01:00Z' }), 'list:3'))
  expect(checked('A')).toBeChecked()
  expect(screen.getByRole('group', { name: 'Selected session actions' })).toHaveTextContent('1 selected')
})

it.each([404, 429])('sends no Close for an initial missing-row preflight refused with %i', async status => {
  const f = await open(); await userEvent.click(checked('B')); await userEvent.click(checked('C'))
  f.rows([summary('A')]); f.read(() => new Response(JSON.stringify({ code: status === 404 ? 'not_found' : 'rate_limited' }), { status, headers: { 'Content-Type': 'application/json', 'Retry-After': '8' } }))
  act(() => f.streams[0].event('resync_required', {}, 'list:2'))
  await waitFor(() => expect(screen.queryByRole('checkbox', { name: 'Select B' })).not.toBeInTheDocument())
  await userEvent.click(screen.getByRole('button', { name: 'Close selected' }))
  await screen.findByRole('list', { name: 'Selection check results' })
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  expect(f.calls.filter(call => call.method === 'POST')).toHaveLength(0)
  if (status === 429) {
    expect(f.calls.filter(call => call.path === '/api/sessions/C')).toHaveLength(0)
    expect(screen.getByRole('button', { name: 'Close selected' })).toBeDisabled()
    expect(screen.getByText(/Rate limited. Retry remaining/)).toBeInTheDocument()
  }
})
