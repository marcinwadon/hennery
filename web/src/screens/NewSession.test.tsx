// The New Session screen (frontend spec §7): hosts, agents, the project
// picker, the hat preview, Start then the first prompt, every refusal of a
// start, and the session it opens as an explicit selection.
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { Client } from '../api/client'
import { ClientContext } from '../app-client'
import { DESKTOP } from '../components/Shell'
import type { HatItem, HostItem, HostProjects } from '../generated/protocol'
import type { SessionSummary } from '../generated/view'
import type { HostAgentChoices } from '../lib/agents'
import { newSessionHref, readStartNotice } from '../lib/start'
import { navigate } from '../router'
import { json, stubServer, type Answer } from '../test-server'
import { liveStream } from '../test-stream'
import { completeLogout } from '../pwa/logout'
import NewSession, { RESOLVE_DEBOUNCE_MS } from './NewSession'

// The agents a test wants `agentsFor` to answer, or `null` for its own.
const agents = vi.hoisted(() => ({ current: null as HostAgentChoices | null }))
vi.mock('../lib/agents', async (importOriginal) => {
  const real = await importOriginal<typeof import('../lib/agents')>()
  return {
    ...real,
    agentsFor: (client: Client, hostId: string) =>
      agents.current ? Promise.resolve(agents.current) : real.agentsFor(client, hostId),
  }
})

function host(id: string, name: string, patch: Partial<HostItem> = {}): HostItem {
  return {
    mcp_source_policies: {},
    host_id: id,
    name,
    platform: 'linux',
    host_version: '0.1.0',
    capabilities: ['projects', 'resolve_path'],
    default_hat_id: 'hat-a',
    workspace_roots: ['/srv/work'],
    connected: true,
    mcp_delivery: { claude: 'isolated', codex: 'default_hat_only' },
    created_at: '2026-10-01T00:00:00Z',
    ...patch,
  }
}

// Server order: an offline host between two connected ones, and a revoked one.
const HOSTS = [
  host('h1', 'laptop'),
  host('h2', 'server', { connected: false }),
  host('h3', 'retired', { revoked_at: '2026-10-01T12:00:00Z' }),
  host('h4', 'desk'),
]

function hat(id: string, name: string): HatItem {
  return { id, name, colour: '#112233', created_at: '2026-10-01T00:00:00Z', default_for_new_hosts: false, purging: false, mcp_source_policy: 'legacy' as const, mcp_policy_revision: 0 }
}
const HATS = [hat('hat-a', 'Work'), hat('hat-b', 'Home')]

const PROJECTS: HostProjects = {
  recents_hat_id: 'hat-a',
  // Recents come first, so a recent prefix match (cat-tools) is held before
  // the exact repository (cat); the picker keeps recents ahead of it.
  recents: [
    { path: '/srv/work/data-cat', last_used_at: '2026-10-01T10:00:00Z' },
    { path: '/srv/work/cat-tools', last_used_at: '2026-09-30T10:00:00Z' },
  ],
  items: [
    { path: '/srv/work/concat' },
    { path: '/srv/work/chart' },
    { path: '/srv/work/catalog' },
    { path: '/srv/work/cat' },
    { path: '/srv/work/one/app' },
    { path: '/srv/work/two/app' },
  ],
  partial: false,
  home: '/srv/work',
}

const RESOLVED = { canonical: '/srv/work/cat', exists: true, is_dir: true, hat_id: 'hat-a' }

it.each(['start', 'shortcut', 'edit', 'outside', 'failure', 'blocked', 'again', 'shortcut-again', 'modifier', 'repeat', 'submit-enter', 'submit-space', 'submit-tab', 'submit-escape'])('a browse pending start: %s', async action => {
  let finish!: (response: Response) => void
  const path = '/srv/work/cat/child'
  const server = mount({
    'POST /api/hats/resolve': [json(200, RESOLVED), () => new Promise<Response>(resolve => { finish = resolve })],
    'GET /api/hosts/h1/browse?path=%2Fsrv%2Fwork%2Fcat': json(200, { path: RESOLVED.canonical, entries: [{ name: 'child' }], truncated: false }),
    'GET /api/hosts/h1/browse?path=%2Fsrv%2Fwork%2Fcat%2Fchild': json(200, { path, parent: RESOLVED.canonical, entries: [], truncated: false }),
    'GET /api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fcat%2Fchild': base()['GET /api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fcat'],
    ...(action === 'blocked' ? { 'GET /api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fcat': json(200, { agent: 'claude', adapter_revision: 'test', freshness: 'fresh', defaults: {}, security: { known: true, axes: {} }, config_options: [{ id: 'model', name: 'Model', type: 'select', currentValue: 'a', options: [{ value: 'a', name: 'A' }, { value: 'b', name: 'B' }] }] }) } : {}),
  }, '/new?host=h1&cwd=%2Fsrv%2Fwork%2Fcat')
  const user = userEvent.setup()
  await waitFor(() => expect(startButton()).toBeEnabled())
  if (action === 'blocked') {
    await user.click(screen.getByRole('button', { name: 'Session settings' }))
    await user.selectOptions(screen.getByRole('combobox', { name: 'Model' }), 'b')
    await user.click(screen.getByRole('button', { name: 'Close session settings' }))
  }
  await user.click(screen.getByRole('button', { name: 'Browse' }))
  const browse = screen.getByRole('group', { name: 'Browse directories' })
  await user.click(await within(browse).findByRole('button', { name: 'child' }))
  await within(browse).findByRole('button', { name: 'Use this directory' })
  if (action === 'shortcut') {
    await user.keyboard('{Control>}{Enter}{/Control}')
  } else {
    await user.click(startButton())
  }
  await waitFor(() => expect(finish).toBeDefined())
  expect(screen.getByText('Preparing session…', { exact: true })).toBeVisible()
  expect(server.sent.filter(s => s.path === '/api/sessions')).toHaveLength(0)
  expect(await projectField()).toHaveValue(path)
  if (action === 'start') {
    expect(startButton()).toHaveAttribute('aria-busy', 'true')
    expect(startButton()).toBeEnabled()
  }
  if (action === 'again') { fireEvent.pointerDown(startButton()); fireEvent.click(startButton()); fireEvent.submit(screen.getByRole('form', { name: 'New session' })) }
  if (action === 'shortcut-again') fireEvent.keyDown(await projectField(), { key: 'Enter', ctrlKey: true })
  if (action === 'modifier') for (const key of ['Control', 'Meta', 'Shift', 'Alt', 'AltGraph']) fireEvent.keyDown(await projectField(), { key })
  if (action === 'repeat') fireEvent.keyDown(await projectField(), { key: 'Enter', repeat: true })
  const submitKeys: Record<string, string> = { 'submit-enter': 'Enter', 'submit-space': ' ', 'submit-tab': 'Tab', 'submit-escape': 'Escape' }
  if (action in submitKeys) fireEvent.keyDown(startButton(), { key: submitKeys[action] })
  if (action === 'edit') await user.type(screen.getByRole('textbox', { name: /^First prompt/ }), 'Changed')
  if (action === 'outside') fireEvent.pointerDown(document.body)
  await act(async () => { finish(action === 'failure' ? json(503, { message: 'Resolution unavailable' }) : json(200, { ...RESOLVED, canonical: path })) })
  if (action === 'edit' || action === 'outside' || action === 'failure' || action === 'submit-tab' || action === 'submit-escape') {
    if (action === 'failure') await screen.findByRole('alert')
    else await waitFor(() => expect(startButton()).toBeEnabled())
    await waitFor(() => expect(screen.queryByText('Preparing session…', { exact: true })).toBeNull())
    expect(server.sent.filter(s => s.path === '/api/sessions')).toHaveLength(0)
    expect(screen.getByText('Start cancelled. Review your choices, then start again.', { exact: true })).toBeVisible()
    expect(screen.getByText('Start cancelled. Review your choices, then start again.', { exact: true })).not.toHaveAttribute('aria-live')
  } else {
    await waitFor(() => expect(server.sent.find(s => s.path === '/api/sessions')?.body).toMatchObject({ cwd: path }))
    expect(server.sent.filter(s => s.path === '/api/sessions')).toHaveLength(1)
  }
})

