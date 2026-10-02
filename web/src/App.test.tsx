import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import App from './App'
import { FULL, GATEWAY, json, stubServer } from './test-server'

function at(path: string) {
  history.replaceState(null, '', path)
}

afterEach(() => at('/'))

describe('the shell', () => {
  it('shows every view of the whole cockpit, the current one marked', async () => {
    at('/hosts')
    const server = stubServer({ 'GET /api/capabilities': json(200, FULL) })
    render(<App fetchImpl={server.fetch} />)
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    const names = within(rail)
      .getAllByRole('link')
      .map((a) => a.textContent?.trim())
    expect(names).toEqual(['New session', 'Sessions', 'Hosts', 'MCP', 'Hats', 'Settings'])
    expect(within(rail).getByRole('link', { name: 'Hosts' })).toHaveAttribute('aria-current', 'page')
    const tabs = screen.getByRole('navigation', { name: 'Tabs' })
    expect(within(tabs).getAllByRole('link').map((a) => a.textContent)).toEqual(['Sessions', 'New', 'Hosts', 'Settings'])
  })

  it('shows only MCP and Settings in gateway mode, and no other view', async () => {
    at('/hosts')
    const server = stubServer({ 'GET /api/capabilities': json(200, GATEWAY) })
    render(<App fetchImpl={server.fetch} />)
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    expect(within(rail).getAllByRole('link').map((a) => a.textContent?.trim())).toEqual(['MCP', 'Settings'])
    const tabs = screen.getByRole('navigation', { name: 'Tabs' })
    expect(within(tabs).getAllByRole('link').map((a) => a.textContent)).toEqual(['MCP', 'Settings'])
    expect(screen.getByText('This view is not part of this deployment.')).toBeInTheDocument()
  })

  it('ignores features it does not know', async () => {
    at('/settings')
    const server = stubServer({ 'GET /api/capabilities': json(200, { mode: 'full', features: ['from_the_future'] }) })
    render(<App fetchImpl={server.fetch} />)
    expect(await screen.findByRole('heading', { name: 'Settings' })).toBeInTheDocument()
  })

  it('moves between views without reloading, and follows the back button', async () => {
    at('/sessions')
    const server = stubServer({ 'GET /api/capabilities': json(200, FULL) })
    render(<App fetchImpl={server.fetch} />)
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    await userEvent.click(within(rail).getByRole('link', { name: 'Hats' }))
    expect(location.pathname).toBe('/hats')
    expect(screen.getByRole('heading', { name: 'Hats' })).toBeInTheDocument()
    act(() => {
      history.back()
    })
    await waitFor(() => expect(location.pathname).toBe('/sessions'))
    await waitFor(() => expect(screen.getByRole('heading', { name: 'Sessions' })).toBeInTheDocument())
    expect(server.sent.filter((s) => s.path === '/api/capabilities')).toHaveLength(1)
  })

  it('opens a session by its link, and renders its id as text', async () => {
    at('/sessions/%3Cb%3Ex%3C%2Fb%3E')
    const server = stubServer({ 'GET /api/capabilities': json(200, FULL) })
    render(<App fetchImpl={server.fetch} />)
    expect(await screen.findByText('<b>x</b>')).toBeInTheDocument()
  })

  it('goes from / to /sessions', async () => {
    at('/')
    const server = stubServer({ 'GET /api/capabilities': json(200, FULL) })
    render(<App fetchImpl={server.fetch} />)
    await waitFor(() => expect(location.pathname).toBe('/sessions'))
  })

  it('says an unknown path is not found', async () => {
    at('/no/such/page')
    const server = stubServer({ 'GET /api/capabilities': json(200, FULL) })
    render(<App fetchImpl={server.fetch} />)
    expect(await screen.findByRole('heading', { name: 'Not found' })).toBeInTheDocument()
  })

  it('sends a signed-out browser to sign in, to come back afterwards', async () => {
    at('/hosts?x=1')
    const server = stubServer({
      'GET /api/capabilities': json(401, { code: 'unauthenticated', message: 'sign in' }),
      'POST /api/auth/passkeys/login/start': json(409, { code: 'no_passkeys', message: 'm' }),
    })
    render(<App fetchImpl={server.fetch} />)
    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeInTheDocument()
    expect(location.pathname + location.search).toBe('/login?next=%2Fhosts%3Fx%3D1')
  })

  it('signs out', async () => {
    at('/settings')
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/auth/logout': new Response(null, { status: 204 }),
      'POST /api/auth/passkeys/login/start': json(409, { code: 'no_passkeys', message: 'm' }),
    })
    render(<App fetchImpl={server.fetch} />)
    await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
    await waitFor(() => expect(location.pathname).toBe('/login'))
    expect(server.sent.some((s) => s.method === 'POST' && s.path === '/api/auth/logout')).toBe(true)
  })
})
