import '@testing-library/jest-dom/vitest'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { createRef, useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Client } from '../api/client'
import { resume } from '../api/turns'
import { ClientContext } from '../app-client'
import type { Capabilities, SessionCatalog } from '../generated/protocol'
import { forgetAllAttachments, heldFor } from '../lib/attachments'
import { forgetAllSends } from '../lib/sending'
import { Composer, STILL_SENDING, type ComposerHandle, type ComposerProps } from './Composer'

const WAIT = { timeout: 3000 }

interface Post {
  path: string
  body: unknown
}

type Handler = (method: string, path: string, body: unknown) => Response | Promise<Response>

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

const accepted: Handler = () => json({ turn_id: 't-new' }, 202)

function catalogOf(model = 'a', mode = 'plan', cmds: unknown[] = []): SessionCatalog {
  return {
    session_id: 's1',
    config_options: [
      {
        id: 'model',
        name: 'Model',
        category: 'model',
        type: 'select',
        currentValue: model,
        options: ['a', 'b', 'c'].map((v) => ({ value: v, name: v.toUpperCase() })),
      },
      {
        id: 'mode',
        name: 'Mode',
        category: 'mode',
        type: 'select',
        currentValue: mode,
        options: [
          { value: 'plan', name: 'Plan' },
          { value: 'edit', name: 'Edit' },
        ],
      },
      { id: 'think', name: 'Think', type: 'boolean', currentValue: false },
    ],
    commands: cmds,
  } as SessionCatalog
}

type HostProps = Omit<ComposerProps, 'catalog' | 'onCatalog'> & { initialCatalog?: SessionCatalog | null }

/** The composer as the session screen holds it: the catalogue in state,
 *  replaced by what `onCatalog` hands back. */
function Host({ initialCatalog = null, ...rest }: HostProps) {
  const [catalog, setCatalog] = useState<SessionCatalog | null>(initialCatalog)
  return <Composer {...rest} catalog={catalog} onCatalog={setCatalog} />
}

function mount(handler: Handler = accepted, props: Partial<HostProps> = {}) {
  const posts: Post[] = []
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input)
    const body = typeof init?.body === 'string' ? (JSON.parse(init.body) as unknown) : undefined
    if ((init?.method ?? 'GET') === 'POST') posts.push({ path, body })
    return handler(init?.method ?? 'GET', path, body)
  })
  const client = new Client({
    fetch: fetch as unknown as typeof globalThis.fetch,
    navigate: vi.fn(),
    here: () => ({ pathname: '/sessions/s1', search: '' }),
    stepUp: async () => {},
  })
  const base: HostProps = {
    sessionId: 's1',
    session: { lifecycle: 'active', activity: 'idle' },
    capabilities: ['images'],
    // As the view resumes: `POST …/resume`.
    onResume: () => resume(client, 's1'),
    ...props,
  }
  const view = render(
    <ClientContext.Provider value={client}>
      <Host {...base} />
    </ClientContext.Provider>,
  )
  const rerender = (next: Partial<HostProps>) =>
    view.rerender(
      <ClientContext.Provider value={client}>
        <Host {...base} {...next} />
      </ClientContext.Provider>,
    )
  const prompts = () => posts.filter((p) => p.path.endsWith('/prompt'))
  return { ...view, rerender, posts, prompts, fetch }
}

const textarea = () => screen.getByLabelText('Prompt') as HTMLTextAreaElement
const type = (text: string) => fireEvent.change(textarea(), { target: { value: text } })
const sendButton = () => screen.getByRole('button', { name: 'Send' })

function png(name: string, body = name): File {
  return new File([body], name, { type: 'image/png' })
}

function paste(...files: File[]) {
  fireEvent.paste(textarea(), {
    clipboardData: { items: files.map((f) => ({ kind: 'file', type: f.type, getAsFile: () => f })) },
  })
}