function base(): Record<string, Answer | Answer[]> {
  const config_options = [{ id: 'model', name: 'Model', type: 'select', currentValue: 'a', options: [{ value: 'a', name: 'A' }] }]
  const catalog = { agent: 'claude', adapter_revision: 'test', freshness: 'fresh', config_options, defaults: {}, security: { known: true, axes: {} } }
  const settings: Record<string, Answer> = {}
  for (const hostId of ['h1', 'h4']) for (const agent of ['claude', 'codex', 'my-agent']) settings[`GET /api/hosts/${hostId}/runtime-catalog?agent=${agent}&cwd=%2Fsrv%2Fwork%2Fcat`] = json(200, { ...catalog, agent })
  for (const agent of ['claude', 'codex', 'my-agent']) for (const hatId of ['hat-a','hat-b']) settings[`GET /api/hats/${hatId}/session-defaults?agent=${agent}`] = json(200, { hat_id: hatId, agent, revision: 0, config: {} })
  return {
    ...settings,
    'GET /api/view/sessions/s-1?limit=1': json(200, { items: [] }),
    'GET /api/sessions/s-1': json(200, { session_id: 's-1', hat_id: 'hat-a', lifecycle: 'active' }),
    'GET /api/sessions/s-1/catalog': json(200, { session_id: 's-1', config_options, commands: [] }),
    'GET /api/hosts': json(200, HOSTS),
    'GET /api/hats': json(200, HATS),
    'GET /api/hosts/h1/projects': json(200, PROJECTS),
    'GET /api/hosts/h4/projects': json(200, { ...PROJECTS, recents: [], items: [] }),
    'POST /api/hats/resolve': json(200, RESOLVED),
    'POST /api/sessions': json(202, { session_id: 's-1' }),
    'POST /api/sessions/s-1/prompt': json(202, { turn_id: 't-1' }),
  }
}

function mount(routes: Record<string, Answer | Answer[]> = {}, path = '/new') {
  history.replaceState(null, '', path)
  const server = stubServer({ ...base(), ...routes })
  const client = new Client({
    fetch: server.fetch,
    navigate: (to) => navigate(to),
    here: () => ({ pathname: location.pathname, search: location.search }),
    stepUp: async () => {},
  })
  const view = render(
    <ClientContext.Provider value={client}>
      <NewSession />
    </ClientContext.Provider>,
  )
  return { ...server, client, unmount: view.unmount }
}

it('keeps technical MCP predictions out of the launcher while retaining the resolved hat and agent choice', async () => {
  mount({ 'GET /api/hosts': json(200, [host('h1', 'laptop', {
    capabilities: ['projects', 'resolve_path', 'mcp_servers'],
    mcp_delivery: { claude: 'isolated', codex: 'default_hat_only' },
  })]) })
  const field = await screen.findByRole('combobox', { name: /^Project/ })
  await userEvent.type(field, '/srv/work/cat')
  await waitFor(() => expect(startButton()).toBeEnabled())
  expect(screen.queryByRole('region', { name: 'MCP prediction' })).toBeNull()
  await userEvent.click(screen.getByRole('radio', { name: 'Codex' }))
  expect(screen.getByRole('radio', { name: 'Codex' })).toBeChecked()
  expect(screen.queryByText(/not isolated per hat/)).toBeNull()
})

const hostButton = (name: string) => screen.getByRole('radio', { name: new RegExp(`^${name}`) })
const projectField = () => screen.findByRole('combobox', { name: /^Project/ })
const startButton = () => screen.getByRole('button', { name: /Start session/ })
const posts = (sent: { method: string; path: string }[]) => sent.filter((s) => s.method === 'POST').map((s) => s.path)

/** Host laptop, Claude, `path` typed as a path, `prompt` typed. */
async function fill(path = '/srv/work/cat', prompt = '') {
  const user = userEvent.setup()
  await screen.findByRole('radio', { name: 'Claude' })
  await waitFor(() => expect(screen.getByRole('radio', { name: 'Claude' })).toBeChecked())
  await user.type(await projectField(), path)
  if (prompt) await user.type(screen.getByLabelText(/^First prompt/), prompt)
  return user
}

beforeEach(() => {
  agents.current = null
  sessionStorage.clear()
  localStorage.clear()
})

afterEach(() => {
  vi.useRealTimers()
  history.replaceState(null, '', '/')
})

