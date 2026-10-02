import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { useClient } from '../app-client'
import { json, stubServer, FULL } from '../test-server'

const STEP_UP = { code: 'step_up_required', message: 'm' }

/** A form whose submit needs a step-up: what the dialog must not disturb. */
function Form() {
  const client = useClient()
  const [name, setName] = useState('')
  const [result, setResult] = useState('')
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        client.request('POST', '/api/hosts/pairing-codes', { name }).then(
          () => setResult('done'),
          (err: Error) => setResult(err.name),
        )
      }}
    >
      <input aria-label="Name" value={name} onChange={(e) => setName(e.target.value)} />
      <button type="submit">Pair</button>
      <output>{result}</output>
    </form>
  )
}

vi.mock('../screens/Placeholder', () => ({ default: () => <Form /> }))

afterEach(() => history.replaceState(null, '', '/'))

async function open(server: ReturnType<typeof stubServer>) {
  history.replaceState(null, '', '/hosts')
  render(<App fetchImpl={server.fetch} />)
  await userEvent.type(await screen.findByLabelText('Name'), 'laptop')
  await userEvent.click(screen.getByRole('button', { name: 'Pair' }))
  return screen.findByRole('dialog')
}

describe('the step-up dialog', () => {
  it('confirms with the password, then the request goes once more, with the form as it was', async () => {
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/hosts/pairing-codes': [json(403, STEP_UP), json(201, { code: 'x' })],
      'POST /api/auth/step-up/password': new Response(null, { status: 204 }),
    })
    const dialog = await open(server)
    expect(dialog).toHaveAttribute('aria-modal', 'true')
    expect(screen.getByLabelText('Your password')).toHaveFocus()
    await userEvent.type(screen.getByLabelText('Your password'), 'correct horse battery')
    await userEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('done'))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.getByLabelText('Name')).toHaveValue('laptop')
    const paths = server.sent.map((s) => `${s.method} ${s.path}`)
    expect(paths.filter((p) => p === 'POST /api/hosts/pairing-codes')).toHaveLength(2)
    expect(server.sent.at(-1)?.body).toEqual({ name: 'laptop' })
  })

  it('keeps itself open on a wrong password, and sends the request nowhere', async () => {
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/hosts/pairing-codes': json(403, STEP_UP),
      'POST /api/auth/step-up/password': json(401, { code: 'invalid_password', message: 'm' }),
    })
    await open(server)
    await userEvent.type(screen.getByLabelText('Your password'), 'wrong')
    await userEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Wrong password.')
    expect(screen.getByRole('dialog')).toBeInTheDocument()
    expect(server.sent.filter((s) => s.path === '/api/hosts/pairing-codes')).toHaveLength(1)
  })

  it('cancelled, the request is not sent again', async () => {
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/hosts/pairing-codes': json(403, STEP_UP),
    })
    await open(server)
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('StepUpCancelled'))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.getByLabelText('Name')).toHaveValue('laptop')
    expect(server.sent.filter((s) => s.path === '/api/hosts/pairing-codes')).toHaveLength(1)
    // Focus is back on what opened the dialog.
    expect(screen.getByRole('button', { name: 'Pair' })).toHaveFocus()
  })

  it('closes on Escape, as a cancel', async () => {
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/hosts/pairing-codes': json(403, STEP_UP),
    })
    await open(server)
    await userEvent.keyboard('{Escape}')
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('StepUpCancelled'))
  })

  it('offers the passkey first when there is one, and confirms with it', async () => {
    vi.stubGlobal('PublicKeyCredential', function PublicKeyCredential() {})
    const get = vi.fn(async () => ({ toJSON: () => ({ id: 'cred' }) }))
    Object.defineProperty(navigator, 'credentials', { value: { get, create: vi.fn() }, configurable: true })
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/hosts/pairing-codes': [json(403, STEP_UP), json(201, { code: 'x' })],
      'POST /api/auth/step-up/passkey/start': json(200, {
        ceremony_id: 's-1',
        options: { publicKey: { challenge: 'AQID', allowCredentials: [] } },
      }),
      'POST /api/auth/step-up/passkey/finish': new Response(null, { status: 204 }),
    })
    await open(server)
    await userEvent.click(await screen.findByRole('button', { name: 'Confirm with passkey' }))
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('done'))
    expect(server.sent.find((s) => s.path === '/api/auth/step-up/passkey/finish')?.body).toEqual({
      ceremony_id: 's-1',
      credential: { id: 'cred' },
    })
    vi.unstubAllGlobals()
    Object.defineProperty(navigator, 'credentials', { value: undefined, configurable: true })
  })
})
