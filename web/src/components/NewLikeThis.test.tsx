import type { ComponentProps } from 'react'
import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { json, routed } from '../test-stream'
import { navigate } from '../router'
import SessionMenu from './SessionMenu'
import SessionRowView from './SessionRow'
import SessionListActions from './SessionListActions'
import { catalogOf } from '../screens/test-session'
import SessionView from '../screens/Session'
import { FAST, sessionServer } from '../screens/test-session'
import { forgetAllDrafts } from '../lib/drafts'
import { readStartNotice, startNoticeText } from '../lib/start'
import { useNewLikeThis } from '../hooks/useNewLikeThis'
import type { SessionSummary } from '../generated/view'

const config = { model: 'b', mode: 'plan', axes: { effort: 'high', fast: false } }
const source: SessionSummary = { session_id: 'source/a', host_id: 'host', agent: 'claude', cwd: '/work', hat_id: 'hat', lifecycle: 'closed', presumed_parked: false, created_at: '', last_event_at: '', question_waits: false, operator_attention: 0, parent_handled_questions: 0 }
function SessionRow(props: ComponentProps<typeof SessionRowView>) {
  return <SessionListActions sessions={new Map([[props.session.session_id, props.session]])} hosts={null} onRemoved={() => {}}><SessionRowView {...props} /></SessionListActions>
}
const options = [
  ...catalogOf('new').config_options.map(option => ({ ...option as object, currentValue: 'b' })),
  ...[['mode', 'plan'], ['effort', 'high']].map(([id, value]) => ({ id, category: id === 'mode' ? 'mode' : 'thought_level', type: 'select', name: id, currentValue: value, options: [{ value, name: value }] })),
  { id: 'fast', name: 'Fast', type: 'boolean', currentValue: false },
]
function server(post: () => Response | Promise<Response> = () => json({ session_id: 'new/a', hat_id: 'hat', config }, 202), actual = config) {
  return routed(call => {
    if (call.method === 'POST') return post()
    if (call.path.endsWith('/idle-hold')) return json({ pinned: false, held: false, family_active: false, notices_pending: false })
    if (call.path.endsWith('/catalog')) return json({ session_id: 'new/a', config_options: options, commands: [], ...actual })
    if (call.path.startsWith('/api/view/')) return json({ items: [], older: false, epoch: 'e', revision: 0 })
    return json({ session_id: 'new/a', hat_id: 'hat', lifecycle: 'active' })
  })
}
beforeEach(() => { sessionStorage.clear(); navigate('/sessions') })

