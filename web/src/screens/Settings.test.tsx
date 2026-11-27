import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import Settings from './Settings'
import { Client } from '../api/client'
import { ClientContext } from '../app-client'
import { json, stubServer, type Answer } from '../test-server'
const mocked = vi.hoisted(() => ({ permission: vi.fn(), enable: vi.fn(), disable: vi.fn(), reg: {} }))
vi.mock('../pwa/lifecycle', () => ({ usePwa: () => ({ registration: mocked.reg, error: null }), registerWorker: vi.fn() }))
vi.mock('../pwa/device', async (original) => {
  const actual = await original<typeof import('../pwa/device')>()
  return { ...actual, BrowserPush: class {
    enable = mocked.enable; disable = mocked.disable
    reconcile = async () => ({ subscription: null, record: { generation: 0, enabled: false }, matches: true })
  } }
})
vi.mock('../pwa/journal', () => ({ pushJournal: () => ({ read: async () => ({ generation: 0, enabled: false }) }) }))
beforeEach(() => {
  vi.stubGlobal('Notification', { permission: 'default', requestPermission: mocked.permission.mockReset().mockResolvedValue('granted') })
  vi.stubGlobal('PushManager', class {})
  vi.stubGlobal('isSecureContext', true)
  Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: {} })
  Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: vi.fn() } })
  mocked.enable.mockReset(); mocked.disable.mockReset()
})
afterEach(() => vi.unstubAllGlobals())
function page(push = true, routes: Record<string, Answer | Answer[]> = {}) {
  const server = stubServer({
    'GET /api/push/vapid': json(200, { public_key: btoa(String.fromCharCode(4, ...Array(64).fill(1))) }),
    'GET /api/push/subscriptions': json(200, []),
    'GET /api/auth/sessions': json(200, [{ id: 'login', user_agent: '<script>bad</script>', current: true, created_at: 'today', last_seen_at: 'today', expires_at: 'tomorrow' }]),
    'GET /api/settings': json(200, { public_url: 'https://app.example', contact: 'mailto:test@example.invalid' }),
    'GET /api/auth/passkeys': json(200, [{ id: 'key/one', label: 'Personal key', created_at: '2026-10-20T09:00:00Z' }]),
    'DELETE /api/auth/passkeys/key%2Fone': new Response(null, { status: 204 }),
    'GET /api/settings/attachments': json(200, { count: 2, bytes: 42 }),
    'GET /api/settings/host-removals': json(200, []),
    'GET /api/hats': json(200, [{ id: 'hat', name: 'Personal', colour: '#000', default_for_new_hosts: true }]),
    'GET /api/hosts': json(200, []),
    'GET /api/hats/hat/session-tools': json(200, { delegation_enabled: false, revision: 1 }),
    'GET /api/hats/hat/peer-access': json(200, { access: 'off', revision: 0 }),
    'GET /api/settings/delegation-limits': json(200, { children: 8, starts_per_operator_epoch: 32, automatic_wakes_per_operator_epoch: 20 }),
    'GET /api/settings/idle-parking': json(200, { idle_timeout_secs: null }),
    'GET /api/push/policies': json(200, [{ hat_id: 'hat', muted: false, details: false, generic_title: false }]),
    'PATCH /api/settings': json(200, { public_url: 'https://app.example', contact: 'mailto:new@example.invalid' }),
    'PUT /api/push/policies/hat': json(200, { hat_id: 'hat', muted: false, details: true, generic_title: true }),
    ...routes,
  })
  const confirm = vi.fn(async () => {})
  const client = new Client({ fetch: server.fetch, here: () => ({ pathname: '/settings', search: '' }), navigate: vi.fn(), stepUp: confirm })
  render(<ClientContext.Provider value={client}><Settings push={push} sessions={push} /></ClientContext.Provider>)
  return { server, confirm }
}
describe('minimum device Settings', () => {
  it('keeps About after every settings section', async () => {
    page(false)
    await screen.findByRole('heading', { name: 'Signed-in devices' })
    const about = screen.getByRole('region', { name: 'About' })
    expect(about.parentElement?.lastElementChild).toBe(about)
  })
  it('loads a read-only Defaults summary and leaves editing on the Defaults screen', async () => {
    const { server } = page(true, {
      'GET /api/hosts': json(200, [{ host_id: 'host', name: 'Host', mcp_delivery: { custom: 'isolated' } }]),
      'GET /api/hats/hat/session-defaults?agent=custom': json(200, { hat_id: 'hat', agent: 'custom', revision: 1, config: { model: 'stored-model', axes: { fast: false } } }),
    })
    const section = await screen.findByRole('region', { name: 'Session defaults' })
    expect(await within(section).findByText('stored-model', { exact: true })).toBeVisible()
    expect(within(section).getByText('Off', { exact: true })).toBeVisible()
    expect(within(section).getByRole('link', { name: 'Session defaults' })).toHaveAttribute('href', '/defaults?hat=hat&agent=custom')
    expect(server.sent.filter(row => row.path.includes('session-defaults')).map(row => row.method)).toEqual(['GET'])
  })
  it('registers a passkey through the ceremony and displays the returned credential label', async () => {
    vi.stubGlobal('PublicKeyCredential', class {})
    const original = Object.getOwnPropertyDescriptor(navigator, 'credentials')
    const credential = { id: 'credential', type: 'public-key' }
    const create = vi.fn(async () => ({ toJSON: () => credential }))
    Object.defineProperty(navigator, 'credentials', { configurable: true, value: { create } })
    try {
      const { server } = page(true, {
        'POST /api/auth/passkeys/register/start': json(200, { ceremony_id: 'ceremony', options: { publicKey: { challenge: 'AQ', user: { id: 'Ag' } } } }),
        'POST /api/auth/passkeys/register/finish': json(200, { id: 'new-key', label: 'Returned label', created_at: '2026-10-20T12:00:00Z' }),
      })
      await userEvent.type(await screen.findByLabelText('Passkey label', { exact: true }), 'New key')
      await waitFor(() => expect(screen.getByRole('button', { name: 'Add passkey' })).toBeEnabled())
      await userEvent.click(screen.getByRole('button', { name: 'Add passkey' }))
      expect(await screen.findByRole('heading', { name: 'Returned label' })).toBeVisible()
      expect(create).toHaveBeenCalledTimes(1)
      expect(server.sent.find(row => row.path.endsWith('/register/start'))?.body).toEqual({ label: 'New key' })
      expect(server.sent.find(row => row.path.endsWith('/register/finish'))?.body).toEqual({ ceremony_id: 'ceremony', credential })
    } finally {
      if (original) Object.defineProperty(navigator, 'credentials', original)
      else Reflect.deleteProperty(navigator, 'credentials')
    }
  })
  it('describes cleanup from actual receipts without claiming unreported removal or copies', async () => {
    page(true, { 'GET /api/settings/host-removals': json(200, [
      { id: 'pending', agent: 'Pending agent', state: 'pending', attempts: 0 },
      { id: 'missing', agent: 'Unknown agent', state: 'final', attempts: 1 },
      ...(['removed', 'partial', 'none', 'pending'] as const).map((state, index) => ({ id: String(index), agent: `Agent ${index}`, state: 'final', attempts: 1, last_result: { state, remaining: [], notes: [] } })),
    ]) })
    for (const text of ['Pending agent: Waiting for the host to retry', 'Unknown agent: No cleanup result reported', 'Agent 0: Transcript removed', 'Agent 1: Transcript removed in part', 'Agent 2: No agent transcript record', 'Agent 3: Transcript removal unconfirmed']) {
      expect(await screen.findByText((_, element) => element?.tagName === 'P' && element.textContent === text)).toBeVisible()
    }
  })
  it('isolates failed notification checks from passkeys, privacy and collector reads', async () => {
    page(true, { 'GET /api/push/vapid': json(500, { message: 'Notification service unavailable' }) })
    expect(await screen.findByRole('button', { name: 'Remove Personal key' })).toBeVisible()
    expect(await screen.findByText('https://app.example', { exact: true })).toBeVisible()
    expect(await screen.findByRole('group', { name: 'Personal' })).toBeVisible()
    expect(await screen.findByRole('alert')).toHaveTextContent('Notification service unavailable')
    expect(screen.queryByText('Notifications are off on this browser.', { exact: true })).toBeNull()
  })
  it('keeps a passkey when removal confirmation is cancelled', async () => {
    const { server } = page()
    await userEvent.click(await screen.findByRole('button', { name: 'Remove Personal key' }))
    const dialog = screen.getByRole('dialog', { name: 'Remove this passkey?' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(screen.getByRole('button', { name: 'Remove Personal key' })).toBeVisible()
    expect(server.sent.filter(row => row.method === 'DELETE')).toEqual([])
  })
  it('shows appearance, real collector information and removable passkeys', async () => {
    const { server } = page()
    expect(await screen.findByRole('group', { name: 'Appearance' })).toBeVisible()
    expect(await screen.findByText('https://app.example', { exact: true })).toBeVisible()
    expect(await screen.findByText('2 images · 42 bytes', { exact: true })).toBeVisible()
    await userEvent.click(await screen.findByRole('button', { name: 'Remove Personal key' }))
    const dialog = screen.getByRole('dialog', { name: 'Remove this passkey?' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Remove passkey' }))
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Remove Personal key' })).toBeNull())
    expect(server.sent.find(row => row.method === 'DELETE')?.path).toBe('/api/auth/passkeys/key%2Fone')
  })
  it('without origin locks omits notification enable and preserves mobile sign-out', async () => {
    Object.defineProperty(navigator, 'locks', { configurable: true, value: undefined })
    page()
    await screen.findByRole('heading', { name: 'Push devices' })
    expect(screen.queryByRole('button', { name: 'Prepare notifications' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Sign out' })).toBeVisible()
    expect(mocked.permission).not.toHaveBeenCalled()
  })
  it('never prompts on mount; preparation completes step-up, then a fresh enable tap prompts first', async () => {
    const { confirm } = page()
    await userEvent.click(await screen.findByRole('button', { name: 'Prepare notifications' }))
    expect(confirm).toHaveBeenCalledTimes(1)
    expect(mocked.permission).not.toHaveBeenCalled()
    mocked.enable.mockImplementation(() => { expect(mocked.permission).toHaveBeenCalledTimes(1); return Promise.resolve() })
    await userEvent.click(screen.getByRole('button', { name: 'Enable notifications' }))
    await waitFor(() => expect(mocked.enable).toHaveBeenCalledTimes(1))
    expect(mocked.permission).toHaveBeenCalledTimes(1)
    expect(mocked.permission.mock.invocationCallOrder[0]).toBeLessThan(mocked.enable.mock.invocationCallOrder[0])
  })
  it('gateway without web_push omits absent APIs and exposes mobile sign-out', async () => {
    const { server } = page(false)
    await screen.findByRole('heading', { name: 'Signed-in devices' })
    expect(screen.queryByRole('heading', { name: 'Push devices' })).not.toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Notification privacy by hat' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Sign out' })).toBeVisible()
    expect(server.sent.some((c) => c.path.includes('/api/push'))).toBe(false)
    expect(server.sent.some(c => /^\/api\/(?:hats|hosts|settings\/(?:attachments|host-removals))/.test(c.path))).toBe(false)
    expect(screen.queryByRole('heading', { name: 'Delegation by hat' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Delegation limits' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Automatic session parking' })).toBeNull()
    expect(server.sent.some(c => c.path === '/api/settings/idle-parking')).toBe(false)
  })
  it('sends only contact and uses read-back, with no public_url field', async () => {
    const { server } = page()
    const contact = await screen.findByLabelText('Push contact', { exact: true })
    await userEvent.clear(contact); await userEvent.type(contact, 'mailto:new@example.invalid')
    await userEvent.click(screen.getByRole('button', { name: 'Save contact' }))
    await waitFor(() => expect(server.sent.filter((c) => c.method === 'PATCH')).toHaveLength(1))
    expect(server.sent.find((c) => c.method === 'PATCH')?.body).toEqual({ contact: 'mailto:new@example.invalid' })
  })
  it('keeps three independent hat controls, renders User-Agent as isolated text', async () => {
    const { server } = page()
    const card = await screen.findByRole('group', { name: 'Personal' })
    await userEvent.click(within(card).getByLabelText('Mute', { exact: true }))
    await userEvent.click(within(card).getByLabelText('Include question details', { exact: true }))
    await userEvent.click(within(card).getByRole('button', { name: 'Save policy' }))
    await waitFor(() => expect(within(card).getByLabelText('Mute', { exact: true })).not.toBeChecked())
    expect(within(card).getByLabelText('Use a generic title', { exact: true })).toBeChecked()
    expect(server.sent.find((r) => r.method === 'PUT')?.body).toEqual({ muted: true, details: true, generic_title: false })
    const agent = screen.getByText('<script>bad</script>', { exact: true })
    expect(agent.tagName).toBe('BDI')
    expect(document.querySelector('script')).toBeNull()
  })
})
