// A session the New Session screen opened with a notice: the notice shows
// above the composer, the link is cleaned so a reload does not show it
// again, and the first prompt that was not sent is the composer's draft.
import '@testing-library/jest-dom/vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { forgetAllAttachments } from '../lib/attachments'
import SessionView from './Session'
import { FAST, sessionServer } from './test-session'

const HOST_OFFLINE = 'The host went offline before the first prompt reached it; send it again once it is back. It waits as this session’s draft.'
const textarea = () => screen.getByLabelText('Prompt') as HTMLTextAreaElement

beforeEach(() => {
  sessionStorage.clear()
  forgetAllAttachments()
  URL.createObjectURL = vi.fn(() => 'blob:u')
  URL.revokeObjectURL = vi.fn()
  history.replaceState(null, '', '/')
})

describe('SessionView: the notice a start leaves', () => {
  it('shows a refused first prompt’s notice above the composer, and cleans the link without a new history entry', async () => {
    history.replaceState(null, '', '/sessions/s1?notice=prompt_failed&code=host_offline')
    const entries = history.length
    const s = sessionServer()
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    const notice = await screen.findByText(HOST_OFFLINE)
    expect(notice.compareDocumentPosition(textarea()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    await waitFor(() => expect(location.search).toBe(''))
    expect(location.pathname).toBe('/sessions/s1')
    expect(history.length).toBe(entries)
    // The link is clean: the notice stays for as long as the view does.
    expect(screen.getByText(HOST_OFFLINE)).toBeInTheDocument()
  })

  it('says a start whose delivery is unknown may still start', async () => {
    history.replaceState(null, '', '/sessions/s1?notice=start_unknown')
    const s = sessionServer()
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    expect(await screen.findByText(/it may still start/)).toBeInTheDocument()
  })

  it('shows nothing once the link is clean: a reload does not show it again', async () => {
    history.replaceState(null, '', '/sessions/s1?notice=prompt_failed&code=host_offline')
    const s = sessionServer()
    const first = render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await waitFor(() => expect(location.search).toBe(''))
    first.unmount()
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByLabelText('Prompt')
    expect(screen.queryByText(HOST_OFFLINE)).toBeNull()
  })

  it('drops the notice when the view moves to another session', async () => {
    history.replaceState(null, '', '/sessions/s1?notice=prompt_failed&code=host_offline')
    const s = sessionServer()
    const view = render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText(HOST_OFFLINE)
    history.replaceState(null, '', '/sessions/s2')
    view.rerender(<SessionView id="s2" timing={FAST} />)
    await waitFor(() => expect(screen.queryByText(HOST_OFFLINE)).toBeNull())
  })

  it('opens the composer with the first prompt that was not sent', async () => {
    history.replaceState(null, '', '/sessions/s1?notice=prompt_failed&code=host_offline')
    // Where the New Session screen keeps it (its own test reads this key).
    sessionStorage.setItem('hennery.draft.s1', 'Fix the tests')
    const s = sessionServer()
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText(HOST_OFFLINE)
    expect(textarea().value).toBe('Fix the tests')
  })
})
