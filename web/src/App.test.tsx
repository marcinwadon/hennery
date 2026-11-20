import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import { heldFor, hold } from './lib/attachments'
import { sendingFor, track } from './lib/sending'
import { FULL, GATEWAY, json, stubServer } from './test-server'
import { useActionSnapshot } from './app-actions'
vi.mock('./screens/Defaults', () => ({ default: () => <h1>Defaults entry point</h1> }))
vi.mock('./components/CommandHost', () => ({ default: () => {
  const actions = useActionSnapshot()
  return <output aria-label="Defaults action availability">{String(actions?.get('open-defaults')?.enabled ?? false)}</output>
} }))

function at(path: string) {
  history.replaceState(null, '', path)
}

afterEach(() => {
  at('/')
  sessionStorage.clear()
})

describe('the shell', () => {
  it('shows every view of the whole cockpit, the current one marked', async () => {
    at('/hosts')
    const server = stubServer({ 'GET /api/capabilities': json(200, { ...FULL, features: ['session_defaults'] }) })
    render(<App fetchImpl={server.fetch} />)
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    const names = within(rail)
      .getAllByRole('link')
      .filter(a => a.classList.contains('nav-item'))
      .map((a) => a.textContent?.trim())
    expect(names).toEqual(['Needs you', 'All sessions', 'New session', 'Defaults', 'Hosts', 'MCP', 'Hats', 'Settings'])
    expect(within(rail).getByRole('link', { name: 'Hosts' })).toHaveAttribute('aria-current', 'page')
    const tabs = screen.getByRole('navigation', { name: 'Primary navigation' })
    expect(within(tabs).getAllByRole('link').map((a) => a.textContent)).toEqual(['Needs you', 'Sessions', 'New', 'Hosts', 'More'])
  })

  it('shows only MCP and Settings in gateway mode, and no other view', async () => {
    at('/hosts')
    const server = stubServer({ 'GET /api/capabilities': json(200, GATEWAY) })
    render(<App fetchImpl={server.fetch} />)
    const rail = await screen.findByRole('complementary', { name: 'Views' })
    expect(within(rail).getAllByRole('link').map((a) => a.textContent?.trim())).toEqual(['MCP', 'Settings'])
    const tabs = screen.getByRole('navigation', { name: 'Primary navigation' })
    expect(within(tabs).getAllByRole('link').map((a) => a.textContent)).toEqual(['MCP', 'More'])
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
    expect(await screen.findByRole('heading', { name: 'Hats' })).toBeInTheDocument()
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
    // The view shows the id while it loads, and with the deleted notice
    // (this stub has no session): text either way.
    await waitFor(() => expect(screen.getByText('<b>x</b>')).toBeInTheDocument())
    expect(document.querySelector('b')).toBeNull()
    // The session view, not a placeholder: this stub knows no session.
    expect(await screen.findByRole('heading', { name: 'This session was deleted' })).toBeInTheDocument()
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

  it('signs out, forgetting the images held for every draft', async () => {
    at('/settings')
    hold('s1', { attachments: [{ n: 1, file: new File(['x'], 'a.png', { type: 'image/png' }) }], nextN: 2 })
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/auth/logout': new Response(null, { status: 204 }),
      'POST /api/auth/passkeys/login/start': json(409, { code: 'no_passkeys', message: 'm' }),
    })
    render(<App fetchImpl={server.fetch} />)
    await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
    await waitFor(() => expect(location.pathname).toBe('/login'))
    expect(server.sent.some((s) => s.method === 'POST' && s.path === '/api/auth/logout')).toBe(true)
    expect(heldFor('s1').attachments).toEqual([])
  })

  it('signs out, letting go of a send still in flight', async () => {
    at('/settings')
    track('s1', () => new Promise<never>(() => {}), String)
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/auth/logout': new Response(null, { status: 204 }),
      'POST /api/auth/passkeys/login/start': json(409, { code: 'no_passkeys', message: 'm' }),
    })
    render(<App fetchImpl={server.fetch} />)
    expect(sendingFor('s1')).toBeDefined()
    await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
    await waitFor(() => expect(location.pathname).toBe('/login'))
    expect(sendingFor('s1')).toBeUndefined()
  })

  it('signs out, forgetting every draft’s text and keeping the rest of sessionStorage', async () => {
    at('/settings')
    sessionStorage.setItem('hennery.draft.s1', 'the token is in here')
    sessionStorage.setItem('hennery.draft.s2', 'and another')
    sessionStorage.setItem('hennery.hideClosed', '1')
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/auth/logout': new Response(null, { status: 204 }),
      'POST /api/auth/passkeys/login/start': json(409, { code: 'no_passkeys', message: 'm' }),
    })
    render(<App fetchImpl={server.fetch} />)
    await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
    await waitFor(() => expect(location.pathname).toBe('/login'))
    expect(sessionStorage.getItem('hennery.draft.s1')).toBeNull()
    expect(sessionStorage.getItem('hennery.draft.s2')).toBeNull()
    expect(sessionStorage.getItem('hennery.hideClosed')).toBe('1')
  })
})

