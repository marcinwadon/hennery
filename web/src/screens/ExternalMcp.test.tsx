import { StrictMode } from 'react'
import { act, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, expect, it } from 'vitest'
import App from '../App'
import { navigate } from '../router'
import { json, stubServer } from '../test-server'
const pending = (id = 'request') => ({
  request: { request_id: id, client_id: 'app-id', client_name: '<script>untrusted app</script>', redirect_uri: 'https://client.example/callback', resource: 'https://collector.example/mcp/external/hennery', scope: 'hennery:discover hennery:sessions:read offline_access', expires_at: 9999999999 },
  hats: [{ id: 'hat', name: 'Work' }], hosts: [{ id: 'host', name: 'Agent host' }],
})
const load = 'GET /api/external-oauth/requests/request'
const consent = 'POST /api/external-oauth/consent'
beforeEach(() => history.replaceState(null, '', '/external-mcp/consent?request=request'))
afterEach(() => history.replaceState(null, '', '/'))
it('keeps older grants reachable through bounded management pages', async () => {
  history.replaceState(null, '', '/external-mcp')
  const rows = Array.from({ length: 100 }, (_, i) => ({ client_id: 'g' + String(i).padStart(3, '0'), label: 'Client ' + i, hat_id: 'hat', roots: [], read_metadata: false, existing_sessions: false, expires_at: 9999999999, revoked: true }))
  const s = stubServer({ 'GET /api/external-clients': json(200, rows), 'GET /api/external-clients?after=g099': json(200, [{ ...rows[0], client_id: 'g100', label: 'Last client', revoked: false }]) })
  render(<App fetchImpl={s.fetch} />)
  await userEvent.click(await screen.findByRole('button', { name: 'Load more clients' }))
  expect(await screen.findByRole('button', { name: 'Revoke access to Last client' })).toBeVisible()
  expect(screen.queryByRole('button', { name: 'Load more clients' })).toBeNull()
})
async function fill() {
  await userEvent.selectOptions(await screen.findByLabelText('Hat'), 'hat')
  await userEvent.selectOptions(screen.getByLabelText('Host'), 'host')
  await userEvent.type(screen.getByLabelText('Workspace root'), '/project')
}
it('shows real callback identity and never grants existing sessions or scope by default', async () => {
  const s = stubServer({ [load]: json(200, pending()) })
  render(<App fetchImpl={s.fetch} />)
  expect(await screen.findByText('client.example', { exact: true })).toBeVisible()
  expect(screen.getByText('<script>untrusted app</script>', { exact: true })).toBeVisible()
  expect(document.querySelectorAll('script')).toHaveLength(0)
  expect(screen.getByLabelText('Also allow existing sessions within this hat, host and root')).not.toBeChecked()
  expect(screen.getByRole('button', { name: 'Allow selected access' })).toBeDisabled()
  expect(s.sent.every(r => r.method === 'GET')).toBe(true)
})
it('captures selected boundaries and blocks duplicate clicks until the result', async () => {
  let finish!: (r: Response) => void
  const s = stubServer({ [load]: json(200, pending()), [consent]: () => new Promise<Response>(resolve => { finish = resolve }) })
  render(<StrictMode><App fetchImpl={s.fetch} /></StrictMode>)
  await fill()
  await userEvent.click(screen.getByLabelText('Also allow existing sessions within this hat, host and root'))
  await userEvent.dblClick(screen.getByRole('button', { name: 'Allow selected access' }))
  expect(s.sent.filter(r => r.method === 'POST')).toHaveLength(1)
  expect(s.sent.find(r => r.method === 'POST')?.body).toEqual({ request_id: 'request', allow: true, hat_id: 'hat', roots: [{ host_id: 'host', root: '/project' }], read_metadata: true, existing_sessions: true, expires_in_days: 7 })
  await act(async () => finish(json(400, { code: 'invalid_request', message: 'Expired request' })))
  expect(await screen.findByRole('alert')).toHaveTextContent('Expired request')
})
it('cannot check metadata or existing-session authority absent from requested scope', async () => {
  const p = pending(); p.request.scope = 'hennery:discover'
  const s = stubServer({ [load]: json(200, p) })
  render(<App fetchImpl={s.fetch} />)
  const read = await screen.findByLabelText('Read session metadata')
  expect(read).not.toBeChecked(); expect(read).toBeDisabled()
  expect(screen.getByLabelText('Also allow existing sessions within this hat, host and root')).toBeDisabled()
})
it('requires explicit control and transcript consent and clears both when metadata is removed', async () => {
  const p = pending(); p.request.scope += ' hennery:sessions:control hennery:sessions:transcript'
  const s = stubServer({ [load]: json(200, p), [consent]: json(400, { code: 'expired', message: 'Expired' }) })
  render(<App fetchImpl={s.fetch} />)
  await fill()
  const control = screen.getByLabelText('Create, prompt and control sessions')
  const transcript = screen.getByLabelText('Read session conversations')
  expect(control).not.toBeChecked(); expect(transcript).not.toBeChecked()
  await userEvent.click(control); await userEvent.click(transcript)
  await userEvent.click(screen.getByLabelText('Read session metadata'))
  expect(control).not.toBeChecked(); expect(control).toBeDisabled()
  expect(transcript).not.toBeChecked(); expect(transcript).toBeDisabled()
  await userEvent.click(screen.getByLabelText('Read session metadata'))
  await userEvent.click(control); await userEvent.click(transcript)
  await userEvent.click(screen.getByRole('button', { name: 'Allow selected access' }))
  await screen.findByRole('alert')
  expect(s.sent.at(-1)?.body).toMatchObject({ read_metadata: true, read_transcript: true, control_sessions: true, existing_sessions: false })
})
it('cannot grant control or transcripts omitted from requested scopes', async () => {
  const s = stubServer({ [load]: json(200, pending()) })
  render(<App fetchImpl={s.fetch} />)
  expect(await screen.findByLabelText('Create, prompt and control sessions')).toBeDisabled()
  expect(screen.getByLabelText('Read session conversations')).toBeDisabled()
})
it('denies without requiring a host, root or hat and never sends an allow', async () => {
  const s = stubServer({ [load]: json(200, pending()), [consent]: json(400, { code: 'invalid_request', message: 'Already denied' }) })
  render(<App fetchImpl={s.fetch} />)
  await userEvent.click(await screen.findByRole('button', { name: 'Deny' }))
  await screen.findByRole('alert')
  expect(s.sent.filter(r => r.method === 'POST')).toHaveLength(1)
  expect(s.sent.at(-1)?.body).toMatchObject({ request_id: 'request', allow: false, existing_sessions: false })
})
it('preserves request through login and returns only to the local consent route', async () => {
  const s = stubServer({ [load]: json(401, { code: 'unauthenticated', message: 'Sign in' }) })
  render(<App fetchImpl={s.fetch} />)
  await screen.findByRole('heading', { name: 'Sign in' })
  expect(location.pathname).toBe('/login')
  expect(new URLSearchParams(location.search).get('next')).toBe('/external-mcp/consent?request=request')
  expect(s.sent.some(r => r.method === 'POST')).toBe(false)
})
it('a cancelled step-up does not retry consent or change the displayed scope', async () => {
  const s = stubServer({ [load]: json(200, pending()), [consent]: json(403, { code: 'step_up_required', message: 'Confirm' }) })
  render(<App fetchImpl={s.fetch} />)
  await fill(); await userEvent.click(screen.getByRole('button', { name: 'Allow selected access' }))
  await screen.findByRole('dialog')
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  expect(s.sent.filter(r => r.method === 'POST')).toHaveLength(1)
  expect(screen.getByLabelText('Workspace root')).toHaveValue('/project')
})
it('late request data after navigation cannot authorize the old request', async () => {
  let finish!: (r: Response) => void
  const s = stubServer({ [load]: () => new Promise<Response>(r => { finish = r }), 'GET /api/external-oauth/requests/new': json(200, pending('new')) })
  render(<App fetchImpl={s.fetch} />)
  await waitFor(() => expect(s.sent.length).toBe(1))
  act(() => navigate('/external-mcp/consent?request=new'))
  await fill()
  await act(async () => finish(json(200, pending())))
  expect(screen.getByLabelText('Workspace root')).toHaveValue('/project')
  expect(s.sent.some(r => r.method === 'POST')).toBe(false)
})
it('management revocation needs step-up and updates only the selected grant', async () => {
  history.replaceState(null, '', '/external-mcp')
  const grant = { client_id: 'g', label: 'Remote app', hat_id: 'hat', roots: [{ host_id: 'host', root: '/project' }], read_metadata: true, existing_sessions: false, expires_at: 9999999999, revoked: false }
  const s = stubServer({ 'GET /api/external-clients': json(200, [grant]), 'DELETE /api/external-clients/g': [json(403, { code: 'step_up_required', message: 'Confirm' }), new Response(null, { status: 204 })], 'POST /api/auth/step-up/password': new Response(null, { status: 204 }) })
  render(<App fetchImpl={s.fetch} />)
  await userEvent.click(await screen.findByRole('button', { name: 'Revoke access to Remote app' }))
  await screen.findByRole('dialog')
  await userEvent.type(screen.getByLabelText('Your password'), 'correct password')
  await userEvent.click(screen.getByRole('button', { name: 'Confirm' }))
  expect(await screen.findByText('Revoked', { exact: true })).toBeVisible()
  expect(s.sent.filter(r => r.method === 'DELETE')).toHaveLength(2)
})
