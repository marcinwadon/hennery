import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { FULL, json, stubServer } from '../test-server'

const PASSWORD = 'correct horse battery'

/** The app as a browser opening `url` loads it: the token module first. */
async function load(url: string) {
  history.replaceState(null, '', url)
  vi.resetModules()
  await import('../setup-token')
  const { default: App } = await import('../App')
  return App
}

async function fill() {
  await userEvent.type(screen.getByLabelText(/^Password at least/), PASSWORD)
  await userEvent.type(screen.getByLabelText('Password again'), PASSWORD)
}

beforeEach(() => vi.unstubAllGlobals())
afterEach(() => history.replaceState(null, '', '/'))

describe('setup', () => {
  it('takes the token from the fragment and out of the address bar at once', async () => {
    const before = history.length
    await load('/setup#tok-123')
    expect(location.href).toBe(`${location.origin}/setup`)
    expect(history.length).toBe(before)
  })

  it('sends the token, password, this origin and the hat name, then offers a passkey', async () => {
    const App = await load('/setup#tok-123')
    const server = stubServer({ 'POST /api/setup': json(201, { public_url: location.origin }) })
    render(<App fetchImpl={server.fetch} />)
    expect(screen.getByLabelText(/^Public URL/)).toHaveValue(location.origin)
    expect(screen.getByLabelText(/^Default hat/)).toHaveValue('Personal')
    await fill()
    await userEvent.click(screen.getByRole('button', { name: 'Set up' }))
    expect(await screen.findByRole('heading', { name: 'hennery is set up' })).toBeInTheDocument()
    expect(server.sent).toEqual([
      {
        method: 'POST',
        path: '/api/setup',
        body: { token: 'tok-123', password: PASSWORD, public_url: location.origin, default_hat_name: 'Personal' },
      },
    ])
    expect((await import('../setup-token')).setupToken()).toBeNull()
  })

  it('registers a passkey right after, with no second password', async () => {
    const create = vi.fn(async () => ({ toJSON: () => ({ id: 'new-cred' }) }))
    vi.stubGlobal('PublicKeyCredential', function PublicKeyCredential() {})
    Object.defineProperty(navigator, 'credentials', { value: { create, get: vi.fn() }, configurable: true })
    const App = await load('/setup#tok-123')
    const server = stubServer({
      'POST /api/setup': json(201, { public_url: location.origin }),
      'POST /api/auth/passkeys/register/start': json(200, {
        ceremony_id: 'r-1',
        options: { publicKey: { challenge: 'AQID', rp: { name: 'hennery' }, user: { id: 'AQ', name: 'o', displayName: 'o' } } },
      }),
      'POST /api/auth/passkeys/register/finish': json(201, { id: 'passkey-1', label: 'Laptop', created_at: 'x' }),
      'GET /api/capabilities': json(200, FULL),
    })
    render(<App fetchImpl={server.fetch} />)
    await fill()
    await userEvent.click(screen.getByRole('button', { name: 'Set up' }))
    const label = await screen.findByLabelText('Passkey name')
    await userEvent.clear(label)
    await userEvent.type(label, 'Laptop')
    await userEvent.click(screen.getByRole('button', { name: 'Add a passkey' }))
    expect(await screen.findByText(/Your passkey is added/)).toBeInTheDocument()
    expect(server.sent.map((s) => s.path)).toEqual([
      '/api/setup',
      '/api/auth/passkeys/register/start',
      '/api/auth/passkeys/register/finish',
    ])
    expect(server.sent[1].body).toEqual({ label: 'Laptop' })
    expect(server.sent[2].body).toEqual({ ceremony_id: 'r-1', credential: { id: 'new-cred' } })
    await userEvent.click(screen.getByRole('button', { name: 'Continue' }))
    await waitFor(() => expect(location.pathname).toBe('/sessions'))
    Object.defineProperty(navigator, 'credentials', { value: undefined, configurable: true })
  })

  it('says a used link is used, and forgets its token', async () => {
    const App = await load('/setup#old')
    const server = stubServer({ 'POST /api/setup': json(401, { code: 'invalid_setup_token', message: 'm' }) })
    render(<App fetchImpl={server.fetch} />)
    await fill()
    await userEvent.click(screen.getByRole('button', { name: 'Set up' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('hennery admin setup-url')
    expect(location.pathname).toBe('/setup')
    expect((await import('../setup-token')).setupToken()).toBeNull()
  })

  it('points to sign-in once set up already', async () => {
    const App = await load('/setup#tok')
    const server = stubServer({ 'POST /api/setup': json(409, { code: 'already_set_up', message: 'm' }) })
    render(<App fetchImpl={server.fetch} />)
    await fill()
    await userEvent.click(screen.getByRole('button', { name: 'Set up' }))
    expect(await screen.findByRole('link', { name: 'Sign in' })).toHaveAttribute('href', '/login')
  })

  it('refuses two different passwords without asking the server', async () => {
    const App = await load('/setup#tok')
    const server = stubServer({})
    render(<App fetchImpl={server.fetch} />)
    await userEvent.type(screen.getByLabelText(/^Password at least/), PASSWORD)
    await userEvent.type(screen.getByLabelText('Password again'), PASSWORD + 'x')
    await userEvent.click(screen.getByRole('button', { name: 'Set up' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('The two passwords differ.')
    expect(server.sent).toHaveLength(0)
  })

  it('without a token, names the command that prints the link and shows no form', async () => {
    const App = await load('/setup')
    const server = stubServer({})
    render(<App fetchImpl={server.fetch} />)
    expect(screen.getByRole('alert')).toHaveTextContent('hennery admin setup-url')
    expect(screen.queryByRole('button', { name: 'Set up' })).toBeNull()
    expect(server.sent).toHaveLength(0)
  })

  it('asks nothing of the server before the form is sent', async () => {
    const App = await load('/setup#tok')
    const server = stubServer({})
    render(<App fetchImpl={server.fetch} />)
    await new Promise((r) => setTimeout(r, 20))
    expect(server.sent).toHaveLength(0)
  })
})
