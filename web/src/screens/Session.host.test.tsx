// Whether a question card says its answer "delivers when the host
// reconnects" follows the host while the view is open (decision 22): the
// view's one hosts request seeds it, and then the item stream's host
// markers and the summary's `presumed_parked` say it. No event fetches.
import '@testing-library/jest-dom/vitest'
import { act, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { SessionSummary } from '../generated/view'
import type { Item } from '../generated/view'
import { forgetAllAttachments } from '../lib/attachments'
import { json } from '../test-stream'
import SessionView from './Session'
import { FAST, message, sessionServer } from './test-session'

const TS = '2026-10-02T10:00:00.000Z'
const NOTE = 'delivers when the host reconnects'
const WAIT = { timeout: 5000 }

const sent: Item = {
  id: 'question:p1',
  version: 5,
  ts: TS,
  turn_id: 't1',
  kind: 'question',
  pending_id: 'p1',
  question_kind: 'permission',
  request: { type: 'permission', title: 'Run it?', options: [{ option_id: 'a', name: 'Allow', option_kind: 'allow_once' }] },
  answerable: false,
  state: 'open',
  answered: true,
} as Item

const hostMarker = (kind: string, ts: string): Item => ({ id: `marker:${kind}:${ts}`, version: 1, ts, kind: 'marker', marker: kind }) as Item

function server(connected: boolean, items: Item[] = [message('m1', 't1'), sent]) {
  return sessionServer({
    items: () => items,
    hosts: () => json([{ host_id: 'h1', name: 'build-box', connected, capabilities: [] }]),
  })
}

/** Once the view has made every request it makes on opening. */
async function settled(s: ReturnType<typeof server>) {
  await screen.findByText('Sent', {}, WAIT)
  await screen.findByText('build-box', {}, WAIT)
  await waitFor(() => expect(s.streams).toHaveLength(1), WAIT)
  await waitFor(() => expect(s.of('/api/sessions/s1/catalog')).toHaveLength(1), WAIT)
}

beforeEach(() => {
  sessionStorage.clear()
  forgetAllAttachments()
  URL.createObjectURL = vi.fn(() => 'blob:u')
  URL.revokeObjectURL = vi.fn()
})

describe('SessionView: the host’s reach on a question card', () => {
  it('a host_offline marker on the stream turns the note on, and host_back turns it off, with no request', async () => {
    const s = server(true)
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await settled(s)
    expect(screen.queryByText(NOTE, { exact: true })).toBeNull()
    const calls = s.calls.length

    act(() => s.streams[0].event('item', hostMarker('host_offline', '2026-10-02T10:05:00.000Z')))
    expect(await screen.findByText(NOTE, { exact: true }, WAIT)).toBeInTheDocument()

    act(() => s.streams[0].event('item', hostMarker('host_back', '2026-10-02T10:07:00.000Z')))
    await waitFor(() => expect(screen.queryByText(NOTE, { exact: true })).toBeNull(), WAIT)
    expect(screen.getByText('Sent', { exact: true })).toBeInTheDocument()
    expect(s.calls.length).toBe(calls)
  })

  it('a host the hosts list said was not connected comes back by host_restarted, with no request', async () => {
    const s = server(false)
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await settled(s)
    expect(await screen.findByText(NOTE, { exact: true }, WAIT)).toBeInTheDocument()
    const calls = s.calls.length

    act(() => s.streams[0].event('item', hostMarker('host_restarted', '2026-10-02T10:05:00.000Z')))
    await waitFor(() => expect(screen.queryByText(NOTE, { exact: true })).toBeNull(), WAIT)
    expect(s.calls.length).toBe(calls)
  })

  it('a host_back already in the first page does not hide a host the hosts list says is not connected', async () => {
    const s = server(false, [hostMarker('host_back', '2026-10-02T09:30:00.000Z'), message('m1', 't1'), sent])
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await settled(s)
    expect(await screen.findByText(NOTE, { exact: true }, WAIT)).toBeInTheDocument()
  })

  it('the summary’s presumed_parked, as a session_upsert changes it, turns the note on and off, with no request', async () => {
    const s = server(true)
    const summary = (presumed_parked: boolean): SessionSummary =>
      ({
        session_id: 's1',
        host_id: 'h1',
        agent: 'claude',
        cwd: '/srv/work/project',
        hat_id: 'hat1',
        lifecycle: presumed_parked ? 'parked' : 'active',
        activity: presumed_parked ? undefined : 'idle',
        presumed_parked,
        created_at: '2026-10-02T09:00:00.000Z',
        last_event_at: TS,
        question_waits: true,
      }) as SessionSummary
    const r = render(<SessionView id="s1" summary={summary(false)} timing={FAST} />, { wrapper: s.wrapper })
    await settled(s)
    expect(screen.queryByText(NOTE, { exact: true })).toBeNull()
    const calls = s.calls.length

    r.rerender(<SessionView id="s1" summary={summary(true)} timing={FAST} />)
    expect(await screen.findByText(NOTE, { exact: true }, WAIT)).toBeInTheDocument()

    r.rerender(<SessionView id="s1" summary={summary(false)} timing={FAST} />)
    await waitFor(() => expect(screen.queryByText(NOTE, { exact: true })).toBeNull(), WAIT)
    expect(s.calls.length).toBe(calls)
  })
})
