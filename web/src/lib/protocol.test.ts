import { expect, it, vi } from 'vitest'
import { Client } from '../api/client'
import { json, stubServer } from '../test-server'
import { confirmStart } from './start'

it('confirms negotiated protocol independently of the adapter option catalog', async () => {
  const server = stubServer({
    'GET /api/sessions/s': json(200, { hat_id: 'hat', lifecycle: 'active', acp_protocol: 1 }),
    'GET /api/sessions/s/catalog': json(200, { config_options: [], commands: [] }),
  })
  const client = new Client({ fetch: server.fetch, navigate: vi.fn(), here: () => ({ pathname: '/', search: '' }), stepUp: vi.fn() })
  expect(await confirmStart(client, 's', 'hat', { protocol: 'v2-v1' }, 100)).toBe('confirmed')
  expect(await confirmStart(client, 's', 'hat', { protocol: 'v2' }, 100)).toBe('config_unconfirmed')
})
