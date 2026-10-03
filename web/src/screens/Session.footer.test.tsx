import '@testing-library/jest-dom/vitest'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import type { SessionDetail } from '../generated/protocol'
import { forgetAllAttachments } from '../lib/attachments'
import { json } from '../test-stream'
import SessionView from './Session'
import { FAST, sessionServer, type Opts } from './test-session'

const WAIT = { timeout: 5000 }

beforeEach(() => {
  sessionStorage.clear()
  forgetAllAttachments()
})

/** The view of session `s1` as its detail says, on host `build-box`. */
async function show(detail: Partial<SessionDetail>, opts: Opts = {}) {
  const s = sessionServer({ detail, ...opts })
  const r = render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
  // The header names the host once the hosts are in.
  await waitFor(() => expect(screen.getByLabelText('Host')).toHaveTextContent('build-box'), WAIT)
  return { ...s, ...r }
}

const footer = (r: { container: HTMLElement }) => r.container.querySelector('.session-footer')
const text = (el: Element | null | undefined) => el?.textContent?.replace(/\s+/g, ' ').trim()
const resumeButton = () => screen.getByRole('button', { name: 'Resume' })

async function refused(answer: Response, detail: Partial<SessionDetail> = { lifecycle: 'parked' }, opts: Opts = {}) {
  const r = await show(detail, { resume: () => answer, ...opts })
  fireEvent.click(resumeButton())
  const alert = await screen.findByRole('alert', {}, WAIT)
  return { ...r, alert }
}

