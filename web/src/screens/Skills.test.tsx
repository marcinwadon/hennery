import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, expect, it, vi } from 'vitest'
import App from '../App'
import { hat, host, skill } from '../test-fixtures'
import { FULL, json, stubServer, type Answer } from '../test-server'

const STEP_UP = json(403, { code: 'step_up_required', message: 'm' })
const STEPPED_UP = new Response(null, { status: 204 })
const WITH_SKILLS = { ...FULL, features: ['skills'] }
const HOSTS = [host({ host_id: 'host-a', name: 'alpha' }), host({ host_id: 'host-b', name: 'beta' })]
const state = (host_id: string, status: string, extra = {}) => ({ host_id, status, ...extra })

afterEach(() => { history.replaceState(null, '', '/'); vi.restoreAllMocks() })

function open(routes: Record<string, Answer | Answer[]>, capabilities: unknown = WITH_SKILLS) {
  history.replaceState(null, '', '/skills')
  const server = stubServer({
    'GET /api/capabilities': json(200, capabilities),
    'GET /api/hats': json(200, [hat({ id: 'hat-1', name: 'Personal' })]),
    'GET /api/hosts': json(200, HOSTS),
    'GET /api/skills': json(200, [skill()]),
    'GET /api/skills/s1': json(200, { item: skill(), skill_md: '# Using PDFs\n\nBody.' }),
    'GET /api/skills/s1/hosts': json(200, { skill_id: 's1', hosts: [] }),
    'POST /api/auth/step-up/password': STEPPED_UP,
    ...routes,
  })
  render(<App fetchImpl={server.fetch} />)
  return server
}

async function stepUp() {
  const dialog = await screen.findByRole('dialog', { name: 'This needs a fresh confirmation' })
  await userEvent.type(within(dialog).getByLabelText('Your password'), 'correct horse battery')
  await userEvent.click(within(dialog).getByRole('button', { name: 'Confirm' }))
}

const sent = (server: ReturnType<typeof stubServer>, method: string, path: string) =>
  server.sent.filter((s) => s.method === method && s.path === path)

async function select() {
  await userEvent.click(await screen.findByRole('button', { name: 'pdf-tools' }))
  return screen.findByRole('region', { name: 'Skill details' })
}

it('lists skills and shows a detail with its SKILL.md', async () => {
  open({})
  const card = await screen.findByRole('listitem', { name: 'pdf-tools' })
  expect(card).toHaveTextContent('Work with PDF files')
  expect(card).toHaveTextContent('abcdef012345')
  expect(card).not.toHaveTextContent('abcdef0123456')
  expect(card).toHaveTextContent('Hosts: 0')
  const detail = await select()
  expect(await within(detail).findByRole('heading', { name: 'Using PDFs' })).toBeInTheDocument()
  expect(within(detail).getByRole('button', { name: 'Replace archive' })).toBeInTheDocument()
  expect(within(detail).getByRole('button', { name: 'Delete' })).toBeInTheDocument()
})

it('uploads a tar.gz behind step-up', async () => {
  const server = open({ 'POST /api/skills': [STEP_UP, json(201, skill({ id: 's2', name: 'new-skill' }))] })
  await screen.findByRole('listitem', { name: 'pdf-tools' })
  const file = new File([new Uint8Array([31, 139, 8, 0])], 'new-skill.tar.gz', { type: 'application/gzip' })
  fireEvent.change(screen.getByTestId('skill-input'), { target: { files: [file] } })
  await stepUp()
  expect(await screen.findByRole('listitem', { name: 'new-skill' })).toBeInTheDocument()
  const posts = sent(server, 'POST', '/api/skills')
  expect(posts).toHaveLength(2)
  expect(typeof (posts[0].body as { data: unknown }).data).toBe('string')
  expect(posts[1].body).toEqual(posts[0].body)
})

it('shows the server’s reason when an upload is refused as invalid_skill', async () => {
  const reason = 'the archive has no SKILL.md at its root; most often the skill is wrapped in a top-level directory'
  open({ 'POST /api/skills': json(400, { code: 'invalid_skill', message: reason }) })
  await screen.findByRole('listitem', { name: 'pdf-tools' })
  const file = new File([new Uint8Array([31, 139, 8, 0])], 'pdf.tar.gz', { type: 'application/gzip' })
  fireEvent.change(screen.getByTestId('skill-input'), { target: { files: [file] } })
  expect(await screen.findByRole('alert')).toHaveTextContent(reason)
})

it('ticks hosts and saves the whole set', async () => {
  const server = open({ 'PUT /api/skills/s1/hosts': json(200, { skill_id: 's1', hosts: [state('host-a', 'pending')] }) })
  const detail = await select()
  const alpha = await within(detail).findByRole('checkbox', { name: 'alpha' })
  await userEvent.click(alpha)
  await waitFor(() => expect(alpha).toBeChecked())
  expect(sent(server, 'PUT', '/api/skills/s1/hosts').map((s) => s.body)).toEqual([{ host_ids: ['host-a'] }])
  expect(within(detail).getByRole('checkbox', { name: 'beta' })).not.toBeChecked()
})

