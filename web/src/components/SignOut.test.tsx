import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import App from '../App'
import { FULL, json, stubServer } from '../test-server'

afterEach(() => history.replaceState(null, '', '/'))

function signedIn(logout: Response | (() => Response | Promise<Response>)) {
  history.replaceState(null, '', '/mcp')
  const server = stubServer({
    'GET /api/capabilities': json(200, FULL),
    'POST /api/auth/logout': logout,
    'POST /api/auth/passkeys/login/start': json(409, { code: 'no_passkeys', message: 'm' }),
  })
  render(<App fetchImpl={server.fetch} />)
  return server
}

describe('signing out', () => {
  it('goes to the login screen once the server has ended the session', async () => {
    signedIn(new Response(null, { status: 204 }))
    await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
    await waitFor(() => expect(location.pathname).toBe('/login'))
  })

  it('stays, and says the browser is still signed in, when the server refused', async () => {
    signedIn(json(403, { code: 'origin_mismatch', message: 'm' }))
    await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Not signed out: this browser is still signed in. The public URL must be the address this page is open at.',
    )
    expect(location.pathname).toBe('/mcp')
    expect(screen.getByRole('button', { name: 'Sign out' })).toBeEnabled()
  })

  it('stays when the request never reached the server', async () => {
    signedIn(() => {
      throw new TypeError('Failed to fetch')
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Not signed out: this browser is still signed in.')
    expect(location.pathname).toBe('/mcp')
  })
})