let urls = 0
beforeEach(() => {
  urls = 0
  sessionStorage.clear()
  forgetAllAttachments()
  forgetAllSends()
  URL.createObjectURL = vi.fn(() => `blob:u${++urls}`)
  URL.revokeObjectURL = vi.fn()
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('Composer: sending', () => {
  it('Ctrl+Enter sends the prompt', async () => {
    const c = mount()
    type('hello')
    fireEvent.keyDown(textarea(), { key: 'Enter', ctrlKey: true })
    await waitFor(() => expect(c.prompts()).toEqual([{ path: '/api/sessions/s1/prompt', body: { content: [{ type: 'text', text: 'hello' }] } }]), WAIT)
  })

  it('Cmd+Enter sends the prompt', async () => {
    const c = mount()
    type('hello')
    fireEvent.keyDown(textarea(), { key: 'Enter', metaKey: true })
    await waitFor(() => expect(c.prompts()).toHaveLength(1), WAIT)
  })

  it('Enter alone sends nothing and leaves the newline to the text area', async () => {
    const c = mount()
    type('hello')
    const notPrevented = fireEvent.keyDown(textarea(), { key: 'Enter' })
    expect(notPrevented).toBe(true)
    await new Promise((r) => setTimeout(r, 30))
    expect(c.prompts()).toEqual([])
  })

  it('the Send button sends, then clears the text, the stored draft and the images', async () => {
    const c = mount()
    type('see ')
    paste(png('a.png', 'ONE'))
    await screen.findByRole('img', { name: 'Image #1' }, WAIT)
    expect(sessionStorage.getItem('hennery.draft.s1')).toBe('see [Image #1] ')
    fireEvent.click(sendButton())
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    expect(c.prompts()[0].body).toEqual({
      content: [
        { type: 'text', text: 'see ' },
        { type: 'image', mimeType: 'image/png', data: btoa('ONE') },
      ],
    })
    expect(sessionStorage.getItem('hennery.draft.s1')).toBeNull()
    expect(screen.queryByRole('img')).toBeNull()
    // A new draft: its images are numbered from 1 again.
    paste(png('b.png'))
    expect(textarea().value).toBe('[Image #1] ')
  })

  it('Send is disabled for an empty or blank draft', () => {
    mount()
    expect(sendButton()).toBeDisabled()
    type('   \n ')
    expect(sendButton()).toBeDisabled()
    type('x')
    expect(sendButton()).toBeEnabled()
  })

  it('keeps the draft in sessionStorage as it is typed, and brings it back', () => {
    sessionStorage.setItem('hennery.draft.s1', 'kept from before')
    mount()
    expect(textarea().value).toBe('kept from before')
    type('changed')
    expect(sessionStorage.getItem('hennery.draft.s1')).toBe('changed')
  })
})

describe('Composer: a turn in flight', () => {
  it.each(['running', 'blocked'])('Cancel replaces Send while the session is %s, and Ctrl+Enter sends nothing', async (activity) => {
    const c = mount(accepted, { session: { lifecycle: 'active', activity } })
    type('queued')
    expect(screen.queryByRole('button', { name: 'Send' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeInTheDocument()
    fireEvent.keyDown(textarea(), { key: 'Enter', ctrlKey: true })
    await new Promise((r) => setTimeout(r, 30))
    expect(c.prompts()).toEqual([])
  })

  it('offers Send, not Cancel, when idle', () => {
    mount()
    expect(screen.queryByRole('button', { name: 'Cancel' })).toBeNull()
    expect(sendButton()).toBeInTheDocument()
  })

  it.each([
    ['completed', 'The turn finished before it could be stopped.'],
    ['cancelled', 'Stopped.'],
    ['failed', 'The turn failed before it could be stopped.'],
    ['interrupted', 'The turn was interrupted: the session was parked or closed, or its agent exited.'],
  ])('cancel outcome %s is said as it was', async (outcome, words) => {
    const c = mount(() => json({ turn_id: 't1', outcome }, 202), { session: { lifecycle: 'active', activity: 'running' } })
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(await screen.findByText(words, undefined, WAIT)).toBeInTheDocument()
    expect(c.posts.map((p) => p.path)).toEqual(['/api/sessions/s1/cancel'])
  })

  it.each([
    [409, 'not_attached', 'The session is not running: there is nothing to stop.'],
    [409, 'host_offline', 'The host is offline: the turn cannot be stopped from here until it is back.'],
  ])('cancel refused with %i %s says why', async (status, code, words) => {
    mount(() => json({ code, message: 'srv-x' }, status), { session: { lifecycle: 'active', activity: 'running' } })
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(await screen.findByText(words, undefined, WAIT)).toBeInTheDocument()
  })

  it.each(['no_open_turn', 'not_running'])('cancel refused with %s says nothing was running', async (code) => {
    mount(() => json({ code, message: 'srv-x' }, 409), { session: { lifecycle: 'active', activity: 'running' } })
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(await screen.findByText('Nothing was running.', undefined, WAIT)).toBeInTheDocument()
  })
})

describe('Composer: prompt refusals keep the draft', () => {
  it.each([
    [409, 'host_offline', 'The host is offline; send again once it is back.'],
    [409, 'turn_in_progress', 'A turn is still running: wait for it to end, or stop it, then send again.'],
    [409, 'images_unsupported', 'This host takes no images: remove them, then send again.'],
    [400, 'empty_prompt', 'There is nothing to send: write something, or attach an image.'],
    [400, 'invalid_content', 'This prompt holds something that cannot be sent (srv-x).'],
    [400, 'invalid', 'The prompt was refused as not valid (srv-x).'],
    [413, 'content_too_large', 'The prompt is too large (srv-x).'],
    [413, 'body_too_large', 'The prompt is too large to send: take out some images, or send them in parts.'],
    [503, 'delivery_unknown', 'Delivery unknown: the outcome shows when the host reconnects.'],
  ])('refusal %i %s keeps the draft and says why', async (status, code, words) => {
    mount(() => json({ code, message: 'srv-x' }, status))
    type('my words ')
    paste(png('a.png'))
    await screen.findByRole('img', { name: 'Image #1' }, WAIT)
    fireEvent.click(sendButton())
    expect(await screen.findByText(words, undefined, WAIT)).toBeInTheDocument()
    expect(textarea().value).toBe('my words [Image #1] ')
    expect(sessionStorage.getItem('hennery.draft.s1')).toBe('my words [Image #1] ')
    expect(screen.getByRole('img', { name: 'Image #1' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Resume and send' })).toBeNull()
  })

  it('refusal not_attached keeps the draft and offers Resume and send', async () => {
    mount(() => json({ code: 'not_attached', message: 'srv-x' }, 409))
    type('later')
    fireEvent.click(sendButton())
    expect(await screen.findByText('The session is not running: resume it to send this.', undefined, WAIT)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Resume and send' })).toBeInTheDocument()
    expect(textarea().value).toBe('later')
    expect(textarea().readOnly).toBe(false)
  })

  it.each([
    ['empty', ''],
    ['only blanks', '   \n '],
  ])('Resume and send waits for words: a draft %s resumes nothing and stays editable', async (_what, words) => {
    const c = mount(() => json({ code: 'not_attached', message: 'srv-x' }, 409))
    type('later')
    fireEvent.click(sendButton())
    const button = await screen.findByRole('button', { name: 'Resume and send' }, WAIT)
    type(words)
    expect(button).toBeDisabled()
    fireEvent.click(button)
    await act(() => new Promise((resolve) => setTimeout(resolve, 200)))
    expect(textarea().readOnly).toBe(false)
    expect(c.posts.map((p) => p.path)).toEqual(['/api/sessions/s1/prompt'])
    // Words again: the draft sends as before.
    type('later again')
    expect(button).not.toBeDisabled()
    expect(sendButton()).not.toBeDisabled()
  })

  it('Send clicked twice before the composer renders again sends once', async () => {
    const c = mount()
    type('once')
    const button = sendButton()
    act(() => {
      button.click()
      button.click()
    })
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    expect(c.posts.map((p) => p.path)).toEqual(['/api/sessions/s1/prompt'])
  })

  it('Resume and send clicked twice before the composer renders again resumes once', async () => {
    const c = mount((_m, path) =>
      path.endsWith('/resume')
        ? json({ session_id: 's1', lifecycle: 'active' }, 202)
        : json({ code: 'not_attached', message: 'srv-x' }, 409),
    )
    type('later')
    fireEvent.click(sendButton())
    const button = await screen.findByRole('button', { name: 'Resume and send' }, WAIT)
    act(() => {
      button.click()
      button.click()
    })
    await waitFor(() => expect(c.posts).toHaveLength(3), WAIT)
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)))
    expect(c.posts.map((p) => p.path)).toEqual([
      '/api/sessions/s1/prompt',
      '/api/sessions/s1/resume',
      '/api/sessions/s1/prompt',
    ])
  })

  it('Resume and send resumes the session, then sends the draft', async () => {
    let attached = false
    const c = mount((_m, path) => {
      if (path.endsWith('/resume')) {
        attached = true
        return json({ session_id: 's1', lifecycle: 'active' }, 202)
      }
      return attached ? accepted('POST', path, undefined) : json({ code: 'not_attached', message: 'srv-x' }, 409)
    })
    type('later')
    fireEvent.click(sendButton())
    fireEvent.click(await screen.findByRole('button', { name: 'Resume and send' }, WAIT))
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    expect(c.posts.map((p) => p.path)).toEqual([
      '/api/sessions/s1/prompt',
      '/api/sessions/s1/resume',
      '/api/sessions/s1/prompt',
    ])
  })

  it('Resume and send shows a resume refusal and sends nothing', async () => {
    const c = mount((_m, path) =>
      path.endsWith('/resume')
        ? json({ code: 'hat_mismatch', message: 'srv-x' }, 409)
        : json({ code: 'not_attached', message: 'srv-x' }, 409),
    )
    type('later')
    fireEvent.click(sendButton())
    fireEvent.click(await screen.findByRole('button', { name: 'Resume and send' }, WAIT))
    expect(await screen.findByText('This directory now belongs to another hat than the session’s.', undefined, WAIT)).toBeInTheDocument()
    expect(c.prompts()).toHaveLength(1)
    expect(textarea().value).toBe('later')
    expect(textarea().readOnly).toBe(false)
    expect(sendButton()).not.toBeDisabled()
  })

  it('Resume and send refused at the prompt leaves the draft editable', async () => {
    const c = mount((_m, path) =>
      path.endsWith('/resume')
        ? json({ session_id: 's1', lifecycle: 'active' }, 202)
        : json({ code: 'not_attached', message: 'srv-x' }, 409),
    )
    type('later')
    fireEvent.click(sendButton())
    fireEvent.click(await screen.findByRole('button', { name: 'Resume and send' }, WAIT))
    await waitFor(() => expect(c.prompts()).toHaveLength(2), WAIT)
    await waitFor(() => expect(textarea().readOnly).toBe(false), WAIT)
    expect(textarea().value).toBe('later')
    expect(sendButton()).not.toBeDisabled()
  })

  it('Resume and send goes through the onResume it is given', async () => {
    const onResume = vi.fn(async () => {})
    let resumed = false
    onResume.mockImplementation(async () => {
      resumed = true
    })
    const c = mount((_m, path) => (resumed ? accepted('POST', path, undefined) : json({ code: 'not_attached', message: 'x' }, 409)), {
      onResume,
    })
    type('later')
    fireEvent.click(sendButton())
    fireEvent.click(await screen.findByRole('button', { name: 'Resume and send' }, WAIT))
    await waitFor(() => expect(c.prompts()).toHaveLength(2), WAIT)
    expect(onResume).toHaveBeenCalledOnce()
    expect(c.posts.some((p) => p.path.endsWith('/resume'))).toBe(false)
  })

  it('a refusal code named like an Object property shows the server’s message', async () => {
    mount(() => json({ code: 'toString', message: 'srv-x' }, 400))
    type('x')
    fireEvent.click(sendButton())
    expect(await screen.findByText('srv-x', undefined, WAIT)).toBeInTheDocument()
  })

  it('a session still starting is never offered a resume', async () => {
    mount(() => json({ code: 'not_attached', message: 'srv-x' }, 409), { session: { lifecycle: 'starting' } })
    type('early')
    fireEvent.click(sendButton())
    expect(await screen.findByText('The session is still starting: send again once it runs.', undefined, WAIT)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Resume and send' })).toBeNull()
  })
})

describe('Composer: images', () => {
  it('paste inserts an [Image #1] marker at the cursor and shows the image', async () => {
    mount()
    type('ab')
    textarea().setSelectionRange(1, 1)
    paste(png('a.png'))
    expect(textarea().value).toBe('a[Image #1] b')
    expect(await screen.findByRole('img', { name: 'Image #1' }, WAIT)).toHaveAttribute('src', 'blob:u1')
  })

  it('a drop of two images splices both markers in at once, over the selection', async () => {
    mount()
    type('ab XX cd')
    textarea().setSelectionRange(3, 5)
    fireEvent.drop(textarea().closest('.composer-box')!, { dataTransfer: { files: [png('1.png'), png('2.png')], types: ['Files'] } })
    expect(textarea().value).toBe('ab [Image #1] [Image #2]  cd')
    expect(await screen.findAllByRole('img', undefined, WAIT)).toHaveLength(2)
  })

  it('the picker attaches the files chosen', async () => {
    mount()
    fireEvent.change(screen.getByTestId('image-input'), { target: { files: [png('p.png')] } })
    expect(textarea().value).toBe('[Image #1] ')
    expect(await screen.findByRole('img', { name: 'Image #1' }, WAIT)).toBeInTheDocument()
  })

  it('says which image was refused, and why', () => {
    mount()
    paste(new File(['<svg/>'], 'v.svg', { type: 'image/svg+xml' }))
    expect(screen.getByRole('alert')).toHaveTextContent('v.svg is not a PNG, JPEG, GIF or WebP image.')
    expect(textarea().value).toBe('')
  })

  it('hides images when the host is known to lack the images capability', () => {
    mount(accepted, { capabilities: ['projects', 'park'] as Capabilities })
    expect(screen.queryByRole('button', { name: 'Attach images' })).toBeNull()
    expect(screen.queryByTestId('image-input')).toBeNull()
    paste(png('a.png'))
    expect(textarea().value).toBe('')
  })

  it('shows images while the host’s capabilities are not known yet', () => {
    mount(accepted, { capabilities: null })
    expect(screen.getByRole('button', { name: 'Attach images' })).toBeInTheDocument()
  })

  it('sends only the images whose marker is still in the text', async () => {
    const c = mount()
    paste(png('a.png', 'A'), png('b.png', 'B'))
    expect(textarea().value).toBe('[Image #1] [Image #2] ')
    type('[Image #2] only')
    fireEvent.click(sendButton())
    await waitFor(() => expect(c.prompts()).toHaveLength(1), WAIT)
    expect(c.prompts()[0].body).toEqual({
      content: [
        { type: 'image', mimeType: 'image/png', data: btoa('B') },
        { type: 'text', text: ' only' },
      ],
    })
  })

  it('removing an image takes its marker out, and its number is never used again', async () => {
    mount()
    paste(png('a.png'), png('b.png'))
    await screen.findAllByRole('img', undefined, WAIT)
    fireEvent.click(screen.getByRole('button', { name: 'Remove image #2' }))
    expect(textarea().value).toBe('[Image #1] ')
    paste(png('c.png'))
    expect(textarea().value).toBe('[Image #1] [Image #3] ')
    expect(screen.queryByText('#2')).toBeNull()
    expect(screen.getByText('#3')).toBeInTheDocument()
  })

  it('a restored draft numbers new images past the markers it holds', () => {
    sessionStorage.setItem('hennery.draft.s1', 'old [Image #4] ')
    mount()
    textarea().setSelectionRange(15, 15)
    paste(png('a.png'))
    expect(textarea().value).toBe('old [Image #4] [Image #5] ')
  })

  it('a marker of more than six digits is text: two images pasted after it get two numbers', async () => {
    sessionStorage.setItem('hennery.draft.s1', 'old [Image #99999999999999999999] ')
    mount()
    textarea().setSelectionRange(34, 34)
    paste(png('a.png'), png('b.png'))
    expect(textarea().value).toBe('old [Image #99999999999999999999] [Image #1] [Image #2] ')
    expect((await screen.findAllByRole('img', undefined, WAIT)).map((i) => i.getAttribute('alt'))).toEqual(['Image #1', 'Image #2'])
  })

  it('revokes an image’s object URL when it is removed', async () => {
    mount()
    paste(png('a.png'))
    await screen.findByRole('img', { name: 'Image #1' }, WAIT)
    fireEvent.click(screen.getByRole('button', { name: 'Remove image #1' }))
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:u1')
  })

  it('revokes the object URLs once the prompt is sent', async () => {
    mount()
    paste(png('a.png'))
    await screen.findByRole('img', { name: 'Image #1' }, WAIT)
    expect(URL.revokeObjectURL).not.toHaveBeenCalled()
    fireEvent.click(sendButton())
    await waitFor(() => expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:u1'), WAIT)
  })

  it('revokes the object URLs when the composer goes', async () => {
    const c = mount()
    paste(png('a.png'))
    await screen.findByRole('img', { name: 'Image #1' }, WAIT)
    c.unmount()
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:u1')
  })
})

describe('Composer: slash commands', () => {
  const menuOptions = () => within(screen.getByRole('listbox')).getAllByRole('option')
  const cmds = [{ name: 'compact', description: 'Compact the context' }, { name: 'cost' }, { name: 'review', input: { hint: 'pr' } }]

  it('lists the catalogue’s commands while the text is a slash and a word', () => {
    mount(accepted, { initialCatalog: catalogOf('a', 'plan', cmds) })
    type('/co')
    const menu = screen.getByRole('listbox', { name: 'Slash commands' })
    expect(within(menu).getAllByRole('option').map((o) => o.textContent)).toEqual(['/compactCompact the context', '/cost'])
    type('/co x')
    expect(screen.queryByRole('listbox')).toBeNull()
    type('a /co')
    expect(screen.queryByRole('listbox')).toBeNull()
  })

  it('the menu rule holds for any command name the adapter reports', () => {
    // Names are the adapter's data: one may hold a space or a slash.
    mount(accepted, { initialCatalog: catalogOf('a', 'plan', [{ name: 'review pr' }, { name: 'x/co' }]) })
    type('/review')
    expect(screen.getByRole('listbox')).toBeInTheDocument()
    type('/review p')
    expect(screen.queryByRole('listbox')).toBeNull()
    type('x/co')
    expect(screen.queryByRole('listbox')).toBeNull()
  })

  it('arrow keys move through the menu, wrapping, and Enter picks', () => {
    const c = mount(accepted, { initialCatalog: catalogOf('a', 'plan', cmds) })
    type('/')
    expect(menuOptions()[0]).toHaveAttribute('aria-selected', 'true')
    fireEvent.keyDown(textarea(), { key: 'ArrowUp' })
    expect(menuOptions()[2]).toHaveAttribute('aria-selected', 'true')
    fireEvent.keyDown(textarea(), { key: 'ArrowDown' })
    fireEvent.keyDown(textarea(), { key: 'ArrowDown' })
    expect(menuOptions()[1]).toHaveAttribute('aria-selected', 'true')
    fireEvent.keyDown(textarea(), { key: 'Enter' })
    expect(textarea().value).toBe('/cost ')
    expect(screen.queryByRole('listbox')).toBeNull()
    expect(c.prompts()).toEqual([])
  })

  it('Escape dismisses the menu until the text changes', () => {
    mount(accepted, { initialCatalog: catalogOf('a', 'plan', cmds) })
    type('/c')
    fireEvent.keyDown(textarea(), { key: 'Escape' })
    expect(screen.queryByRole('listbox')).toBeNull()
    type('/co')
    expect(screen.getByRole('listbox')).toBeInTheDocument()
  })

  it('a click picks a command', () => {
    mount(accepted, { initialCatalog: catalogOf('a', 'plan', cmds) })
    type('/r')
    fireEvent.mouseDown(within(screen.getByRole('listbox')).getByRole('option'))
    expect(textarea().value).toBe('/review ')
  })
})

describe('Composer: config bar', () => {
  function deferred() {
    let resolve!: (r: Response) => void
    const promise = new Promise<Response>((r) => (resolve = r))
    return { promise, resolve }
  }

  it('shows one switcher per option: model, mode, then the rest', () => {
    mount(accepted, { initialCatalog: catalogOf() })
    const bar = screen.getByRole('group', { name: 'Session settings' })
    expect(within(bar).getAllByRole('combobox').map((s) => s.getAttribute('aria-label'))).toEqual(['Model', 'Mode'])
    expect(within(bar).getByRole('checkbox', { name: 'Think' })).not.toBeChecked()
  })

  it('config success: shows the pick at once, then the catalogue the 202 returns', async () => {
    const answer = deferred()
    const c = mount(() => answer.promise, { initialCatalog: catalogOf('a') })
    const model = screen.getByRole('combobox', { name: 'Model' })
    fireEvent.change(model, { target: { value: 'b' } })
    expect(model).toHaveValue('b')
    expect(model).toBeDisabled()
    expect(c.posts).toEqual([{ path: '/api/sessions/s1/config', body: { config_id: 'model', value: 'b' } }])
    // The agent clamped it: what it reports wins over the pick.
    await act(async () => answer.resolve(json(catalogOf('c', 'edit'), 202)))
    await waitFor(() => expect(model).toHaveValue('c'), WAIT)
    expect(model).toBeEnabled()
    expect(screen.getByRole('combobox', { name: 'Mode' })).toHaveValue('edit')
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('switches an on/off option', async () => {
    const c = mount(() => json(catalogOf(), 202), { initialCatalog: catalogOf() })
    fireEvent.click(screen.getByRole('checkbox', { name: 'Think' }))
    await waitFor(() => expect(c.posts).toEqual([{ path: '/api/sessions/s1/config', body: { config_id: 'think', value: true } }]), WAIT)
  })

  it.each([
    [409, 'unknown_option', 'Model: The agent no longer offers this.'],
    [409, 'not_attached', 'Model: The session is not running: resume it to change this.'],
    [400, 'invalid', 'Model: The agent refused this value (srv-x).'],
    [502, 'config_failed', 'Model: The agent could not apply it (srv-x).'],
    [409, 'host_offline', 'Model: The host is offline: try again once it is back.'],
  ])('config rollback on %i %s, with the message', async (status, code, words) => {
    mount(() => json({ code, message: 'srv-x' }, status), { initialCatalog: catalogOf('a') })
    const model = screen.getByRole('combobox', { name: 'Model' })
    fireEvent.change(model, { target: { value: 'b' } })
    expect(await screen.findByRole('alert', undefined, WAIT)).toHaveTextContent(words)
    expect(model).toHaveValue('a')
    expect(model).toBeEnabled()
  })
})

describe('Composer: per session', () => {
  it('switching sessions shows the other session’s draft and images, and sends only to it', async () => {
    const c = mount()
    type('for a ')
    paste(png('a.png', 'AAA'))
    await screen.findByRole('img', { name: 'Image #1' }, WAIT)

    c.rerender({ sessionId: 's2' })
    expect(textarea().value).toBe('')
    expect(screen.queryByRole('img')).toBeNull()
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:u1')
    type('for b')
    fireEvent.click(sendButton())
    await waitFor(() => expect(c.prompts()).toHaveLength(1), WAIT)
    expect(c.prompts()[0]).toEqual({ path: '/api/sessions/s2/prompt', body: { content: [{ type: 'text', text: 'for b' }] } })
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)

    c.rerender({ sessionId: 's1' })
    expect(textarea().value).toBe('for a [Image #1] ')
    expect(await screen.findByRole('img', { name: 'Image #1' }, WAIT)).toBeInTheDocument()
    textarea().setSelectionRange(17, 17)
    paste(png('b.png'))
    expect(textarea().value).toBe('for a [Image #1] [Image #2] ')
  })

  it('a sent prompt’s images do not come back with its session', async () => {
    const c = mount()
    paste(png('a.png'))
    await screen.findByRole('img', { name: 'Image #1' }, WAIT)
    fireEvent.click(sendButton())
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    c.rerender({ sessionId: 's2' })
    c.rerender({ sessionId: 's1' })
    expect(screen.queryByRole('img')).toBeNull()
    expect(screen.queryByRole('list', { name: 'Images' })).toBeNull()
  })

  it('an image number stays used across a session switch', async () => {
    const c = mount()
    paste(png('a.png'), png('b.png'))
    await screen.findAllByRole('img', undefined, WAIT)
    fireEvent.click(screen.getByRole('button', { name: 'Remove image #2' }))
    c.rerender({ sessionId: 's2' })
    c.rerender({ sessionId: 's1' })
    expect(textarea().value).toBe('[Image #1] ')
    textarea().setSelectionRange(11, 11)
    paste(png('c.png'))
    expect(textarea().value).toBe('[Image #1] [Image #3] ')
  })

  it('a send answered after a switch clears only its own session’s draft', async () => {
    let release!: () => void
    const held = new Promise<void>((r) => (release = r))
    const c = mount(async (_m, path) => {
      if (path === '/api/sessions/s1/prompt') await held
      return accepted('POST', path, undefined)
    })
    type('for a')
    fireEvent.click(sendButton())
    await waitFor(() => expect(c.prompts()).toHaveLength(1), WAIT)
    c.rerender({ sessionId: 's2' })
    type('for b')
    await act(async () => release())
    await waitFor(() => expect(sessionStorage.getItem('hennery.draft.s1')).toBeNull(), WAIT)
    expect(textarea().value).toBe('for b')
    expect(sessionStorage.getItem('hennery.draft.s2')).toBe('for b')
  })
})

describe('Composer: words put in the draft', () => {
  function withHandle(handler: Handler = accepted) {
    const handle = createRef<ComposerHandle>()
    const c = mount(handler, { handle })
    return { ...c, handle: () => handle.current! }
  }

  it('an agent’s marker of more than six digits is text: two images pasted after it get two numbers', async () => {
    const c = withHandle()
    act(() => c.handle().prefill('You asked: [Image #99999999999999999999]? My answer: '))
    paste(png('a.png'), png('b.png'))
    expect((await screen.findAllByRole('img', undefined, WAIT)).map((i) => i.getAttribute('alt'))).toEqual(['Image #1', 'Image #2'])
  })

  it('an agent’s words never link an image the operator took out of the text', async () => {
    const c = withHandle()
    paste(png('a.png', 'A'))
    await screen.findByRole('img', { name: 'Image #1' }, WAIT)
    // The marker deleted by hand: the chip stays, unlinked.
    type('mine')
    act(() => c.handle().prefill('You asked: show [Image #1]? My answer: '))
    expect(textarea().value.replaceAll('\u2060', '')).toBe('mine\n\nYou asked: show [Image #1]? My answer: ')
    expect(textarea().value).not.toContain('[Image #1]')
    fireEvent.click(sendButton())
    await waitFor(() => expect(c.prompts()).toHaveLength(1), WAIT)
    const content = (c.prompts()[0].body as { content: { type: string }[] }).content
    expect(content.map((b) => b.type)).toEqual(['text'])
  })
})

describe('Composer: work added while a prompt is being sent', () => {
  /** Every prompt POST held until `release`. */
  function heldSend(handler: Handler = accepted) {
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    const handle = createRef<ComposerHandle>()
    const c = mount(
      async (method, path, body) => {
        if (path.endsWith('/prompt')) await gate
        return handler(method, path, body)
      },
      { handle },
    )
    return { ...c, handle: () => handle.current!, release: () => act(async () => release()) }
  }

  const refusal = () => screen.getByRole('alert').textContent

  async function sendFirst(c: ReturnType<typeof heldSend>) {
    type('first ')
    fireEvent.click(sendButton())
    await waitFor(() => expect(c.prompts()).toHaveLength(1), WAIT)
  }

  it.each([
    ['a paste', () => paste(png('late.png'))],
    ['a drop', () => fireEvent.drop(textarea().closest('.composer-box')!, { dataTransfer: { files: [png('late.png')], types: ['Files'] } })],
    ['a pick', () => fireEvent.change(screen.getByTestId('image-input'), { target: { files: [png('late.png')] } })],
  ])('refuses %s, saying why, and the image is not lost in silence', async (_what, add) => {
    const c = heldSend()
    await sendFirst(c)
    add()
    expect(refusal()).toBe(STILL_SENDING)
    expect(textarea().value).toBe('first ')
    expect(screen.queryByRole('list', { name: 'Images' })).toBeNull()
    await c.release()
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    // Still said once the send has gone: the operator adds the image again.
    expect(refusal()).toBe(STILL_SENDING)
    expect(heldFor('s1').attachments).toEqual([])
    expect(c.prompts()[0].body).toEqual({ content: [{ type: 'text', text: 'first ' }] })
  })

  it('refuses an answer as a new message, saying why, and the 202 clears the draft that was sent', async () => {
    const c = heldSend()
    await sendFirst(c)
    act(() => c.handle().prefill('You asked: go? My answer: ', 'Answering a question as a new message'))
    expect(refusal()).toBe(STILL_SENDING)
    expect(textarea().value).toBe('first ')
    expect(screen.queryByText('Answering a question as a new message', { exact: true })).toBeNull()
    await c.release()
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    expect(refusal()).toBe(STILL_SENDING)
    expect(sessionStorage.getItem('hennery.draft.s1')).toBeNull()
  })

  it('refuses a turn put back that lands while the send is in flight, with its images', async () => {
    const c = heldSend((method, path, body) => {
      if (path.startsWith('/api/view/sessions/s1/turns/')) {
        return json({
          turn_id: 't9',
          content: [
            { type: 'text', text: 'look at' },
            { type: 'image', mimeType: 'image/png', sha256: 'ab', size: 4 },
          ],
        })
      }
      if (path.startsWith('/api/attachments/')) return new Response('PNG!', { status: 200 })
      return accepted(method, path, body)
    })
    await sendFirst(c)
    await act(() => c.handle().refill('t9'))
    expect(c.fetch.mock.calls.some((call) => String(call[0]) === '/api/attachments/ab')).toBe(true)
    expect(refusal()).toBe(STILL_SENDING)
    expect(textarea().value).toBe('first ')
    expect(screen.queryByRole('list', { name: 'Images' })).toBeNull()
    await c.release()
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    expect(heldFor('s1').attachments).toEqual([])
  })

  it('refuses a turn put back that was asked for before the send and lands during it', async () => {
    let image!: (r: Response) => void
    const c = heldSend((method, path, body) => {
      if (path.startsWith('/api/view/sessions/s1/turns/')) {
        return json({ turn_id: 't9', content: [{ type: 'image', mimeType: 'image/png', sha256: 'ab', size: 4 }] })
      }
      if (path.startsWith('/api/attachments/')) return new Promise<Response>((r) => (image = r))
      return accepted(method, path, body)
    })
    let landed!: Promise<void>
    act(() => {
      landed = c.handle().refill('t9')
    })
    await waitFor(() => expect(image).toBeDefined(), WAIT)
    await sendFirst(c)
    await act(async () => {
      image(new Response('PNG!', { status: 200 }))
      await landed
    })
    expect(refusal()).toBe(STILL_SENDING)
    expect(textarea().value).toBe('first ')
    expect(screen.queryByRole('list', { name: 'Images' })).toBeNull()
    await c.release()
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    expect(heldFor('s1').attachments).toEqual([])
  })

  it('takes work again once the send has gone', async () => {
    const c = heldSend()
    await sendFirst(c)
    await c.release()
    await waitFor(() => expect(textarea().value).toBe(''), WAIT)
    paste(png('next.png'))
    expect(textarea().value).toBe('[Image #1] ')
    expect(screen.queryByRole('alert')).toBeNull()
  })
})
