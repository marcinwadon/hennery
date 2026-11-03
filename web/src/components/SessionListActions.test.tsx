import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { HostItem } from '../generated/protocol'
import type { SessionSummary } from '../generated/view'
import { host } from '../test-fixtures'
import { json, routed, type Call } from '../test-stream'
import { navigate } from '../router'
import { loadDraft, saveDraft } from '../lib/drafts'
import { forgetAllAttachments, heldFor, hold } from '../lib/attachments'
import SessionListActions from './SessionListActions'
import SessionMenu from './SessionMenu'
import SessionRow from './SessionRow'

const initialHold = { pinned: false, held: true, family_active: true, notices_pending: false }
const hosts = [host({ host_id: 'h1', capabilities: ['park', 'idle_hold'] })]
function summary(patch: Partial<SessionSummary> = {}): SessionSummary {
  return { session_id: 's/1', host_id: 'h1', agent: 'claude', cwd: '/work', hat_id: 'hat', lifecycle: 'active', activity: 'idle', title: 'Source', title_source: 'operator', sidebar_pinned: false, presumed_parked: false, created_at: '', last_event_at: '', question_waits: false, operator_attention: 0, parent_handled_questions: 0, ...patch }
}
function List({ rows = [summary()], visible = rows, hostItems = hosts, removed = () => {} }: {
  rows?: SessionSummary[]; visible?: SessionSummary[]; hostItems?: HostItem[] | null; removed?: (id: string) => void
}) {
  return <SessionListActions sessions={new Map(rows.map(row => [row.session_id, row]))} hosts={hostItems} onRemoved={removed}>
    <section><h1 tabIndex={-1}>Sessions</h1>{visible.map(row => <SessionRow key={row.session_id} session={row} selected={false} now={0} />)}</section>
  </SessionListActions>
}
function server(answer?: (call: Call) => Response | Promise<Response>) {
  return routed(call => answer?.(call) ?? (call.path.endsWith('/queue') ? json([]) : json(initialHold)))
}
const trigger = () => screen.getByRole('button', { name: 'Session actions' })
const pick = (name: string) => { fireEvent.click(trigger()); fireEvent.click(screen.getByRole('button', { name })) }
const names = () => [...document.querySelectorAll('.session-menu-list button')].map(button => button.getAttribute('aria-label') || button.textContent)
function deferred() { let finish!: (r: Response) => void; const promise = new Promise<Response>(r => { finish = r }); return { promise, finish } }
beforeEach(() => { navigate('/sessions'); sessionStorage.clear(); forgetAllAttachments() })