describe('hosts', () => {
  it('predicts source policy for the chosen ACP within the same resolved hat', async () => {
    mount({ 'GET /api/hats': json(200, [{ ...HATS[0], mcp_source_overrides: {
      claude: { policy: 'managed_only', revision: 2 }, codex: { policy: 'legacy', revision: 1 },
    } }]), 'GET /api/hosts': json(200, [host('h1', 'laptop', {
      mcp_source_policies: { claude: ['managed_only', 'managed_plus_native'] },
    })]) })
    const user = await fill()
    await waitFor(() => expect(screen.getByRole('region', { name: 'MCP sources prediction' })).toHaveTextContent('Only MCP configured in Hennery'))
    await user.click(screen.getByRole('radio', { name: 'Codex' }))
    expect(screen.getByRole('region', { name: 'MCP sources prediction' })).toHaveTextContent('Existing / adapter defaults')
    expect(screen.getByRole('region', { name: 'MCP sources prediction' })).not.toHaveTextContent('Starting will be refused')
    await user.click(screen.getByRole('radio', { name: 'Claude' }))
    expect(screen.getByRole('region', { name: 'MCP sources prediction' })).toHaveTextContent('Only MCP configured in Hennery')
  })
  it('lists connected hosts first and offline ones disabled, never a revoked one', async () => {
    mount()
    await screen.findByRole('radio', { name: /^laptop/ })
    const group = screen.getByRole('group', { name: /Host/ })
    const names = within(group)
      .getAllByRole('radio')
      .map((b) => b.closest('label')?.querySelector('.seg-name')?.textContent)
    expect(names).toEqual(['laptop', 'desk', 'server'])
    expect(hostButton('server')).toBeDisabled()
    expect(hostButton('server').closest('label')).toHaveTextContent('offline')
    expect(hostButton('laptop')).toBeEnabled()
    expect(screen.queryByRole('button', { name: /retired/ })).toBeNull()
  })

  it('picks the first connected host', async () => {
    mount({ 'GET /api/hosts': json(200, [host('h2', 'server', { connected: false }), host('h4', 'desk')]) })
    await waitFor(() => expect(hostButton('desk')).toBeChecked())
  })

  it('prefills a link’s text that is not a path as a search: no path until a project is picked', async () => {
    const server = mount({}, '/new?host=h1&cwd=cat')
    await waitFor(() => expect(hostButton('laptop')).toBeChecked())
    await waitFor(() => expect(screen.getByRole('radio', { name: 'Claude' })).toBeChecked())
    expect(await projectField()).toHaveValue('cat')
    expect(startButton()).toBeDisabled()
    await new Promise((r) => setTimeout(r, RESOLVE_DEBOUNCE_MS + 50))
    expect(server.sent.some((s) => s.path === '/api/hats/resolve')).toBe(false)
  })

  it('prefills the host and the path from a link', async () => {
    mount({}, '/new?host=h4&cwd=%2Fsrv%2Fwork%2Fapp')
    await waitFor(() => expect(hostButton('desk')).toBeChecked())
    expect(await projectField()).toHaveValue('/srv/work/app')
  })

  it('prefills from the link a failed session offers, whatever its path holds', async () => {
    mount({}, newSessionHref('h4', '/srv/work/a b&c=d#e?f+g%h'))
    await waitFor(() => expect(hostButton('desk')).toBeChecked())
    expect(await projectField()).toHaveValue('/srv/work/a b&c=d#e?f+g%h')
  })

  it('says why a prefilled offline host is not chosen, and chooses none', async () => {
    mount({}, '/new?host=h2&cwd=%2Fsrv%2Fwork%2Fapp')
    expect(await screen.findByText(/server is offline, so the session cannot start there/)).toBeInTheDocument()
    expect(hostButton('laptop')).not.toBeChecked()
    expect(screen.queryByRole('combobox', { name: /^Project/ })).toBeNull()
  })
})

describe('agents', () => {
  it('offers the fallback agents and Other, and starts one named by hand', async () => {
    const server = mount()
    const user = await fill()
    expect(screen.getByRole('radio', { name: 'Codex' })).toBeEnabled()
    await user.click(screen.getByRole('radio', { name: 'Other…' }))
    await user.type(screen.getByRole('textbox', { name: 'Agent name' }), 'my-agent')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find((s) => s.path === '/api/sessions')?.body).toEqual({
      host_id: 'h1',
      agent: 'my-agent',
      cwd: '/srv/work/cat',
    })
    // The fallback asks the server nothing.
    expect(server.sent.some((s) => s.path.endsWith('/agents'))).toBe(false)
  })

  it('disables an agent not signed in with its note, and lists no unavailable one', async () => {
    agents.current = {
      agents: [
        { agent: 'claude', available: true, auth: 'unknown', cli: 'bundled' },
        { agent: 'codex', available: true, auth: 'missing', cli: 'bundled', note: 'Run the login on the host.' },
        { agent: 'gemini', available: false, auth: 'ok', cli: 'given' },
      ],
      other: false,
    }
    mount()
    const codex = await screen.findByRole('radio', { name: /^Codex/ })
    expect(codex).toBeDisabled()
    expect(codex.closest('label')).toHaveTextContent('Run the login on the host.')
    expect(screen.getByRole('radio', { name: 'Claude' })).toBeEnabled()
    expect(screen.queryByRole('button', { name: /gemini/i })).toBeNull()
    expect(screen.queryByRole('radio', { name: 'Other…' })).toBeNull()
  })
})

