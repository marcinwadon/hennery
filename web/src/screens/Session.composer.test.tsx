import '@testing-library/jest-dom/vitest'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Item } from '../generated/view'
import { STILL_SENDING } from '../components/Composer'
import { forgetAllAttachments, heldFor, hold } from '../lib/attachments'
import { forgetAllSends } from '../lib/sending'
import { json } from '../test-stream'
import SessionView from './Session'
import { FAST, UNDELIVERED, catalogOf, message, sessionServer } from './test-session'

const WAIT = { timeout: 5000 }

const textarea = () => screen.getByLabelText('Prompt') as HTMLTextAreaElement
const type = (text: string) => fireEvent.change(textarea(), { target: { value: text } })
const send = () => fireEvent.click(screen.getByRole('button', { name: 'Send' }))

function png(name: string, body = name): File {
  return new File([body], name, { type: 'image/png' })
}

function paste(...files: File[]) {
  fireEvent.paste(textarea(), {
    clipboardData: { items: files.map((f) => ({ kind: 'file', type: f.type, getAsFile: () => f })) },
  })
}

const base64 = (s: string) => Buffer.from(s).toString('base64')

beforeEach(() => {
  sessionStorage.clear()
  forgetAllAttachments()
  forgetAllSends()
  URL.createObjectURL = vi.fn(() => 'blob:u')
  URL.revokeObjectURL = vi.fn()
})

/** The view as the shell mounts it: a new one per session id. */
function view(id: string) {
  return <SessionView key={id} id={id} timing={FAST} />
}

describe('SessionView: the composer', () => {
  it('sends a prompt to the session shown', async () => {
    const s = sessionServer({ items: (id) => [message(`${id}-m`, 't1')] })
    render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('s1-m')
    type('hello there')
    send()
    await waitFor(() => expect(s.posted('/prompt')).toHaveLength(1), WAIT)
    expect(s.posted('/prompt')).toEqual([
      { path: '/api/sessions/s1/prompt', body: { content: [{ type: 'text', text: 'hello there' }] } },
    ])
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
  })

  it('a config switch is replaced by the catalogue the 202 returns', async () => {
    const s = sessionServer({ config: (id) => json(catalogOf(id, 'c'), 202) })
    render(view('s1'), { wrapper: s.wrapper })
    const select = (await screen.findByRole('combobox', { name: 'Model' }, WAIT)) as HTMLSelectElement
    expect(select.value).toBe('a')
    fireEvent.change(select, { target: { value: 'b' } })
    await waitFor(() => expect(select.value).toBe('c'), WAIT)
    expect(s.posted('/config')).toEqual([{ path: '/api/sessions/s1/config', body: { config_id: 'model', value: 'b' } }])
    expect(select).not.toBeDisabled()
  })

  it('hides images on a host without the images capability, fetching the hosts once', async () => {
    const s = sessionServer({
      hosts: () =>
        json([
          { host_id: 'h0', name: 'other-box', capabilities: ['images'] },
          { host_id: 'h1', name: 'build-box', capabilities: ['park'] },
        ]),
    })
    render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('build-box', {}, WAIT)
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Attach images' })).toBeNull(), WAIT)
    expect(s.of('/api/hosts')).toHaveLength(1)
  })

  it('offers images while the host reports no list of capabilities', async () => {
    const s = sessionServer({ hosts: () => json([{ host_id: 'h1', name: 'build-box' }]) })
    render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('build-box', {}, WAIT)
    expect(screen.getByRole('button', { name: 'Attach images' })).toBeInTheDocument()
  })

  it('offers Cancel, not Send, while the session runs a turn', async () => {
    const s = sessionServer({ detail: { activity: 'running' } })
    render(view('s1'), { wrapper: s.wrapper })
    await screen.findByRole('button', { name: 'Cancel' }, WAIT)
    expect(screen.queryByRole('button', { name: 'Send' })).toBeNull()
  })

  it('offers images on a host with the images capability', async () => {
    const s = sessionServer({ hosts: () => json([{ host_id: 'h1', name: 'build-box', capabilities: ['images'] }]) })
    render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('build-box', {}, WAIT)
    expect(screen.getByRole('button', { name: 'Attach images' })).toBeInTheDocument()
  })

  it('switching sessions keeps each draft to its own session, and sends only the one shown (F-17)', async () => {
    const s = sessionServer({ items: (id) => [message(`${id}-m`, 't1')] })
    const r = render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('s1-m')
    type('first ')
    paste(png('one.png', 'ONE'))
    await waitFor(() => expect(textarea().value).toBe('first [Image #1] '))

    r.rerender(view('s2'))
    await screen.findByText('s2-m')
    expect(textarea().value).toBe('')
    expect(screen.queryByRole('list', { name: 'Images' })).toBeNull()
    type('second ')
    paste(png('two.png', 'TWO'))
    await waitFor(() => expect(textarea().value).toBe('second [Image #1] '))
    send()
    await waitFor(() => expect(s.posted('/prompt')).toHaveLength(1), WAIT)
    expect(s.posted('/prompt')).toEqual([
      {
        path: '/api/sessions/s2/prompt',
        body: {
          content: [
            { type: 'text', text: 'second ' },
            { type: 'image', mimeType: 'image/png', data: base64('TWO') },
          ],
        },
      },
    ])
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)

    r.rerender(view('s1'))
    await screen.findByText('s1-m')
    expect(textarea().value).toBe('first [Image #1] ')
    expect(within(screen.getByRole('list', { name: 'Images' })).getByAltText('Image #1')).toBeInTheDocument()
    expect(s.posted('/prompt')).toHaveLength(1)
  })

  it('hides the composer of a deleted session and drops its draft and images', async () => {
    sessionStorage.setItem('hennery.draft.s1', 'unsent [Image #1] ')
    hold('s1', { attachments: [{ n: 1, file: png('a.png') }], nextN: 2 })
    const s = sessionServer({ page: () => json({ code: 'not_found', message: 'gone' }, 404) })
    render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('This session was deleted', {}, WAIT)
    expect(screen.queryByLabelText('Prompt')).toBeNull()
    await waitFor(() => expect(sessionStorage.getItem('hennery.draft.s1')).toBeNull(), WAIT)
    expect(heldFor('s1').attachments).toEqual([])
  })
})

