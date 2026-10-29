import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it } from 'vitest'
import { json, routed } from '../test-stream'
import PinSessionAction from './PinSessionAction'

it('pins and unpins through the session API, with encoded ids and no parking request', async () => {
  let pinned = false
  const server = routed(() => { pinned = !pinned; return json({ sidebar_pinned: pinned }) })
  const h = render(<PinSessionAction id="session/1" pinned={false} disabled={false} />, { wrapper: server.wrapper })
  const pin = screen.getByRole('switch', { name: 'Pin to sidebar', checked: false })
  expect(pin).toHaveTextContent('Off')
  pin.focus()
  fireEvent.click(pin)
  await waitFor(() => expect(pin).toBeEnabled())
  h.rerender(<PinSessionAction id="session/1" pinned disabled={false} />)
  const unpin = await screen.findByRole('switch', { name: 'Pin to sidebar', checked: true })
  expect(unpin).toHaveTextContent('✓ On')
  await waitFor(() => expect(unpin).toHaveFocus())
  expect(server.calls.map(c => [c.method, c.path])).toEqual([['PATCH', '/api/sessions/session%2F1']])
  expect(JSON.parse(String(server.fetch.mock.calls[0][1]?.body))).toEqual({ sidebar_pinned: true })
  fireEvent.click(unpin)
  await waitFor(() => expect(unpin).toBeEnabled())
  h.rerender(<PinSessionAction id="session/1" pinned={false} disabled={false} />)
  expect(await screen.findByRole('switch', { name: 'Pin to sidebar', checked: false })).toHaveTextContent('Off')
  expect(JSON.parse(String(server.fetch.mock.calls[1][1]?.body))).toEqual({ sidebar_pinned: false })
})

it('keeps the latest streamed state when an older save response arrives after another window unpinned', async () => {
  let finish!: (response: Response) => void
  const response = new Promise<Response>(resolve => { finish = resolve })
  const server = routed(() => response)
  const h = render(<PinSessionAction id="s" pinned={false} disabled={false} />, { wrapper: server.wrapper })
  fireEvent.click(screen.getByRole('switch', { name: 'Pin to sidebar', checked: false }))
  await waitFor(() => expect(server.calls).toHaveLength(1))
  h.rerender(<PinSessionAction id="s" pinned disabled={false} />)
  h.rerender(<PinSessionAction id="s" pinned={false} disabled={false} />)
  await act(async () => { finish(json({ sidebar_pinned: true })); await response })
  await waitFor(() => expect(screen.getByRole('switch', { name: 'Pin to sidebar', checked: false })).toBeEnabled())
  expect(screen.queryByRole('switch', { name: 'Pin to sidebar', checked: true })).not.toBeInTheDocument()
})

it('keeps the saved state on failure and follows later server changes', async () => {
  const server = routed(() => json({ message: 'Unavailable' }, 503))
  const h = render(<PinSessionAction id="s" pinned disabled={false} />, { wrapper: server.wrapper })
  fireEvent.click(screen.getByRole('switch', { name: 'Pin to sidebar', checked: true }))
  expect(await screen.findByRole('alert')).toHaveTextContent('Could not unpin')
  await waitFor(() => expect(screen.getByRole('switch', { name: 'Pin to sidebar', checked: true })).toBeEnabled())
  h.rerender(<PinSessionAction id="s" pinned={false} disabled={false} />)
  expect(screen.getByRole('switch', { name: 'Pin to sidebar', checked: false })).toBeEnabled()
})
