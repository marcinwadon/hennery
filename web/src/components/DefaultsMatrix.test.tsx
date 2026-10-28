import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import { Client } from '../api/client'
import { ClientContext } from '../app-client'
import { json, stubServer } from '../test-server'
import { catalog } from '../test-launch-catalog'
import DefaultsMatrix from './DefaultsMatrix'
const hat = { id: 'hat-a', name: 'Research', colour: '#64748b', created_at: '', default_for_new_hosts: false, purging: false, mcp_source_policy: 'legacy' as const, mcp_policy_revision: 0 }
it('edits a matrix cell through the revisioned API and preserves unrelated overrides', async () => {
  const server = stubServer({ 'PUT /api/hats/hat-a/session-defaults?agent=claude': json(200, { hat_id: hat.id, agent: 'claude', revision: 8, config: { model: 'sonnet', axes: { fast: false } } }) })
  const stepUp = vi.fn(), saved = vi.fn()
  const client = new Client({ fetch: server.fetch, navigate: vi.fn(), here: () => ({ pathname: '/defaults', search: '' }), stepUp })
  render(<ClientContext.Provider value={client}><DefaultsMatrix hats={[hat]} records={{ [hat.id]: { hat_id: hat.id, agent: 'claude', revision: 7, config: { axes: { fast: false } } } }} catalog={catalog} onSaved={saved} /></ClientContext.Provider>)
  const group = screen.getByRole('group', { name: 'model for Research' })
  fireEvent.change(within(group).getByRole('combobox'), { target: { value: 'sonnet' } })
  expect(server.sent).toHaveLength(0)
  fireEvent.click(screen.getByRole('button', { name: 'Confirm save' }))
  await waitFor(() => expect(saved).toHaveBeenCalled())
  expect(server.sent[0].body).toEqual({ revision: 7, config: { model: 'sonnet', axes: { fast: false } } })
})