describe('SessionView: the footer of a session that is not running', () => {
  it('a parked session offers Resume, which resumes it; the composer stays', async () => {
    const r = await show({ lifecycle: 'parked' })
    expect(text(footer(r)?.querySelector('.txt'))).toBe('This session is parked.')
    expect(r.container.querySelector('.session-footer-offline')).toBeNull()
    expect(screen.getByLabelText('Prompt')).toBeInTheDocument()
    fireEvent.click(resumeButton())
    await waitFor(() => expect(r.changes()).toEqual(['POST /api/sessions/s1/resume']), WAIT)
    await waitFor(() => expect(resumeButton()).toBeEnabled(), WAIT)
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('a closed session offers Resume', async () => {
    const r = await show({ lifecycle: 'closed' })
    expect(text(footer(r)?.querySelector('.txt'))).toBe('This session is closed.')
    expect(resumeButton()).toBeEnabled()
  })

  it('a presumed parked session says its host is offline, and still offers Resume', async () => {
    const r = await show({ lifecycle: 'parked', presumed_parked: true })
    expect(text(r.container.querySelector('.session-footer-offline'))).toBe(
      'The host is offline: it has been away, and may still be running this session.',
    )
    expect(resumeButton()).toBeEnabled()
  })

  it('a parked session on a host that is not connected says so', async () => {
    const r = await show(
      { lifecycle: 'parked' },
      { hosts: () => json([{ host_id: 'h1', name: 'build-box', capabilities: [], connected: false }]) },
    )
    expect(text(r.container.querySelector('.session-footer-offline'))).toBe('The host is offline: resume once it is back.')
  })

  it('a parked session on a connected host says nothing of it', async () => {
    const r = await show(
      { lifecycle: 'parked' },
      { hosts: () => json([{ host_id: 'h1', name: 'build-box', capabilities: [], connected: true }]) },
    )
    expect(r.container.querySelector('.session-footer-offline')).toBeNull()
  })

  it('a host_offline marker on the stream turns the parked footer’s note on, and host_back turns it off, with no request', async () => {
    const r = await show(
      { lifecycle: 'parked' },
      { hosts: () => json([{ host_id: 'h1', name: 'build-box', capabilities: [], connected: true }]) },
    )
    await waitFor(() => expect(r.streams).toHaveLength(1), WAIT)
    await waitFor(() => expect(r.of('/api/sessions/s1/catalog')).toHaveLength(1), WAIT)
    expect(r.container.querySelector('.session-footer-offline')).toBeNull()
    const calls = r.calls.length
    const mark = (marker: string, ts: string) => ({ id: `marker:${marker}`, version: 1, ts, kind: 'marker', marker })

    act(() => r.streams[0].event('item', mark('host_offline', '2026-10-02T10:05:00.000Z')))
    await waitFor(
      () => expect(text(r.container.querySelector('.session-footer-offline'))).toBe('The host is offline: resume once it is back.'),
      WAIT,
    )

    act(() => r.streams[0].event('item', mark('host_back', '2026-10-02T10:07:00.000Z')))
    await waitFor(() => expect(r.container.querySelector('.session-footer-offline')).toBeNull(), WAIT)
    expect(resumeButton()).toBeEnabled()
    expect(r.calls.length).toBe(calls)
  })

  it('a starting session shows a spinner, in words, and no Resume', async () => {
    const r = await show({ lifecycle: 'starting' })
    const bar = footer(r)
    expect(bar).toHaveAttribute('role', 'status')
    expect(text(bar)).toBe('The session is starting…')
    expect(bar?.querySelector('.spinner')).toHaveAttribute('aria-hidden', 'true')
    expect(screen.queryByRole('button', { name: 'Resume' })).toBeNull()
  })

  it('an active session has no footer', async () => {
    const r = await show({ lifecycle: 'active' })
    expect(footer(r)).toBeNull()
    expect(screen.getByLabelText('Prompt')).toBeInTheDocument()
  })

  it('a failed session the agent has no record of offers a new session in its project, not a resume', async () => {
    const r = await show({ lifecycle: 'failed', failure_reason: 'agent_has_no_record', cwd: '/srv/work/a b&c' })
    expect(text(footer(r)?.querySelector('.txt'))).toBe('This session failed: the agent has no record of this session.')
    const link = screen.getByRole('link', { name: 'Start a new session in this project' })
    expect(link).toHaveAttribute('href', '/new?host=h1&cwd=%2Fsrv%2Fwork%2Fa+b%26c')
    expect(screen.queryByRole('button', { name: 'Resume' })).toBeNull()
  })

  it.each([
    ['claude', 'Run `claude` in a terminal there and sign in (`/login`), then resume.'],
    ['codex', 'Run `codex login` in a terminal there, then resume.'],
    ['my-agent', 'Sign the agent’s command-line tool in there, then resume. `hennery doctor` on that host checks it.'],
  ])('a failed %s session that is not logged in says how to log it in on its host, then offers Resume', async (agent, steps) => {
    const r = await show({ lifecycle: 'failed', failure_reason: 'agent_not_logged_in', agent })
    const lines = [...(footer(r)?.querySelectorAll('.txt') ?? [])].map(text)
    expect(lines).toEqual([
      'This session failed: the agent is not logged in on the host.',
      `The agent is not logged in on build-box. ${steps}`,
    ])
    expect(footer(r)?.querySelector('bdi')?.textContent).toBe('build-box')
    expect(resumeButton()).toBeEnabled()
  })

  it.each([
    ['start_failed', 'This session failed: the agent could not start.'],
    ['start_not_delivered', 'This session failed: the start never reached the host.'],
    ['some_new_reason', 'This session failed: some_new_reason.'],
    [undefined, 'This session failed.'],
  ])('a failed session (%s) says why, and offers Resume', async (reason, words) => {
    const r = await show({ lifecycle: 'failed', failure_reason: reason })
    expect(text(footer(r)?.querySelector('.txt'))).toBe(words)
    expect(resumeButton()).toBeEnabled()
  })

  it('a reason it does not know is shown as sent, isolated in a <bdi>; a known one is not', async () => {
    const r = await show({ lifecycle: 'failed', failure_reason: 'סיבה_חדשה' })
    const said = footer(r)?.querySelector('.txt')
    expect(text(said)).toBe('This session failed: סיבה_חדשה.')
    expect(said?.querySelector('bdi')?.textContent).toBe('סיבה_חדשה')
    r.unmount()
    const known = await show({ lifecycle: 'failed', failure_reason: 'start_failed' })
    expect(footer(known)?.querySelector('.txt bdi')).toBeNull()
  })

  it('a reason named like an Object member (constructor) is shown as sent', async () => {
    const r = await show({ lifecycle: 'failed', failure_reason: 'constructor' })
    expect(text(footer(r)?.querySelector('.txt'))).toBe('This session failed: constructor.')
    expect(footer(r)?.querySelector('.txt bdi')?.textContent).toBe('constructor')
  })
})

describe('SessionView: a refused resume, in words', () => {
  it('hat_mismatch names both hats by name, and links to Hats', async () => {
    const r = await refused(json({ code: 'hat_mismatch', message: 'the session belongs to Work, but …' }, 409), { lifecycle: 'parked' }, {
      hats: () => json([{ id: 'hat1', name: 'Work' }, { id: 'hat2', name: 'Home' }]),
      resolve: () => json({ canonical: '/srv/work/project', exists: true, is_dir: true, hat_id: 'hat2' }),
    })
    await waitFor(
      () =>
        expect(text(r.alert)).toBe(
          'This session belongs to Work, but its directory now belongs to Home. Change the path rules in Hats, then resume.',
        ),
      WAIT,
    )
    expect([...r.alert.querySelectorAll('bdi')].map((b) => b.textContent)).toEqual(['Work', 'Home'])
    expect(screen.getByRole('link', { name: 'Hats' })).toHaveAttribute('href', '/hats')
    expect(r.of('/api/hats/resolve')).toHaveLength(1)
    expect(JSON.parse(String(r.fetch.mock.calls.find((c) => String(c[0]) === '/api/hats/resolve')?.[1]?.body))).toEqual({
      host_id: 'h1',
      path: '/srv/work/project',
    })
  })

  it('hat_mismatch names the hats by id when their names are not known', async () => {
    const r = await refused(json({ code: 'hat_mismatch', message: 'm' }, 409), { lifecycle: 'parked' }, {
      hats: () => json({ code: 'internal', message: 'no' }, 500),
      resolve: () => json({ canonical: '/srv/work/project', exists: true, is_dir: true, hat_id: 'hat2' }),
    })
    await waitFor(() => expect(text(r.alert)).toContain('now belongs to hat2.'), WAIT)
    expect([...r.alert.querySelectorAll('bdi')].map((b) => b.textContent)).toEqual(['hat1', 'hat2'])
  })

  it('hat_mismatch says "another hat" when the directory’s hat cannot be resolved, and "no hat" for a session from before hats', async () => {
    const r = await refused(json({ code: 'hat_mismatch', message: 'm' }, 409), { lifecycle: 'parked', hat_id: '' })
    await waitFor(() => expect(r.of('/api/hats/resolve')).toHaveLength(1), WAIT)
    expect(text(r.alert)).toBe(
      'This session belongs to no hat, but its directory now belongs to another hat. Change the path rules in Hats, then resume.',
    )
  })

  it('agent_has_no_record offers a new session in the project', async () => {
    const r = await refused(json({ code: 'agent_has_no_record', message: 'm' }, 409))
    expect(text(r.alert)).toBe(
      'The agent has no record of this session, so it cannot be resumed. Start a new session in this project',
    )
    expect(screen.getByRole('link', { name: 'Start a new session in this project' })).toHaveAttribute(
      'href',
      '/new?host=h1&cwd=%2Fsrv%2Fwork%2Fproject',
    )
  })

  it('agent_not_logged_in (the host’s 502) says how to log in on the host', async () => {
    const r = await refused(json({ code: 'agent_not_logged_in', message: 'm' }, 502))
    expect(text(r.alert)).toBe(
      'The agent is not logged in on build-box. Run `claude` in a terminal there and sign in (`/login`), then resume.',
    )
  })

  it.each([
    ['cwd_moved', 409, 'The session’s directory now resolves to another place on its host: it cannot be resumed there. Start a new session in that directory instead.'],
    ['hat_ambiguous', 409, 'The session’s directory now matches the path rules of more than one hat: change the rules so that one hat claims it, then resume.'],
    ['host_offline', 409, 'The host is offline: resume once it is back.'],
    ['starting', 409, 'The session is already starting.'],
    ['active', 409, 'The session is already running.'],
    ['invalid_cwd', 400, 'The session’s directory is no longer a directory on its host.'],
    ['delivery_unknown', 503, 'The host went away while the session was resuming: its outcome shows when the host reconnects.'],
    ['load_unsupported', 502, 'The agent cannot open an earlier session, so this one cannot be resumed.'],
    ['start_failed', 502, 'The agent could not start.'],
  ] as const)('%s (%i) is put in words', async (code, status, words) => {
    const r = await refused(json({ code, message: `server words for ${code}` }, status))
    expect(text(r.alert)).toBe(words)
    expect(resumeButton()).toBeEnabled()
  })

  it('a code it does not know shows the server’s message, as text', async () => {
    const r = await refused(json({ code: 'something_new', message: 'the host said <b>no</b>' }, 409))
    expect(text(r.alert)).toBe('the host said <b>no</b>')
    expect(r.alert.querySelector('b')).toBeNull()
  })

  it('a code named like an Object property is the server’s, not a wording', async () => {
    const r = await refused(json({ code: 'constructor', message: 'odd' }, 409))
    expect(text(r.alert)).toBe('odd')
  })

  it('a resume tried again clears the refusal before it', async () => {
    let n = 0
    await show({ lifecycle: 'parked' }, { resume: () => (n++ === 0 ? json({ code: 'host_offline', message: 'm' }, 409) : new Promise<Response>(() => {})) })
    fireEvent.click(resumeButton())
    await screen.findByRole('alert', {}, WAIT)
    fireEvent.click(resumeButton())
    await waitFor(() => expect(screen.getByRole('button', { name: 'Resuming…' })).toBeDisabled())
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('a refusal goes when the lifecycle moves on', async () => {
    const s = sessionServer({ detail: { lifecycle: 'parked' }, resume: () => json({ code: 'host_offline', message: 'm' }, 409) })
    const summary = (lifecycle: string) =>
      ({
        session_id: 's1',
        host_id: 'h1',
        agent: 'claude',
        cwd: '/srv/work/project',
        hat_id: 'hat1',
        lifecycle,
        presumed_parked: false,
        created_at: '2026-10-02T09:00:00.000Z',
        last_event_at: '2026-10-02T10:00:00.000Z',
        question_waits: false,
      }) as const
    const r = render(<SessionView id="s1" summary={summary('parked')} timing={FAST} />, { wrapper: s.wrapper })
    fireEvent.click(await screen.findByRole('button', { name: 'Resume' }, WAIT))
    await screen.findByRole('alert', {}, WAIT)
    r.rerender(<SessionView id="s1" summary={summary('closed')} timing={FAST} />)
    await waitFor(() => expect(text(r.container.querySelector('.session-footer .txt'))).toBe('This session is closed.'))
    expect(screen.queryByRole('alert')).toBeNull()
  })
})
