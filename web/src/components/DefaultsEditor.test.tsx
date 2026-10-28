import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import { Client } from '../api/client'
import { ClientContext } from '../app-client'
import { json, stubServer } from '../test-server'
import { catalog } from '../test-launch-catalog'
import type { HatSessionDefaults, HostCatalog } from '../generated/protocol'
import DefaultsEditor from './DefaultsEditor'
const hat = { id: 'hat-a', name: 'Research', colour: '#64748b', created_at: '', default_for_new_hosts: false, purging: false, mcp_source_policy: 'legacy' as const, mcp_policy_revision: 0 }
function mount(config: HatSessionDefaults['config'], preview: HostCatalog | null = catalog, status = 200) {
  const server = stubServer({ 'PUT /api/hats/hat-a/session-defaults?agent=claude': json(status, status === 200 ? { hat_id: 'hat-a', agent: 'claude', revision: 2, config: {} } : { code: 'defaults_changed', message: 'Reload required' }) })
  const client = new Client({ fetch: server.fetch, navigate: vi.fn(), here: () => ({ pathname: '/defaults', search: '' }), stepUp: vi.fn() })
  const saved = vi.fn(), reload = vi.fn()
  render(<ClientContext.Provider value={client}><DefaultsEditor hat={hat} record={{ hat_id: hat.id, agent: 'claude', revision: 1, config }} catalog={preview} onSaved={saved} onReload={reload} /></ClientContext.Provider>)
  return { ...server, saved, reload }
}
it('resets offline, confirms the affected hat/agent, and sends the saved revision', async () => {
  const f = mount({ model: 'unavailable' }, null)
  fireEvent.click(screen.getByRole('button', { name: 'Reset model to inherited' }))
  fireEvent.click(screen.getByRole('button', { name: 'Save defaults' }))
  expect(screen.getByRole('dialog', { name: 'Save defaults for Research' })).toHaveTextContent('Research / claude')
  fireEvent.click(screen.getByRole('button', { name: 'Confirm save' }))
  await waitFor(() => expect(f.saved).toHaveBeenCalled())
  expect(f.sent[0].body).toEqual({ revision: 1, config: {} })
})
it('keeps edits on a revision conflict and requires deliberate reload', async () => {
  const f = mount({ mode: 'bypass' }, catalog, 409)
  fireEvent.click(screen.getByRole('button', { name: 'Reset mode to inherited' }))
  fireEvent.click(screen.getByRole('button', { name: 'Save defaults' }))
  fireEvent.click(screen.getByRole('button', { name: 'Confirm save' }))
  await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('Your edits are retained'))
  expect(screen.getByRole('button', { name: 'Save defaults' })).toBeDisabled()
  fireEvent.click(screen.getByRole('button', { name: 'Reload and review' }))
  expect(f.reload).toHaveBeenCalledOnce()
})
it('preserves false as an explicit override and shows reduced-prompting warning', () => {
  mount({ mode: 'bypass', axes: { fast: false } })
  expect(screen.getByRole('checkbox', { name: 'Fast' })).not.toBeChecked()
  expect(screen.getByRole('button', { name: 'Reset Fast to inherited' })).toBeEnabled()
  expect(screen.getByText('mode is outside the agent’s permission baseline. The agent may act with fewer prompts.', { exact: true })).toBeVisible()
})
it('holds a new replacement when its catalog context is no longer usable', () => {
  const client = new Client({ fetch: vi.fn(), navigate: vi.fn(), here: () => ({ pathname: '/defaults', search: '' }), stepUp: vi.fn() })
  const record = { hat_id: hat.id, agent: 'claude', revision: 0, config: {} }
  const view = (preview: HostCatalog | null) => <ClientContext.Provider value={client}><DefaultsEditor hat={hat} record={record} catalog={preview} onSaved={vi.fn()} onReload={vi.fn()} /></ClientContext.Provider>
  const { rerender } = render(view(catalog))
  fireEvent.change(screen.getByRole('combobox', { name: 'model' }), { target: { value: 'sonnet' } })
  expect(screen.getByRole('button', { name: 'Save defaults' })).toBeEnabled()
  rerender(view(null))
  expect(screen.getByRole('button', { name: 'Save defaults' })).toBeDisabled()
  expect(screen.getByRole('alert')).toHaveTextContent('A chosen value is unavailable in this preview.')
})
it('pins the edited revision when a later record arrives instead of silently rebasing the write', async () => {
  const server = stubServer({ 'PUT /api/hats/hat-a/session-defaults?agent=claude': json(409, { code: 'defaults_changed', message: 'Defaults changed' }) })
  const client = new Client({ fetch: server.fetch, navigate: vi.fn(), here: () => ({ pathname: '/defaults', search: '' }), stepUp: vi.fn() })
  const view = (revision: number) => <ClientContext.Provider value={client}><DefaultsEditor hat={hat} record={{ hat_id: hat.id, agent: 'claude', revision, config: {} }} catalog={catalog} onSaved={vi.fn()} onReload={vi.fn()} /></ClientContext.Provider>
  const { rerender } = render(view(1))
  fireEvent.change(screen.getByRole('combobox', { name: 'mode' }), { target: { value: 'bypass' } })
  rerender(view(2))
  fireEvent.click(screen.getByRole('button', { name: 'Save defaults' }))
  fireEvent.click(screen.getByRole('button', { name: 'Confirm save' }))
  await waitFor(() => expect(server.sent).toHaveLength(1))
  expect(server.sent[0].body).toEqual({ revision: 1, config: { mode: 'bypass' } })
})