it.each(['/needs-you', '/defaults', '/new', '/hats', '/sessions/one'])('gateway refuses %s without session/default/delegation reads', async path => {
  at(path)
  const server = stubServer({ 'GET /api/capabilities': json(200, GATEWAY) })
  render(<App fetchImpl={server.fetch} />)
  await screen.findByText('This view is not part of this deployment.', { exact: true })
  expect(server.sent.map(request => request.path)).toEqual(['/api/capabilities'])
})
it('More contains every configured destination and sign out', async () => {
  at('/more')
  const server = stubServer({ 'GET /api/capabilities': json(200, { ...FULL, features: ['session_defaults'] }) })
  render(<App fetchImpl={server.fetch} />)
  const nav = await screen.findByRole('navigation', { name: 'More navigation' })
  expect(within(nav).getAllByRole('link').map(link => link.getAttribute('href'))).toEqual(['/mcp', '/hats', '/defaults', '/settings'])
  expect(screen.getByRole('button', { name: 'Sign out' })).toBeInTheDocument()
})
it('More lists Skills when the collector serves them', async () => {
  at('/more')
  const server = stubServer({ 'GET /api/capabilities': json(200, { ...FULL, features: ['session_defaults', 'skills'] }) })
  render(<App fetchImpl={server.fetch} />)
  const nav = await screen.findByRole('navigation', { name: 'More navigation' })
  expect(within(nav).getAllByRole('link').map(link => link.getAttribute('href'))).toEqual(['/mcp', '/skills', '/hats', '/defaults', '/settings'])
})

it('disables the command entry until this deployment has an enabled provider', async () => {
  at('/more')
  render(<App fetchImpl={stubServer({ 'GET /api/capabilities': json(200, GATEWAY) }).fetch} />)
  const rail = await screen.findByRole('complementary', { name: 'Views' })
  expect(within(rail).getByRole('button', { name: 'Jump to' })).toBeDisabled()
})

it.each([false, true])('mounts the Defaults entry point only with its backend capability: %s', async enabled => {
  at('/defaults')
  render(<App fetchImpl={stubServer({ 'GET /api/capabilities': json(200, { ...FULL, features: enabled ? ['session_defaults'] : [] }), 'GET /api/hats': json(200, []), 'GET /api/hosts': json(200, []) }).fetch} />)
  if (enabled) expect(await screen.findByRole('heading', { name: 'Defaults entry point' })).toBeInTheDocument()
  else {
    expect(await screen.findByRole('heading', { name: 'Defaults' })).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Defaults entry point' })).toBeNull()
  }
  await waitFor(() => expect(screen.getByRole('status', { name: 'Defaults action availability' })).toHaveTextContent(String(enabled)))
})