describe('project picker', () => {
  it('keeps recents first, then ranks repository names by the typed match', async () => {
    mount()
    await screen.findByText(/Recent projects of the hat/)
    const user = userEvent.setup()
    await user.type(await projectField(), 'cat')
    const options = within(screen.getByRole('listbox'))
      .getAllByRole('option')
      .map((o) => o.querySelector('bdi')?.textContent)
    expect(options).toEqual(['cat-tools', 'data-cat', 'cat', 'catalog', 'concat', 'chart'])
  })

  it('labels colliding names as parent and name', async () => {
    mount()
    await screen.findByText(/Recent projects of the hat/)
    const user = userEvent.setup()
    await user.type(await projectField(), 'app')
    const options = within(screen.getByRole('listbox'))
      .getAllByRole('option')
      .map((o) => o.textContent)
    expect(options).toEqual(['one/app', 'two/app'])
  })

  it('names the hat the recents belong to and marks them', async () => {
    mount()
    expect(await screen.findByText(/Recent projects of the hat/)).toHaveTextContent('Recent projects of the hat Work')
    const user = userEvent.setup()
    await user.click(await projectField())
    expect(screen.getByRole('option', { name: /data-cat/ })).toHaveTextContent('recent')
  })

  it('asks for the recents of the hat the chosen path resolves to', async () => {
    const notes = { canonical: '/srv/notes', exists: true, is_dir: true, hat_id: 'hat-b' }
    const server = mount({
      'POST /api/hats/resolve': json(200, notes),
      'GET /api/hosts/h1/projects?path=%2Fsrv%2Fnotes': json(200, {
        ...PROJECTS,
        recents_hat_id: 'hat-b',
        recents: [{ path: '/srv/notes', last_used_at: '2026-10-01T10:00:00Z' }],
      }),
    })
    await fill('~/notes')
    await waitFor(() => expect(server.sent.map((s) => s.path)).toContain('/api/hosts/h1/projects?path=%2Fsrv%2Fnotes'))
    await userEvent.clear(await projectField())
    await waitFor(() => expect(screen.getByText(/Recent projects of the hat/)).toHaveTextContent('of the hat Home'), {
      timeout: 3000,
    })
    expect(server.sent.map((s) => s.path)).toContain('/api/hosts/h1/projects?path=%2Fsrv%2Fnotes')
  })

  it('says the host listed only some of its repositories', async () => {
    mount({ 'GET /api/hosts/h1/projects': json(200, { ...PROJECTS, partial: true }) })
    expect(await screen.findByText('The host listed only some of its repositories.', { exact: true })).toBeInTheDocument()
  })

  it('says the projects could not be listed, and a path can still be typed', async () => {
    const server = mount({ 'GET /api/hosts/h1/projects': json(409, { code: 'host_offline', message: 'the host is not connected' }) })
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toMatch(/^Projects could not be listed: .+ You can still type a path\.$/)
    const user = await fill('/srv/work/cat')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find((s) => s.path === '/api/sessions')?.body).toMatchObject({ cwd: '/srv/work/cat' })
  })

  it('tells a screen reader which option the arrow keys reach, and names the list only while it shows', async () => {
    mount()
    await screen.findByText(/Recent projects of the hat/)
    const field = await projectField()
    expect(field).toHaveAttribute('aria-controls', screen.getByRole('listbox').id)
    const user = userEvent.setup()
    await user.type(field, 'cat')
    const list = screen.getByRole('listbox')
    expect(field).toHaveAttribute('aria-controls', list.id)
    expect(field).not.toHaveAttribute('aria-activedescendant')
    const options = within(list).getAllByRole('option')
    await user.keyboard('{ArrowDown}')
    expect(document.getElementById(field.getAttribute('aria-activedescendant')!)).toBe(options[0])
    await user.keyboard('{ArrowDown}')
    expect(document.getElementById(field.getAttribute('aria-activedescendant')!)).toBe(options[1])
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('listbox')).toBeNull()
    expect(field).not.toHaveAttribute('aria-activedescendant')
    expect(field).not.toHaveAttribute('aria-controls')
  })

  it('says why a directory could not be browsed', async () => {
    mount({ 'GET /api/hosts/h1/browse?path=%2Fsrv%2Fwork': json(409, { code: 'host_offline', message: 'the host is not connected' }) })
    await screen.findByText(/Recent projects of the hat/)
    const user = userEvent.setup()
    await user.click(screen.getByRole('button', { name: 'Browse' }))
    const panel = await screen.findByRole('group', { name: 'Browse directories' })
    expect(await within(panel).findByRole('alert')).toHaveTextContent(/\S/)
    expect(within(panel).getByText('/srv/work', { exact: true })).toBeInTheDocument()
    expect(within(panel).queryByRole('button', { name: 'Use this directory' })).toBeNull()
  })

  it('says when not every directory is listed', async () => {
    mount({
      'GET /api/hosts/h1/browse?path=%2Fsrv%2Fwork': json(200, { path: '/srv/work', entries: [{ name: 'atlas' }], truncated: true }),
    })
    await screen.findByText(/Recent projects of the hat/)
    const user = userEvent.setup()
    await user.click(screen.getByRole('button', { name: 'Browse' }))
    const panel = await screen.findByRole('group', { name: 'Browse directories' })
    expect(await within(panel).findByText('Not every directory is listed.', { exact: true })).toBeInTheDocument()
  })

  it('takes text starting with a slash or a tilde as a path, not a search', async () => {
    const server = mount()
    const user = await fill('~/work/cat')
    expect(screen.queryByRole('listbox')).toBeNull()
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find((s) => s.path === '/api/sessions')?.body).toMatchObject({ cwd: '/srv/work/cat' })
  })

  it('a search is no path until a project is picked', async () => {
    mount()
    await screen.findByText(/Recent projects of the hat/)
    const user = userEvent.setup()
    await user.type(await projectField(), 'catal')
    expect(startButton()).toBeDisabled()
    await user.click(screen.getByRole('option', { name: /catalog/ }))
    expect(await projectField()).toHaveValue('catalog')
    await waitFor(() => expect(startButton()).toBeEnabled())
  })

  it('browses the host directories and uses the one chosen', async () => {
    const server = mount({
      'POST /api/hats/resolve': json(200, { ...RESOLVED, canonical: '/srv/work/atlas' }),
      'GET /api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fatlas': base()['GET /api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fcat'],
      'GET /api/hosts/h1/browse?path=%2Fsrv%2Fwork': json(200, {
        path: '/srv/work',
        parent: '/srv',
        entries: [{ name: 'atlas', git: true }],
        truncated: false,
      }),
      'GET /api/hosts/h1/browse?path=%2Fsrv%2Fwork%2Fatlas': json(200, {
        path: '/srv/work/atlas',
        parent: '/srv/work',
        entries: [],
        truncated: false,
      }),
    })
    await screen.findByText(/Recent projects of the hat/)
    const user = userEvent.setup()
    await user.click(screen.getByRole('button', { name: 'Browse' }))
    const panel = await screen.findByRole('group', { name: 'Browse directories' })
    await user.click(await within(panel).findByRole('button', { name: 'atlas' }))
    await within(panel).findByText('/srv/work/atlas')
    await user.click(within(panel).getByRole('button', { name: 'Use this directory' }))
    expect(screen.queryByRole('group', { name: 'Browse directories' })).toBeNull()
    expect(await projectField()).toHaveValue('/srv/work/atlas')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find((s) => s.path === '/api/sessions')?.body).toMatchObject({ cwd: '/srv/work/atlas' })
  })
})

describe('hat preview', () => {
  it('resolves the hat only once the path has rested', async () => {
    const server = mount()
    const field = await projectField()
    await waitFor(() => expect(screen.getByRole('radio', { name: 'Claude' })).toBeChecked())
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const resolves = () => server.sent.filter((s) => s.path === '/api/hats/resolve')
    for (const text of ['/s', '/srv/wo', '/srv/work/cat']) {
      fireEvent.change(field, { target: { value: text } })
      await act(async () => {
        vi.advanceTimersByTime(RESOLVE_DEBOUNCE_MS - 100)
      })
    }
    expect(resolves()).toHaveLength(0)
    await act(async () => {
      vi.advanceTimersByTime(99)
    })
    expect(resolves()).toHaveLength(0)
    await act(async () => {
      vi.advanceTimersByTime(1)
    })
    expect(resolves().map((s) => s.body)).toEqual([{ host_id: 'h1', path: '/srv/work/cat' }])
    vi.useRealTimers()
    expect(await screen.findByText(/^Starts in the hat/)).toHaveTextContent('Starts in the hat Work at /srv/work/cat')
  })

  it('says when the path is not a directory', async () => {
    mount({ 'POST /api/hats/resolve': json(200, { ...RESOLVED, canonical: '/srv/work/cat.txt', exists: true, is_dir: false }) })
    await fill('/srv/work/cat.txt')
    expect(await screen.findByText(/^Starts in the hat/, undefined, { timeout: 3000 })).toHaveTextContent(
      /^Starts in the hat Work at \/srv\/work\/cat\.txt: this is not a directory$/,
    )
  })

  it('shows a refused resolve as an alert under the preview', async () => {
    mount({ 'POST /api/hats/resolve': json(400, { code: 'invalid_cwd', message: '"/srv/work/x" is not a directory on that host' }) })
    await fill('/srv/work/x')
    const alert = await screen.findByText('"/srv/work/x" is not a directory on that host', { exact: true }, { timeout: 3000 })
    expect(alert.closest('[role="alert"]')).not.toBeNull()
  })

  it('says when the directory does not exist yet', async () => {
    mount({ 'POST /api/hats/resolve': json(200, { ...RESOLVED, canonical: '/srv/work/new', exists: false, is_dir: false }) })
    await fill('/srv/work/new')
    expect(await screen.findByText(/^Starts in the hat/, undefined, { timeout: 3000 })).toHaveTextContent(
      'this directory does not exist yet',
    )
  })
})

