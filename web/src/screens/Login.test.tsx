import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { FULL, json, stubServer } from '../test-server'

const CEREMONY = {
  ceremony_id: 'c-1',
  options: { publicKey: { challenge: 'AQID', allowCredentials: [], userVerification: 'required' } },
}

/** A browser with passkeys: `get` answers with a credential whose `toJSON`
 *  says which ceremony it answered. */
function withPasskeys(get = vi.fn(async () => ({ toJSON: () => ({ id: 'cred' }) }))) {
  vi.stubGlobal('PublicKeyCredential', function PublicKeyCredential() {})
  Object.defineProperty(navigator, 'credentials', { value: { get, create: vi.fn() }, configurable: true })
  return get
}

beforeEach(() => history.replaceState(null, '', '/login?next=%2Fhats'))
afterEach(() => {
  vi.unstubAllGlobals()
  Object.defineProperty(navigator, 'credentials', { value: undefined, configurable: true })
  history.replaceState(null, '', '/')
})

describe('login', () => {
  it('offers the passkey first, signs in with it, and returns to next', async () => {
    const get = withPasskeys()
    const server = stubServer({
      'POST /api/auth/passkeys/login/start': json(200, CEREMONY),
      'POST /api/auth/passkeys/login/finish': new Response(null, { status: 204 }),
      'GET /api/capabilities': json(200, FULL),
    })
    render(<App fetchImpl={server.fetch} />)
    const passkey = await screen.findByRole('button', { name: 'Sign in with passkey' })
    const buttons = screen.getAllByRole('button').map((b) => b.textContent)
    expect(buttons.indexOf('Sign in with passkey')).toBeLessThan(buttons.indexOf('Sign in'))
    await userEvent.click(passkey)
    await waitFor(() => expect(location.pathname).toBe('/hats'))
    expect(get).toHaveBeenCalledTimes(1)
    const finish = server.sent.find((s) => s.path === '/api/auth/passkeys/login/finish')
    expect(finish?.body).toEqual({ ceremony_id: 'c-1', credential: { id: 'cred' } })
  })

  it.each(['no_passkeys', 'passkeys_unavailable'])('hides the passkey on 409 %s', async (code) => {
    withPasskeys()
    const server = stubServer({ 'POST /api/auth/passkeys/login/start': json(409, { code, message: 'm' }) })
    render(<App fetchImpl={server.fetch} />)
    await waitFor(() => expect(server.sent).toHaveLength(1))
    expect(screen.queryByRole('button', { name: 'Sign in with passkey' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeInTheDocument()
  })

  it('starts no ceremony in a browser without passkeys', async () => {
    const server = stubServer({})
    render(<App fetchImpl={server.fetch} />)
    expect(await screen.findByRole('button', { name: 'Sign in' })).toBeInTheDocument()
    expect(server.sent).toHaveLength(0)
  })

  it('starts a new ceremony after a refused passkey', async () => {
    withPasskeys()
    const server = stubServer({
      'POST /api/auth/passkeys/login/start': [json(200, CEREMONY), json(200, { ...CEREMONY, ceremony_id: 'c-2' })],
      'POST /api/auth/passkeys/login/finish': json(400, { code: 'invalid_ceremony', message: 'm' }),
    })
    render(<App fetchImpl={server.fetch} />)
    await userEvent.click(await screen.findByRole('button', { name: 'Sign in with passkey' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('The passkey prompt expired. Try again.')
    await waitFor(() =>
      expect(server.sent.filter((s) => s.path === '/api/auth/passkeys/login/start')).toHaveLength(2),
    )
    expect(location.pathname).toBe('/login')
  })

  it('sends nothing when the prompt is dismissed', async () => {
    withPasskeys(
      vi.fn(async () => {
        throw new DOMException('cancelled', 'NotAllowedError')
      }),
    )
    const server = stubServer({ 'POST /api/auth/passkeys/login/start': json(200, CEREMONY) })
    render(<App fetchImpl={server.fetch} />)
    await userEvent.click(await screen.findByRole('button', { name: 'Sign in with passkey' }))
    expect(server.sent.map((s) => s.path)).toEqual(['/api/auth/passkeys/login/start'])
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('signs in with the password, and never follows next off this origin', async () => {
    history.replaceState(null, '', '/login?next=%2F%2Fevil.example%2Fx')
    const server = stubServer({
      'POST /api/auth/login': new Response(null, { status: 204 }),
      'GET /api/capabilities': json(200, FULL),
    })
    render(<App fetchImpl={server.fetch} />)
    await userEvent.type(screen.getByLabelText('Password'), 'correct horse battery')
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    await waitFor(() => expect(location.pathname).toBe('/sessions'))
    expect(server.sent.find((s) => s.path === '/api/auth/login')?.body).toEqual({ password: 'correct horse battery' })
  })

  it('says a wrong password is wrong', async () => {
    const server = stubServer({ 'POST /api/auth/login': json(401, { code: 'invalid_password', message: 'm' }) })
    render(<App fetchImpl={server.fetch} />)
    await userEvent.type(screen.getByLabelText('Password'), 'not the password')
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Wrong password.')
    expect(location.pathname).toBe('/login')
  })

  it('waits out Retry-After before the next try', async () => {
    const server = stubServer({
      'POST /api/auth/login': json(429, { code: 'rate_limited', message: 'm' }, { 'Retry-After': '30' }),
    })
    render(<App fetchImpl={server.fetch} />)
    await userEvent.type(screen.getByLabelText('Password'), 'not the password')
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(await screen.findByText('You can try again in 30 s.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeDisabled()
  })

  it('explains a collector that is not set up', async () => {
    withPasskeys()
    const server = stubServer({
      'POST /api/auth/passkeys/login/start': json(403, { code: 'setup_required', message: 'm' }),
    })
    render(<App fetchImpl={server.fetch} />)
    expect(await screen.findByRole('alert')).toHaveTextContent('hennery admin setup-url')
  })
})
