import { act, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { Client } from '../api/client'
import { ClientContext } from '../app-client'
import { StepUpCancelled } from '../api/errors'
import { json, stubServer, type Answer } from '../test-server'
import type { HatItem } from '../generated/protocol'
import { host } from '../test-fixtures'
import DelegationSettings from './DelegationSettings'

const HAT: HatItem = { id: 'hat', name: 'Personal', colour: '#64748b', default_for_new_hosts: true, created_at: '2026-10-20T09:00:00Z', purging: false, mcp_source_policy: 'legacy' as const, mcp_policy_revision: 0 }
function open(routes: Record<string, Answer | Answer[]> = {}, stepUp = vi.fn(async () => {}), hat = HAT) {
  const server = stubServer({
    'GET /api/hats/hat/session-tools': json(200, { delegation_enabled: false, revision: 1 }),
    'GET /api/hosts': json(200, [host({ host_id: 'host', connected: true, mcp_delivery: { claude: 'isolated' } })]),
    'GET /api/hosts/host/projects': json(200, { home: '/project', recents: [], items: [], partial: false }),
    'PATCH /api/hats/hat/session-tools': json(200, { delegation_enabled: true, revision: 2 }),
    ...routes,
  })
  const client = new Client({ fetch: server.fetch, here: () => ({ pathname: '/settings', search: '' }), navigate: vi.fn(), stepUp })
  render(<ClientContext.Provider value={client}><DelegationSettings hats={[hat]} /></ClientContext.Provider>)
  return { server, stepUp }
}
const writes = (server: ReturnType<typeof stubServer>) => server.sent.filter(r => r.method === 'PATCH').map(r => [r.path, r.body])
const toggle = () => screen.findByRole('switch', { name: 'Delegation' })
describe('the plain per-hat Delegation switch', () => {
  it('enables when a connected runtime is unauthenticated, without probing hosts', async () => {
    const { server, stepUp } = open({
      'GET /api/hosts/host/runtime-catalog?agent=claude&cwd=%2Fproject': json(409, { code: 'auth_required', message: 'Authentication required' }),
    })
    await userEvent.click(await toggle())
    await waitFor(() => expect(screen.getByRole('switch')).toBeChecked())
    expect(stepUp).toHaveBeenCalledTimes(1)
    expect(writes(server)).toEqual([['/api/hats/hat/session-tools', { delegation_enabled: true }]])
    expect(server.sent.some(request => request.path.startsWith('/api/hosts'))).toBe(false)
  })
  it('uses one prompt for the enabled flag, with no grant editor', async () => {
    const { server, stepUp } = open()
    await userEvent.click(await toggle())
    await waitFor(() => expect(screen.getByRole('switch')).toBeChecked())
    expect(stepUp).toHaveBeenCalledTimes(1)
    expect(writes(server)).toEqual([
      ['/api/hats/hat/session-tools', { delegation_enabled: true }],
    ])
    expect(screen.getByText('Sessions in this hat can start child sessions with any model and mode, including modes that don’t ask for permission. Children cannot start their own children.', { exact: true })).toBeVisible()
    expect(screen.getByText('Takes effect at the next start or resume.', { exact: true })).toBeVisible()
    expect(screen.queryByText('Execution grants', { exact: true })).toBeNull()
    expect(screen.queryByLabelText('Grant agent', { exact: true })).toBeNull()
  })
  it('disables delegation without requiring a host', async () => {
    const { server, stepUp } = open({
      'GET /api/hats/hat/session-tools': json(200, { delegation_enabled: true, revision: 1 }),
      'PATCH /api/hats/hat/session-tools': json(200, { delegation_enabled: false, revision: 2 }),
    })
    await userEvent.click(await toggle())
    await waitFor(() => expect(screen.getByRole('switch')).not.toBeChecked())
    expect(stepUp).toHaveBeenCalledTimes(1)
    expect(writes(server)).toEqual([['/api/hats/hat/session-tools', { delegation_enabled: false }]])
    expect(server.sent.some(r => r.path === '/api/hosts')).toBe(false)
  })
  it('enables without any connected hosts', async () => {
    const { server } = open({ 'GET /api/hosts': json(200, []) })
    await userEvent.click(await toggle())
    await waitFor(() => expect(screen.getByRole('switch')).toBeChecked())
    expect(writes(server)).toEqual([['/api/hats/hat/session-tools', { delegation_enabled: true }]])
    expect(server.sent.some(request => request.path.startsWith('/api/hosts'))).toBe(false)
  })
  it('cancelling confirmation sends no write', async () => {
    const { server, stepUp } = open({}, vi.fn(async () => { throw new StepUpCancelled() }))
    await userEvent.click(await toggle())
    expect(await screen.findByRole('alert')).toHaveTextContent('Change cancelled')
    expect(screen.getByRole('switch')).not.toBeChecked()
    expect(stepUp).toHaveBeenCalledTimes(1)
    expect(writes(server)).toEqual([])
  })
  it('locks the switch and hat selector until the write answers, and trusts readback', async () => {
    let finish!: (response: Response) => void
    open({ 'PATCH /api/hats/hat/session-tools': () => new Promise<Response>(resolve => { finish = resolve }) })
    const control = await toggle(); await userEvent.click(control)
    await waitFor(() => expect(finish).toBeTypeOf('function'))
    expect(control).toBeDisabled(); expect(control).not.toBeChecked()
    expect(screen.getByLabelText('Delegation hat', { exact: true })).toBeDisabled()
    await act(async () => finish(json(200, { delegation_enabled: false, revision: 7 })))
    await waitFor(() => expect(control).toBeEnabled())
    expect(control).not.toBeChecked()
    expect(screen.getByRole('alert')).toHaveTextContent('could not be saved')
  })
  it('refuses a second step-up instead of opening a second prompt or enabling delegation', async () => {
    const { server, stepUp } = open({ 'PATCH /api/hats/hat/session-tools': json(403, { code: 'step_up_required' }) })
    await userEvent.click(await toggle())
    expect(await screen.findByRole('alert')).toHaveTextContent('could not be saved')
    expect(stepUp).toHaveBeenCalledTimes(1)
    expect(writes(server)).toHaveLength(1)
    expect(screen.getByRole('switch')).not.toBeChecked()
  })
  it('does not label a read failure as OFF and offers retry', async () => {
    open({ 'GET /api/hats/hat/session-tools': json(500, { message: 'Temporarily unavailable' }) })
    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to load session tools')
    expect(screen.queryByRole('switch')).toBeNull()
    expect(screen.getByRole('button', { name: 'Retry session tools' })).toBeVisible()
  })
  it('keeps a purging hat read-only', async () => {
    const { server } = open({}, undefined, { ...HAT, purging: true })
    expect(await toggle()).toBeDisabled()
    expect(writes(server)).toEqual([])
  })
})