describe('start', () => {
  it('starts the session, then sends the first prompt, then opens the session', async () => {
    const server = mount()
    const user = await fill('/srv/work/cat', 'Fix the tests')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(location.search).toBe('')
    expect(posts(server.sent).filter((p) => p !== '/api/hats/resolve')).toEqual([
      '/api/sessions',
      '/api/sessions/s-1/prompt',
    ])
    expect(server.sent.find((s) => s.path === '/api/sessions/s-1/prompt')?.body).toEqual({
      content: [{ type: 'text', text: 'Fix the tests' }],
    })
  })

  it('sends no prompt when the first prompt is empty', async () => {
    const server = mount()
    const user = await fill('/srv/work/cat', '   ')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
  })

  it.each([
    ['hat_ambiguous', 409, 'the rule for "/srv/Work" matches only in another case', /more than one hat.*the rule for/],
    ['host_offline', 409, 'the host has not connected since it was paired', /^the host has not connected since it was paired$/],
    ['invalid_cwd', 400, '"/srv/work/cat" is not a directory on that host', /^"\/srv\/work\/cat" is not a directory on that host$/],
    ['unknown_host', 400, 'no host is paired with that id', /^That host is no longer paired/],
    ['start_failed', 502, 'the adapter exited', /^The host refused to start the session \(start_failed\): the adapter exited$/],
  ])('says why a start was refused: %s', async (code, status, message, shown) => {
    const server = mount({ 'POST /api/sessions': json(status, { code, message }) })
    const user = await fill('/srv/work/cat', 'Fix the tests')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    const alert = await screen.findByText(shown)
    expect(alert.closest('[role="alert"]')).not.toBeNull()
    expect(location.pathname).toBe('/new')
    expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
    // The form stays, ready for another try.
    expect(startButton()).toBeEnabled()
    expect(screen.getByLabelText(/^First prompt/)).toHaveValue('Fix the tests')
  })

  it('a start whose delivery is unknown opens its session with a notice and keeps the prompt', async () => {
    const server = mount({
      'POST /api/sessions': json(503, {
        code: 'delivery_unknown',
        message: 'host disconnected; delivery unknown',
        session_id: 's-9',
      }),
    })
    const user = await fill('/srv/work/cat', 'Fix the tests')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-9'))
    expect(location.search).toBe('?notice=start_unknown_draft')
    expect(sessionStorage.getItem('hennery.draft.s-9')).toBe('Fix the tests')
    expect(posts(server.sent).some((p) => p.endsWith('/prompt'))).toBe(false)
  })

  it('a start whose delivery is unknown, with no first prompt, says nothing of a draft', async () => {
    mount({ 'POST /api/sessions': json(503, { code: 'delivery_unknown', message: 'gone', session_id: 's-9' }) })
    const user = await fill('/srv/work/cat')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-9'))
    expect(location.search).toBe('?notice=start_unknown')
    expect(sessionStorage.getItem('hennery.draft.s-9')).toBeNull()
  })

  it('a delivery unknown that names no session stays on the form', async () => {
    mount({ 'POST /api/sessions': json(503, { code: 'delivery_unknown', message: 'gone' }) })
    const user = await fill('/srv/work/cat', 'Fix the tests')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    expect(await screen.findByText(/its outcome is not known/)).toBeInTheDocument()
    expect(location.pathname).toBe('/new')
  })

  it('a refused first prompt opens the session with a notice and keeps the prompt as its draft', async () => {
    mount({ 'POST /api/sessions/s-1/prompt': json(409, { code: 'host_offline', message: 'the host is not connected' }) })
    const user = await fill('/srv/work/cat', 'Fix the tests')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(location.search).toBe('?notice=prompt_failed&code=host_offline')
    expect(sessionStorage.getItem('hennery.draft.s-1')).toBe('Fix the tests')
  })
})

describe('changing the host', () => {
  it('resets the agent, the path and the browser', async () => {
    mount({
      'GET /api/hosts/h1/browse?path=%2Fsrv%2Fwork%2Fcat': json(200, {
        path: '/srv/work/cat',
        entries: [],
        truncated: false,
      }),
    })
    const user = await fill('/srv/work/cat')
    await user.click(screen.getByRole('radio', { name: 'Codex' }))
    await user.click(screen.getByRole('button', { name: 'Browse' }))
    await screen.findByRole('group', { name: 'Browse directories' })
    await screen.findByText(/^Starts in the hat/, undefined, { timeout: 3000 })

    await user.click(hostButton('desk'))
    await waitFor(() => expect(screen.getByRole('radio', { name: 'Claude' })).toBeChecked())
    expect(screen.getByRole('radio', { name: 'Codex' })).not.toBeChecked()
    expect(await projectField()).toHaveValue('')
    expect(screen.queryByRole('group', { name: 'Browse directories' })).toBeNull()
    expect(screen.queryByText(/^Starts in the hat/)).toBeNull()
    expect(startButton()).toBeDisabled()
  })
})

