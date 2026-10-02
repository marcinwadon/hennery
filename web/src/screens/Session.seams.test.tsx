// The seams the question cards reach the composer through, from the view:
// the transcript is replaced by one that keeps the env it is given.
import '@testing-library/jest-dom/vitest'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ItemEnv } from '../components/items/types'
import type { Item } from '../generated/view'
import { forgetAllAttachments } from '../lib/attachments'
import SessionView from './Session'
import { FAST, message, sessionServer } from './test-session'

const seen = vi.hoisted(() => ({ envs: [] as unknown[] }))

vi.mock('../components/Transcript', () => ({
  default: ({ items, env }: { items: Item[]; env: unknown }) => {
    seen.envs.push(env)
    return (
      <ul>
        {items.map((i) => (
          <li key={i.id}>{i.id}</li>
        ))}
      </ul>
    )
  },
}))

const WAIT = { timeout: 5000 }
const textarea = () => screen.getByLabelText('Prompt') as HTMLTextAreaElement
const lastEnv = () => seen.envs[seen.envs.length - 1] as ItemEnv

beforeEach(() => {
  seen.envs = []
  sessionStorage.clear()
  forgetAllAttachments()
  URL.createObjectURL = vi.fn(() => 'blob:u')
  URL.revokeObjectURL = vi.fn()
})

describe('SessionView: the seams of the question cards', () => {
  it('onAnswerAsMessage puts the question in the draft, labelled, the cursor at its end, and sends nothing', async () => {
    const s = sessionServer({ items: () => [message('m1', 't1')] })
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('m1')
    act(() => lastEnv().onAnswerAsMessage?.('Which branch'))
    const want = 'You asked: Which branch. My answer: '
    await waitFor(() => expect(textarea().value).toBe(want))
    expect(screen.getByText('Answering a question as a new message')).toBeInTheDocument()
    expect(document.activeElement).toBe(textarea())
    expect(textarea().selectionStart).toBe(want.length)
    expect(textarea().selectionEnd).toBe(want.length)
    expect(s.posted('/prompt')).toEqual([])
  })

  it('the label goes once the draft is emptied', async () => {
    const s = sessionServer()
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await waitFor(() => expect(seen.envs.length).toBeGreaterThan(0), WAIT)
    act(() => lastEnv().onAnswerAsMessage?.('Which branch'))
    await screen.findByText('Answering a question as a new message')
    fireEvent.change(textarea(), { target: { value: '' } })
    expect(screen.queryByText('Answering a question as a new message')).toBeNull()
  })

  it('composerEmpty says whether the draft holds text or an image', async () => {
    const s = sessionServer()
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await waitFor(() => expect(seen.envs.length).toBeGreaterThan(0), WAIT)
    expect(lastEnv().composerEmpty?.()).toBe(true)
    fireEvent.change(textarea(), { target: { value: '   ' } })
    expect(lastEnv().composerEmpty?.()).toBe(true)
    fireEvent.change(textarea(), { target: { value: 'half an answer' } })
    expect(lastEnv().composerEmpty?.()).toBe(false)
    fireEvent.change(textarea(), { target: { value: '' } })
    expect(lastEnv().composerEmpty?.()).toBe(true)
    const file = new File(['x'], 'a.png', { type: 'image/png' })
    fireEvent.paste(textarea(), { clipboardData: { items: [{ kind: 'file', type: file.type, getAsFile: () => file }] } })
    await waitFor(() => expect(textarea().value).toBe('[Image #1] '))
    // Only an image, its marker taken out of the text: still a draft.
    fireEvent.change(textarea(), { target: { value: '' } })
    expect(lastEnv().composerEmpty?.()).toBe(false)
  })

  it('the env stays the same object while the view re-renders for other reasons', async () => {
    const s = sessionServer({ items: () => [message('m1', 't1')] })
    render(<SessionView id="s1" timing={FAST} />, { wrapper: s.wrapper })
    await screen.findByText('m1')
    await screen.findByText('build-box', {}, WAIT)
    await screen.findByRole('combobox', { name: 'Model' }, WAIT)
    const before = lastEnv()
    const renders = seen.envs.length
    act(() => s.streams[0].event('item', message('m2', 't1')))
    await screen.findByText('m2')
    fireEvent.change(textarea(), { target: { value: 'typing re-renders the composer only' } })
    expect(seen.envs.length).toBeGreaterThan(renders)
    expect(lastEnv()).toBe(before)
    expect(typeof before.onSendAgain).toBe('function')
    expect(typeof before.onAnswerAsMessage).toBe('function')
    expect(typeof before.composerEmpty).toBe('function')
  })
})
