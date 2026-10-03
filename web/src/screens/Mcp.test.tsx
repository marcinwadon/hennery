import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import App from '../App'
import { connection, hat, host } from '../test-fixtures'
import { FULL, json, stubServer, type Answer } from '../test-server'
import { loadManageCss, shortTargets } from '../test-targets'

const STEP_UP = json(403, { code: 'step_up_required', message: 'm' })
const STEPPED_UP = new Response(null, { status: 204 })
const NO_CONTENT = () => new Response(null, { status: 204 })
const WITH_MCP = { mode: 'full', features: ['mcp_connections'] }
const HATS = [
  hat({ id: 'hat-a', name: 'Personal' }),
  hat({ id: 'hat-b', name: 'Work', default_for_new_hosts: true }),
  hat({ id: 'hat-c', name: 'Old', purging: true }),
]
const PATH = '/api/mcp/connections'
const ITEM = `${PATH}/conn-0000000000000001`

afterEach(() => history.replaceState(null, '', '/'))

function open(routes: Record<string, Answer | Answer[]>, capabilities: unknown = WITH_MCP) {
  history.replaceState(null, '', '/mcp')
  const server = stubServer({
    'GET /api/capabilities': json(200, capabilities),
    'GET /api/hats': json(200, HATS),
    'GET /api/hosts': json(200, [host()]),
    'GET /api/mcp/connections': json(200, [connection()]),
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

const card = (name: string) => screen.findByRole('listitem', { name })

describe('the MCP screen behind its feature', () => {
  it('is not shown, and nothing is read, when the collector does not serve connections', async () => {
    const server = open({}, FULL)
    expect(await screen.findByText('The MCP gateway is not part of this deployment.')).toBeInTheDocument()
    expect(sent(server, 'GET', PATH)).toHaveLength(0)
  })

  it('is shown with the feature, ignoring names it does not know', async () => {
    open({}, { mode: 'full', features: ['from_the_future', 'mcp_connections'] })
    expect(await screen.findByRole('heading', { name: 'MCP', level: 1 })).toBeInTheDocument()
    expect(await card('Docs')).toBeInTheDocument()
  })
})

describe('the connection list', () => {
  it('shows what agents see, the server, the hat, the token’s state and the hosts', async () => {
    open({
      'GET /api/hosts': json(200, [
        host(),
        host({ host_id: 'host-2', name: 'old box', revoked_at: '2026-10-02T11:00:00Z' }),
      ]),
      'GET /api/mcp/connections': json(200, [connection({ mounts: ['host-1'] })]),
    })
    const docs = await card('Docs')
    expect(within(docs).getByText('hennery-docs', { exact: true })).toBeInTheDocument()
    expect(within(docs).getByText('https://mcp.example.com/mcp', { exact: true })).toBeInTheDocument()
    // On the card and beside the token field.
    await waitFor(() => expect(within(docs).getAllByText('Personal', { exact: true })).toHaveLength(2))
    expect(within(docs).getByText('Not used yet', { exact: true })).toBeInTheDocument()
    expect(within(docs).getByText('Not set yet', { exact: true })).toBeInTheDocument()
    expect(within(docs).getByText('someone', { exact: true })).toBeInTheDocument()
    expect(await within(docs).findByRole('checkbox', { name: 'laptop' })).toBeChecked()
    expect(within(docs).queryByRole('checkbox', { name: 'old box' })).toBeNull()
  })

  it('says sessions get changes when they start or resume, and how Codex gets them', async () => {
    open({})
    const note = await screen.findByRole('region', { name: 'How sessions get these' })
    expect(note).toHaveTextContent('Changes apply to new and resumed sessions.')
    expect(note).toHaveTextContent(
      'Codex, Claude run with your own CLI, and any other agent command (this may change): a session gets a connection only when both are in its host’s default hat, and it also loads your own MCP servers (such as ~/.codex), so it is not isolated. Sessions in other hats on that host get none.',
    )
  })

  it('words each state, and never asks to reconnect a failing one', async () => {
    open({
      'GET /api/mcp/connections': json(200, [
        connection({ id: 'conn-1', label: 'A', status: 'ok' }),
        connection({ id: 'conn-2', label: 'B', status: 'needs_auth', status_note: 'token revoked' }),
        connection({ id: 'conn-3', label: 'C', status: 'error', status_note: 'upstream 503' }),
        connection({ id: 'conn-4', label: 'D', status: 'from_the_future' as never }),
      ]),
    })
    expect(within(await card('A')).getByText('Working', { exact: true })).toBeInTheDocument()
    const b = await card('B')
    expect(within(b).getByText('Needs sign-in again', { exact: true })).toBeInTheDocument()
    expect(within(b).getByText('The server refused the token: set a new one below.')).toBeInTheDocument()
    const c = await card('C')
    expect(within(c).getByText('Is failing', { exact: true })).toBeInTheDocument()
    expect(within(c).getByText('upstream 503', { exact: true })).toBeInTheDocument()
    expect(c).not.toHaveTextContent(/connect again|reconnect|sign in again|set a new one/i)
    expect(within(await card('D')).getByText('Unknown state', { exact: true })).toBeInTheDocument()
  })

  it('renders a label and a vendor’s note as escaped, isolated text, the note cut at 300 characters', async () => {
    const long = '<b>x</b>' + 'y'.repeat(400)
    open({ 'GET /api/mcp/connections': json(200, [connection({ label: 'Do\u202Ecs<i>', status: 'error', status_note: long })]) })
    const title = await screen.findByRole('heading', { name: 'Do<U+202E>cs<i>' })
    expect(title.querySelector('bdi')).not.toBeNull()
    expect(title.querySelector('i')).toBeNull()
    const docs = screen.getByRole('listitem', { name: 'Do<U+202E>cs<i>' })
    const note = within(docs).getByText([...long].slice(0, 300).join('') + '…', { exact: true })
    expect(note.tagName).toBe('BDI')
    expect(docs.querySelector('b')).toBeNull()
  })

  it('tells a connection without authentication that the server wants one, without asking to reconnect', async () => {
    open({ 'GET /api/mcp/connections': json(200, [connection({ cred_kind: 'none', status: 'needs_auth' })]) })
    const docs = await card('Docs')
    expect(
      within(docs).getByText('The server asks for credentials: edit the connection to send a token.', { exact: true }),
    ).toBeInTheDocument()
  })

  it('cuts the account a vendor reports at 300 characters, as text', async () => {
    const long = '<i>a</i>' + 'z'.repeat(400)
    open({ 'GET /api/mcp/connections': json(200, [connection({ account_label: long })]) })
    const docs = await card('Docs')
    const account = within(docs).getByText([...long].slice(0, 300).join('') + '…', { exact: true })
    expect(account.tagName).toBe('BDI')
    expect(docs.querySelector('i')).toBeNull()
  })

  it('renders the URL, the hat and the host names as escaped text', async () => {
    open({
      'GET /api/hats': json(200, [hat({ id: 'hat-a', name: 'Pers\u202Eonal' })]),
      'GET /api/hosts': json(200, [host({ name: 'lap\u202Etop' })]),
      'GET /api/mcp/connections': json(200, [connection({ url: 'https://x.example/\u202Emcp' })]),
    })
    const docs = await card('Docs')
    expect(within(docs).getByText('https://x.example/<U+202E>mcp', { exact: true }).tagName).toBe('BDI')
    const hats = await within(docs).findAllByText('Pers<U+202E>onal', { exact: true })
    expect(hats.map((h) => h.tagName)).toEqual(['BDI', 'BDI'])
    expect(await within(docs).findByRole('checkbox', { name: 'lap<U+202E>top' })).toBeInTheDocument()
  })

  it('offers no token for a connection without authentication', async () => {
    open({ 'GET /api/mcp/connections': json(200, [connection({ cred_kind: 'none' })]) })
    const docs = await card('Docs')
    expect(within(docs).queryByRole('button', { name: 'Save token' })).toBeNull()
    expect(within(docs).getByText('None', { exact: true })).toBeInTheDocument()
  })

  it('says so when there is no connection', async () => {
    open({ 'GET /api/mcp/connections': json(200, []) })
    expect(await screen.findByText('No connection yet.')).toBeInTheDocument()
  })

  it('has every button, picker and tick box at least 44 px tall under 768 px', async () => {
    const unload = loadManageCss()
    try {
      open({})
      const docs = await card('Docs')
      await within(docs).findByRole('checkbox', { name: 'laptop' })
      const page = document.querySelector('.manage')!
      expect(shortTargets(page)).toEqual([])
      expect([...page.querySelectorAll('label.check')].filter((l) => !tallLabel(l))).toEqual([])
      await userEvent.click(screen.getByRole('button', { name: 'Add connection' }))
      await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
      expect(shortTargets(page)).toEqual([])
      expect([...page.querySelectorAll('label.check')].filter((l) => !tallLabel(l))).toEqual([])
    } finally {
      unload()
    }
  })
})

/** Whether a narrow-screen rule makes a tick box's label 44 px tall. */
function tallLabel(label: Element): boolean {
  for (const sheet of document.styleSheets) {
    for (const rule of sheet.cssRules) {
      if (!(rule instanceof CSSMediaRule) || !/max-width:\s*767px/.test(rule.media.mediaText)) continue
      for (const inner of rule.cssRules) {
        if (inner instanceof CSSStyleRule && label.matches(inner.selectorText) && parseFloat(inner.style.minHeight) >= 44)
          return true
      }
    }
  }
  return false
}

describe('adding a connection', () => {
  it('starts in the default hat for new hosts, offers no hat being purged, steps up and sends one body twice', async () => {
    const created = connection({ id: 'conn-2', slug: 'tracker', label: 'Tracker', hat_id: 'hat-b' })
    const server = open({ 'POST /api/mcp/connections': [STEP_UP, json(201, created)] })
    await userEvent.click(await screen.findByRole('button', { name: 'Add connection' }))
    const form = screen.getByRole('region', { name: 'Add a connection' })
    const picker = within(form).getByRole('combobox', { name: 'Hat' })
    expect(picker).toHaveValue('hat-b')
    expect(within(picker).queryByRole('option', { name: 'Old' })).toBeNull()
    await userEvent.type(within(form).getByRole('textbox', { name: 'Name' }), 'Tracker')
    await userEvent.type(within(form).getByRole('textbox', { name: 'Slug' }), 'tracker')
    expect(within(form).getByText('hennery-tracker', { exact: true })).toBeInTheDocument()
    await userEvent.type(within(form).getByRole('textbox', { name: 'Server URL' }), 'https://tracker.example/mcp')
    await userEvent.click(within(form).getByRole('button', { name: 'Create' }))
    await stepUp()
    const tracker = await card('Tracker')
    await waitFor(() => expect(within(tracker).getByRole('heading', { name: 'Tracker' })).toHaveFocus())
    const posts = sent(server, 'POST', PATH)
    expect(posts).toHaveLength(2)
    expect(posts[1].body).toEqual(posts[0].body)
    expect(posts[0].body).toEqual({
      slug: 'tracker',
      label: 'Tracker',
      url: 'https://tracker.example/mcp',
      hat_id: 'hat-b',
      cred_kind: 'static',
      static_header: 'Authorization',
      static_prefix: 'Bearer ',
      tool_allowlist: null,
      internal_network: false,
    })
    expect(screen.queryByRole('region', { name: 'Add a connection' })).toBeNull()
  })

  it('warns before saving no tool at all, and with the internal network', async () => {
    open({})
    await userEvent.click(await screen.findByRole('button', { name: 'Add connection' }))
    const form = screen.getByRole('region', { name: 'Add a connection' })
    await userEvent.click(within(form).getByRole('checkbox', { name: 'Every tool' }))
    expect(within(form).getByText('No tool is allowed: agents see this server with none.')).toBeInTheDocument()
    await userEvent.type(within(form).getByRole('textbox', { name: 'Allowed tools' }), 'search')
    expect(within(form).queryByText('No tool is allowed: agents see this server with none.')).toBeNull()
    await userEvent.click(within(form).getByRole('checkbox', { name: 'Internal network' }))
    expect(
      within(form).getByText(
        'The gateway may then reach addresses on your own network for this connection, and plain http to them. Turn it on only for a server you run there.',
        { exact: true },
      ),
    ).toBeInTheDocument()
  })

  it('sends one request however often “Create” is pressed while it runs', async () => {
    let answer: (r: Response) => void = () => {}
    const server = open({ 'POST /api/mcp/connections': () => new Promise<Response>((done) => (answer = done)) })
    await userEvent.click(await screen.findByRole('button', { name: 'Add connection' }))
    const form = screen.getByRole('region', { name: 'Add a connection' })
    const create = within(form).getByRole('button', { name: 'Create' })
    await userEvent.click(create)
    await waitFor(() => expect(create).toHaveAttribute('aria-disabled', 'true'))
    await userEvent.click(create)
    expect(sent(server, 'POST', PATH)).toHaveLength(1)
    // Cancel waits too: the request already sent will land.
    await userEvent.click(within(form).getByRole('button', { name: 'Cancel' }))
    expect(screen.getByRole('region', { name: 'Add a connection' })).toBeInTheDocument()
    answer(json(201, connection({ id: 'conn-2', label: 'New' })))
    expect(await card('New')).toBeInTheDocument()
  })

  it('opens with focus on the name', async () => {
    open({})
    await userEvent.click(await screen.findByRole('button', { name: 'Add connection' }))
    const form = screen.getByRole('region', { name: 'Add a connection' })
    expect(within(form).getByRole('textbox', { name: 'Name' })).toHaveFocus()
  })

  it('returns focus to “Add connection” when the form is cancelled', async () => {
    open({})
    await userEvent.click(await screen.findByRole('button', { name: 'Add connection' }))
    const form = screen.getByRole('region', { name: 'Add a connection' })
    await userEvent.click(within(form).getByRole('button', { name: 'Cancel' }))
    expect(screen.queryByRole('region', { name: 'Add a connection' })).toBeNull()
    await waitFor(() => expect(screen.getByRole('button', { name: 'Add connection' })).toHaveFocus())
  })

  it('shows a refusal, and keeps the form', async () => {
    open({ 'POST /api/mcp/connections': json(409, { code: 'slug_taken', message: 'x' }) })
    await userEvent.click(await screen.findByRole('button', { name: 'Add connection' }))
    const form = screen.getByRole('region', { name: 'Add a connection' })
    await userEvent.click(within(form).getByRole('button', { name: 'Create' }))
    expect(await within(form).findByRole('alert')).toHaveTextContent('This slug is taken: pick another.')
  })

  it('shows the server’s own words for a field it refused, as text', async () => {
    open({ 'POST /api/mcp/connections': json(400, { code: 'invalid', message: 'url: <b>not https</b>' }) })
    await userEvent.click(await screen.findByRole('button', { name: 'Add connection' }))
    const form = screen.getByRole('region', { name: 'Add a connection' })
    await userEvent.click(within(form).getByRole('button', { name: 'Create' }))
    const alert = await within(form).findByRole('alert')
    expect(alert).toHaveTextContent('url: <b>not https</b>')
    expect(alert.querySelector('b')).toBeNull()
  })
})

describe('editing a connection', () => {
  it('sends only the label, with no step-up, and returns focus to Edit', async () => {
    const server = open({ [`PATCH ${ITEM}`]: json(200, connection({ label: 'Docs 2' })) })
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    const name = within(docs).getByRole('textbox', { name: 'Name' })
    await userEvent.clear(name)
    await userEvent.type(name, 'Docs 2')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save' }))
    const renamed = await card('Docs 2')
    await waitFor(() => expect(within(renamed).getByRole('button', { name: 'Edit' })).toHaveFocus())
    expect(sent(server, 'PATCH', ITEM).map((s) => s.body)).toEqual([{ label: 'Docs 2' }])
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('opens with focus on the name', async () => {
    open({})
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    expect(within(docs).getByRole('textbox', { name: 'Name' })).toHaveFocus()
  })

  it('compares with the connection as the form opened, not an answer folded in since', async () => {
    const server = open({
      // Another tab moved the URL; the mount's answer brings that copy in.
      [`PUT ${ITEM}/mounts`]: json(200, connection({ mounts: ['host-1'], url: 'https://moved.example/mcp' })),
      [`PATCH ${ITEM}`]: json(200, connection({ label: 'Docs 2', url: 'https://moved.example/mcp' })),
    })
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    const laptop = await within(docs).findByRole('checkbox', { name: 'laptop' })
    await userEvent.click(laptop)
    await waitFor(() => expect(laptop).toBeChecked())
    const name = within(docs).getByRole('textbox', { name: 'Name' })
    await userEvent.clear(name)
    await userEvent.type(name, 'Docs 2')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save' }))
    await card('Docs 2')
    expect(sent(server, 'PATCH', ITEM).map((s) => s.body)).toEqual([{ label: 'Docs 2' }])
  })

  it('sends one request however often “Save” is pressed while it runs', async () => {
    let answer: (r: Response) => void = () => {}
    const server = open({ [`PATCH ${ITEM}`]: () => new Promise<Response>((done) => (answer = done)) })
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    await userEvent.type(within(docs).getByRole('textbox', { name: 'Name' }), '!')
    const save = within(docs).getByRole('button', { name: 'Save' })
    await userEvent.click(save)
    await waitFor(() => expect(save).toHaveAttribute('aria-disabled', 'true'))
    await userEvent.click(save)
    expect(sent(server, 'PATCH', ITEM)).toHaveLength(1)
    answer(json(200, connection({ label: 'Docs!' })))
    expect(await card('Docs!')).toBeInTheDocument()
  })

  it('sends nothing when nothing changed', async () => {
    const server = open({})
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    await userEvent.click(within(docs).getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(within(docs).getByRole('button', { name: 'Edit' })).toHaveFocus())
    expect(sent(server, 'PATCH', ITEM)).toHaveLength(0)
  })

  it('says the token goes before another origin is saved, then steps up and sends one body twice', async () => {
    const server = open({
      'GET /api/mcp/connections': json(200, [connection({ has_credential: true })]),
      [`PATCH ${ITEM}`]: [STEP_UP, json(200, connection({ url: 'https://other.example/mcp' }))],
    })
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    const url = within(docs).getByRole('textbox', { name: 'Server URL' })
    await userEvent.clear(url)
    await userEvent.type(url, 'https://other.example/mcp')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save' }))
    const confirm = await screen.findByRole('dialog', { name: 'Delete the stored token?' })
    expect(sent(server, 'PATCH', ITEM)).toHaveLength(0)
    await userEvent.click(within(confirm).getByRole('button', { name: 'Save and delete the token' }))
    await stepUp()
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    const patches = sent(server, 'PATCH', ITEM)
    expect(patches.map((s) => s.body)).toEqual([{ url: 'https://other.example/mcp' }, { url: 'https://other.example/mcp' }])
    expect(within(await card('Docs')).getByText('https://other.example/mcp', { exact: true })).toBeInTheDocument()
    await waitFor(() => expect(within(screen.getByRole('listitem', { name: 'Docs' })).getByRole('button', { name: 'Edit' })).toHaveFocus())
  })

  it('sends nothing, and keeps the form, when the token’s loss is not confirmed', async () => {
    const server = open({ 'GET /api/mcp/connections': json(200, [connection({ has_credential: true })]) })
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    await userEvent.selectOptions(within(docs).getByRole('combobox', { name: 'Authentication' }), 'none')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save' }))
    const confirm = await screen.findByRole('dialog', { name: 'Delete the stored token?' })
    await userEvent.click(within(confirm).getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(sent(server, 'PATCH', ITEM)).toHaveLength(0)
    expect(within(docs).getByRole('combobox', { name: 'Authentication' })).toHaveValue('none')
  })

  it('says the token goes before another kind is saved', async () => {
    const server = open({
      'GET /api/mcp/connections': json(200, [connection({ has_credential: true })]),
      [`PATCH ${ITEM}`]: json(200, connection({ cred_kind: 'none' })),
    })
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    await userEvent.selectOptions(within(docs).getByRole('combobox', { name: 'Authentication' }), 'none')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save' }))
    const confirm = await screen.findByRole('dialog', { name: 'Delete the stored token?' })
    await userEvent.click(within(confirm).getByRole('button', { name: 'Save and delete the token' }))
    await waitFor(() => expect(sent(server, 'PATCH', ITEM).map((s) => s.body)).toEqual([{ cred_kind: 'none' }]))
  })

  it('asks nothing first when no token is stored to lose', async () => {
    const server = open({ [`PATCH ${ITEM}`]: json(200, connection({ cred_kind: 'none' })) })
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    await userEvent.selectOptions(within(docs).getByRole('combobox', { name: 'Authentication' }), 'none')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(sent(server, 'PATCH', ITEM).map((s) => s.body)).toEqual([{ cred_kind: 'none' }]))
    expect(screen.queryByRole('dialog', { name: 'Delete the stored token?' })).toBeNull()
  })

  it('keeps the form and shows a refusal', async () => {
    open({ [`PATCH ${ITEM}`]: json(400, { code: 'unsupported_cred_kind', message: 'x' }) })
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    await userEvent.type(within(docs).getByRole('textbox', { name: 'Name' }), '!')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save' }))
    expect(await within(docs).findByRole('alert')).toHaveTextContent('This collector cannot sign in to a server this way yet.')
    expect(within(docs).getByRole('button', { name: 'Save' })).toBeInTheDocument()
  })
})

describe('deleting a connection', () => {
  it('is confirmed, then steps up, sends it twice, and the card goes', async () => {
    const server = open({ [`DELETE ${ITEM}`]: [STEP_UP, NO_CONTENT] })
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Delete' }))
    const confirm = await screen.findByRole('dialog', { name: 'Delete this connection?' })
    expect(confirm).toHaveTextContent('hennery-docs')
    await userEvent.click(within(confirm).getByRole('button', { name: 'Delete' }))
    await stepUp()
    await waitFor(() => expect(screen.queryByRole('listitem', { name: 'Docs' })).toBeNull())
    expect(sent(server, 'DELETE', ITEM)).toHaveLength(2)
    await waitFor(() => expect(screen.getByRole('heading', { name: 'MCP', level: 1 })).toHaveFocus())
  })
})

describe('the token', () => {
  it('steps up, sends one body twice, empties the field at once, and takes the 204 as stored', async () => {
    const server = open({ [`PUT ${ITEM}/credential`]: [STEP_UP, NO_CONTENT] })
    const docs = await card('Docs')
    const field = within(docs).getByLabelText('Token') as HTMLInputElement
    expect(field.type).toBe('password')
    await userEvent.type(field, 'sk-secret-1')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save token' }))
    expect(field.value).toBe('')
    await stepUp()
    expect(await within(docs).findByRole('status')).toHaveTextContent('Token saved. It is never shown again.')
    await waitFor(() => expect(within(docs).getByText('Set', { exact: true })).toBeInTheDocument())
    const puts = sent(server, 'PUT', `${ITEM}/credential`)
    expect(puts.map((s) => s.body)).toEqual([{ token: 'sk-secret-1' }, { token: 'sk-secret-1' }])
    expect(sent(server, 'GET', PATH)).toHaveLength(1)
    expect(document.body.innerHTML).not.toContain('sk-secret-1')
    expect(location.href).not.toContain('sk-secret-1')
  })

  it('is not sent again when the step-up is cancelled, and is not kept', async () => {
    const server = open({ [`PUT ${ITEM}/credential`]: STEP_UP })
    const docs = await card('Docs')
    const field = within(docs).getByLabelText('Token') as HTMLInputElement
    await userEvent.type(field, 'sk-secret-2')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save token' }))
    const dialog = await screen.findByRole('dialog', { name: 'This needs a fresh confirmation' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(await within(docs).findByRole('alert')).toHaveTextContent('Not confirmed.')
    expect(sent(server, 'PUT', `${ITEM}/credential`)).toHaveLength(1)
    expect(field.value).toBe('')
    expect(document.body.innerHTML).not.toContain('sk-secret-2')
  })

  it('keeps a mount saved while the token was being stored', async () => {
    let answer: (r: Response) => void = () => {}
    open({
      [`PUT ${ITEM}/credential`]: () => new Promise<Response>((done) => (answer = done)),
      [`PUT ${ITEM}/mounts`]: json(200, connection({ mounts: ['host-1'] })),
    })
    const docs = await card('Docs')
    await userEvent.type(within(docs).getByLabelText('Token'), 'sk-secret-3')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save token' }))
    const laptop = within(docs).getByRole('checkbox', { name: 'laptop' })
    await userEvent.click(laptop)
    await waitFor(() => expect(laptop).toBeChecked())
    answer(NO_CONTENT())
    await waitFor(() => expect(within(docs).getByText('Set', { exact: true })).toBeInTheDocument())
    expect(laptop).toBeChecked()
  })

  it('asks password managers to leave the field alone, and never submits it natively', async () => {
    open({})
    const docs = await card('Docs')
    const field = within(docs).getByLabelText('Token')
    expect(field).toHaveAttribute('type', 'password')
    expect(field).toHaveAttribute('autocomplete', 'off')
    expect(field).toHaveAttribute('data-1p-ignore')
    expect(field).toHaveAttribute('data-lpignore', 'true')
    expect(field).toHaveAttribute('data-bwignore')
    expect(field).toHaveAttribute('data-form-type', 'other')
    expect(field.closest('form')).toHaveAttribute('method', 'post')
  })

  it('says where the token goes, and for which hat', async () => {
    open({})
    const docs = await card('Docs')
    await waitFor(() =>
      expect(within(docs).getByText((_, el) => el?.textContent === 'Sent only to https://mcp.example.com, for sessions in Personal.' && el.tagName === 'P')).toBeInTheDocument(),
    )
  })

  it('stops saying it was saved once a change deletes it', async () => {
    const server = open({
      [`PUT ${ITEM}/credential`]: NO_CONTENT,
      [`PATCH ${ITEM}`]: json(200, connection({ has_credential: false, static_header: 'Authorization' })),
    })
    const docs = await card('Docs')
    await userEvent.type(within(docs).getByLabelText('Token'), 'sk-secret-5')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save token' }))
    expect(await within(docs).findByRole('status')).toHaveTextContent('Token saved. It is never shown again.')
    await userEvent.click(within(docs).getByRole('button', { name: 'Edit' }))
    const url = within(docs).getByRole('textbox', { name: 'Server URL' })
    await userEvent.clear(url)
    await userEvent.type(url, 'https://other.example/mcp')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save' }))
    const confirm = await screen.findByRole('dialog', { name: 'Delete the stored token?' })
    await userEvent.click(within(confirm).getByRole('button', { name: 'Save and delete the token' }))
    await waitFor(() => expect(within(docs).getByText('Not set yet', { exact: true })).toBeInTheDocument())
    expect(within(docs).queryByRole('status')).toBeNull()
    expect(sent(server, 'PATCH', ITEM)).toHaveLength(1)
  })

  it('ignores a second send while the first runs', async () => {
    let answer: (r: Response) => void = () => {}
    const server = open({ [`PUT ${ITEM}/credential`]: () => new Promise<Response>((done) => (answer = done)) })
    const docs = await card('Docs')
    await userEvent.type(within(docs).getByLabelText('Token'), 'sk-secret-4')
    const save = within(docs).getByRole('button', { name: 'Save token' })
    await userEvent.click(save)
    await waitFor(() => expect(save).toHaveAttribute('aria-disabled', 'true'))
    await userEvent.click(save)
    expect(within(docs).queryByRole('alert')).toBeNull()
    expect(sent(server, 'PUT', `${ITEM}/credential`)).toHaveLength(1)
    answer(NO_CONTENT())
    expect(await within(docs).findByRole('status')).toHaveTextContent('Token saved. It is never shown again.')
  })

  it('sends nothing for an empty field', async () => {
    const server = open({})
    const docs = await card('Docs')
    await userEvent.click(within(docs).getByRole('button', { name: 'Save token' }))
    expect(await within(docs).findByRole('alert')).toHaveTextContent('Type the token first.')
    expect(sent(server, 'PUT', `${ITEM}/credential`)).toHaveLength(0)
  })
})

describe('the mounts', () => {
  it('sends the whole set with no step-up, keeping a host this list does not show', async () => {
    const server = open({
      'GET /api/hosts': json(200, [host(), host({ host_id: 'host-2', name: 'build box' })]),
      'GET /api/mcp/connections': json(200, [connection({ mounts: ['host-9'] })]),
      [`PUT ${ITEM}/mounts`]: json(200, connection({ mounts: ['host-1', 'host-9'] })),
    })
    const docs = await card('Docs')
    const laptop = await within(docs).findByRole('checkbox', { name: 'laptop' })
    expect(laptop).not.toBeChecked()
    await userEvent.click(laptop)
    await waitFor(() => expect(laptop).toBeChecked())
    expect(sent(server, 'PUT', `${ITEM}/mounts`).map((s) => s.body)).toEqual([{ host_ids: ['host-1', 'host-9'] }])
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(within(docs).getByRole('checkbox', { name: 'build box' })).not.toBeChecked()
  })

  it('ignores a tick while a set is being saved', async () => {
    let answer: (r: Response) => void = () => {}
    const server = open({
      'GET /api/hosts': json(200, [host(), host({ host_id: 'host-2', name: 'build box' })]),
      [`PUT ${ITEM}/mounts`]: () => new Promise<Response>((done) => (answer = done)),
    })
    const docs = await card('Docs')
    await userEvent.click(await within(docs).findByRole('checkbox', { name: 'laptop' }))
    await waitFor(() => expect(within(docs).getByRole('group', { name: 'Hosts whose sessions get it' })).toHaveAttribute('aria-busy', 'true'))
    await userEvent.click(within(docs).getByRole('checkbox', { name: 'build box' }))
    expect(sent(server, 'PUT', `${ITEM}/mounts`).map((s) => s.body)).toEqual([{ host_ids: ['host-1'] }])
    answer(json(200, connection({ mounts: ['host-1'] })))
    await waitFor(() => expect(within(docs).getByRole('checkbox', { name: 'laptop' })).toBeChecked())
    expect(within(docs).getByRole('checkbox', { name: 'build box' })).not.toBeChecked()
  })

  it('unticks a host', async () => {
    const server = open({
      'GET /api/mcp/connections': json(200, [connection({ mounts: ['host-1'] })]),
      [`PUT ${ITEM}/mounts`]: json(200, connection({ mounts: [] })),
    })
    const laptop = await within(await card('Docs')).findByRole('checkbox', { name: 'laptop' })
    await userEvent.click(laptop)
    await waitFor(() => expect(laptop).not.toBeChecked())
    expect(sent(server, 'PUT', `${ITEM}/mounts`).map((s) => s.body)).toEqual([{ host_ids: [] }])
  })

  it('shows a refusal and keeps the set as it was', async () => {
    open({ [`PUT ${ITEM}/mounts`]: json(400, { code: 'invalid', message: 'host-1 is revoked' }) })
    const docs = await card('Docs')
    const laptop = await within(docs).findByRole('checkbox', { name: 'laptop' })
    await userEvent.click(laptop)
    expect(await within(docs).findByRole('alert')).toHaveTextContent('host-1 is revoked')
    expect(laptop).not.toBeChecked()
  })

  it('cannot be changed while the hosts cannot be read', async () => {
    open({ 'GET /api/hosts': json(500, { code: 'internal', message: 'the hosts are away' }) })
    const docs = await card('Docs')
    expect(await screen.findByRole('alert')).toHaveTextContent('the hosts are away')
    expect(within(docs).getByRole('group', { name: 'Hosts whose sessions get it' })).toBeDisabled()
  })
})