describe('in the app', () => {
  function summary(id: string, hat_id: string): SessionSummary {
    return {
      session_id: id,
      host_id: 'h1',
      agent: 'claude',
      cwd: `/srv/work/${id}`,
      hat_id,
      lifecycle: 'active',
      activity: 'idle',
      presumed_parked: false,
      created_at: '2026-10-01T00:00:00.000Z',
      last_event_at: '2026-10-02T10:00:00.000Z',
      question_waits: false,
        operator_attention: 0,
        parent_handled_questions: 0,
    }
  }

  it('opens the started session as an explicit selection, even outside the hat', async () => {
    window.matchMedia = ((query: string) => ({
      matches: query === DESKTOP,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia
    localStorage.setItem('hennery.hat', 'hat-a')
    history.replaceState(null, '', '/new')
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), 'http://h')
      const method = init?.method ?? 'GET'
      switch (`${method} ${url.pathname}`) {
        case 'GET /api/capabilities':
          return json(200, { mode: 'full', features: [] })
        case 'GET /api/hats':
          return json(200, HATS)
        case 'GET /api/hosts':
          return json(200, HOSTS)
        case 'GET /api/hosts/h1/projects':
          return json(200, PROJECTS)
        case 'GET /api/hosts/h1/runtime-catalog':
          return base()['GET /api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fcat'] as Response
        case 'GET /api/hats/hat-b/session-defaults':
          return json(200, { hat_id: 'hat-b', agent: 'claude', revision: 0, config: {} })
        case 'POST /api/hats/resolve':
          return json(200, { ...RESOLVED, hat_id: 'hat-b' })
        case 'POST /api/sessions':
          return json(202, { session_id: 's-new' })
        case 'GET /api/view/sessions':
          return json(200, {
            sessions: [summary('old', 'hat-a'), summary('s-new', 'hat-b')],
            epoch: 'e1',
            revision: 1,
          })
        case 'GET /api/stream/sessions':
          return liveStream().response
        default:
          return json(404, { code: 'not_found', message: url.pathname })
      }
    })
    render(<App fetchImpl={fetch as unknown as typeof globalThis.fetch} />)
    expect(await screen.findByRole('heading', { name: 'New session' })).toBeInTheDocument()
    const user = await fill('/srv/work/cat')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-new'))
    // The list stream opened, and its known rows did not replace the explicit route.
    await waitFor(() => expect(fetch.mock.calls.filter(([input]) => new URL(String(input), 'http://h').pathname === '/api/stream/sessions')).toHaveLength(1))
    await new Promise((r) => setTimeout(r, 50))
    expect(location.pathname).toBe('/sessions/s-new')
    // @ts-expect-error jsdom has none; each test sets its own
    delete window.matchMedia
  })
})

