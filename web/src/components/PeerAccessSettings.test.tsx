import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it, vi } from 'vitest'
import { Client } from '../api/client'
import { StepUpCancelled } from '../api/errors'
import { ClientContext } from '../app-client'
import { json, stubServer } from '../test-server'
import type { HatItem, PeerAccess } from '../generated/protocol'
import PeerAccessSettings from './PeerAccessSettings'

const hat: HatItem = { id: 'hat', name: 'Project', colour: '#64748b', default_for_new_hosts: true, created_at: '2026-10-20T09:00:00Z', purging: false, mcp_source_policy: 'legacy', mcp_policy_revision: 0 }
function open(access: PeerAccess, saved: PeerAccess, confirm = vi.fn(async () => {})) {
  const server = stubServer({
    'GET /api/hats/hat/peer-access': json(200, { access, revision: 1 }),
    'PATCH /api/hats/hat/peer-access': json(200, { access: saved, revision: 2 }),
  })
  const client = new Client({ fetch: server.fetch, here: () => ({ pathname: '/settings', search: '' }), navigate: vi.fn(), stepUp: confirm })
  render(<ClientContext.Provider value={client}><PeerAccessSettings hats={[hat]} /></ClientContext.Provider>)
  return { server, confirm }
}

it('enables read access separately from delegation, after operator confirmation', async () => {
  const { server, confirm } = open('off', 'read')
  await userEvent.selectOptions(await screen.findByRole('combobox', { name: 'Session access' }), 'read')
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'Session access' })).toHaveValue('read'))
  expect(confirm).toHaveBeenCalledTimes(1)
  expect(server.sent.filter(r => r.method === 'PATCH').map(r => [r.path, r.body])).toEqual([['/api/hats/hat/peer-access', { access: 'read' }]])
  expect(server.sent.some(r => r.path.includes('session-tools'))).toBe(false)
})

it('can explicitly permit messages in addition to reads', async () => {
  const { server } = open('read', 'read_write')
  await userEvent.selectOptions(await screen.findByRole('combobox', { name: 'Session access' }), 'read_write')
  await screen.findByText('Session collaboration saved. Applies at the next start or resume.', { exact: true })
  expect(server.sent.find(r => r.method === 'PATCH')?.body).toEqual({ access: 'read_write' })
})

it('keeps the saved access and makes no write when confirmation is cancelled', async () => {
  const { server } = open('off', 'read', vi.fn(async () => { throw new StepUpCancelled() }))
  await userEvent.selectOptions(await screen.findByRole('combobox', { name: 'Session access' }), 'read')
  await screen.findByText('Change cancelled. The saved setting is unchanged.', { exact: true })
  expect(screen.getByRole('combobox', { name: 'Session access' })).toHaveValue('off')
  expect(server.sent.filter(r => r.method === 'PATCH')).toHaveLength(0)
})

it('does not report success for a collector response that did not save the selected access', async () => {
  open('off', 'off')
  await userEvent.selectOptions(await screen.findByRole('combobox', { name: 'Session access' }), 'read')
  await screen.findByText('The collector did not save the requested session access.', { exact: true })
  expect(screen.getByRole('combobox', { name: 'Session access' })).toHaveValue('off')
  expect(screen.queryByText('Session collaboration saved. Applies at the next start or resume.', { exact: true })).toBeNull()
})