describe('shared list actions', () => {
  it.each(['active', 'starting', 'parked', 'failed', 'closed'])('matches header actions and guards for %s', async lifecycle => {
    const s = server(), row = summary({ lifecycle })
    const r = render(<SessionMenu id={row.session_id} lifecycle={lifecycle} sidebarPinned={false} canPark supportsIdleHold hostConnected onRename={() => {}} onDeleted={() => {}} />, { wrapper: s.wrapper })
    fireEvent.click(trigger())
    await waitFor(() => expect(screen.getByRole('switch', { name: 'Do not park' })).toBeEnabled())
    const expected = names()
    r.unmount()
    render(<List rows={[row]} />, { wrapper: s.wrapper })
    fireEvent.click(trigger())
    await waitFor(() => expect(screen.getByRole('switch', { name: 'Do not park' })).toBeEnabled())
    expect(names()).toEqual(expected)
    expect(screen.getByRole('button', { name: 'Rename' })).toHaveFocus()
    expect(screen.getByRole('button', { name: 'New like this' })).toBeEnabled()
    expect(location.pathname).toBe('/sessions')
  })

  it('uses current host metadata, preserves offline Close, and omits unknown Park', () => {
    const s = server(), r = render(<List hostItems={null} />, { wrapper: s.wrapper })
    expect(s.calls).toHaveLength(0)
    fireEvent.click(trigger())
    expect(screen.queryByRole('button', { name: 'Park' })).toBeNull()
    r.rerender(<List hostItems={[host({ host_id: 'h1', capabilities: ['park'], connected: false })]} />)
    expect(screen.getByRole('button', { name: 'Park' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'New like this' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Close' })).toBeEnabled()
  })

  it('keeps sidebar pin and parking intent separate and follows updated summaries', async () => {
    const s = server(), r = render(<List />, { wrapper: s.wrapper })
    fireEvent.click(trigger())
    await waitFor(() => expect(screen.getByRole('switch', { name: 'Do not park' })).toBeEnabled())
    expect(screen.getByRole('switch', { name: 'Do not park' })).toHaveAttribute('aria-checked', 'false')
    expect(screen.getByRole('status')).toHaveTextContent('Family work requests a parking hold.')
    fireEvent.click(screen.getByRole('switch', { name: 'Pin to sidebar', checked: false }))
    await waitFor(() => expect(s.calls.filter(call => call.method === 'PATCH')).toHaveLength(1))
    expect(JSON.parse(String(s.fetch.mock.calls.find(call => call[1]?.method === 'PATCH')?.[1]?.body))).toEqual({ sidebar_pinned: true })
    r.rerender(<List rows={[summary({ sidebar_pinned: true })]} />)
    expect(screen.getByRole('switch', { name: 'Pin to sidebar', checked: true })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('switch', { name: 'Do not park' }))
    await waitFor(() => expect(s.calls.filter(call => call.method === 'PATCH')).toHaveLength(2))
    const patch = s.fetch.mock.calls.filter(call => call[1]?.method === 'PATCH')[1]
    expect(String(patch[0])).toBe('/api/sessions/s%2F1/idle-hold')
    expect(JSON.parse(String(patch[1]?.body))).toEqual({ pinned: true })
  })

  it('renames locally with reset/cancel/errors and returns focus, even after the row leaves a filter', async () => {
    let fail = true
    const s = server(call => call.method === 'PATCH' ? fail ? json({ code: 'invalid', message: 'Try again' }, 400) : json({ ...summary(), title: 'Renamed' }) : json(initialHold))
    const r = render(<List />, { wrapper: s.wrapper })
    pick('Rename')
    const dialog = screen.getByRole('dialog', { name: 'Rename session' })
    const input = within(dialog).getByRole('textbox', { name: 'Session name' })
    expect(input).toHaveFocus(); expect(input).toHaveValue('Source')
    fireEvent.change(input, { target: { value: '  Renamed  ' } })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }))
    expect(await within(dialog).findByRole('alert')).toBeInTheDocument()
    fail = false
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(JSON.parse(String(s.fetch.mock.calls.filter(call => call[1]?.method === 'PATCH')[1][1]?.body))).toEqual({ title: 'Renamed' })
    expect(trigger()).toHaveFocus(); expect(location.pathname).toBe('/sessions')
    r.rerender(<List rows={[summary({ title: 'Renamed' })]} />)
    pick('Rename')
    fireEvent.click(screen.getByRole('button', { name: 'Reset to agent title' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(JSON.parse(String(s.fetch.mock.calls.filter(call => call[1]?.method === 'PATCH')[2][1]?.body))).toEqual({ title: null })
    pick('Rename')
    r.rerender(<List visible={[]} />)
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Escape' })
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.getByRole('heading', { name: 'Sessions' })).toHaveFocus()
  })

  it('loads the queue only for Delete, blocks unknown counts and retries before showing family and queued+failed warnings', async () => {
    const queue = deferred(); let reads = 0
    const s = server(call => call.path.endsWith('/queue') ? ++reads === 1 ? json({ code: 'error' }, 500) : queue.promise : json(initialHold))
    const family = { revision: 1, child_counts: { total: 1, active: 1, waiting: 0 }, children: [{ session_id: 'child/2', title: 'Worker', agent: 'codex' }], attention: 0 }
    render(<List rows={[summary({ family })]} />, { wrapper: s.wrapper })
    expect(s.calls).toHaveLength(0)
    pick('Delete session')
    const dialog = screen.getByRole('dialog', { name: 'Delete this session?' })
    const confirm = within(dialog).getByRole('button', { name: 'Delete' })
    expect(confirm).toBeDisabled()
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('Queued messages could not be read')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Retry' }))
    expect(confirm).toBeDisabled()
    await act(async () => queue.finish(json([{ state: 'queued' }, { state: 'failed' }, { state: 'delivering' }])))
    expect(confirm).toBeEnabled()
    expect(dialog).toHaveTextContent('2 unsent queued messages and their images will also be deleted.')
    expect(dialog).toHaveTextContent('Those sessions are kept.')
    expect(within(dialog).getByRole('link', { name: 'Worker' })).toHaveAttribute('href', '/sessions/child%2F2')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(trigger()).toHaveFocus()
    expect(s.calls.some(call => call.method === 'DELETE')).toBe(false)
  })

  it('keeps Delete alive after SSE removes the row, retains late host results, cleans local data and restores fallback focus', async () => {
    const deletion = deferred(), removed = vi.fn()
    const s = server(call => call.method === 'DELETE' ? deletion.promise : call.path.endsWith('/queue') ? json([]) : json(initialHold))
    saveDraft('s/1', 'draft')
    hold('s/1', { attachments: [{ n: 1, file: new File(['x'], 'a.png', { type: 'image/png' }) }], nextN: 2 })
    const r = render(<List removed={removed} />, { wrapper: s.wrapper })
    pick('Delete session')
    const confirm = screen.getByRole('button', { name: 'Delete' })
    await waitFor(() => expect(confirm).toBeEnabled())
    fireEvent.click(confirm)
    r.rerender(<List rows={[]} removed={removed} />)
    expect(screen.getByRole('dialog')).toBeInTheDocument()
    await act(async () => deletion.finish(json({ host_transcript: { state: 'pending', pending: 'no_reply', notes: ['<b>host note</b>'] } })))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(screen.getByRole('status')).toHaveTextContent('the host did not answer in time')
    expect(screen.getByRole('status')).toHaveTextContent('<b>host note</b>')
    expect(screen.getByRole('status').querySelector('b')).toBeNull()
    expect(removed).toHaveBeenCalledWith('s/1')
    expect(loadDraft('s/1')).toBe(''); expect(heldFor('s/1').attachments).toEqual([])
    expect(screen.getByRole('heading', { name: 'Sessions' })).toHaveFocus()
  })

  it.each([404, 204])('handles Delete status %s as already deleted, through step-up when requested', async status => {
    let writes = 0; const removed = vi.fn()
    const s = server(call => call.method === 'DELETE' ? ++writes === 1 ? json({ code: 'step_up_required' }, 403) : status === 204 ? new Response(null, { status }) : json({ code: 'not_found' }, status) : call.path.endsWith('/queue') ? json([]) : json(initialHold))
    render(<List removed={removed} />, { wrapper: s.wrapper })
    pick('Delete session')
    const confirm = screen.getByRole('button', { name: 'Delete' })
    await waitFor(() => expect(confirm).toBeEnabled()); fireEvent.click(confirm)
    await waitFor(() => expect(removed).toHaveBeenCalledWith('s/1'))
    expect(writes).toBe(2)
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('retains Delete refusals in the dialog without removing local data', async () => {
    const removed = vi.fn(), s = server(call => call.method === 'DELETE' ? json({ code: 'starting' }, 409) : call.path.endsWith('/queue') ? json([]) : json(initialHold))
    saveDraft('s/1', 'retained')
    render(<List removed={removed} />, { wrapper: s.wrapper })
    pick('Delete session')
    const confirm = screen.getByRole('button', { name: 'Delete' })
    await waitFor(() => expect(confirm).toBeEnabled()); fireEvent.click(confirm)
    expect(await within(screen.getByRole('dialog')).findByRole('alert')).toHaveTextContent('The session is still starting')
    expect(removed).not.toHaveBeenCalled(); expect(loadDraft('s/1')).toBe('retained')
  })

  it('updates lifecycle guards in an open menu and keeps Close refusal semantics', async () => {
    const s = server(call => call.method === 'POST' ? json({ code: 'delivery_unknown' }, 503) : json(initialHold))
    const r = render(<List />, { wrapper: s.wrapper })
    fireEvent.click(trigger())
    r.rerender(<List rows={[summary({ lifecycle: 'closed' })]} />)
    expect(screen.queryByRole('button', { name: 'Close' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Park' })).toBeNull()
    r.rerender(<List rows={[summary({ lifecycle: 'parked' })]} />)
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('the session closes when the host is back')
    expect(s.calls.filter(call => call.method === 'POST')).toEqual([expect.objectContaining({ path: '/api/sessions/s%2F1/close' })])
  })

  it('fences late hold and Close responses when another row becomes the target', async () => {
    const late = deferred(), closing = deferred()
    const rows = [summary(), summary({ session_id: 's2', title: 'Second' })]
    let reads = 0
    const s = server(call => call.method === 'POST' ? closing.promise : ++reads === 1 ? late.promise : json({ ...initialHold, pinned: true }))
    render(<List rows={rows} />, { wrapper: s.wrapper })
    const triggers = screen.getAllByRole('button', { name: 'Session actions' })
    fireEvent.click(triggers[0]); fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    fireEvent.click(triggers[1])
    await waitFor(() => expect(screen.getByRole('switch', { name: 'Do not park' })).toHaveAttribute('aria-checked', 'true'))
    await act(async () => { late.finish(json(initialHold)); closing.finish(json({ code: 'starting' }, 409)) })
    expect(screen.getByRole('switch', { name: 'Do not park' })).toHaveAttribute('aria-checked', 'true')
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('restores fallback focus when the open row disappears without stealing filter typing', () => {
    const s = server(), r = render(<List />, { wrapper: s.wrapper })
    fireEvent.click(trigger())
    r.rerender(<List visible={[]} />)
    expect(screen.queryByRole('button', { name: 'Rename' })).toBeNull()
    expect(screen.getByRole('heading', { name: 'Sessions' })).toHaveFocus()
    r.rerender(<List />)
    fireEvent.click(trigger())
    const filter = document.createElement('input'); document.body.append(filter); filter.focus()
    r.rerender(<List visible={[]} />)
    expect(filter).toHaveFocus(); filter.remove()
  })

  it('does not revive New like this navigation after the source row disappeared and returned', async () => {
    const starting = deferred()
    const s = server(call => call.method === 'POST' ? starting.promise : json(initialHold))
    const r = render(<List />, { wrapper: s.wrapper })
    pick('New like this')
    r.rerender(<List visible={[]} />)
    r.rerender(<List />)
    await act(async () => starting.finish(json({ session_id: 'new', hat_id: 'hat', config: { axes: {} } }, 202)))
    expect(location.pathname).toBe('/sessions')
    expect(s.calls.some(call => call.path.endsWith('/catalog'))).toBe(false)
  })

  it('ignores a delayed scroll already included in placement, but dismisses on a new scroll', () => {
    const s = server(), r = render(<List />, { wrapper: s.wrapper })
    const scrolling = r.container.querySelector('.session-list-actions')!
    scrolling.scrollTop = 30
    fireEvent.click(trigger())
    fireEvent.scroll(scrolling)
    expect(trigger()).toHaveAttribute('aria-expanded', 'true')
    scrolling.scrollTop = 50
    fireEvent.scroll(scrolling)
    expect(trigger()).toHaveAttribute('aria-expanded', 'false')
    expect(trigger()).toHaveFocus()
  })

  it('discloses from contextmenu, portals outside the row, handles keyboard and internal/external scroll', async () => {
    const s = server(), user = userEvent.setup()
    const r = render(<List />, { wrapper: s.wrapper })
    fireEvent.contextMenu(screen.getByRole('link'))
    const menu = document.querySelector('.session-row-menu')!
    expect(menu.parentElement).toBe(document.body)
    expect(r.container).not.toContainElement(menu as HTMLElement)
    await user.tab(); expect(screen.getByRole('button', { name: 'New like this' })).toHaveFocus()
    fireEvent.scroll(menu)
    expect(document.querySelector('.session-row-menu')).toBe(menu)
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' })
    expect(trigger()).toHaveFocus()
    fireEvent.click(trigger()); fireEvent.mouseDown(document.querySelector('.session-row-menu')!)
    expect(trigger()).toHaveAttribute('aria-expanded', 'true')
    fireEvent.mouseDown(document.body)
    expect(trigger()).toHaveAttribute('aria-expanded', 'false')
    fireEvent.click(trigger()); document.body.scrollTop = 10; fireEvent.scroll(document.body); document.body.scrollTop = 0
    expect(trigger()).toHaveAttribute('aria-expanded', 'false')
    expect(location.pathname).toBe('/sessions')
  })
})