describe('SessionView: a send in flight when the composer is mounted again', () => {
  /** A response held until `answer` is called. */
  function held() {
    let answer!: (r: Response) => void
    const promise = new Promise<Response>((resolve) => (answer = resolve))
    return { respond: () => promise, answer: (r: Response) => act(async () => answer(r)) }
  }

  /** Away to s2 and back to s1, the composer mounted again. */
  async function awayAndBack(r: ReturnType<typeof render>) {
    r.rerender(view('s2'))
    await screen.findByText('s2-m')
    r.rerender(view('s1'))
    await screen.findByText('s1-m')
  }

  const items = (id: string) => [message(`${id}-m`, 't1')]

  it('stays read-only with Send disabled until the 202, which clears the draft it shows', async () => {
    const answer = held()
    const s = sessionServer({ items, prompt: answer.respond })
    const r = render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('s1-m')
    type('sent once')
    send()
    await waitFor(() => expect(s.posted('/prompt')).toHaveLength(1), WAIT)
    await awayAndBack(r)
    expect(textarea().value).toBe('sent once')
    expect(textarea().readOnly).toBe(true)
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled()
    // Work added meanwhile is refused, as in the composer that sent.
    paste(png('late.png'))
    expect(screen.getByRole('alert').textContent).toBe(STILL_SENDING)
    expect(screen.queryByRole('list', { name: 'Images' })).toBeNull()

    await answer.answer(json({ turn_id: 'new' }, 202))
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    expect(sessionStorage.getItem('hennery.draft.s1')).toBeNull()
    expect(textarea().readOnly).toBe(false)
    type('next')
    expect(sessionStorage.getItem('hennery.draft.s1')).toBe('next')
    expect(s.posted('/prompt')).toHaveLength(1)
  })

  it('a refusal after the composer was mounted again keeps the draft, editable, and says why', async () => {
    const answer = held()
    const s = sessionServer({ items, prompt: answer.respond })
    const r = render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('s1-m')
    type('kept')
    send()
    await waitFor(() => expect(s.posted('/prompt')).toHaveLength(1), WAIT)
    await awayAndBack(r)
    await answer.answer(json({ code: 'not_attached', message: 'srv-x' }, 409))
    expect(await screen.findByText('The session is not running: resume it to send this.', undefined, WAIT)).toBeInTheDocument()
    expect(textarea().readOnly).toBe(false)
    expect(textarea().value).toBe('kept')
    expect(sessionStorage.getItem('hennery.draft.s1')).toBe('kept')
    expect(screen.getByRole('button', { name: 'Send' })).not.toBeDisabled()
  })

  it('once the send has ended, a composer mounted again is not held', async () => {
    const s = sessionServer({ items })
    const r = render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('s1-m')
    type('first')
    send()
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    type('second')
    await awayAndBack(r)
    expect(textarea().readOnly).toBe(false)
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)))
    expect(textarea().value).toBe('second')
    expect(screen.getByRole('button', { name: 'Send' })).not.toBeDisabled()
  })

  it('a composer mounted again while Resume and send waits on the resume waits for the send too', async () => {
    const resumed = held()
    let attached = false
    const s = sessionServer({
      items,
      prompt: () => (attached ? json({ turn_id: 'new' }, 202) : json({ code: 'not_attached', message: 'srv-x' }, 409)),
      resume: () => resumed.respond().then((res) => ((attached = true), res)),
    })
    const r = render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('s1-m')
    type('later')
    send()
    fireEvent.click(await screen.findByRole('button', { name: 'Resume and send' }, WAIT))
    await waitFor(() => expect(s.of('/api/sessions/s1/resume')).toHaveLength(1), WAIT)
    await awayAndBack(r)
    expect(textarea().readOnly).toBe(true)
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled()
    await resumed.answer(json({ session_id: 's1', lifecycle: 'active' }, 202))
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    expect(textarea().readOnly).toBe(false)
    expect(s.posted('/prompt').map((p) => p.body)).toEqual([
      { content: [{ type: 'text', text: 'later' }] },
      { content: [{ type: 'text', text: 'later' }] },
    ])
  })
})