it('a revoked assigned host is not sent on the next tick', async () => {
  const server = open({
    'GET /api/hosts': json(200, [...HOSTS, host({ host_id: 'host-r', name: 'gone', revoked_at: '2026-11-01T00:00:00Z' })]),
    'GET /api/skills/s1/hosts': json(200, { skill_id: 's1', hosts: [state('host-r', 'applied')] }),
    'PUT /api/skills/s1/hosts': json(200, { skill_id: 's1', hosts: [state('host-a', 'pending')] }),
  })
  const detail = await select()
  const alpha = await within(detail).findByRole('checkbox', { name: 'alpha' })
  expect(within(detail).queryByRole('checkbox', { name: /gone/ })).toBeNull()
  await userEvent.click(alpha)
  await waitFor(() => expect(alpha).toBeChecked())
  expect(sent(server, 'PUT', '/api/skills/s1/hosts').map((s) => s.body)).toEqual([{ host_ids: ['host-a'] }])
})

it('replacing the archive reads the host states again', async () => {
  const server = open({
    'GET /api/skills/s1/hosts': [
      json(200, { skill_id: 's1', hosts: [state('host-a', 'applied')] }),
      json(200, { skill_id: 's1', hosts: [state('host-a', 'pending')] }),
    ],
    'PUT /api/skills/s1/archive': json(200, skill({ content_hash: 'f'.repeat(64) })),
  })
  const detail = await select()
  const row = (await within(detail).findByRole('checkbox', { name: /alpha/ })).closest('label')!
  await waitFor(() => expect(row).toHaveTextContent('applied'))
  const file = new File([new Uint8Array([31, 139, 8, 0])], 'pdf.tar.gz', { type: 'application/gzip' })
  fireEvent.change(screen.getByTestId('skill-replace-input'), { target: { files: [file] } })
  await waitFor(() => expect(row).toHaveTextContent('pending'))
  expect(sent(server, 'GET', '/api/skills/s1/hosts')).toHaveLength(2)
})

it('an offline host is marked', async () => {
  open({ 'GET /api/skills/s1/hosts': json(200, { skill_id: 's1', hosts: [state('host-a', 'offline')] }) })
  const detail = await select()
  const row = (await within(detail).findByRole('checkbox', { name: /alpha/ })).closest('label')!
  await waitFor(() => expect(row).toHaveTextContent('host offline'))
})

it('an assigned host missing from the loaded list stays in the set', async () => {
  const server = open({
    'GET /api/skills/s1/hosts': json(200, { skill_id: 's1', hosts: [state('host-new', 'applied')] }),
    'PUT /api/skills/s1/hosts': json(200, {
      skill_id: 's1',
      hosts: [state('host-new', 'applied'), state('host-a', 'pending')],
    }),
  })
  const detail = await select()
  const alpha = await within(detail).findByRole('checkbox', { name: 'alpha' })
  await userEvent.click(alpha)
  await waitFor(() => expect(alpha).toBeChecked())
  expect(sent(server, 'PUT', '/api/skills/s1/hosts').map((s) => s.body)).toEqual([
    { host_ids: ['host-new', 'host-a'] },
  ])
})

it('a_host_without_the_capability_is_marked', async () => {
  open({ 'GET /api/skills/s1/hosts': json(200, { skill_id: 's1', hosts: [state('host-a', 'unsupported')] }) })
  const detail = await select()
  const row = (await within(detail).findByRole('checkbox', { name: /alpha/ })).closest('label')!
  expect(row).toHaveTextContent('host too old')
})

it('select this hat’s hosts ticks the hat’s hosts', async () => {
  const server = open({
    'GET /api/skills/hat-hosts/hat-1': json(200, { host_ids: ['host-b'] }),
    'PUT /api/skills/s1/hosts': json(200, { skill_id: 's1', hosts: [state('host-b', 'applied')] }),
  })
  const detail = await select()
  await within(detail).findByRole('checkbox', { name: 'alpha' })
  await userEvent.selectOptions(within(detail).getByRole('combobox', { name: 'Hat' }), 'hat-1')
  await userEvent.click(within(detail).getByRole('button', { name: "Select this hat's hosts" }))
  await waitFor(() => expect(within(detail).getByRole('checkbox', { name: /beta/ })).toBeChecked())
  expect(sent(server, 'PUT', '/api/skills/s1/hosts').map((s) => s.body)).toEqual([{ host_ids: ['host-b'] }])
})

it('a hat name’s hidden characters are shown escaped', async () => {
  open({ 'GET /api/hats': json(200, [hat({ id: 'hat-1', name: 'Work\u202Eevil' })]) })
  const detail = await select()
  await within(detail).findByRole('checkbox', { name: 'alpha' })
  const option = within(within(detail).getByRole('combobox', { name: 'Hat' })).getByRole('option', { name: /Work/ })
  expect(option.textContent).toBe('Work<U+202E>evil')
})

it('is hidden without the feature', async () => {
  const server = open({}, FULL)
  expect(await screen.findByText('Skills are not part of this deployment.')).toBeInTheDocument()
  expect(sent(server, 'GET', '/api/skills')).toHaveLength(0)
})