describe('reviewed configuration', () => {
  const options = [
    { id: 'model', name: 'Model', type: 'select', currentValue: 'opus', options: [{ value: 'opus', name: 'Opus' }, { value: 'sonnet', name: 'Sonnet' }] },
    { id: 'mode', name: 'Permission', type: 'select', currentValue: 'default', options: [{ value: 'default', name: 'Default' }, { value: 'bypass', name: 'Bypass' }] },
    { id: 'fast', name: 'Fast', type: 'boolean', currentValue: false },
  ]
  const runtime = '/api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fcat'
  const catalog = { agent: 'claude', adapter_revision: 'pin-1', freshness: 'fresh', config_options: options, defaults: { model: 'opus', mode: 'default', axes: { fast: false } }, security: { known: true, axes: { mode: ['default'] } } }
  it('offers a one-click retry for an unconfirmed nonsecurity override without a preview, retaining the prompt and security requirements', async () => {
    const actual = { session_id: 's-1', commands: [], model: 'sonnet', mode: 'default', axes: { fast: true, sandbox: true }, config_options: [...options.map(option => ({ ...option, currentValue: option.id === 'model' ? 'sonnet' : option.id === 'fast' ? true : 'default' })), { id: 'sandbox', name: 'Sandbox', type: 'boolean', currentValue: true }] }
    const server = mount({
      [`GET ${runtime}`]: [json(409, { code: 'catalog_unavailable', message: 'No preview' }), json(200, { ...catalog, security: { known: true, axes: { mode: ['default'], sandbox: [true] } } })],
      'GET /api/hats/hat-a/session-defaults?agent=claude': json(200, { hat_id: 'hat-a', agent: 'claude', revision: 1, config: { model: 'sonnet', mode: 'default', axes: { fast: false, sandbox: true } } }),
      'POST /api/sessions': [json(202, { session_id: 's-1' }), json(202, { session_id: 's-2' })],
      'GET /api/sessions/s-1/catalog': json(200, actual),
      'GET /api/sessions/s-2': json(200, { hat_id: 'hat-a', lifecycle: 'active' }),
      'GET /api/sessions/s-2/catalog': json(200, { ...actual, session_id: 's-2' }),
      'GET /api/view/sessions/s-2?limit=1': json(200, { items: [] }),
      'POST /api/sessions/s-2/prompt': json(202, { turn_id: 't-2' }),
    })
    const user = await fill('/srv/work/cat', 'Keep this prompt')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    expect(await screen.findByRole('alert')).toHaveTextContent('Fast: Off')
    const retry = await screen.findByRole('button', { name: 'Start with agent default for Fast' })
    expect(screen.queryByRole('button', { name: 'Start with agent default for Permission' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Start with agent default for Sandbox' })).toBeNull()
    expect(server.sent.filter(s => s.path === '/api/sessions' && s.method === 'POST')).toHaveLength(1)
    expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
    expect(sessionStorage.getItem('hennery.draft.s-1')).toBe('Keep this prompt')
    expect(screen.getByRole('link', { name: 'Review started session' })).toHaveAttribute('href', '/sessions/s-1')
    await user.click(retry)
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-2'))
    expect(server.sent.filter(s => s.path === '/api/sessions' && s.method === 'POST').map(s => s.body)).toEqual([
      { host_id: 'h1', agent: 'claude', cwd: '/srv/work/cat', model: 'sonnet', mode: 'default', axes: { fast: false, sandbox: true } },
      { host_id: 'h1', agent: 'claude', cwd: '/srv/work/cat', model: 'sonnet', mode: 'default', axes: { sandbox: true } },
    ])
    expect(posts(server.sent)).toContain('/api/sessions/s-2/prompt')
    expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
  })
  it('does not offer default retries for unconfirmed profile security overrides without a preview', async () => {
    const server = mount({
      [`GET ${runtime}`]: [json(409, { code: 'catalog_unavailable', message: 'No preview' }), json(200, { ...catalog, security: { known: false, axes: { mode: ['safe'], sandbox: [true] } } })],
      'GET /api/hats/hat-a/session-defaults?agent=claude': json(200, { hat_id: 'hat-a', agent: 'claude', revision: 1, config: { mode: 'safe', axes: { sandbox: true } } }),
    })
    const user = await fill('/srv/work/cat', 'Keep this prompt')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find(s => s.method === 'POST' && s.path === '/api/sessions')?.body).toMatchObject({ mode: 'safe', axes: { sandbox: true } })
    expect(screen.queryByRole('button', { name: /^Start with agent default/ })).toBeNull()
    expect(location.search).toContain('config_unconfirmed')
    expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
  })
  it.each(['unknown', 'unavailable'])('offers a nonsecurity retry even when profile metadata is %s', async metadata => {
    const server = mount({
      [`GET ${runtime}`]: [json(409, { code: 'catalog_unavailable', message: 'No preview' }), metadata === 'unknown' ? json(200, { ...catalog, security: { known: false, axes: {} } }) : json(409, { code: 'catalog_unavailable', message: 'No profile' })],
      'GET /api/hats/hat-a/session-defaults?agent=claude': json(200, { hat_id: 'hat-a', agent: 'claude', revision: 1, config: { axes: { fast: false } } }),
    })
    const user = await fill('/srv/work/cat', 'Keep this prompt')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    expect(await screen.findByRole('button', { name: 'Start with agent default for fast' })).toBeEnabled()
    expect(posts(server.sent).filter(path => path === '/api/sessions')).toHaveLength(1)
    expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
  })
  it('can retry a model with agent default even when post-start profile metadata is unavailable', async () => {
    const server = mount({
      [`GET ${runtime}`]: json(409, { code: 'catalog_unavailable', message: 'No preview' }),
      'GET /api/hats/hat-a/session-defaults?agent=claude': json(200, { hat_id: 'hat-a', agent: 'claude', revision: 1, config: { model: 'removed' } }),
    })
    const user = await fill('/srv/work/cat', 'Keep this prompt')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    expect(await screen.findByRole('button', { name: 'Start with agent default for Model' })).toBeEnabled()
    expect(posts(server.sent).filter(path => path === '/api/sessions')).toHaveLength(1)
    expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
  })
  it('applies a saved Sonnet model, prunes inherited fast and confirms actual settings before the first prompt', async () => {
    const server = mount({
      [`GET ${runtime}`]: json(200, catalog),
      'GET /api/hats/hat-a/session-defaults?agent=claude': json(200, { hat_id: 'hat-a', agent: 'claude', revision: 1, config: { model: 'sonnet' } }),
      'GET /api/sessions/s-1/catalog': json(200, { session_id: 's-1', config_options: options.slice(0, 2).map(option => ({ ...option, currentValue: option.id === 'model' ? 'sonnet' : 'default' })), commands: [], model: 'sonnet', mode: 'default' }),
    })
    const user = await fill('/srv/work/cat', 'Keep this prompt')
    await waitFor(() => expect(startButton()).toBeEnabled())
    expect(screen.getByRole('button', { name: 'Session settings' })).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByRole('combobox', { name: 'Model' })).toBeNull()
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find(s => s.path === '/api/sessions')?.body).toEqual({ host_id: 'h1', agent: 'claude', cwd: '/srv/work/cat', model: 'sonnet', mode: 'default' })
    const read = server.sent.findIndex(s => s.path === '/api/sessions/s-1/catalog')
    const prompt = server.sent.findIndex(s => s.path === '/api/sessions/s-1/prompt')
    expect(prompt).toBeGreaterThan(read)
  })
  it('retains unsupported deliberate fast=false and holds the prompt when read-back omits it', async () => {
    const server = mount({
      [`GET ${runtime}`]: json(200, catalog),
      'GET /api/hats/hat-a/session-defaults?agent=claude': json(200, { hat_id: 'hat-a', agent: 'claude', revision: 1, config: { model: 'sonnet', axes: { fast: false } } }),
      'GET /api/sessions/s-1/catalog': json(200, { session_id: 's-1', config_options: options.slice(0, 2).map(option => ({ ...option, currentValue: option.id === 'model' ? 'sonnet' : 'default' })), commands: [], model: 'sonnet', mode: 'default' }),
    })
    const user = await fill('/srv/work/cat', 'Keep this prompt')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find(s => s.path === '/api/sessions')?.body).toMatchObject({ axes: { fast: false } })
    expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
    expect(location.search).toContain('config_unconfirmed')
    expect(sessionStorage.getItem('hennery.draft.s-1')).toBe('Keep this prompt')
    expect(readStartNotice(location.search, 's-1')).toMatchObject({ message: expect.stringContaining('Fast: Off') })
  })
  it('starts with agent defaults when no catalog is available and sends no unverified config', async () => {
    const server = mount({ [`GET ${runtime}`]: json(409, { code: 'catalog_unavailable', message: 'Offline catalog' }) })
    const user = await fill('/srv/work/cat', 'Start deliberately')
    expect(await screen.findByLabelText('Settings need attention')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Session settings' })).toHaveAttribute('aria-expanded', 'false')
    await user.click(await screen.findByRole('button', { name: 'Session settings' }))
    expect(startButton()).toBeEnabled()
    await user.click(screen.getByRole('button', { name: 'Close session settings' }))
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find(s => s.path === '/api/sessions')?.body).toEqual({ host_id: 'h1', agent: 'claude', cwd: '/srv/work/cat' })
    expect(posts(server.sent)).toContain('/api/sessions/s-1/prompt')
    expect(server.sent.some(sent => sent.path === '/api/sessions/s-1/catalog')).toBe(false)
  })
})

it('keeps the first prompt as a session draft when the launcher closes during Start', async () => {
  let finish!: (response: Response) => void
  const start = new Promise<Response>(resolve => { finish = resolve })
  const server = mount({ 'POST /api/sessions': () => start })
  const user = await fill('/srv/work/cat', 'Keep after closing')
  await waitFor(() => expect(startButton()).toBeEnabled())
  await user.click(startButton())
  await waitFor(() => expect(posts(server.sent)).toContain('/api/sessions'))
  server.unmount()
  await act(async () => { finish(json(202, { session_id: 's-1' })); await start })
  expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
  expect(server.sent.some(sent => sent.path === '/api/sessions/s-1')).toBe(false)
  expect(sessionStorage.getItem('hennery.draft.s-1')).toBe('Keep after closing')
})
it('does not release a held first prompt after closing during configuration read-back', async () => {
  let finish!: (response: Response) => void
  const catalogRead = new Promise<Response>(resolve => { finish = resolve })
  const server = mount({ 'GET /api/sessions/s-1/catalog': () => catalogRead })
  const user = await fill('/srv/work/cat', 'Keep during review')
  await waitFor(() => expect(startButton()).toBeEnabled())
  await user.click(startButton())
  await waitFor(() => expect(server.sent.some(sent => sent.path === '/api/sessions/s-1/catalog')).toBe(true))
  server.unmount()
  await act(async () => { finish(json(200, { session_id: 's-1', commands: [], config_options: [{ id: 'model', type: 'select', currentValue: 'a', options: [{ value: 'a' }] }] })); await catalogRead })
  expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
  expect(sessionStorage.getItem('hennery.draft.s-1')).toBe('Keep during review')
})
it('retains the prompt on a hat mismatch when starting without a saved catalog', async () => {
  const server = mount({
    'GET /api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fcat': json(409, { code: 'catalog_unavailable', message: 'No settings preview' }),
    'GET /api/sessions/s-1': json(200, { session_id: 's-1', hat_id: 'hat-b', lifecycle: 'active' }),
  })
  const user = await fill('/srv/work/cat', 'Check the hat first')
  await user.click(await screen.findByRole('button', { name: 'Session settings' }))
  await user.click(screen.getByRole('button', { name: 'Close session settings' }))
  await user.click(startButton())
  await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
  expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
  expect(location.search).toContain('hat_changed')
  expect(sessionStorage.getItem('hennery.draft.s-1')).toBe('Check the hat first')
})

