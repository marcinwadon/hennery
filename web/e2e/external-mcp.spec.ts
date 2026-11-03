import { expect, test, request } from '@playwright/test'
import { createServer } from 'node:https'
import { request as httpRequest } from 'node:http'
import { readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { startCollector } from './collector'
import { testHost } from './host'

const PASSWORD = 'correct horse battery staple'
for (const width of [1280, 390]) for (const outcome of ['allow', 'deny'] as const) {
  test(`external remote OAuth ${outcome} over isolated HTTPS at ${width}px`, async ({ browser }) => {
    const collector = await startCollector()
    const host = testHost()
    const proxy = createServer({
      key: readFileSync(new URL('./fixtures/oauth-local-key.pem', import.meta.url)),
      cert: readFileSync(new URL('./fixtures/oauth-local-cert.pem', import.meta.url)),
    }, (incoming, outgoing) => {
      const upstream = httpRequest(collector.origin + incoming.url, { method: incoming.method, headers: incoming.headers }, response => {
        outgoing.writeHead(response.statusCode!, response.headers); response.pipe(outgoing)
      })
      upstream.on('error', () => outgoing.destroy())
      outgoing.on('close', () => upstream.destroy())
      incoming.pipe(upstream)
    })
    await new Promise<void>(resolve => proxy.listen(0, '127.0.0.1', resolve))
    const address = proxy.address(); if (!address || typeof address === 'string') throw Error('No TLS listener')
    const origin = `https://localhost:${address.port}`
    const setupContext = await browser.newContext({ baseURL: collector.origin })
    const context = await browser.newContext({ baseURL: origin, ignoreHTTPSErrors: true, viewport: { width, height: 844 } })
    const remote = await request.newContext({ baseURL: origin, ignoreHTTPSErrors: true })
    try {
      const setup = await setupContext.request.post('/api/setup', { headers: { Origin: collector.origin }, data: { token: new URL(collector.setupLink).hash.slice(1), password: PASSWORD, public_url: collector.origin } })
      expect(setup.status()).toBe(201)
      const setupPage = await setupContext.newPage()
      await setupPage.goto('/hosts')
      await setupPage.getByRole('button', { name: 'Add host', exact: true }).click()
      const command = (await setupPage.getByLabel('Pairing command', { exact: true }).textContent())!
      expect(await host.join(command, ['--name', 'External fixture host', '--no-runtime'])).toBe(0)
      const changed = await setupContext.request.patch('/api/settings', { headers: { Origin: collector.origin }, data: { public_url: origin } })
      expect(changed.status()).toBe(200)
      const challenge = await remote.post('/mcp/external/hennery', { data: {} })
      expect(challenge.status()).toBe(401)
      expect(challenge.headers()['www-authenticate']).toContain('resource_metadata=')
      const metadata = await remote.get('/.well-known/oauth-authorization-server')
      expect((await metadata.json()).issuer).toBe(origin)
      const clientResponse = await remote.post('/oauth/external/register', { data: { client_name: 'Remote fixture app', redirect_uris: ['https://client.example/callback'], token_endpoint_auth_method: 'none', grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'] } })
      expect(clientResponse.status()).toBe(201)
      const client = (await clientResponse.json()).client_id as string
      const verifier = 'a'.repeat(43)
      const query = new URLSearchParams({ client_id: client, redirect_uri: 'https://client.example/callback', response_type: 'code', code_challenge: createHash('sha256').update(verifier).digest('base64url'), code_challenge_method: 'S256', resource: origin + '/mcp/external/hennery', scope: 'hennery:discover hennery:sessions:read hennery:sessions:control hennery:sessions:transcript offline_access', state: 'remote-state' })
      let callback: URL | null = null
      await context.route('https://client.example/callback**', async route => { callback = new URL(route.request().url()); await route.fulfill({ status: 200, contentType: 'text/html', body: '<h1>Remote client callback fixture</h1>' }) })
      const page = await context.newPage()
      await page.goto(origin + '/oauth/external/authorize?' + query)
      await expect(page.getByRole('heading', { name: 'Sign in', exact: true })).toBeVisible()
      expect(page.url()).toContain('/login?next=')
      await page.getByLabel('Password', { exact: true }).fill(PASSWORD)
      await page.getByRole('button', { name: 'Sign in', exact: true }).click()
      await expect(page.getByRole('heading', { name: 'Connect an external MCP client', exact: true })).toBeVisible()
      await expect(page.getByRole('heading', { name: 'Connect an external MCP client', exact: true })).toBeInViewport()
      await expect(page.getByText('client.example', { exact: true })).toBeVisible()
      await expect(page.getByLabel('Also allow existing sessions within this hat, host and root', { exact: true })).not.toBeChecked()
      await expect(page.getByLabel('Create, prompt and control sessions', { exact: true })).not.toBeChecked()
      await expect(page.getByLabel('Read session conversations', { exact: true })).not.toBeChecked()
      const before = await page.request.get('/api/external-clients'); expect(await before.json()).toEqual([])
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      if (outcome === 'allow') {
        await page.getByLabel('Hat', { exact: true }).selectOption({ index: 1 })
        await page.getByLabel('Host', { exact: true }).selectOption({ index: 1 })
        await page.getByLabel('Workspace root', { exact: true }).fill('/project')
        await page.getByLabel('Also allow existing sessions within this hat, host and root', { exact: true }).check()
        await page.getByLabel('Create, prompt and control sessions', { exact: true }).check()
        await page.getByLabel('Read session conversations', { exact: true }).check()
        await page.getByRole('heading', { name: 'Connect an external MCP client', exact: true }).scrollIntoViewIfNeeded()
        await page.screenshot({ path: test.info().outputPath('external-consent.png'), fullPage: true })
        await page.getByRole('button', { name: 'Allow selected access', exact: true }).click()
      } else await page.getByRole('button', { name: 'Deny', exact: true }).click()
      await expect(page.getByRole('heading', { name: 'Remote client callback fixture', exact: true })).toBeVisible()
      const returned = callback as URL | null
      expect(returned).not.toBeNull()
      expect(returned!.searchParams.get('state')).toBe('remote-state')
      expect(returned!.searchParams.get('iss')).toBe(origin)
      if (outcome === 'deny') {
        expect(returned!.searchParams.get('error')).toBe('access_denied')
        expect(returned!.searchParams.has('code')).toBe(false)
        await page.goto(origin + '/external-mcp')
        const listed = await page.request.get('/api/external-clients'); expect(await listed.json()).toEqual([])
      } else {
        const tokenResponse = await remote.post('/oauth/external/token', { form: { grant_type: 'authorization_code', client_id: client, resource: origin + '/mcp/external/hennery', redirect_uri: 'https://client.example/callback', code: returned!.searchParams.get('code')!, code_verifier: verifier } })
        expect(tokenResponse.status()).toBe(200)
        const token = await tokenResponse.json()
        const rpc = async (bearer: string) => remote.post('/mcp/external/hennery', { headers: { Authorization: `Bearer ${bearer}`, Accept: 'application/json, text/event-stream' }, data: { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'host_list', arguments: {} } } })
        const hosts = await rpc(token.access_token)
        expect(hosts.status()).toBe(200)
        expect((await hosts.json()).result.structuredContent.data.hosts[0].name).toBe('External fixture host')
        const capabilities = await remote.post('/mcp/external/hennery', { headers: { Authorization: `Bearer ${token.access_token}`, Accept: 'application/json, text/event-stream' }, data: { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'hennery_capabilities', arguments: {} } } })
        expect((await capabilities.json()).result.structuredContent.data.execution).toBe(true)
        await page.goto(origin + '/external-mcp')
        await page.getByRole('button', { name: 'Revoke access to Remote fixture app', exact: true }).click()
        await expect(page.getByText('Revoked', { exact: true })).toBeVisible()
        expect((await rpc(token.access_token)).status()).toBe(401)
        expect((await remote.post('/oauth/external/token', { form: { grant_type: 'refresh_token', client_id: client, resource: origin + '/mcp/external/hennery', refresh_token: token.refresh_token } })).status()).toBe(400)
      }
    } finally {
      proxy.closeAllConnections()
      const stopped = await Promise.allSettled([
        remote.dispose(), setupContext.close(), context.close(), host.stop(), collector.stop(),
        new Promise<void>(resolve => proxy.close(() => resolve())),
      ])
      const failed = stopped.find(result => result.status === 'rejected')
      if (failed?.status === 'rejected') throw failed.reason
    }
  })
}
