import '@testing-library/jest-dom/vitest'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { SessionDetail } from '../generated/protocol'
import { forgetAllAttachments, heldFor, hold } from '../lib/attachments'
import { loadDraft, saveDraft } from '../lib/drafts'
import { json } from '../test-stream'
import SessionView from './Session'
import { FAST, sessionServer, type Opts } from './test-session'

const WAIT = { timeout: 5000 }

beforeEach(() => {
  sessionStorage.clear()
  forgetAllAttachments()
})

const PARK_HOST = () => json([{ host_id: 'h1', name: 'build-box', capabilities: ['park'], connected: true }])

/** Session `s1` as its detail says; its host announces `park` unless the
 *  options say otherwise. */
async function show(detail: Partial<SessionDetail> = {}, opts: Opts = {}, onRemoved = vi.fn()) {
  const s = sessionServer({ detail, hosts: PARK_HOST, ...opts })
  const r = render(<SessionView id="s1" timing={FAST} onRemoved={onRemoved} />, { wrapper: s.wrapper })
  await waitFor(() => expect(screen.getByLabelText('Host')).toHaveTextContent('build-box'), WAIT)
  await waitFor(() => expect(s.streams).toHaveLength(1), WAIT)
  return { ...s, ...r, onRemoved }
}

const trigger = () => screen.getByRole('button', { name: 'Session actions' })
const entries = () =>
  [...(document.querySelector('.session-menu-list')?.querySelectorAll('button') ?? [])].map((b) => b.textContent)
const text = (el: Element | null | undefined) => el?.textContent?.replace(/\s+/g, ' ').trim()

function openMenu() {
  fireEvent.click(trigger())
  expect(trigger()).toHaveAttribute('aria-expanded', 'true')
}

function pick(name: string) {
  openMenu()
  fireEvent.click(screen.getByRole('button', { name }))
}

