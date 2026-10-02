import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import App from '../App'
import { hat, host, preview } from '../test-fixtures'
import { FULL, json, stubServer, type Answer } from '../test-server'
import { loadManageCss, shortTargets } from '../test-targets'

const STEP_UP = json(403, { code: 'step_up_required', message: 'm' })
const PERSONAL = hat({ id: 'hat-a', name: 'Personal', colour: '#4c5fd5', default_for_new_hosts: true })
const WORK = hat()

afterEach(() => history.replaceState(null, '', '/'))

function open(routes: Record<string, Answer | Answer[]> = {}) {
  history.replaceState(null, '', '/hats')
  const server = stubServer({
    'GET /api/capabilities': json(200, FULL),
    'GET /api/hats': json(200, [PERSONAL, WORK]),
    'GET /api/hosts': json(200, [host()]),
    'GET /api/hosts/host-1/path-rules': json(200, []),
    'POST /api/auth/step-up/password': new Response(null, { status: 204 }),
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

describe('the hats', () => {
  it('lists each hat with its colour, the default for new hosts marked', async () => {
    open()
    const personal = await screen.findByRole('listitem', { name: 'Personal' })
    expect(within(personal).getByText('Default for new hosts')).toBeInTheDocument()
    expect(personal.querySelector<HTMLElement>('.swatch')!.style.background).toBe('rgb(76, 95, 213)')
  })

  it('applies no colour the server did not give as #rrggbb', async () => {
    open({ 'GET /api/hats': json(200, [hat({ name: 'Odd', colour: 'red;background:url(x)' })]) })
    const odd = await screen.findByRole('listitem', { name: 'Odd' })
    expect(odd.querySelector<HTMLElement>('.swatch')!.getAttribute('style')).toBeNull()
  })

  it('creates a hat with no step-up', async () => {
    const created = hat({ id: 'hat-c', name: 'Clients', colour: '#64748b' })
    const server = open({ 'POST /api/hats': json(201, created) })
    await userEvent.type(await screen.findByRole('textbox', { name: 'Name' }), 'Clients')
    await userEvent.click(screen.getByRole('button', { name: 'Create' }))
    expect(await screen.findByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
    expect(sent(server, 'POST', '/api/hats')[0].body).toEqual({ name: 'Clients', colour: '#64748b' })
  })

  it('asks for another name when it is taken', async () => {
    open({ 'POST /api/hats': json(409, { code: 'name_taken', message: 'm' }) })
    await userEvent.type(await screen.findByRole('textbox', { name: 'Name' }), 'work')
    await userEvent.click(screen.getByRole('button', { name: 'Create' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Another hat has this name.')
  })

  it('renames and recolours a hat after a step-up, sending the same change again', async () => {
    const server = open({
      'PATCH /api/hats/hat-b': [STEP_UP, json(200, hat({ name: 'Day job', colour: '#112233' }))],
    })
    const work = await screen.findByRole('listitem', { name: 'Work' })
    await userEvent.click(within(work).getByRole('button', { name: 'Edit' }))
    const name = within(work).getByRole('textbox', { name: 'Name' })
    await userEvent.clear(name)
    await userEvent.type(name, 'Day job')
    await userEvent.click(within(work).getByRole('button', { name: 'Save' }))
    await stepUp()
    expect(await screen.findByRole('listitem', { name: 'Day job' })).toBeInTheDocument()
    expect(sent(server, 'PATCH', '/api/hats/hat-b').map((p) => p.body)).toEqual([{ name: 'Day job' }, { name: 'Day job' }])
  })

  it('makes a hat the default for new hosts, and the other one no longer', async () => {
    const server = open({
      'PATCH /api/hats/hat-b': [STEP_UP, json(200, hat({ default_for_new_hosts: true }))],
    })
    const work = await screen.findByRole('listitem', { name: 'Work' })
    await userEvent.click(within(work).getByRole('button', { name: 'Make default for new hosts' }))
    await stepUp()
    await waitFor(() => expect(within(screen.getByRole('listitem', { name: 'Work' })).getByText('Default for new hosts')).toBeInTheDocument())
    expect(within(screen.getByRole('listitem', { name: 'Personal' })).queryByText('Default for new hosts')).toBeNull()
    expect(sent(server, 'PATCH', '/api/hats/hat-b').map((p) => p.body)).toEqual([
      { default_for_new_hosts: true },
      { default_for_new_hosts: true },
    ])
  })

  it('keeps both of two changes that land together', async () => {
    let answerPatch: (r: Response) => void = () => {}
    open({
      'PATCH /api/hats/hat-b': () => new Promise<Response>((resolve) => (answerPatch = resolve)),
      'POST /api/hats': json(201, hat({ id: 'hat-c', name: 'Clients' })),
    })
    const work = await screen.findByRole('listitem', { name: 'Work' })
    await userEvent.click(within(work).getByRole('button', { name: 'Make default for new hosts' }))
    await userEvent.type(screen.getByRole('textbox', { name: 'Name' }), 'Clients')
    await userEvent.click(screen.getByRole('button', { name: 'Create' }))
    expect(await screen.findByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
    answerPatch(json(200, hat({ default_for_new_hosts: true })))
    await waitFor(() => expect(within(screen.getByRole('listitem', { name: 'Work' })).getByText('Default for new hosts')).toBeInTheDocument())
    expect(screen.getByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
    expect(within(screen.getByRole('listitem', { name: 'Personal' })).queryByText('Default for new hosts')).toBeNull()
  })

  it('keeps a change that lands while a new hat is being created', async () => {
    let answerCreate: (r: Response) => void = () => {}
    open({
      'PATCH /api/hats/hat-b': json(200, hat({ default_for_new_hosts: true })),
      'POST /api/hats': () => new Promise<Response>((resolve) => (answerCreate = resolve)),
    })
    await userEvent.type(await screen.findByRole('textbox', { name: 'Name' }), 'Clients')
    await userEvent.click(screen.getByRole('button', { name: 'Create' }))
    await userEvent.click(within(screen.getByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Make default for new hosts' }))
    await waitFor(() => expect(within(screen.getByRole('listitem', { name: 'Work' })).getByText('Default for new hosts')).toBeInTheDocument())
    answerCreate(json(201, hat({ id: 'hat-c', name: 'Clients' })))
    expect(await screen.findByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
    expect(within(screen.getByRole('listitem', { name: 'Work' })).getByText('Default for new hosts')).toBeInTheDocument()
  })

  it('returns focus to Edit after an edit is saved', async () => {
    open({ 'PATCH /api/hats/hat-b': json(200, hat({ name: 'Day job' })) })
    const work = await screen.findByRole('listitem', { name: 'Work' })
    await userEvent.click(within(work).getByRole('button', { name: 'Edit' }))
    await userEvent.type(within(work).getByRole('textbox', { name: 'Name' }), ' 2')
    await userEvent.click(within(work).getByRole('button', { name: 'Save' }))
    const dayJob = await screen.findByRole('listitem', { name: 'Day job' })
    await waitFor(() => expect(within(dayJob).getByRole('button', { name: 'Edit' })).toHaveFocus())
  })

  it('cannot save a name of spaces only', async () => {
    const server = open({})
    const work = await screen.findByRole('listitem', { name: 'Work' })
    await userEvent.click(within(work).getByRole('button', { name: 'Edit' }))
    await userEvent.clear(within(work).getByRole('textbox', { name: 'Name' }))
    await userEvent.type(within(work).getByRole('textbox', { name: 'Name' }), '   ')
    expect(within(work).getByRole('button', { name: 'Save' })).toBeDisabled()
    expect(sent(server, 'PATCH', '/api/hats/hat-b')).toHaveLength(0)
  })

  it('puts focus on the card’s title once “Make default for new hosts” is gone', async () => {
    open({ 'PATCH /api/hats/hat-b': json(200, hat({ default_for_new_hosts: true })) })
    const work = await screen.findByRole('listitem', { name: 'Work' })
    await userEvent.click(within(work).getByRole('button', { name: 'Make default for new hosts' }))
    await waitFor(() => expect(within(work).queryByRole('button', { name: 'Make default for new hosts' })).toBeNull())
    expect(within(work).getByRole('heading', { name: 'Work' })).toHaveFocus()
  })

  it('puts focus back in the name after a hat is created', async () => {
    open({ 'POST /api/hats': json(201, hat({ id: 'hat-c', name: 'Clients' })) })
    const name = await screen.findByRole('textbox', { name: 'Name' })
    await userEvent.type(name, 'Clients')
    await userEvent.click(screen.getByRole('button', { name: 'Create' }))
    expect(await screen.findByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
    expect(name).toHaveFocus()
  })

  it('says so when the hosts cannot be read', async () => {
    open({ 'GET /api/hosts': json(500, { code: 'internal', message: 'the hosts are away' }) })
    expect(await screen.findByRole('alert')).toHaveTextContent('the hosts are away')
  })

  it('has every button, picker and colour at least 44 px tall under 768 px', async () => {
    const unload = loadManageCss()
    try {
      open({ 'GET /api/hosts/host-1/path-rules': json(200, [{ id: 'r-1', prefix: '/home/me/work', hat_id: 'hat-b', verified: true }]) })
      await screen.findByDisplayValue('/home/me/work')
      const work = screen.getByRole('listitem', { name: 'Work' })
      await userEvent.click(within(work).getByRole('button', { name: 'Edit' }))
      expect(shortTargets(document.querySelector('.manage')!)).toEqual([])
    } finally {
      unload()
    }
  })
})

describe('purging a hat', () => {
  const result = { sessions: 3, rules: 2, unconfirmed: ['s-9'], host_transcripts: { removed: 2, partial: 0, pending: 1, pending_sessions: ['s-9'] } }

  it('shows what goes, then purges after a confirmation and a step-up', async () => {
    const server = open({
      'GET /api/hats/hat-b/purge': json(200, preview()),
      'POST /api/hats/hat-b/purge': [STEP_UP, json(200, result)],
    })
    await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
    const dialog = await screen.findByRole('dialog', { name: 'Purge this hat?' })
    expect(dialog).toHaveTextContent('3 sessions of the hat')
    expect(dialog).toHaveTextContent('2 path rules')
    expect(dialog).toHaveTextContent('1 recent project')
    expect(sent(server, 'POST', '/api/hats/hat-b/purge')).toHaveLength(0)
    await userEvent.click(within(dialog).getByRole('button', { name: 'Purge' }))
    await stepUp()
    const outcome = await screen.findByRole('status', { name: 'Purged Work' })
    expect(outcome).toHaveTextContent('Deleted 3 sessions and 2 path rules.')
    expect(outcome).toHaveTextContent('s-9')
    expect(outcome).toHaveTextContent('2 removed, 0 removed in part, 1 still to remove')
    expect(sent(server, 'POST', '/api/hats/hat-b/purge')).toHaveLength(2)
  })

  it('puts focus on what the purge deleted, once the dialog has gone', async () => {
    open({
      'GET /api/hats/hat-b/purge': json(200, preview()),
      'POST /api/hats/hat-b/purge': json(200, result),
    })
    await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
    await userEvent.click(within(await screen.findByRole('dialog', { name: 'Purge this hat?' })).getByRole('button', { name: 'Purge' }))
    const outcome = await screen.findByRole('status', { name: 'Purged Work' })
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    await waitFor(() => expect(within(outcome).getByRole('heading', { name: 'Purged Work' })).toHaveFocus())
  })

  it('cannot purge while a session runs, and names it', async () => {
    open({ 'GET /api/hats/hat-b/purge': json(200, preview({ running: ['s-1'] })) })
    await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
    const dialog = await screen.findByRole('dialog', { name: 'Purge this hat?' })
    expect(dialog).toHaveTextContent('Close these sessions first')
    expect(within(dialog).getByRole('link', { name: 's-1' })).toHaveAttribute('href', '/sessions/s-1')
    expect(within(dialog).getByRole('button', { name: 'Purge' })).toBeDisabled()
  })

  it('cannot purge a default hat', async () => {
    open({ 'GET /api/hosts': json(200, [host({ default_hat_id: 'hat-b' })]) })
    const work = await screen.findByRole('listitem', { name: 'Work' })
    await waitFor(() => expect(within(work).getByRole('button', { name: 'Purge' })).toBeDisabled())
    expect(work).toHaveTextContent('A default hat cannot be purged')
  })

  it('resumes a purge that began', async () => {
    const server = open({
      'GET /api/hats': json(200, [PERSONAL, hat({ purging: true })]),
      'GET /api/hats/hat-b/purge': json(200, preview({ purging: true })),
      'POST /api/hats/hat-b/purge': json(200, result),
    })
    await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Resume purge' }))
    const dialog = await screen.findByRole('dialog', { name: 'Resume this hat’s purge?' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Resume purge' }))
    expect(await screen.findByRole('status', { name: 'Purged Work' })).toBeInTheDocument()
    expect(sent(server, 'POST', '/api/hats/hat-b/purge')).toHaveLength(1)
  })

  it('reads the hats again when a purge fails, so a hat it froze offers “Resume purge”', async () => {
    open({
      'GET /api/hats': [json(200, [PERSONAL, WORK]), json(200, [PERSONAL, hat({ purging: true })])],
      'GET /api/hats/hat-b/purge': json(200, preview()),
      'POST /api/hats/hat-b/purge': json(500, { code: 'internal', message: 'stopped half way' }),
    })
    await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
    const dialog = await screen.findByRole('dialog', { name: 'Purge this hat?' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Purge' }))
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('stopped half way')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(await within(screen.getByRole('listitem', { name: 'Work' })).findByRole('button', { name: 'Resume purge' })).toBeInTheDocument()
  })

  it('reads the rules again after a purge, which deleted the hat’s, and asks the tester again', async () => {
    const server = open({
      'GET /api/hosts/host-1/path-rules': [json(200, [{ id: 'r-1', prefix: '/home/me/work', hat_id: 'hat-b', verified: true }]), json(200, [])],
      'GET /api/hats/hat-b/purge': json(200, preview()),
      'POST /api/hats/hat-b/purge': json(200, result),
      'POST /api/hats/resolve': [
        json(200, { canonical: '/home/me/work/app', exists: true, is_dir: true, hat_id: 'hat-b', rule_id: 'r-1' }),
        json(200, { canonical: '/home/me/work/app', exists: true, is_dir: true, hat_id: 'hat-a' }),
      ],
    })
    await screen.findByDisplayValue('/home/me/work')
    await userEvent.type(screen.getByLabelText('Test a path'), '/home/me/work/app')
    expect(await screen.findByLabelText('Resolution', {}, { timeout: 2000 })).toHaveTextContent('a path rule')
    await userEvent.click(within(screen.getByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
    await userEvent.click(within(await screen.findByRole('dialog', { name: 'Purge this hat?' })).getByRole('button', { name: 'Purge' }))
    expect(await screen.findByText('No rules: every session on this host gets its default hat.')).toBeInTheDocument()
    expect(screen.queryByDisplayValue('/home/me/work')).toBeNull()
    await waitFor(() => expect(screen.getByLabelText('Resolution')).toHaveTextContent('the host’s default hat'), { timeout: 2000 })
    expect(sent(server, 'POST', '/api/hats/resolve')).toHaveLength(2)
  })

  it('lists the sessions of no hat, which no purge deletes', async () => {
    const lost = { session_id: 's-0', host_id: 'host-1', agent: 'claude', cwd: '/srv/old', hat_id: '', lifecycle: 'closed', presumed_parked: false, created_at: '2026-10-01T10:00:00Z', last_event_at: '2026-10-01T10:00:00Z' }
    open({ 'GET /api/hats/hat-b/purge': json(200, preview({ unassigned: [lost], unassigned_count: 1 })) })
    await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
    const dialog = await screen.findByRole('dialog', { name: 'Purge this hat?' })
    expect(dialog).toHaveTextContent('1 session belong to no hat')
    expect(within(dialog).getByRole('link', { name: '/srv/old' })).toHaveAttribute('href', '/sessions/s-0')
  })
})

describe('path rules', () => {
  const RULES = [{ id: 'r-1', prefix: '/home/me/work', hat_id: 'hat-b', verified: true }]

  it('shows a host’s rules, unverified ones marked', async () => {
    open({
      'GET /api/hosts/host-1/path-rules': json(200, [...RULES, { id: 'r-2', prefix: '/home/me/later', hat_id: 'hat-b', verified: false }]),
    })
    expect(await screen.findByDisplayValue('/home/me/work')).toBeInTheDocument()
    expect(screen.getAllByText('Unverified')).toHaveLength(1)
  })

  it('sends the whole set after a step-up, and shows the set as the host stored it', async () => {
    const server = open({
      'GET /api/hosts/host-1/path-rules': json(200, RULES),
      'PUT /api/hosts/host-1/path-rules': [
        STEP_UP,
        json(200, [...RULES, { id: 'r-3', prefix: '/private/tmp/x', hat_id: 'hat-a', verified: true }]),
      ],
    })
    await screen.findByDisplayValue('/home/me/work')
    await userEvent.click(screen.getByRole('button', { name: 'Add rule' }))
    await userEvent.type(screen.getByLabelText('Path 2'), '/tmp/x')
    await userEvent.selectOptions(screen.getByLabelText('Hat 2'), 'hat-a')
    await userEvent.click(screen.getByRole('button', { name: 'Save rules' }))
    await stepUp()
    expect(await screen.findByDisplayValue('/private/tmp/x')).toBeInTheDocument()
    const puts = sent(server, 'PUT', '/api/hosts/host-1/path-rules')
    expect(puts).toHaveLength(2)
    expect(puts[0].body).toEqual(puts[1].body)
    expect(puts[1].body).toEqual({
      rules: [
        { prefix: '/home/me/work', hat_id: 'hat-b' },
        { prefix: '/tmp/x', hat_id: 'hat-a' },
      ],
    })
  })

  it('shows a rule’s hat that is being purged, rather than another', async () => {
    open({
      'GET /api/hats': json(200, [PERSONAL, hat({ purging: true })]),
      'GET /api/hosts/host-1/path-rules': json(200, RULES),
    })
    await screen.findByDisplayValue('/home/me/work')
    const picker = screen.getByLabelText('Hat 1') as HTMLSelectElement
    expect(picker.value).toBe('hat-b')
    expect(within(picker).getByRole('option', { name: 'Work' })).toBeDisabled()
  })

  it('cannot save a rule with no path, and says why', async () => {
    open({ 'GET /api/hosts/host-1/path-rules': json(200, RULES) })
    await screen.findByDisplayValue('/home/me/work')
    expect(screen.getByRole('button', { name: 'Save rules' })).toBeEnabled()
    await userEvent.click(screen.getByRole('button', { name: 'Add rule' }))
    await userEvent.type(screen.getByLabelText('Path 2'), '   ')
    expect(screen.getByRole('button', { name: 'Save rules' })).toBeDisabled()
    expect(screen.getByText('Every rule needs a path.')).toBeInTheDocument()
    await userEvent.type(screen.getByLabelText('Path 2'), '/srv')
    expect(screen.getByRole('button', { name: 'Save rules' })).toBeEnabled()
  })

  it('moves focus to the next rule when one is removed, and to “Add rule” after the last', async () => {
    open({
      'GET /api/hosts/host-1/path-rules': json(200, [...RULES, { id: 'r-2', prefix: '/srv', hat_id: 'hat-a', verified: true }]),
    })
    await screen.findByDisplayValue('/home/me/work')
    await userEvent.click(screen.getByRole('button', { name: 'Remove rule 1' }))
    expect(screen.getByLabelText('Path 1')).toHaveValue('/srv')
    expect(screen.getByLabelText('Path 1')).toHaveFocus()
    await userEvent.click(screen.getByRole('button', { name: 'Remove rule 1' }))
    expect(screen.getByRole('button', { name: 'Add rule' })).toHaveFocus()
  })

  it('says why a set was refused', async () => {
    open({
      'GET /api/hosts/host-1/path-rules': json(200, RULES),
      'PUT /api/hosts/host-1/path-rules': json(409, { code: 'host_offline', message: 'm' }),
    })
    await screen.findByDisplayValue('/home/me/work')
    await userEvent.click(screen.getByRole('button', { name: 'Save rules' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('The host is offline.')
  })
})

describe('the path tester', () => {
  it('asks the host which hat a typed path resolves to, and says which rule decided', async () => {
    const server = open({
      'POST /api/hats/resolve': json(200, { canonical: '/home/me/work/app', exists: true, is_dir: true, hat_id: 'hat-b', rule_id: 'r-1' }),
    })
    await userEvent.type(await screen.findByLabelText('Test a path'), '~/work/app')
    const resolution = await screen.findByLabelText('Resolution', {}, { timeout: 2000 })
    expect(resolution).toHaveTextContent('/home/me/work/app')
    expect(resolution).toHaveTextContent('Work')
    expect(resolution).toHaveTextContent('a path rule')
    const asked = sent(server, 'POST', '/api/hats/resolve')
    expect(asked.at(-1)?.body).toEqual({ host_id: 'host-1', path: '~/work/app' })
    // Typing paused once: one question, not one per key.
    expect(asked).toHaveLength(1)
  })

  it('names the host’s default hat when no rule decided, and a path that is missing', async () => {
    open({ 'POST /api/hats/resolve': json(200, { canonical: '/srv/x', exists: false, is_dir: false, hat_id: 'hat-a' }) })
    await userEvent.type(await screen.findByLabelText('Test a path'), '/srv/x')
    const resolution = await screen.findByLabelText('Resolution', {}, { timeout: 2000 })
    expect(resolution).toHaveTextContent('the host’s default hat')
    expect(resolution).toHaveTextContent('This path does not exist on the host.')
  })

  it('shows the answer for the path typed last, never an older one', async () => {
    let answerFirst: (r: Response) => void = () => {}
    open({
      'POST /api/hats/resolve': [
        () => new Promise<Response>((resolve) => (answerFirst = resolve)),
        json(200, { canonical: '/srv/b', exists: true, is_dir: true, hat_id: 'hat-a' }),
      ],
    })
    const tester = await screen.findByLabelText('Test a path')
    await userEvent.type(tester, '/srv/a')
    await screen.findByText('Asking the host…', {}, { timeout: 2000 })
    await userEvent.clear(tester)
    await userEvent.type(tester, '/srv/b')
    const resolution = await screen.findByLabelText('Resolution', {}, { timeout: 2000 })
    expect(resolution).toHaveTextContent('/srv/b')
    // The first question's answer arrives late, and is not shown.
    answerFirst(json(200, { canonical: '/srv/a', exists: true, is_dir: true, hat_id: 'hat-b' }))
    await new Promise((r) => setTimeout(r, 50))
    expect(screen.getByLabelText('Resolution')).toHaveTextContent('/srv/b')
  })

  it('asks again after the rules are saved, since it answers under the rules as saved', async () => {
    open({
      'GET /api/hosts/host-1/path-rules': json(200, []),
      'PUT /api/hosts/host-1/path-rules': json(200, [{ id: 'r-1', prefix: '/srv', hat_id: 'hat-b', verified: true }]),
      'POST /api/hats/resolve': [
        json(200, { canonical: '/srv/x', exists: true, is_dir: true, hat_id: 'hat-a' }),
        json(200, { canonical: '/srv/x', exists: true, is_dir: true, hat_id: 'hat-b', rule_id: 'r-1' }),
      ],
    })
    await userEvent.type(await screen.findByLabelText('Test a path'), '/srv/x')
    expect(await screen.findByLabelText('Resolution', {}, { timeout: 2000 })).toHaveTextContent('the host’s default hat')
    await userEvent.click(screen.getByRole('button', { name: 'Add rule' }))
    await userEvent.type(screen.getByLabelText('Path 1'), '/srv')
    await userEvent.click(screen.getByRole('button', { name: 'Save rules' }))
    await waitFor(() => expect(screen.getByLabelText('Resolution')).toHaveTextContent('a path rule'), { timeout: 2000 })
    expect(screen.getByLabelText('Test a path')).toHaveValue('/srv/x')
  })

  it('shows no answer for a path typed after it', async () => {
    open({ 'POST /api/hats/resolve': json(200, { canonical: '/srv/x', exists: true, is_dir: true, hat_id: 'hat-a' }) })
    const tester = await screen.findByLabelText('Test a path')
    await userEvent.type(tester, '/srv/x')
    expect(await screen.findByLabelText('Resolution', {}, { timeout: 2000 })).toBeInTheDocument()
    await userEvent.type(tester, 'y')
    expect(screen.queryByLabelText('Resolution')).toBeNull()
  })

  it('announces each answer once: in one live region, nested in none', async () => {
    open({
      'POST /api/hats/resolve': [
        json(409, { code: 'host_offline', message: 'm' }),
        json(200, { canonical: '/srv/xy', exists: true, is_dir: true, hat_id: 'hat-a' }),
      ],
    })
    const LIVE = '[role="status"], [role="alert"], [aria-live]:not([aria-live="off"])'
    const nested = () => [...document.querySelectorAll(LIVE)].filter((r) => r.parentElement?.closest(LIVE))
    const tester = await screen.findByLabelText('Test a path')
    await userEvent.type(tester, '/srv/x')
    await screen.findByText('The host is offline: it resolves the path.', {}, { timeout: 2000 })
    expect(nested()).toEqual([])
    await userEvent.type(tester, 'y')
    const resolution = await screen.findByLabelText('Resolution', {}, { timeout: 2000 })
    expect(resolution.closest(LIVE)).not.toBeNull()
    expect(nested()).toEqual([])
  })

  it.each([
    ['host_offline', 'The host is offline: it resolves the path.'],
    ['resolve_unsupported', 'This host cannot resolve paths yet: update hennery on it.'],
    ['hat_ambiguous', 'This path matches more than one hat'],
  ])('says why it could not tell (%s)', async (code, message) => {
    open({ 'POST /api/hats/resolve': json(409, { code, message: 'm' }) })
    await userEvent.type(await screen.findByLabelText('Test a path'), '/srv/x')
    expect(await screen.findByRole('alert', {}, { timeout: 2000 })).toHaveTextContent(message)
  })

  it('shows a host’s own refusal as escaped text', async () => {
    open({ 'POST /api/hats/resolve': json(502, { code: 'host_refused', message: '<b>no</b>\u202E' }) })
    await userEvent.type(await screen.findByLabelText('Test a path'), '/srv/x')
    const alert = await screen.findByRole('alert', {}, { timeout: 2000 })
    expect(alert.textContent).toBe('<b>no</b><U+202E>')
    expect(alert.querySelector('b')).toBeNull()
  })
})