describe('SessionView: Send again', () => {
  const marker = {
    id: 'mk',
    version: 1,
    ts: '2026-10-02T10:00:00.000Z',
    turn_id: 't9',
    kind: 'marker',
    marker: 'turn_not_delivered',
    about_turn: 't9',
  } as Item

  it('puts the turn back in the draft with its images, sends nothing, then sends it as the operator does', async () => {
    const s = sessionServer({
      items: () => [message('m1', 't8'), marker],
      turn: () => json(UNDELIVERED),
      attachment: () => new Response('PNG!', { status: 200 }),
    })
    render(view('s1'), { wrapper: s.wrapper })
    fireEvent.click(await screen.findByRole('button', { name: 'Send again' }, WAIT))
    await waitFor(() => expect(textarea().value).toBe('look at [Image #1] and fix it'), WAIT)
    expect(screen.getByText(/The turn that was not delivered, back in the draft/)).toBeInTheDocument()
    expect(within(screen.getByRole('list', { name: 'Images' })).getByAltText('Image #1')).toBeInTheDocument()
    expect(s.of('/api/view/sessions/s1/turns/t9')).toHaveLength(1)
    expect(s.of('/api/attachments/ab%2Fc')).toHaveLength(1)
    expect(s.posted('/prompt')).toEqual([])

    send()
    await waitFor(() => expect(s.posted('/prompt')).toHaveLength(1), WAIT)
    expect(s.posted('/prompt')[0].body).toEqual({
      content: [
        { type: 'text', text: 'look at ' },
        { type: 'image', mimeType: 'image/png', data: base64('PNG!') },
        { type: 'text', text: ' and fix it' },
      ],
    })
    await waitFor(() => expect(screen.queryByText(/^The turn that was not delivered, back in the draft/)).toBeNull(), WAIT)
  })

  it('a marker written in the turn’s own text never names its image', async () => {
    const s = sessionServer({
      items: () => [marker],
      turn: () =>
        json({
          turn_id: 't9',
          content: [
            { type: 'text', text: 'see [Image #1] above' },
            { type: 'image', mimeType: 'image/png', sha256: 'ab/c', size: 4 },
          ],
        }),
      attachment: () => new Response('PNG!', { status: 200 }),
    })
    render(view('s1'), { wrapper: s.wrapper })
    fireEvent.click(await screen.findByRole('button', { name: 'Send again' }, WAIT))
    await waitFor(() => expect(textarea().value).toBe('see [Image #1] above [Image #2]'), WAIT)
    send()
    await waitFor(() => expect(s.posted('/prompt')).toHaveLength(1), WAIT)
    expect(s.posted('/prompt')[0].body).toEqual({
      content: [
        { type: 'text', text: 'see [Image #1] above ' },
        { type: 'image', mimeType: 'image/png', data: base64('PNG!') },
      ],
    })
  })

  it('an image attached after a refill takes the next number', async () => {
    const s = sessionServer({
      items: () => [marker],
      turn: () => json(UNDELIVERED),
      attachment: () => new Response('PNG!', { status: 200 }),
    })
    render(view('s1'), { wrapper: s.wrapper })
    fireEvent.click(await screen.findByRole('button', { name: 'Send again' }, WAIT))
    await waitFor(() => expect(textarea().value).toBe('look at [Image #1] and fix it'), WAIT)
    textarea().setSelectionRange(textarea().value.length, textarea().value.length)
    paste(png('more.png'))
    await waitFor(() => expect(textarea().value).toBe('look at [Image #1] and fix it[Image #2] '), WAIT)
  })

  it('a turn the image limits refuse in part leaves the draft as it was, saying why', async () => {
    hold('s1', { attachments: Array.from({ length: 19 }, (_, i) => ({ n: i + 1, file: png(`p${i}.png`) })), nextN: 20 })
    const s = sessionServer({
      items: () => [marker],
      turn: () =>
        json({
          turn_id: 't9',
          content: [
            { type: 'text', text: 'two' },
            { type: 'image', mimeType: 'image/png', sha256: 'ab/c', size: 4 },
            { type: 'image', mimeType: 'image/png', sha256: 'ab/c', size: 4 },
          ],
        }),
      attachment: () => new Response('PNG!', { status: 200 }),
    })
    render(view('s1'), { wrapper: s.wrapper })
    const again = await screen.findByRole('button', { name: 'Send again' }, WAIT)
    type('mine')
    fireEvent.click(again)
    await screen.findByText(/a prompt takes at most 20 images/, {}, WAIT)
    expect(textarea().value).toBe('mine')
    expect(heldFor('s1').attachments).toHaveLength(19)
  })

  it('a turn of only blanks adds nothing to the draft', async () => {
    const s = sessionServer({
      items: () => [marker],
      turn: () => json({ turn_id: 't9', content: [{ type: 'text', text: '  ' }] }),
    })
    render(view('s1'), { wrapper: s.wrapper })
    const again = await screen.findByRole('button', { name: 'Send again' }, WAIT)
    type('mine')
    fireEvent.click(again)
    await waitFor(() => expect(s.of('/api/view/sessions/s1/turns/t9')).toHaveLength(1), WAIT)
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)))
    expect(textarea().value).toBe('mine')
    expect(screen.queryByText(/^The turn that was not delivered, back in the draft/)).toBeNull()
  })

  it('keeps the draft there, adding the turn after it', async () => {
    const s = sessionServer({
      items: () => [marker],
      turn: () => json({ turn_id: 't9', content: [{ type: 'text', text: 'again' }] }),
    })
    render(view('s1'), { wrapper: s.wrapper })
    const again = await screen.findByRole('button', { name: 'Send again' }, WAIT)
    type('mine')
    fireEvent.click(again)
    await waitFor(() => expect(textarea().value).toBe('mine\n\nagain'), WAIT)
    expect(s.posted('/prompt')).toEqual([])
  })

  it('says why a turn cannot be put back, and leaves the draft as it was', async () => {
    const s = sessionServer({
      items: () => [marker],
      turn: () => json(UNDELIVERED),
      attachment: () => json({ code: 'not_found', message: 'no' }, 404),
    })
    render(view('s1'), { wrapper: s.wrapper })
    const again = await screen.findByRole('button', { name: 'Send again' }, WAIT)
    type('mine')
    fireEvent.click(again)
    await screen.findByText(/The turn could not be put back: An image of this prompt could not be read \(404\)/, {}, WAIT)
    expect(textarea().value).toBe('mine')
    expect(screen.queryByRole('list', { name: 'Images' })).toBeNull()
    expect(s.posted('/prompt')).toEqual([])
  })

  it('refuses a turn with images on a host that takes none, leaving the draft as it was', async () => {
    const s = sessionServer({
      items: () => [marker],
      hosts: () => json([{ host_id: 'h1', name: 'build-box', capabilities: [] }]),
      turn: () => json(UNDELIVERED),
      attachment: () => new Response('PNG!', { status: 200 }),
    })
    render(view('s1'), { wrapper: s.wrapper })
    await screen.findByText('build-box', {}, WAIT)
    fireEvent.click(await screen.findByRole('button', { name: 'Send again' }, WAIT))
    await screen.findByText('This turn holds images, and this host takes no images.', {}, WAIT)
    expect(textarea().value).toBe('')
  })

  it('a turn answering after the session was left lands in no draft', async () => {
    let answer!: (r: Response) => void
    const s = sessionServer({
      items: (id) => (id === 's1' ? [marker] : [message('s2-m', 't1')]),
      turn: () => new Promise<Response>((resolve) => (answer = resolve)),
      attachment: () => new Response('PNG!', { status: 200 }),
    })
    const r = render(view('s1'), { wrapper: s.wrapper })
    fireEvent.click(await screen.findByRole('button', { name: 'Send again' }, WAIT))
    await waitFor(() => expect(s.of('/api/view/sessions/s1/turns/t9')).toHaveLength(1), WAIT)
    r.rerender(view('s2'))
    await screen.findByText('s2-m')
    await act(async () => answer(json(UNDELIVERED)))
    await waitFor(() => expect(s.of('/api/attachments/ab%2Fc')).toHaveLength(1), WAIT)
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)))
    expect(textarea().value).toBe('')
    expect(heldFor('s1').attachments).toEqual([])
    expect(heldFor('s2').attachments).toEqual([])
    r.rerender(view('s1'))
    await screen.findByRole('button', { name: 'Send again' }, WAIT)
    expect(textarea().value).toBe('')
    expect(screen.queryByRole('list', { name: 'Images' })).toBeNull()
  })
})