describe('SessionView: the header menu', () => {
  it('an active session on a host that parks offers Park, Close and Delete', async () => {
    await show()
    expect(trigger()).toHaveAttribute('aria-expanded', 'false')
    expect(document.querySelector('.session-menu-list')).toBeNull()
    openMenu()
    expect(entries()).toEqual(['Park', 'Close', 'Delete session'])
    // Opened from a click or a key, focus goes to its first entry.
    expect(screen.getByRole('button', { name: 'Park' })).toHaveFocus()
  })

  it('offers no Park when the host does not announce it', async () => {
    await show({}, { hosts: () => json([{ host_id: 'h1', name: 'build-box', capabilities: ['images'] }]) })
    openMenu()
    expect(entries()).toEqual(['Close', 'Delete session'])
  })

  it('offers no Park while the host’s capabilities are unknown', async () => {
    await show({}, { hosts: () => json([{ host_id: 'h1', name: 'build-box' }]) })
    openMenu()
    expect(entries()).toEqual(['Close', 'Delete session'])
  })

  it.each([
    ['parked', ['Close', 'Delete session']],
    ['starting', ['Close', 'Delete session']],
    ['failed', ['Close', 'Delete session']],
    ['closed', ['Delete session']],
  ])('a %s session offers no Park; a closed one no Close', async (lifecycle, offered) => {
    await show({ lifecycle })
    openMenu()
    expect(entries()).toEqual(offered)
  })

  it('Park parks the session, and the menu closes with focus back on its button', async () => {
    const r = await show()
    pick('Park')
    await waitFor(() => expect(r.changes()).toEqual(['POST /api/sessions/s1/park']), WAIT)
    expect(document.querySelector('.session-menu-list')).toBeNull()
    expect(trigger()).toHaveFocus()
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it.each([
    ['not_attached', 'The session is not running, or its host is not ready: there is nothing to park.'],
    ['park_unsupported', 'This host cannot park sessions: update hennery on it.'],
  ])('a park refused with %s says why', async (code, words) => {
    await show({}, { park: () => json({ code, message: 'm' }, 409) })
    pick('Park')
    expect(text(await screen.findByRole('alert', {}, WAIT))).toBe(words)
  })

  it('Close closes the session', async () => {
    const r = await show({ lifecycle: 'parked' })
    pick('Close')
    await waitFor(() => expect(r.changes()).toEqual(['POST /api/sessions/s1/close']), WAIT)
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it.each([
    ['starting', 409, 'The session is still starting: close it once the start settles.'],
    ['delivery_unknown', 503, 'The host went away before it confirmed the close: the session closes when the host is back.'],
  ] as const)('a close refused with %s says why', async (code, status, words) => {
    await show({ lifecycle: 'starting' }, { close: () => json({ code, message: 'server words' }, status) })
    pick('Close')
    expect(text(await screen.findByRole('alert', {}, WAIT))).toBe(words)
  })

  it('while a Park runs, no entry starts another action: Delete is disabled too', async () => {
    let answer: (r: Response) => void = () => {}
    const r = await show({}, { park: () => new Promise<Response>((done) => (answer = done)) })
    pick('Park')
    await waitFor(() => expect(r.changes()).toEqual(['POST /api/sessions/s1/park']), WAIT)
    openMenu()
    for (const name of ['Park', 'Close', 'Delete session']) expect(screen.getByRole('button', { name })).toBeDisabled()
    await act(async () => answer(json({ session_id: 's1', lifecycle: 'parked' }, 202)))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Delete session' })).toBeEnabled(), WAIT)
  })

  it('Escape closes the menu and gives focus back to its button', async () => {
    await show()
    openMenu()
    fireEvent.keyDown(screen.getByRole('button', { name: 'Close' }), { key: 'Escape' })
    expect(document.querySelector('.session-menu-list')).toBeNull()
    expect(trigger()).toHaveAttribute('aria-expanded', 'false')
    expect(trigger()).toHaveFocus()
  })

  it('a click outside closes the menu', async () => {
    await show()
    openMenu()
    fireEvent.mouseDown(document.body)
    expect(document.querySelector('.session-menu-list')).toBeNull()
  })

  it('a click inside keeps it open, and the button toggles it', async () => {
    await show()
    openMenu()
    fireEvent.mouseDown(document.querySelector('.session-menu-list')!)
    expect(document.querySelector('.session-menu-list')).not.toBeNull()
    fireEvent.click(trigger())
    expect(document.querySelector('.session-menu-list')).toBeNull()
  })
})

describe('SessionView: deleting the session', () => {
  async function confirmDelete(r: Awaited<ReturnType<typeof show>>) {
    pick('Delete session')
    const dialog = await screen.findByRole('dialog', { name: 'Delete this session?' })
    expect(document.querySelector('.session-menu-list')).toBeNull()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }))
    return { dialog, r }
  }

  it('asks first, then deletes; the view says so, the list drops it, and its draft and images go', async () => {
    saveDraft('s1', 'half written')
    hold('s1', { attachments: [{ n: 1, file: new File(['x'], 'a.png', { type: 'image/png' }) }], nextN: 2 })
    const r = await show()
    await confirmDelete(r)
    expect(await screen.findByRole('heading', { name: 'This session was deleted' }, WAIT)).toBeInTheDocument()
    expect(r.changes()).toEqual(['DELETE /api/sessions/s1'])
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(document.querySelector('.delete-note')).toBeNull()
    await waitFor(() => expect(r.onRemoved).toHaveBeenCalledWith('s1'))
    expect(loadDraft('s1')).toBe('')
    expect(heldFor('s1').attachments).toEqual([])
    // Nothing more is read of it: its stream is closed.
    expect(r.streams[0].cancelled).toBe(true)
    // The menu that opened the dialog is gone: focus goes to the way back.
    await waitFor(() => expect(screen.getByRole('link', { name: 'Back to sessions' })).toHaveFocus())
  })

  it('while a delete runs, Delete is disabled: the menu cannot start a second one', async () => {
    let answer: (r: Response) => void = () => {}
    const r = await show({}, { remove: () => new Promise<Response>((done) => (answer = done)) })
    await confirmDelete(r)
    await waitFor(() => expect(r.changes()).toEqual(['DELETE /api/sessions/s1']), WAIT)
    // The menu behind the dialog, reached anyway.
    fireEvent.click(trigger())
    expect(screen.getByRole('button', { name: 'Delete session' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Close' })).toBeDisabled()
    await act(async () => answer(json({ code: 'starting', message: 'm' }, 409)))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Delete session' })).toBeEnabled(), WAIT)
  })

  it('Cancel deletes nothing, and focus goes back to the menu’s button', async () => {
    const r = await show()
    pick('Delete session')
    const dialog = await screen.findByRole('dialog', { name: 'Delete this session?' })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(r.changes()).toEqual([])
    expect(trigger()).toHaveFocus()
  })

  it('the step-up is the client’s: a 403 step_up_required is stepped up and sent once more', async () => {
    let n = 0
    const r = await show(
      {},
      {
        remove: () =>
          n++ === 0
            ? json({ code: 'step_up_required', message: 'm' }, 403)
            : json({ host_transcript: { state: 'removed', remaining: [], notes: [] } }),
      },
    )
    await confirmDelete(r)
    expect(await screen.findByRole('heading', { name: 'This session was deleted' }, WAIT)).toBeInTheDocument()
    expect(r.changes()).toEqual(['DELETE /api/sessions/s1', 'DELETE /api/sessions/s1'])
  })

  it.each([
    [
      'partial',
      { state: 'partial', remaining: [{ kind: 'transcript', count: 2, reason: 'denied' }], notes: [] },
      'The agent’s own transcript on the host was removed only in part: 2 entries are left there.',
    ],
    [
      'pending (host offline)',
      { state: 'pending', pending: 'host_offline', remaining: [], notes: [] },
      'The agent’s own transcript on the host is not removed yet: the host is offline; it is removed when the host is back.',
    ],
    [
      'pending (attached)',
      { state: 'pending', pending: 'attached', remaining: [], notes: [] },
      'The agent’s own transcript on the host is not removed yet: the agent still has it open; it is tried again when the host next connects.',
    ],
    [
      'pending (no reason)',
      { state: 'pending', remaining: [], notes: [] },
      'The agent’s own transcript on the host is not removed yet: it is tried again when the host next connects.',
    ],
  ])('a delete whose transcript removal is %s says so', async (_, transcript, words) => {
    const r = await show({}, { remove: () => json({ host_transcript: transcript }) })
    await confirmDelete(r)
    await screen.findByRole('heading', { name: 'This session was deleted' }, WAIT)
    expect([...document.querySelectorAll('p.delete-note')].map(text)).toEqual([words])
  })

  it('shows the notes the host gave, as text', async () => {
    const r = await show(
      {},
      { remove: () => json({ host_transcript: { state: 'removed', remaining: [], notes: ['<b>logs</b> are kept'] } }) },
    )
    await confirmDelete(r)
    await screen.findByRole('heading', { name: 'This session was deleted' }, WAIT)
    const notes = document.querySelector('ul.delete-note')
    expect(text(notes)).toBe('<b>logs</b> are kept')
    expect(notes?.querySelector('b')).toBeNull()
  })

  it('a server that answers 204 deletes with nothing to say', async () => {
    const r = await show({}, { remove: () => new Response(null, { status: 204 }) })
    await confirmDelete(r)
    await screen.findByRole('heading', { name: 'This session was deleted' }, WAIT)
    expect(document.querySelector('.delete-note')).toBeNull()
  })

  it('a session gone already (404) shows as deleted', async () => {
    const r = await show({}, { remove: () => json({ code: 'not_found', message: 'no such session' }, 404) })
    await confirmDelete(r)
    expect(await screen.findByRole('heading', { name: 'This session was deleted' }, WAIT)).toBeInTheDocument()
    await waitFor(() => expect(r.onRemoved).toHaveBeenCalledWith('s1'))
  })

  it.each([
    ['starting', 409, 'The session is still starting: delete it once the start settles.'],
    ['delivery_unknown', 503, 'The host went away before it confirmed the close, so nothing was deleted: try again once the host is back.'],
    ['active', 409, 'The session changed while it was being deleted, so nothing was deleted: try again.'],
    ['parked', 409, 'The session changed while it was being deleted, so nothing was deleted: try again.'],
    ['closed', 409, 'The session changed while it was being deleted, so nothing was deleted: try again.'],
    ['failed', 409, 'The session changed while it was being deleted, so nothing was deleted: try again.'],
  ] as const)('a delete refused with %s says why in the dialog, and deletes nothing', async (code, status, words) => {
    const r = await show({}, { remove: () => json({ code, message: 'm' }, status) })
    const { dialog } = await confirmDelete(r)
    expect(text(await within(dialog).findByRole('alert', {}, WAIT))).toBe(words)
    expect(screen.getByRole('dialog')).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'This session was deleted' })).toBeNull()
    expect(r.onRemoved).not.toHaveBeenCalled()
  })

  it('says what the delete left on the host even when the stream said the session was removed first', async () => {
    let answer: (r: Response) => void = () => {}
    const r = await show({}, { remove: () => new Promise<Response>((done) => (answer = done)) })
    await confirmDelete(r)
    await waitFor(() => expect(r.changes()).toEqual(['DELETE /api/sessions/s1']), WAIT)
    act(() => r.streams[0].event('session_removed', { session_id: 's1' }))
    await screen.findByRole('heading', { name: 'This session was deleted' }, WAIT)
    await act(async () => answer(json({ host_transcript: { state: 'pending', pending: 'no_reply', remaining: [], notes: [] } })))
    await waitFor(() =>
      expect([...document.querySelectorAll('p.delete-note')].map(text)).toEqual([
        'The agent’s own transcript on the host is not removed yet: the host did not answer in time; it is tried again when the host next connects.',
      ]),
    )
  })

  it('a session the stream says was removed leaves the list too', async () => {
    const r = await show()
    act(() => r.streams[0].event('session_removed', { session_id: 's1' }))
    await screen.findByRole('heading', { name: 'This session was deleted' }, WAIT)
    await waitFor(() => expect(r.onRemoved).toHaveBeenCalledWith('s1'))
  })
})
