import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { formatLeft } from '../components/Pairing'
import { hat, host } from '../test-fixtures'
import { FULL, json, stubServer, type Answer } from '../test-server'
import { loadManageCss, shortTargets } from '../test-targets'

const STEP_UP = json(403, { code: 'step_up_required', message: 'm' })
const STEPPED_UP = new Response(null, { status: 204 })
const HATS = [hat({ id: 'hat-a', name: 'Personal', default_for_new_hosts: true }), hat()]
const SETTINGS = { public_url: 'https://hennery.example.com' }

afterEach(() => {
  history.replaceState(null, '', '/')
  vi.useRealTimers()
})

function open(routes: Record<string, Answer | Answer[]>) {
  history.replaceState(null, '', '/hosts')
  const server = stubServer({
    'GET /api/capabilities': json(200, FULL),
    'GET /api/hats': json(200, HATS),
    'GET /api/settings': json(200, SETTINGS),
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

function card(name: string) {
  return screen.getByRole('listitem', { name })
}

const sent = (server: ReturnType<typeof stubServer>, method: string, path: string) =>
  server.sent.filter((s) => s.method === method && s.path === path)

describe('the host list', () => {
  it('shows each host’s state, versions and default hat, revoked ones as revoked and without actions', async () => {
    open({
      'GET /api/hosts': json(200, [
        host({ host_id: 'host-1', name: 'laptop' }),
        host({ host_id: 'host-2', name: 'build box', connected: false, host_version: '0.2.0', default_hat_id: 'hat-b' }),
        host({ host_id: 'host-3', name: 'old box', connected: false, revoked_at: '2026-10-02T11:00:00Z' }),
      ]),
    })
    const laptop = await screen.findByRole('listitem', { name: 'laptop' })
    expect(within(laptop).getByText('Online')).toBeInTheDocument()
    expect(within(laptop).getByText('linux-x64')).toBeInTheDocument()
    await waitFor(() => expect(within(laptop).getAllByText('Personal').length).toBeGreaterThan(0))
    const build = card('build box')
    expect(within(build).getByText('Offline')).toBeInTheDocument()
    expect(within(build).getByText('0.2.0')).toBeInTheDocument()
    const old = card('old box')
    expect(within(old).getByText('Revoked')).toBeInTheDocument()
    expect(within(old).queryByRole('button')).toBeNull()
    expect(within(laptop).getByRole('button', { name: 'Revoke' })).toBeInTheDocument()
  })

  it('renders a host name as escaped, isolated text', async () => {
    open({ 'GET /api/hosts': json(200, [host({ name: 'box\u202Etxt.exe<b>' })]) })
    const title = await screen.findByRole('heading', { name: 'box<U+202E>txt.exe<b>' })
    expect(title.querySelector('bdi')).not.toBeNull()
    expect(title.querySelector('b')).toBeNull()
  })

  it('says so when no host is paired', async () => {
    open({ 'GET /api/hosts': json(200, []) })
    expect(await screen.findByText('No host is paired yet.')).toBeInTheDocument()
  })

  it('says so when the hats cannot be read', async () => {
    open({ 'GET /api/hosts': json(200, [host()]), 'GET /api/hats': json(500, { code: 'internal', message: 'the hats are away' }) })
    expect(await screen.findByRole('alert')).toHaveTextContent('the hats are away')
  })

  it('has every button and picker at least 44 px tall under 768 px', async () => {
    const unload = loadManageCss()
    try {
      open({
        'GET /api/hosts': json(200, [host()]),
        'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
      })
      const laptop = await screen.findByRole('listitem', { name: 'laptop' })
      await waitFor(() => expect(within(laptop).getByRole('option', { name: 'Work' })).toBeInTheDocument())
      const page = document.querySelector('.manage')!
      expect(shortTargets(page)).toEqual([])
      await userEvent.click(screen.getByRole('button', { name: 'Add host' }))
      expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
      await userEvent.click(within(laptop).getByRole('button', { name: 'Rename' }))
      expect(shortTargets(page)).toEqual([])
    } finally {
      unload()
    }
  })
})

describe('adding a host', () => {
  it('steps up, then shows the code and the exact command with the public URL', async () => {
    const expires = new Date(Date.now() + 600_000).toISOString()
    const server = open({
      'GET /api/hosts': json(200, [host()]),
      'POST /api/hosts/pairing-codes': [STEP_UP, json(201, { code: 'ABCD-EFGH', expires_at: expires })],
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    await stepUp()
    const command = await screen.findByLabelText('Pairing command')
    expect(command).toHaveTextContent('hennery host join https://hennery.example.com ABCD-EFGH')
    expect(sent(server, 'POST', '/api/hosts/pairing-codes')).toHaveLength(2)
    expect(screen.getByRole('timer').textContent).toMatch(/^(10:00|9:5\d)$/)
  })

  it('drops the code at its expiry, and keeps it out of storage and the title', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const expires = new Date(Date.now() + 600_000).toISOString()
    open({
      'GET /api/hosts': json(200, [host()]),
      'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: expires }),
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
    await act(async () => {
      vi.advanceTimersByTime(599_000)
    })
    expect(screen.getByRole('timer')).toHaveTextContent(/^0:0\d$/)
    await act(async () => {
      vi.advanceTimersByTime(2_000)
    })
    expect(await screen.findByText('The code has expired. Add a host again for a new one.')).toBeInTheDocument()
    expect(document.body.textContent).not.toContain('ABCD-EFGH')
    expect(JSON.stringify({ ...localStorage })).not.toContain('ABCD')
    expect(JSON.stringify({ ...sessionStorage })).not.toContain('ABCD')
    expect(document.title).not.toContain('ABCD')
    expect(location.href).not.toContain('ABCD')
  })

  it('offers a new code once the last one expired, counting from the start again', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const expires = new Date(Date.now() + 600_000).toISOString()
    open({
      'GET /api/hosts': json(200, [host()]),
      'POST /api/hosts/pairing-codes': [
        json(201, { code: 'ABCD-EFGH', expires_at: expires }),
        json(201, { code: 'JKLM-NPQR', expires_at: expires }),
      ],
    })
    const add = await screen.findByRole('button', { name: 'Add host' })
    await userEvent.click(add)
    expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
    expect(add).toBeDisabled()
    await act(async () => {
      vi.advanceTimersByTime(601_000)
    })
    expect(await screen.findByText('The code has expired. Add a host again for a new one.')).toBeInTheDocument()
    expect(add).toBeEnabled()
    await userEvent.click(add)
    expect(await screen.findByLabelText('Pairing command')).toHaveTextContent('JKLM-NPQR')
    expect(screen.getByRole('timer').textContent).toMatch(/^(10:00|9:5\d)$/)
    expect(document.body.textContent).not.toContain('ABCD-EFGH')
  })

  it('reads the hosts once at a time while it waits, however slow a read is', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const server = open({
      // The list, the read before the mint, then a poll that never answers.
      'GET /api/hosts': [json(200, [host()]), json(200, [host()]), () => new Promise<Response>(() => {})],
      'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
    await act(async () => {
      vi.advanceTimersByTime(3_100)
    })
    await act(async () => {
      vi.advanceTimersByTime(3_000)
    })
    await act(async () => {
      vi.advanceTimersByTime(3_000)
    })
    expect(sent(server, 'GET', '/api/hosts')).toHaveLength(3)
  })

  it('ends the code when the new host pairs, and lists it', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const expires = new Date(Date.now() + 600_000).toISOString()
    open({
      // The list, the read before the mint, then the polls.
      'GET /api/hosts': [json(200, [host()]), json(200, [host()]), json(200, [host(), host({ host_id: 'host-9', name: 'new box' })])],
      'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: expires }),
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
    await act(async () => {
      vi.advanceTimersByTime(3_100)
    })
    expect(await screen.findByText(/^Paired:/)).toHaveTextContent('Paired: new box')
    expect(document.body.textContent).not.toContain('ABCD-EFGH')
    expect(await screen.findByRole('listitem', { name: 'new box' })).toBeInTheDocument()
    // The code is spent: another can be minted.
    expect(screen.getByRole('button', { name: 'Add host' })).toBeEnabled()
  })

  it('tells a new host from those before it even when the list never loaded', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    open({
      'GET /api/hosts': [json(500, { code: 'internal', message: 'm' }), json(200, [host()])],
      'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
    await act(async () => {
      vi.advanceTimersByTime(3_100)
    })
    expect(screen.queryByText(/^Paired:/)).toBeNull()
    expect(screen.getByLabelText('Pairing command')).toHaveTextContent('ABCD-EFGH')
  })

  it('reads the URL and the hosts before minting, so a failed read spends no code', async () => {
    const server = open({
      'GET /api/hosts': json(200, [host()]),
      'GET /api/settings': json(500, { code: 'internal', message: 'm' }),
      'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    expect(await screen.findByRole('alert')).toBeInTheDocument()
    expect(sent(server, 'POST', '/api/hosts/pairing-codes')).toHaveLength(0)
  })

  it('drops the code when the page is hidden for the back-forward cache', async () => {
    open({
      'GET /api/hosts': json(200, [host()]),
      'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
    act(() => {
      window.dispatchEvent(new Event('pagehide'))
    })
    expect(document.body.textContent).not.toContain('ABCD-EFGH')
  })

  it('hides the code when the panel closes', async () => {
    open({
      'GET /api/hosts': json(200, [host()]),
      'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(document.body.textContent).not.toContain('ABCD-EFGH')
  })

  it('shows a refusal, and no code', async () => {
    open({
      'GET /api/hosts': json(200, [host()]),
      'POST /api/hosts/pairing-codes': json(409, { code: 'too_many_codes', message: 'm' }),
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Sixteen pairing codes are live already')
    expect(screen.queryByLabelText('Pairing command')).toBeNull()
  })
})

describe('the countdown', () => {
  it('counts the code’s ten minutes from its arrival, whatever this clock says of its expiry', async () => {
    open({
      'GET /api/hosts': json(200, [host()]),
      // This browser's clock is an hour fast: the expiry is in its past.
      'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() - 3_600_000).toISOString() }),
    })
    await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
    expect(await screen.findByRole('timer')).toHaveTextContent(/^(10:00|9:5\d)$/)
  })

  it('reads m:ss', () => {
    expect(formatLeft(600)).toBe('10:00')
    expect(formatLeft(59.9)).toBe('0:59')
    expect(formatLeft(0)).toBe('0:00')
  })
})

describe('changing a host', () => {
  it('renames it after a confirmation and a step-up, sending the same name again', async () => {
    const server = open({
      'GET /api/hosts': json(200, [host()]),
      'PATCH /api/hosts/host-1': [STEP_UP, json(200, host({ name: 'desk' }))],
    })
    await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Rename' }))
    const input = screen.getByLabelText('New name')
    await userEvent.clear(input)
    await userEvent.type(input, 'desk')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    const confirm = await screen.findByRole('dialog', { name: 'Rename this host?' })
    expect(sent(server, 'PATCH', '/api/hosts/host-1')).toHaveLength(0)
    await userEvent.click(within(confirm).getByRole('button', { name: 'Rename' }))
    await stepUp()
    expect(await screen.findByRole('listitem', { name: 'desk' })).toBeInTheDocument()
    const patches = sent(server, 'PATCH', '/api/hosts/host-1')
    expect(patches.map((p) => p.body)).toEqual([{ name: 'desk' }, { name: 'desk' }])
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('returns focus to Rename after a rename, done or cancelled', async () => {
    open({
      'GET /api/hosts': json(200, [host()]),
      'PATCH /api/hosts/host-1': json(200, host({ name: 'desk' })),
    })
    await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Rename' }))
    await userEvent.type(screen.getByLabelText('New name'), ' 2')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    await userEvent.click(within(await screen.findByRole('dialog', { name: 'Rename this host?' })).getByRole('button', { name: 'Cancel' }))
    expect(within(card('laptop')).getByRole('button', { name: 'Rename' })).toHaveFocus()
    await userEvent.click(within(card('laptop')).getByRole('button', { name: 'Rename' }))
    await userEvent.type(screen.getByLabelText('New name'), ' 2')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    await userEvent.click(within(await screen.findByRole('dialog', { name: 'Rename this host?' })).getByRole('button', { name: 'Rename' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(within(card('desk')).getByRole('button', { name: 'Rename' })).toHaveFocus()
  })

  it('sends no rename when the trimmed name is empty or unchanged', async () => {
    const server = open({ 'GET /api/hosts': json(200, [host()]) })
    const laptop = await screen.findByRole('listitem', { name: 'laptop' })
    for (const typed of ['laptop  ', '   ']) {
      await userEvent.click(within(laptop).getByRole('button', { name: 'Rename' }))
      const input = screen.getByLabelText('New name')
      await userEvent.clear(input)
      await userEvent.type(input, typed)
      await userEvent.click(screen.getByRole('button', { name: 'Save' }))
      expect(screen.queryByRole('dialog')).toBeNull()
      expect(within(laptop).getByRole('button', { name: 'Rename' })).toHaveFocus()
    }
    await userEvent.click(within(laptop).getByRole('button', { name: 'Rename' }))
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(within(laptop).getByRole('button', { name: 'Rename' })).toHaveFocus()
    expect(sent(server, 'PATCH', '/api/hosts/host-1')).toHaveLength(0)
  })

  it('puts focus on the card’s title once a revoke took its buttons away', async () => {
    open({
      'GET /api/hosts': json(200, [host()]),
      'DELETE /api/hosts/host-1': json(200, host({ connected: false, revoked_at: '2026-10-02T12:00:00Z' })),
    })
    await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
    await userEvent.click(within(await screen.findByRole('dialog', { name: 'Revoke this host?' })).getByRole('button', { name: 'Revoke' }))
    await waitFor(() => expect(within(card('laptop')).getByText('Revoked')).toBeInTheDocument())
    expect(within(card('laptop')).getByRole('heading', { name: 'laptop' })).toHaveFocus()
  })

  it('revokes it after a confirmation that says its agents stop when it next connects', async () => {
    const server = open({
      'GET /api/hosts': json(200, [host()]),
      'DELETE /api/hosts/host-1': [STEP_UP, json(200, host({ connected: false, revoked_at: '2026-10-02T12:00:00Z' }))],
    })
    await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
    const confirm = await screen.findByRole('dialog', { name: 'Revoke this host?' })
    expect(confirm).toHaveTextContent('Agents it is running stop only when it next connects')
    await userEvent.click(within(confirm).getByRole('button', { name: 'Revoke' }))
    await stepUp()
    await waitFor(() => expect(within(card('laptop')).getByText('Revoked')).toBeInTheDocument())
    expect(sent(server, 'DELETE', '/api/hosts/host-1')).toHaveLength(2)
  })

  it('sends nothing when the confirmation is cancelled', async () => {
    const server = open({ 'GET /api/hosts': json(200, [host()]) })
    await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
    await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(sent(server, 'DELETE', '/api/hosts/host-1')).toHaveLength(0)
    expect(within(card('laptop')).getByRole('button', { name: 'Revoke' })).toHaveFocus()
  })

  it('keeps focus inside the confirmation, Tab cycling through its buttons', async () => {
    open({ 'GET /api/hosts': json(200, [host()]) })
    await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
    const confirm = await screen.findByRole('dialog', { name: 'Revoke this host?' })
    expect(within(confirm).getByRole('button', { name: 'Cancel' })).toHaveFocus()
    await userEvent.tab()
    expect(within(confirm).getByRole('button', { name: 'Revoke' })).toHaveFocus()
    await userEvent.tab()
    expect(within(confirm).getByRole('button', { name: 'Cancel' })).toHaveFocus()
    await userEvent.tab({ shift: true })
    expect(within(confirm).getByRole('button', { name: 'Revoke' })).toHaveFocus()
  })

  it('keeps the confirmation open with the reason when the step-up is cancelled', async () => {
    const server = open({ 'GET /api/hosts': json(200, [host()]), 'DELETE /api/hosts/host-1': STEP_UP })
    await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
    await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Revoke' }))
    const stepUpDialog = await screen.findByRole('dialog', { name: 'This needs a fresh confirmation' })
    await userEvent.click(within(stepUpDialog).getByRole('button', { name: 'Cancel' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Not confirmed.')
    const confirm = screen.getByRole('dialog', { name: 'Revoke this host?' })
    expect(sent(server, 'DELETE', '/api/hosts/host-1')).toHaveLength(1)
    // Focus came back into the confirmation, so Escape still closes it.
    expect(confirm.contains(document.activeElement)).toBe(true)
    await userEvent.keyboard('{Escape}')
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('changes its default hat after a confirmation that warns about parked sessions', async () => {
    const server = open({
      'GET /api/hosts': json(200, [host()]),
      'PATCH /api/hosts/host-1': [STEP_UP, json(200, host({ default_hat_id: 'hat-b' }))],
    })
    const laptop = await screen.findByRole('listitem', { name: 'laptop' })
    await waitFor(() => expect(within(laptop).getByRole('option', { name: 'Work' })).toBeInTheDocument())
    await userEvent.selectOptions(within(laptop).getByLabelText('Default hat'), 'hat-b')
    const confirm = await screen.findByRole('dialog', { name: 'Change this host’s default hat?' })
    expect(confirm).toHaveTextContent('resuming one is refused until it is re-assigned')
    await userEvent.click(within(confirm).getByRole('button', { name: 'Change' }))
    await stepUp()
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(sent(server, 'PATCH', '/api/hosts/host-1').map((p) => p.body)).toEqual([
      { default_hat_id: 'hat-b' },
      { default_hat_id: 'hat-b' },
    ])
  })
})
