import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, expect, it, vi } from 'vitest'
import App from '../App'
import { hat, hookSet, host } from '../test-fixtures'
import { FULL, json, stubServer, type Answer } from '../test-server'

const STEP_UP = json(403, { code: 'step_up_required', message: 'm' })
const STEPPED_UP = new Response(null, { status: 204 })
const WITH_HOOKS = { ...FULL, features: ['skills', 'hook_sets'] }
const HOSTS = [host({ host_id: 'host-a', name: 'alpha' }), host({ host_id: 'host-b', name: 'beta' })]
const EVENTS = {
  events: [
    { event: 'PreToolUse', claude: true, codex: true },
    { event: 'UserPromptSubmit', claude: true, codex: false },
  ],
}

afterEach(() => { history.replaceState(null, '', '/'); vi.restoreAllMocks() })

function open(routes: Record<string, Answer | Answer[]>, capabilities: unknown = WITH_HOOKS) {
  history.replaceState(null, '', '/skills')
  const server = stubServer({
    'GET /api/capabilities': json(200, capabilities),
    'GET /api/hats': json(200, [hat({ id: 'hat-1', name: 'Personal' })]),
    'GET /api/hosts': json(200, HOSTS),
    'GET /api/skills': json(200, []),
    'GET /api/hook-sets': json(200, [hookSet()]),
    'GET /api/hook-sets/events': json(200, EVENTS),
    'GET /api/hook-sets/h1': json(200, { item: hookSet(), hooks: {} }),
    'GET /api/hook-sets/h1/hosts': json(200, { hook_set_id: 'h1', hosts: [] }),
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

const openTab = async () => userEvent.click(await screen.findByRole('tab', { name: 'Hook sets' }))

async function select() {
  await openTab()
  await userEvent.click(await screen.findByRole('button', { name: 'guard' }))
  return screen.findByRole('region', { name: 'Hook set details' })
}

const JSON_TEXT = '{"PreToolUse":[{"hooks":[{"type":"command","command":"x"}]}],"UserPromptSubmit":[{"hooks":[{"type":"command","command":"y"}]}]}'

async function fillNew() {
  await openTab()
  await userEvent.click(await screen.findByRole('button', { name: 'New hook set' }))
  const form = await screen.findByRole('region', { name: 'New hook set' })
  await userEvent.type(within(form).getByLabelText('Name'), 'guard')
  const area = within(form).getByLabelText('Hooks (JSON)')
  await userEvent.clear(area)
  await userEvent.click(area)
  await userEvent.paste(JSON_TEXT)
  return form
}

it('shows the Hook sets tab only with the feature', async () => {
  const without = open({}, { ...FULL, features: ['skills'] })
  await screen.findByRole('heading', { name: 'Skills' })
  expect(screen.queryByRole('tab', { name: 'Hook sets' })).toBeNull()
  expect(sent(without, 'GET', '/api/hook-sets')).toHaveLength(0)
})

it('loads the hook sets when the tab opens', async () => {
  const server = open({})
  await openTab()
  expect(await screen.findByRole('listitem', { name: 'guard' })).toHaveTextContent('Hosts: 0')
  expect(sent(server, 'GET', '/api/hook-sets')).toHaveLength(1)
})

it('creates a hook set behind step-up and shows Codex support marks', async () => {
  const server = open({ 'POST /api/hook-sets': [STEP_UP, json(201, hookSet({ id: 'h2', name: 'new-set' }))] })
  const form = await fillNew()
  expect(within(form).getByRole('row', { name: /UserPromptSubmit/ })).toHaveTextContent('Claude only')
  expect(within(form).getByRole('row', { name: /PreToolUse/ })).toHaveTextContent('Claude and Codex')
  await userEvent.click(within(form).getByRole('button', { name: 'Create hook set' }))
  await stepUp()
  expect(await screen.findByRole('listitem', { name: 'new-set' })).toBeInTheDocument()
  const posts = sent(server, 'POST', '/api/hook-sets')
  expect(posts).toHaveLength(2)
  expect(posts[1].body).toEqual({ name: 'guard', hooks: JSON.parse(JSON_TEXT) })
})

it('shows the server reason for an invalid hook set', async () => {
  open({ 'POST /api/hook-sets': json(400, { code: 'invalid_hook_set', message: 'handler type must be command' }) })
  const form = await fillNew()
  await userEvent.click(within(form).getByRole('button', { name: 'Create hook set' }))
  expect(await screen.findByRole('alert')).toHaveTextContent('handler type must be command')
})

it('sends valid JSON of any shape and shows the server reason', async () => {
  const server = open({ 'POST /api/hook-sets': json(400, { code: 'invalid_hook_set', message: 'hooks must be an object' }) })
  await openTab()
  await userEvent.click(await screen.findByRole('button', { name: 'New hook set' }))
  const form = await screen.findByRole('region', { name: 'New hook set' })
  await userEvent.type(within(form).getByLabelText('Name'), 'guard')
  const area = within(form).getByLabelText('Hooks (JSON)')
  await userEvent.clear(area)
  await userEvent.click(area)
  await userEvent.paste('[]')
  await userEvent.click(within(form).getByRole('button', { name: 'Create hook set' }))
  expect(await screen.findByRole('alert')).toHaveTextContent('hooks must be an object')
  expect(sent(server, 'POST', '/api/hook-sets')[0].body).toEqual({ name: 'guard', hooks: [] })
})

it('replacing a hook set shows the saved JSON', async () => {
  const saved = { PreToolUse: [{ hooks: [{ type: 'command', command: 'new' }] }] }
  let replaced = false
  const server = open({
    'PUT /api/hook-sets/h1': () => { replaced = true; return json(200, hookSet({ content_hash: 'newhash0123456789' })) },
    'GET /api/hook-sets/h1': () =>
      replaced
        ? json(200, { item: hookSet({ content_hash: 'newhash0123456789' }), hooks: saved })
        : json(200, { item: hookSet(), hooks: {} }),
  })
  const detail = await select()
  const area = await within(detail).findByLabelText('Hooks (JSON)')
  await userEvent.clear(area)
  await userEvent.click(area)
  await userEvent.paste(JSON.stringify(saved))
  await userEvent.click(within(detail).getByRole('button', { name: 'Save hooks' }))
  await waitFor(() => expect(sent(server, 'PUT', '/api/hook-sets/h1')).toHaveLength(1))
  await waitFor(() => expect((within(detail).getByLabelText('Hooks (JSON)') as HTMLTextAreaElement).value).toBe(JSON.stringify(saved, null, 2)))
})

it('ticks hosts and saves the set', async () => {
  const server = open({
    'PUT /api/hook-sets/h1/hosts': [STEP_UP, json(200, { hook_set_id: 'h1', hosts: [{ host_id: 'host-a', status: 'assigned' }] })],
  })
  const detail = await select()
  await userEvent.click(await within(detail).findByRole('checkbox', { name: /alpha/ }))
  await stepUp()
  await waitFor(() => expect(within(detail).getByRole('checkbox', { name: /alpha/ })).toBeChecked())
  const puts = sent(server, 'PUT', '/api/hook-sets/h1/hosts')
  expect(puts.at(-1)?.body).toEqual({ host_ids: ['host-a'] })
  expect(detail).toHaveTextContent('assigned')
})

it('a_host_without_the_capability_is_marked', async () => {
  open({ 'GET /api/hook-sets/h1/hosts': json(200, { hook_set_id: 'h1', hosts: [
    { host_id: 'host-a', status: 'unsupported' },
    { host_id: 'host-b', status: 'offline' },
  ] }) })
  const detail = await select()
  expect(await within(detail).findByRole('checkbox', { name: /alpha.*host too old/ })).toBeInTheDocument()
  expect(within(detail).getByRole('checkbox', { name: /beta.*host offline/ })).toBeInTheDocument()
})