describe('New like this', () => {
  it('starts from the header menu with one action and opens the new session without a prompt', async () => {
    const s = server()
    render(<SessionMenu id="source/a" lifecycle="active" canPark={false} onDeleted={vi.fn()} />, { wrapper: s.wrapper })
    fireEvent.click(screen.getByRole('button', { name: 'Session actions' }))
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    await waitFor(() => expect(location.pathname).toBe('/sessions/new%2Fa'))
    expect(location.search).toBe('?focus=prompt')
    expect(s.calls.filter(c => c.method === 'POST')).toEqual([expect.objectContaining({ path: '/api/sessions/source%2Fa/new-like-this' })])
    expect(s.fetch.mock.calls.find(c => c[1]?.method === 'POST')?.[1]?.body).toBeUndefined()
  })
  it('opens the same action from a list ellipsis and a context click without opening the source', async () => {
    const s = server()
    const r = render(<SessionRow session={source} now={0} selected={false} />, { wrapper: s.wrapper })
    const trigger = screen.getByRole('button', { name: 'Session actions' })
    expect(screen.getByRole('link')).not.toContainElement(trigger)
    fireEvent.click(trigger)
    expect(screen.getByRole('button', { name: 'Rename' })).toHaveFocus()
    fireEvent.keyDown(screen.getByRole('button', { name: 'New like this' }), { key: 'Escape' })
    expect(trigger).toHaveFocus()
    fireEvent.contextMenu(r.container.querySelector('.sess-item')!)
    expect(location.pathname).toBe('/sessions')
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    await waitFor(() => expect(location.pathname).toBe('/sessions/new%2Fa'))
  })
  it('prevents double creation while start is pending', async () => {
    let finish!: (response: Response) => void
    const s = server(() => new Promise(resolve => { finish = resolve }))
    render(<SessionMenu id="source/a" lifecycle="active" canPark={false} onDeleted={vi.fn()} />, { wrapper: s.wrapper })
    const trigger = screen.getByRole('button', { name: 'Session actions' })
    fireEvent.click(trigger)
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    fireEvent.click(trigger)
    expect(screen.getByRole('button', { name: 'New like this' })).toBeDisabled()
    expect(s.calls.filter(c => c.method === 'POST')).toHaveLength(1)
    await act(async () => finish(json({ session_id: 'new/a', hat_id: 'hat', config }, 202)))
    await waitFor(() => expect(location.pathname).toBe('/sessions/new%2Fa'))
  })
  it('dismisses the list menu when the viewport resizes', () => {
    const s = server()
    render(<SessionRow session={source} now={0} selected={false} />, { wrapper: s.wrapper })
    const trigger = screen.getByRole('button', { name: 'Session actions' })
    fireEvent.click(trigger)
    expect(screen.getByRole('button', { name: 'New like this' })).toBeVisible()
    fireEvent(window, new Event('resize'))
    expect(screen.queryByRole('button', { name: 'New like this' })).not.toBeInTheDocument()
    expect(trigger).toHaveAttribute('aria-expanded', 'false')
  })
  it('shows a hat mismatch without navigation or another start', async () => {
    const s = server(() => json({ code: 'hat_mismatch', message: 'Changed' }, 409))
    render(<SessionRow session={source} now={0} selected={false} />, { wrapper: s.wrapper })
    fireEvent.click(screen.getByRole('button', { name: 'Session actions' }))
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('different hat')
    expect(location.pathname).toBe('/sessions')
    expect(s.calls.filter(c => c.method === 'POST')).toHaveLength(1)
  })
  it('opens the known session on uncertain delivery without retrying', async () => {
    const s = server(() => json({ code: 'delivery_unknown', message: 'Disconnected', session_id: 'new/a' }, 503))
    render(<SessionRow session={source} now={0} selected={false} />, { wrapper: s.wrapper })
    fireEvent.click(screen.getByRole('button', { name: 'Session actions' }))
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    await waitFor(() => expect(location.pathname).toBe('/sessions/new%2Fa'))
    expect(new URLSearchParams(location.search).get('notice')).toBe('start_unknown')
    expect(s.calls.filter(c => c.method === 'POST')).toHaveLength(1)
  })
  it('keeps a clamped setting visible as a settings notice without claiming a saved prompt', async () => {
    const s = server(undefined, { ...config, model: 'a' })
    render(<SessionRow session={source} now={0} selected={false} />, { wrapper: s.wrapper })
    fireEvent.click(screen.getByRole('button', { name: 'Session actions' }))
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    await waitFor(() => expect(location.pathname).toBe('/sessions/new%2Fa'))
    expect(new URLSearchParams(location.search).get('notice')).toBe('settings_unconfirmed')
    expect(startNoticeText(readStartNotice(location.search, 'new/a')!)).toContain('Model: B')
    expect(s.calls.filter(c => c.method === 'POST')).toHaveLength(1)
  })
  it('does not navigate after leaving the source while start is pending', async () => {
    let finish!: (response: Response) => void
    const s = server(() => new Promise(resolve => { finish = resolve }))
    const r = render(<SessionRow session={source} now={0} selected={false} />, { wrapper: s.wrapper })
    fireEvent.click(screen.getByRole('button', { name: 'Session actions' }))
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    r.unmount(); navigate('/hosts')
    await act(async () => finish(json({ session_id: 'new/a', hat_id: 'hat', config }, 202)))
    expect(location.pathname).toBe('/hosts')
  })
  it('admits only one start before React renders the pending state', async () => {
    let finish!: (response: Response) => void
    const s = server(() => new Promise(resolve => { finish = resolve }))
    const hook = renderHook(() => useNewLikeThis('source/a'), { wrapper: s.wrapper })
    act(() => { void hook.result.current.start(); void hook.result.current.start() })
    expect(s.calls.filter(c => c.method === 'POST')).toHaveLength(1)
    await act(async () => finish(json({ session_id: 'new/a', hat_id: 'hat', config }, 202)))
    await waitFor(() => expect(location.pathname).toBe('/sessions/new%2Fa'))
  })
  it('keeps a newer navigation when the initiating view remains mounted', async () => {
    let finish!: (response: Response) => void
    const s = server(() => new Promise(resolve => { finish = resolve }))
    render(<SessionRow session={source} now={0} selected={false} />, { wrapper: s.wrapper })
    fireEvent.click(screen.getByRole('button', { name: 'Session actions' }))
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    navigate('/hosts')
    await act(async () => finish(json({ session_id: 'new/a', hat_id: 'hat', config }, 202)))
    expect(location.pathname).toBe('/hosts')
  })
  it.each([
    ['start', 'app'], ['confirmation', 'app'], ['start', 'history'], ['confirmation', 'history'],
  ])('does not revive a pending %s after leaving and returning through %s navigation', async (phase, navigation) => {
    let finish!: (response: Response) => void
    const delayed = new Promise<Response>(resolve => { finish = resolve })
    const created = () => json({ session_id: 'new/a', hat_id: 'hat', config }, 202)
    const catalog = () => json({ session_id: 'new/a', config_options: options, commands: [], ...config })
    const s = routed(call => {
      if (call.method === 'POST') return phase === 'start' ? delayed : created()
      if (call.path.endsWith('/catalog')) return phase === 'confirmation' ? delayed : catalog()
      if (call.path.startsWith('/api/view/')) return json({ items: [], older: false, epoch: 'e', revision: 0 })
      return json({ session_id: 'new/a', hat_id: 'hat', lifecycle: 'active' })
    })
    const hook = renderHook(() => useNewLikeThis('source/a'), { wrapper: s.wrapper })
    act(() => { void hook.result.current.start() })
    await waitFor(() => expect(s.calls.some(call => phase === 'start' ? call.method === 'POST' : call.path.endsWith('/catalog'))).toBe(true))
    act(() => {
      for (const to of ['/sessions?search=1', '/sessions']) {
        if (navigation === 'app') navigate(to)
        else { history.replaceState(null, '', to); window.dispatchEvent(new PopStateEvent('popstate')) }
      }
    })
    await act(async () => finish(phase === 'start' ? created() : catalog()))
    await waitFor(() => expect(hook.result.current.busy).toBe(false))
    expect(location.pathname + location.search).toBe('/sessions')
    if (phase === 'start') expect(s.calls.filter(call => call.method === 'GET')).toHaveLength(0)
  })
  it('does not navigate or confirm a session after signing out while start is pending', async () => {
    let finish!: (response: Response) => void
    const s = server(() => new Promise(resolve => { finish = resolve }))
    render(<SessionRow session={source} now={0} selected={false} />, { wrapper: s.wrapper })
    fireEvent.click(screen.getByRole('button', { name: 'Session actions' }))
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    forgetAllDrafts()
    await act(async () => finish(json({ session_id: 'new/a', hat_id: 'hat', config }, 202)))
    expect(location.pathname).toBe('/sessions')
    expect(s.calls.filter(c => c.method === 'GET' && !c.path.endsWith('/idle-hold'))).toHaveLength(0)
  })
  it('opens a known failed start without retrying or claiming that a first prompt was kept', async () => {
    const s = server(() => json({ code: 'config_failed', message: 'Model removed', session_id: 'new/a' }, 502))
    render(<SessionRow session={source} now={0} selected={false} />, { wrapper: s.wrapper })
    fireEvent.click(screen.getByRole('button', { name: 'Session actions' }))
    fireEvent.click(screen.getByRole('button', { name: 'New like this' }))
    await waitFor(() => expect(location.pathname).toBe('/sessions/new%2Fa'))
    expect(startNoticeText(readStartNotice(location.search, 'new/a')!)).not.toMatch(/draft|waits as/)
    expect(s.calls.filter(c => c.method === 'POST')).toHaveLength(1)
  })
  it('focuses the empty composer after opening the created session and cleans the focus flag', async () => {
    navigate('/sessions/new?focus=prompt')
    const s = sessionServer()
    render(<SessionView id="new" timing={FAST} />, { wrapper: s.wrapper })
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Prompt' })).toHaveFocus())
    expect(screen.getByRole('textbox', { name: 'Prompt' })).toHaveValue('')
    expect(location.search).toBe('')
    expect(s.changes()).toHaveLength(0)
  })
})