it('starts immediately with no picks while the saved catalog request is still pending', async () => {
  let finish!: (response: Response) => void
  const pending = new Promise<Response>(resolve => { finish = resolve })
  const server = mount({
    'GET /api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fcat': () => pending,
  })
  const user = await fill('/srv/work/cat', 'Start without waiting')
  await waitFor(() => expect(startButton()).toBeEnabled())
  await user.click(startButton())
  await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
  expect(server.sent.find(s => s.path === '/api/sessions')?.body).toEqual({ host_id: 'h1', agent: 'claude', cwd: '/srv/work/cat' })
  expect(server.sent.filter(s => s.path.includes('runtime-catalog')).every(s => s.method === 'GET')).toBe(true)
  expect(posts(server.sent)).toContain('/api/sessions/s-1/prompt')
  await act(async () => { finish(json(200, {})); await pending })
})
it.each(['unknown', 'pending'] as const)('sends the hat permission override on a first start with a %s preview', async state => {
  const server = mount({
    'GET /api/hosts/h1/runtime-catalog?agent=claude&cwd=%2Fsrv%2Fwork%2Fcat': state === 'pending' ? () => new Promise<Response>(() => {}) : json(200, { freshness: 'unknown' }),
    'GET /api/hats/hat-a/session-defaults?agent=claude': json(200, { hat_id: 'hat-a', agent: 'claude', revision: 1, config: { mode: 'default' } }),
    'GET /api/sessions/s-1/catalog': json(200, { session_id: 's-1', config_options: [{ id: 'mode', type: 'select', currentValue: 'default', options: [{ value: 'default' }] }], commands: [], mode: 'default' }),
  })
  const user = await fill('/srv/work/cat', 'Keep the permission requirement')
  await waitFor(() => expect(startButton()).toBeEnabled())
  await user.click(startButton())
  await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
  expect(server.sent.find(s => s.path === '/api/sessions')?.body).toEqual({ host_id: 'h1', agent: 'claude', cwd: '/srv/work/cat', mode: 'default' })
  expect(posts(server.sent)).toContain('/api/sessions/s-1/prompt')
  expect(server.sent.some(s => s.path === '/api/sessions/s-1/catalog')).toBe(true)
})
it('starts only on a deliberate local Mod+Enter, ignoring repeat, IME and extra modifiers', async () => {
  const server = mount()
  await fill('/srv/work/cat', 'Keyboard launch')
  await waitFor(() => expect(startButton()).toBeEnabled())
  const prompt = screen.getByRole('textbox', { name: 'First prompt optional' })
  for (const ignored of [{ altKey: true }, { shiftKey: true }, { repeat: true }, { isComposing: true }]) {
    fireEvent.keyDown(prompt, { key: 'Enter', ctrlKey: true, ...ignored })
  }
  const prevented = new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true, cancelable: true })
  prevented.preventDefault()
  fireEvent(prompt, prevented)
  expect(posts(server.sent)).not.toContain('/api/sessions')
  fireEvent.keyDown(prompt, { key: 'Enter', ctrlKey: true })
  await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
  expect(posts(server.sent).filter(path => path === '/api/sessions')).toHaveLength(1)
})

describe('pending launch persistence ownership', () => {
  type Phase = 'start' | 'confirmation' | 'ambiguous' | 'prompt-failure'
  async function pendingLaunch(phase: Phase) {
    let finish!: (response: Response) => void
    const pending = new Promise<Response>(resolve => { finish = resolve })
    const path = phase === 'confirmation' ? 'GET /api/sessions/s-1' : phase === 'prompt-failure' ? 'POST /api/sessions/s-1/prompt' : 'POST /api/sessions'
    const view = mount({ [path]: () => pending })
    const user = await fill('/srv/work/cat', 'Synthetic private prompt')
    await waitFor(() => expect(startButton()).toBeEnabled())
    await user.click(startButton())
    await waitFor(() => expect(view.sent.some(sent => `${sent.method} ${sent.path}` === path)).toBe(true))
    const answer = phase === 'confirmation' ? json(200, { session_id: 's-1', hat_id: 'hat-a', lifecycle: 'active' })
      : phase === 'ambiguous' ? json(502, { code: 'delivery_unknown', session_id: 's-1' })
      : phase === 'prompt-failure' ? json(409, { code: 'host_offline' }) : json(202, { session_id: 's-1' })
    return { ...view, settle: async () => { finish(answer); await pending } }
  }
  it.each(['start', 'confirmation', 'ambiguous', 'prompt-failure'] as const)('never restores text or navigates after logout during %s', async phase => {
    const view = await pendingLaunch(phase)
    await act(async () => { await completeLogout(view.client, false) })
    view.unmount()
    expect(sessionStorage.getItem('hennery.draft.__launcher__')).toBeNull()
    await act(view.settle)
    expect(Object.keys(sessionStorage).filter(key => key.startsWith('hennery.draft.'))).toEqual([])
    expect(location.pathname).toBe('/login')
    if (phase !== 'prompt-failure') expect(posts(view.sent)).not.toContain('/api/sessions/s-1/prompt')
  })
  it.each(['start', 'confirmation', 'ambiguous', 'prompt-failure'] as const)('retains a newer real launcher after the closed %s completes', async phase => {
    const view = await pendingLaunch(phase)
    view.unmount()
    mount()
    // Capture the new draft after its own initial agent selection settles.
    // The old launch must not alter these exact bytes once it finishes.
    await waitFor(() => expect(screen.getByRole('radio', { name: 'Claude' })).toBeChecked())
    fireEvent.change(await screen.findByRole('textbox', { name: 'First prompt optional' }), { target: { value: 'A newer unsent prompt' } })
    await waitFor(() => expect(sessionStorage.getItem('hennery.draft.__launcher__')).toContain('A newer unsent prompt'))
    const newer = sessionStorage.getItem('hennery.draft.__launcher__')
    await act(view.settle)
    expect(sessionStorage.getItem('hennery.draft.__launcher__')).toBe(newer)
    expect(sessionStorage.getItem('hennery.draft.s-1')).toBe('Synthetic private prompt')
    expect(location.pathname).toBe('/new')
  })
})
