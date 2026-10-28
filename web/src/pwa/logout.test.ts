import { afterEach, expect, it, vi } from 'vitest'
import { Client } from '../api/client'
import { completeLogout } from './logout'
import { hold, heldFor, forgetAllAttachments } from '../lib/attachments'
import * as journals from './journal'
import * as lifecycle from './lifecycle'
import { Journal, type PushRecord, type PushStore } from './journal'
import { deferred, testLocks } from '../test-locks'
import { SESSION_HISTORY_KEY, visitSession } from '../lib/sessionHistory'
import { changeConfig, configuringFor } from '../lib/configuring'

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); sessionStorage.clear(); forgetAllAttachments(); history.replaceState(null, '', '/') })
const client = () => new Client({ fetch: vi.fn(), here: () => ({ pathname: '/settings', search: '' }), navigate: vi.fn(), stepUp: vi.fn() })
it('invalidates a pending configuration on logout so its late answer cannot restore session state', async () => {
  let finish!: (response: Response) => void
  const changingClient = new Client({ fetch: vi.fn(() => new Promise<Response>(resolve => { finish = resolve })), here: () => ({ pathname: '/sessions/s1', search: '' }), navigate: vi.fn(), stepUp: vi.fn() })
  const change = changeConfig(changingClient, 's1', { id: 'model', name: 'Model', kind: 'select', current: 'a', choices: [{ value: 'b', name: 'B' }] }, 'b')
  expect(configuringFor('s1')).toBe(change)
  Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: undefined })
  await completeLogout(client(), false)
  expect(configuringFor('s1')).toBeUndefined()
  expect(change.valid).toBe(false)
  finish(new Response(JSON.stringify({ session_id: 's1', commands: [], config_options: [] }), { headers: { 'Content-Type': 'application/json' } }))
  await change.result
  expect(configuringFor('s1')).toBeUndefined()
})
it('a successful cleanup wins over a delayed outer timeout report', async () => {
  Object.defineProperty(navigator, 'locks', { configurable: true, value: testLocks() })
  const row: PushRecord = { generation: 1, enabled: true, id: 'id' }
  const store: PushStore = { async transact(fn) { return fn(row) } }
  vi.spyOn(journals, 'pushJournal').mockImplementation(() => new Journal(store))
  const registration = { pushManager: { getSubscription: async () => null }, getNotifications: async () => [] }
  Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: { getRegistration: async () => registration } })
  vi.spyOn(lifecycle, 'bounded').mockImplementationOnce((work) => work)
    .mockImplementationOnce((work) => work.then(() => { throw new Error('Delayed timeout report') }))
  await completeLogout(client(), false)
  expect(sessionStorage.getItem('hennery.logout-cleanup')).toBeNull()
})
it('cleanup finishing after the outer deadline clears the unconfirmed warning', async () => {
  vi.useFakeTimers()
  Object.defineProperty(navigator, 'locks', { configurable: true, value: testLocks() })
  const row: PushRecord = { generation: 1, enabled: true, id: 'id' }
  const store: PushStore = { async transact(fn) { return fn(row) } }
  vi.spyOn(journals, 'pushJournal').mockImplementation(() => new Journal(store))
  const cleaning = deferred(), cleaned = deferred<Notification[]>()
  const registration = { pushManager: { getSubscription: async () => null }, getNotifications: () => { cleaning.resolve(); return cleaned.promise } }
  Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: { getRegistration: async () => registration } })
  const logout = completeLogout(client(), false)
  await cleaning.promise
  await vi.advanceTimersByTimeAsync(10001)
  await logout
  expect(sessionStorage.getItem('hennery.logout-cleanup')).toContain('unconfirmed')
  cleaned.resolve([])
  await vi.advanceTimersByTimeAsync(0)
  expect(sessionStorage.getItem('hennery.logout-cleanup')).toBeNull()
})
it('overlapping confirmed logouts leave no warning after both local cleanups finish', async () => {
  vi.useFakeTimers()
  Object.defineProperty(navigator, 'locks', { configurable: true, value: testLocks() })
  const row: PushRecord = { generation: 1, enabled: true, id: 'id', endpoint: 'https://push.example/a' }
  const store: PushStore = { async transact(fn) { return fn(row) } }
  vi.spyOn(journals, 'pushJournal').mockImplementation(() => new Journal(store))
  const registration = { pushManager: { getSubscription: async () => null }, getNotifications: async () => [] }
  Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: { getRegistration: async () => registration } })
  const a = completeLogout(client(), false), b = completeLogout(client(), false)
  await vi.advanceTimersByTimeAsync(10001)
  await a; await b
  await vi.advanceTimersByTimeAsync(2000)
  expect(sessionStorage.getItem('hennery.logout-cleanup')).toBeNull()
})
it('confirmed logout clears drafts and images, broadcasts and goes to login even if worker cleanup fails', async () => {
  visitSession('019a0180-1234-7000-8000-000000000001')
  sessionStorage.setItem('hennery.draft.s1', 'draft')
  hold('s1', { attachments: [{ n: 1, file: new File(['a'], 'a.png', { type: 'image/png' }) }], nextN: 2 })
  const postMessage = vi.fn(), close = vi.fn()
  vi.stubGlobal('BroadcastChannel', class { postMessage = postMessage; close = close })
  Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: { getRegistration: vi.fn().mockRejectedValue(new Error('storage')) } })
  await completeLogout(client())
  expect(location.pathname).toBe('/login')
  expect(localStorage.getItem(SESSION_HISTORY_KEY)).toBeNull()
  expect(sessionStorage.getItem('hennery.draft.s1')).toBeNull()
  expect(heldFor('s1').attachments).toEqual([])
  expect(postMessage).toHaveBeenCalledWith({ version: 1, kind: 'logout' })
  expect(close).toHaveBeenCalled()
  expect(sessionStorage.getItem('hennery.logout-cleanup')).toContain('Signed out.')
})
it('a received logout never broadcasts again, and successful local cleanup clears its warning', async () => {
  sessionStorage.setItem('hennery.logout-cleanup', 'old warning')
  const postMessage = vi.fn()
  vi.stubGlobal('BroadcastChannel', class { postMessage = postMessage; close = vi.fn() })
  Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: { getRegistration: vi.fn().mockResolvedValue(undefined) } })
  await completeLogout(client(), false)
  expect(postMessage).not.toHaveBeenCalled()
  expect(sessionStorage.getItem('hennery.logout-cleanup')).toBeNull()
})
