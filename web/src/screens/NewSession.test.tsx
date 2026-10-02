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
import { navigate } from '../router'
import { json, stubServer, type Answer } from '../test-server'
import { liveStream } from '../test-stream'
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
    host_id: id,
    name,
    platform: 'linux',
    host_version: '0.1.0',
    capabilities: ['projects', 'resolve_path'],
    default_hat_id: 'hat-a',
    workspace_roots: ['/srv/work'],
    connected: true,
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
  return { id, name, colour: '#112233', created_at: '2026-10-01T00:00:00Z', default_for_new_hosts: false, purging: false }
}
const HATS = [hat('hat-a', 'Work'), hat('hat-b', 'Home')]

const PROJECTS: HostProjects = {
  recents_hat_id: 'hat-a',
  // Recents come first, so a recent prefix match (cat-tools) is held before
  // the exact one (cat): only the ranking puts `cat` first.
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

function base(): Record<string, Answer | Answer[]> {
  return {
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
  render(
    <ClientContext.Provider value={client}>
      <NewSession />
    </ClientContext.Provider>,
  )
  return server
}

const hostButton = (name: string) => screen.getByRole('button', { name: new RegExp(`^${name}`) })
const projectField = () => screen.findByRole('combobox', { name: /^Project/ })
const startButton = () => screen.getByRole('button', { name: /Start session/ })
const posts = (sent: { method: string; path: string }[]) => sent.filter((s) => s.method === 'POST').map((s) => s.path)

/** Host laptop, Claude, `path` typed as a path, `prompt` typed. */
async function fill(path = '/srv/work/cat', prompt = '') {
  const user = userEvent.setup()
  await screen.findByRole('button', { name: 'Claude' })
  await waitFor(() => expect(screen.getByRole('button', { name: 'Claude' })).toHaveAttribute('aria-pressed', 'true'))
  await user.type(await projectField(), path)
  if (prompt) await user.type(screen.getByLabelText(/First prompt/), prompt)
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
  it('lists connected hosts first and offline ones disabled, never a revoked one', async () => {
    mount()
    await screen.findByRole('button', { name: /^laptop/ })
    const group = screen.getByRole('group', { name: /Host/ })
    const names = within(group)
      .getAllByRole('button')
      .map((b) => b.querySelector('.seg-name')?.textContent)
    expect(names).toEqual(['laptop', 'desk', 'server'])
    expect(hostButton('server')).toBeDisabled()
    expect(hostButton('server')).toHaveTextContent('offline')
    expect(hostButton('laptop')).toBeEnabled()
    expect(screen.queryByRole('button', { name: /retired/ })).toBeNull()
  })

  it('picks the first connected host', async () => {
    mount({ 'GET /api/hosts': json(200, [host('h2', 'server', { connected: false }), host('h4', 'desk')]) })
    await waitFor(() => expect(hostButton('desk')).toHaveAttribute('aria-pressed', 'true'))
  })

  it('prefills the host and the path from a link', async () => {
    mount({}, '/new?host=h4&cwd=%2Fsrv%2Fwork%2Fapp')
    await waitFor(() => expect(hostButton('desk')).toHaveAttribute('aria-pressed', 'true'))
    expect(await projectField()).toHaveValue('/srv/work/app')
  })

  it('says why a prefilled offline host is not chosen, and chooses none', async () => {
    mount({}, '/new?host=h2&cwd=%2Fsrv%2Fwork%2Fapp')
    expect(await screen.findByText(/server is offline, so the session cannot start there/)).toBeInTheDocument()
    expect(hostButton('laptop')).toHaveAttribute('aria-pressed', 'false')
    expect(screen.queryByRole('combobox', { name: /^Project/ })).toBeNull()
  })
})

describe('agents', () => {
  it('offers the fallback agents and Other, and starts one named by hand', async () => {
    const server = mount()
    const user = await fill()
    expect(screen.getByRole('button', { name: 'Codex' })).toBeEnabled()
    await user.click(screen.getByRole('button', { name: 'Other…' }))
    await user.type(screen.getByRole('textbox', { name: 'Agent name' }), 'my-agent')
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
    const codex = await screen.findByRole('button', { name: /^Codex/ })
    expect(codex).toBeDisabled()
    expect(codex).toHaveTextContent('Run the login on the host.')
    expect(screen.getByRole('button', { name: 'Claude' })).toBeEnabled()
    expect(screen.queryByRole('button', { name: /gemini/i })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Other…' })).toBeNull()
  })
})

describe('project picker', () => {
  it('ranks exact, prefix, substring, subsequence, recents first within a rank', async () => {
    mount()
    await screen.findByText(/Recent projects of the hat/)
    const user = userEvent.setup()
    await user.type(await projectField(), 'cat')
    const options = within(screen.getByRole('listbox'))
      .getAllByRole('option')
      .map((o) => o.querySelector('bdi')?.textContent)
    expect(options).toEqual(['cat', 'cat-tools', 'catalog', 'data-cat', 'concat', 'chart'])
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
    await waitFor(() => expect(screen.getByText(/Recent projects of the hat/)).toHaveTextContent('of the hat Home'), {
      timeout: 3000,
    })
    expect(server.sent.map((s) => s.path)).toContain('/api/hosts/h1/projects?path=%2Fsrv%2Fnotes')
  })

  it('takes text starting with a slash or a tilde as a path, not a search', async () => {
    const server = mount()
    const user = await fill('~/work/cat')
    expect(screen.queryByRole('listbox')).toBeNull()
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find((s) => s.path === '/api/sessions')?.body).toMatchObject({ cwd: '~/work/cat' })
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
    expect(await projectField()).toHaveValue('atlas')
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-1'))
    expect(server.sent.find((s) => s.path === '/api/sessions')?.body).toMatchObject({ cwd: '/srv/work/atlas' })
  })
})

describe('hat preview', () => {
  it('resolves the hat only once the path has rested', async () => {
    const server = mount()
    const field = await projectField()
    await waitFor(() => expect(screen.getByRole('button', { name: 'Claude' })).toHaveAttribute('aria-pressed', 'true'))
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
    expect(await screen.findByText(/Starts in the hat/)).toHaveTextContent('Starts in the hat Work at /srv/work/cat')
  })

  it('says when the directory does not exist yet', async () => {
    mount({ 'POST /api/hats/resolve': json(200, { ...RESOLVED, canonical: '/srv/work/new', exists: false, is_dir: false }) })
    await fill('/srv/work/new')
    expect(await screen.findByText(/Starts in the hat/, undefined, { timeout: 3000 })).toHaveTextContent(
      'this directory does not exist yet',
    )
  })
})

describe('start', () => {
  it('starts the session, then sends the first prompt, then opens the session', async () => {
    const server = mount()
    const user = await fill('/srv/work/cat', 'Fix the tests')
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
    await user.click(startButton())
    const alert = await screen.findByText(shown)
    expect(alert.closest('[role="alert"]')).not.toBeNull()
    expect(location.pathname).toBe('/new')
    expect(posts(server.sent)).not.toContain('/api/sessions/s-1/prompt')
    // The form stays, ready for another try.
    expect(startButton()).toBeEnabled()
    expect(screen.getByLabelText(/First prompt/)).toHaveValue('Fix the tests')
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
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-9'))
    expect(location.search).toBe('?notice=start_unknown')
    expect(sessionStorage.getItem('hennery.draft.s-9')).toBe('Fix the tests')
    expect(posts(server.sent).some((p) => p.endsWith('/prompt'))).toBe(false)
  })

  it('a delivery unknown that names no session stays on the form', async () => {
    mount({ 'POST /api/sessions': json(503, { code: 'delivery_unknown', message: 'gone' }) })
    const user = await fill('/srv/work/cat', 'Fix the tests')
    await user.click(startButton())
    expect(await screen.findByText(/its outcome is not known/)).toBeInTheDocument()
    expect(location.pathname).toBe('/new')
  })

  it('a refused first prompt opens the session with a notice and keeps the prompt as its draft', async () => {
    mount({ 'POST /api/sessions/s-1/prompt': json(409, { code: 'host_offline', message: 'the host is not connected' }) })
    const user = await fill('/srv/work/cat', 'Fix the tests')
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
    await user.click(screen.getByRole('button', { name: 'Codex' }))
    await user.click(screen.getByRole('button', { name: 'Browse' }))
    await screen.findByRole('group', { name: 'Browse directories' })
    await screen.findByText(/Starts in the hat/, undefined, { timeout: 3000 })

    await user.click(hostButton('desk'))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Claude' })).toHaveAttribute('aria-pressed', 'true'))
    expect(screen.getByRole('button', { name: 'Codex' })).toHaveAttribute('aria-pressed', 'false')
    expect(await projectField()).toHaveValue('')
    expect(screen.queryByRole('group', { name: 'Browse directories' })).toBeNull()
    expect(screen.queryByText(/Starts in the hat/)).toBeNull()
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
    await user.click(startButton())
    await waitFor(() => expect(location.pathname).toBe('/sessions/s-new'))
    // The list has loaded, and its newest visible row did not take over.
    await screen.findByText('old')
    await new Promise((r) => setTimeout(r, 50))
    expect(location.pathname).toBe('/sessions/s-new')
    // @ts-expect-error jsdom has none; each test sets its own
    delete window.matchMedia
  })
})
