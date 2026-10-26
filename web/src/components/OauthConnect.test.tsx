import { render, screen, cleanup, fireEvent } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { connection } from '../test-fixtures'
import OauthConnect from './OauthConnect'
import type { OauthState } from '../hooks/useMcpOauth'

const control = vi.hoisted(() => ({ state: { phase: 'idle' } as OauthState, connect: vi.fn(), cancel: vi.fn() }))
it('shows the exact local callback and shell-safe receiver command before connecting', () => {
  render(<OauthConnect connection={connection({ cred_kind: 'oauth_client', oauth: { client_id: 'manual', redirect_uri: 'https://collector.example/api/mcp/oauth/callback', has_client_secret: false } })} refresh={vi.fn()} />)
  fireEvent.change(screen.getByRole('combobox', { name: 'OAuth callback' }), { target: { value: 'local' } })
  const input = screen.getByRole('textbox', { name: 'Local redirect URI' })
  fireEvent.change(input, { target: { value: "http://127.0.0.1:12345/vendor'callback" } })
  expect(screen.getByLabelText('Local callback command', { exact: true })).toHaveTextContent("hennery oauth-callback --collector 'https://collector.example' --redirect-uri 'http://127.0.0.1:12345/vendor'\"'\"'callback'")
  expect(screen.getByText(/A pre-registered client must already permit it/)).toBeVisible()
  fireEvent.click(screen.getByRole('button', { name: 'Connect' }))
  expect(control.connect).toHaveBeenCalledWith(undefined, "http://127.0.0.1:12345/vendor'callback")
  fireEvent.change(input, { target: { value: 'http://evil.example/callback' } })
  expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled()
})
vi.mock('../hooks/useMcpOauth', () => ({ useMcpOauth: () => control }))
afterEach(() => { cleanup(); control.state = { phase: 'idle' }; vi.clearAllMocks() })
it('prepares an HTTPS localhost callback before sending its exact URI to Connect', () => {
  render(<OauthConnect connection={connection({ cred_kind: 'oauth_client', oauth: { client_id: 'manual', redirect_uri: 'https://collector.example/api/mcp/oauth/callback', has_client_secret: true } })} refresh={vi.fn()} />)
  fireEvent.change(screen.getByRole('combobox', { name: 'OAuth callback' }), { target: { value: 'local' } })
  const redirect = 'https://127.0.0.1:9882/callback'
  fireEvent.change(screen.getByRole('textbox', { name: 'Local redirect URI' }), { target: { value: redirect } })
  expect(screen.getByLabelText('Local callback command', { exact: true })).toHaveTextContent(`--redirect-uri '${redirect}'`)
  expect(screen.getByRole('link', { name: 'Prepare HTTPS callback' })).toHaveAttribute('href', redirect)
  expect(screen.getByRole('link', { name: 'Prepare HTTPS callback' })).toHaveAttribute('rel', 'noopener noreferrer')
  fireEvent.click(screen.getByRole('button', { name: 'Connect' }))
  expect(control.connect).toHaveBeenCalledWith(undefined, redirect)
})
it('a recovered gateway report does not assert sign-in completion', () => {
  control.state.phase = 'working'
  render(<OauthConnect connection={connection({ cred_kind: 'oauth_dcr', status: 'needs_auth', has_credential: true })} refresh={vi.fn()} />)
  expect(screen.getByRole('status')).toHaveTextContent('The gateway now reports this connection as Working.')
  expect(document.body.textContent).not.toContain('sign-in completed')
})
it('binds Connect, exact resource acceptance and stop waiting; the isolated link keeps its protections', () => {
  control.state = { phase: 'waiting', link: 'https://vendor.example/authorize', code: 'resource_mismatch' }
  const c = connection({ cred_kind: 'oauth_dcr', oauth: { redirect_uri: '', has_client_secret: false, resource_mismatch: 'https://mcp.example.com/other' } })
  render(<OauthConnect connection={c} refresh={vi.fn()} />)
  fireEvent.click(screen.getByRole('button', { name: 'Connect' })); expect(control.connect).toHaveBeenCalledWith()
  fireEvent.click(screen.getByRole('button', { name: 'Accept and connect' })); expect(control.connect).toHaveBeenLastCalledWith('https://mcp.example.com/other')
  fireEvent.click(screen.getByRole('button', { name: 'Stop waiting' })); expect(control.cancel).toHaveBeenCalledTimes(1)
  expect(screen.getByRole('link', { name: 'Open vendor sign-in' })).toHaveAttribute('rel', 'noopener noreferrer')
  expect(screen.getByRole('link', { name: 'Open vendor sign-in' })).toHaveAttribute('target', '_blank')
})
it('renders unconfirmed and capped errors as text, and own refusal overrides the previous report', () => {
  control.state = { phase: 'unconfirmed', error: '<script>bad</script>' + 'x'.repeat(400), code: 'no_registration_endpoint' }
  const c = connection({ cred_kind: 'oauth_dcr', oauth: { redirect_uri: '', has_client_secret: false, resource_mismatch: 'https://mcp.example.com/old' }, oauth_error: { code: 'resource_mismatch', message: 'old', at: '' } })
  render(<OauthConnect connection={c} refresh={vi.fn()} />)
  expect(screen.getByRole('status')).toHaveTextContent('Sign-in completion could not be confirmed. The card shows the gateway’s last report.')
  expect(screen.getByRole('alert').textContent).toHaveLength(301); expect(document.querySelector('script')).toBeNull()
  expect(screen.queryByRole('button', { name: 'Accept and connect' })).toBeNull()
  expect(screen.getByText('This vendor does not offer automatic registration. Edit the connection to use a pre-registered client.', { exact: true })).toBeVisible()
})
it.each([false, true])('confirmation names the actual connect button with grant=%s', has_credential => {
  control.state.phase = 'confirm'
  render(<OauthConnect connection={connection({ cred_kind: 'oauth_dcr', has_credential })} refresh={vi.fn()} />)
  const name = has_credential ? 'Reconnect' : 'Connect'
  expect(screen.getByRole('button', { name })).toBeVisible()
  expect(screen.getByRole('status')).toHaveTextContent(`Confirmed. Click ${name} again to open sign-in.`)
})
it('confirmation names Accept and connect after resource acceptance', () => {
  control.state = { phase: 'confirm', retry: 'accept' }
  const c = connection({ cred_kind: 'oauth_dcr', oauth: { redirect_uri: 'https://hennery.example/callback', has_client_secret: false, resource_mismatch: 'https://mcp.example.com/other' }, oauth_error: { code: 'resource_mismatch', message: 'compare', at: '2026-10-13T10:00:00Z' } })
  render(<OauthConnect connection={c} refresh={vi.fn()} />)
  expect(screen.getByRole('button', { name: 'Accept and connect' })).toBeVisible()
  expect(screen.getByRole('status')).toHaveTextContent('Confirmed. Click Accept and connect again to open sign-in.')
})
it('drops the retry instruction when a refetch clears acceptance while the gateway reports error', () => {
  control.state = { phase: 'confirm', retry: 'accept' }
  const c = connection({ cred_kind: 'oauth_dcr', status: 'error', has_credential: true, oauth: { redirect_uri: '', has_client_secret: false, resource_mismatch: 'https://mcp.example.com/other' }, oauth_error: { code: 'resource_mismatch', message: 'compare', at: '' } })
  const s = render(<OauthConnect connection={c} refresh={vi.fn()} />)
  expect(screen.getByRole('button', { name: 'Accept and connect' })).toBeVisible()
  expect(screen.getByRole('status')).toHaveTextContent('Confirmed. Click Accept and connect again to open sign-in.')
  s.rerender(<OauthConnect connection={{ ...c, oauth: { redirect_uri: '', has_client_secret: false }, oauth_error: undefined }} refresh={vi.fn()} />)
  expect(screen.queryByRole('button')).toBeNull()
  expect(screen.getByRole('status').textContent).toBe('Confirmed.')
})
it('names the only rendered acceptance button when there is no ordinary Connect action', () => {
  control.state = { phase: 'confirm' }
  const c = connection({ cred_kind: 'oauth_dcr', status: 'error', oauth: { redirect_uri: '', has_client_secret: false, resource_mismatch: 'https://mcp.example.com/other' }, oauth_error: { code: 'resource_mismatch', message: 'compare', at: '' } })
  render(<OauthConnect connection={c} refresh={vi.fn()} />)
  expect(screen.getByRole('button', { name: 'Accept and connect' })).toBeVisible()
  expect(screen.queryByRole('button', { name: 'Connect' })).toBeNull()
  expect(screen.getByRole('status')).toHaveTextContent('Confirmed. Click Accept and connect again to open sign-in.')
})
