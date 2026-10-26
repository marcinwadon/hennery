import { expect, test } from '@playwright/test'
import { createServer } from 'node:net'
import { createServer as httpsServer } from 'node:https'
import { request as httpRequest } from 'node:http'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { startCollector } from './collector'
import { spawnHennery } from './spawn'
import { startVendor } from './oauth-vendor'

test.use({ launchOptions: { args: ['--host-resolver-rules=MAP collector.example 127.0.0.1', '--no-proxy-server'] } })

for (const scheme of ['http', 'https']) for (const width of [1280, 390]) test(`${scheme} local OAuth returns to collector domain with its own cookie at ${width}px`, async ({ browser }) => {
  const collector = await startCollector(), vendor = await startVendor()
  // Public test-only key/cert; no OS trust changes. A real TLS forwarder is
  // needed: Playwright routing does not intercept later hops of a redirect.
  const proxy = httpsServer({
    key: readFileSync(new URL('./fixtures/oauth-local-key.pem', import.meta.url)),
    cert: readFileSync(new URL('./fixtures/oauth-local-cert.pem', import.meta.url)),
  }, (request, response) => {
    const headers = { ...request.headers }; delete headers.host
    const upstream = httpRequest(collector.origin + request.url, { method: request.method, headers }, incoming => {
      response.writeHead(incoming.statusCode!, incoming.headers); incoming.pipe(response)
    })
    upstream.on('error', () => { response.destroy() })
    response.on('close', () => upstream.destroy())
    request.pipe(upstream)
  })
  await new Promise<void>(resolve => proxy.listen(0, '127.0.0.1', resolve))
  const proxyAddress = proxy.address(); if (!proxyAddress || typeof proxyAddress === 'string') throw new Error('no HTTPS port')
  const origin = `https://collector.example:${proxyAddress.port}`
  const context = await browser.newContext({ baseURL: origin, ignoreHTTPSErrors: true, viewport: { width, height: 844 } })
  const dir = mkdtempSync(join(tmpdir(), 'hennery-local-oauth-'))
  // Ask the OS for a free local port; the real CLI, not a mock receiver, binds it.
  const reserve = createServer()
  await new Promise<void>(resolve => reserve.listen(0, '127.0.0.1', resolve))
  const address = reserve.address(); if (!address || typeof address === 'string') throw new Error('no port')
  const local = `${scheme}://127.0.0.1:${address.port}/vendor/return`
  await new Promise<void>(resolve => reserve.close(() => resolve()))
  const helper = spawnHennery(['oauth-callback', '--collector', origin, '--redirect-uri', local], dir, { RUST_LOG: 'off' })
  let diagnostic = ''
  helper.stderr?.on('data', (chunk: Buffer) => { diagnostic += chunk.toString() })
  try {
    await expect.poll(() => diagnostic.includes('Local OAuth callback ready')).toBe(true)
    const page = await context.newPage()
    await page.goto('/setup')
    const setup = await page.evaluate(async ({ token, origin }) => (await fetch('/api/setup', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ token, password: 'correct horse battery staple', public_url: origin }) })).status, { token: new URL(collector.setupLink).hash.slice(1), origin })
    expect(setup).toBe(201)
    await page.goto('/mcp')
    await page.getByRole('button', { name: 'Add connection', exact: true }).click()
    const form = page.getByRole('region', { name: 'Add a connection', exact: true })
    await form.getByRole('textbox', { name: 'Name', exact: true }).fill('Local OAuth')
    await form.getByRole('textbox', { name: 'Slug', exact: true }).fill('local-oauth')
    await form.getByRole('textbox', { name: 'Server URL', exact: true }).fill(vendor.mcpUrl)
    await form.getByRole('combobox', { name: 'Authentication', exact: true }).selectOption('oauth_dcr')
    await form.getByRole('checkbox', { name: 'Internal network', exact: true }).check()
    await form.getByRole('button', { name: 'Create', exact: true }).click()
    const card = page.getByRole('listitem', { name: 'Local OAuth', exact: true })
    await card.locator('.connection-settings > summary').click()
    await card.getByRole('combobox', { name: 'OAuth callback', exact: true }).selectOption('local')
    await card.getByRole('textbox', { name: 'Local redirect URI', exact: true }).fill(local)
    const command = await card.getByLabel('Local callback command', { exact: true }).textContent()
    expect(command).toBe(`hennery oauth-callback --collector '${origin}' --redirect-uri '${local}'`)
    if (scheme === 'https') {
      const opened = context.waitForEvent('page')
      await card.getByRole('link', { name: 'Prepare HTTPS callback', exact: true }).click()
      const ready = await opened
      await expect(ready.getByText('Local OAuth callback ready. Return to Hennery and click Connect (or Reconnect).', { exact: true })).toBeVisible()
      expect(ready.url()).toBe(local)
      expect(vendor.counts.exchanges).toBe(0)
      await ready.close()
    }
    await page.screenshot({ path: test.info().outputPath('local-callback.png'), fullPage: true })
    const connect = async (button: string) => {
      const opened = context.waitForEvent('page')
      await card.getByRole('button', { name: button, exact: true }).click()
      const popup = await opened
      await expect(popup.getByRole('heading', { name: 'Vendor consent', exact: true })).toBeVisible()
      expect(new URL(popup.url()).searchParams.get('redirect_uri')).toBe(local)
      return popup
    }
    let spent = ''
    for (const button of ['Connect', 'Reconnect']) {
      const popup = await connect(button)
      const cookies = await context.cookies(origin + '/api/mcp/oauth/callback')
      const flow = cookies.find(c => c.name.startsWith('hennery_mcp_flow_'))!
      expect(flow).toMatchObject({ domain: 'collector.example', httpOnly: true, secure: true, sameSite: 'Lax', path: '/api/mcp/oauth/callback' })
      expect((await context.cookies(local)).some(c => c.name === flow.name)).toBe(false)
      const localResponse = popup.waitForResponse(r => r.url().startsWith(local))
      const returned = popup.waitForResponse(r => r.url().startsWith(origin + '/api/mcp/oauth/callback?'))
      await popup.getByRole('link', { name: 'Allow', exact: true }).click()
      const relay = await localResponse
      expect(relay.status()).toBe(302)
      expect(relay.headers()['cache-control']).toBe('no-store')
      expect(relay.headers()['referrer-policy']).toBe('no-referrer')
      const response = await returned
      expect(response.status()).toBe(200)
      const headers = await response.request().allHeaders()
      expect(headers.cookie).toContain(`${flow.name}=${flow.value}`)
      expect(headers.cookie).not.toContain('hennery_session=') // Strict session stays withheld cross-site.
      spent = response.url()
      expect(new URL(spent).search).toBe(new URL(relay.url()).search)
      await expect(card.getByText('Working', { exact: true })).toBeVisible()
      await expect.poll(() => popup.isClosed()).toBe(true)
    }
    expect(vendor.counts.registrations).toBe(1)
    expect(vendor.counts.exchanges).toBe(2)
    // Navigating again with a spent response cannot exchange or save again.
    const replay = await context.newPage()
    expect((await replay.goto(spent))?.status()).toBe(400)
    await expect(replay.getByText('This sign-in is unknown or has expired. Start it again from hennery.', { exact: true })).toBeVisible()
    await replay.close()
    const denied = await connect('Reconnect')
    const returned = denied.waitForResponse(r => r.url().startsWith(origin + '/api/mcp/oauth/callback?'))
    await denied.getByRole('link', { name: 'Deny', exact: true }).click()
    expect((await returned).status()).toBe(400)
    await expect(denied.getByText('The vendor said: access_denied: <script>denied</script>', { exact: true })).toBeVisible()
    expect(vendor.counts.exchanges).toBe(2)
    await denied.close()
    const missingCookie = await connect('Reconnect')
    await context.clearCookies({ name: /^hennery_mcp_flow_/ })
    const refused = missingCookie.waitForResponse(r => r.url().startsWith(origin + '/api/mcp/oauth/callback?'))
    await missingCookie.getByRole('link', { name: 'Allow', exact: true }).click()
    expect((await refused).status()).toBe(400)
    await expect(missingCookie.getByText('This sign-in was not started in this browser. Start it again from hennery.', { exact: true })).toBeVisible()
    expect(vendor.counts.exchanges).toBe(2)
    await missingCookie.close()
    await card.getByRole('button', { name: 'Stop waiting', exact: true }).click()
    expect(diagnostic).not.toContain('code=')
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  } finally {
    const stopped = new Promise<void>(resolve => helper.once('exit', () => resolve()))
    if (helper.exitCode === null && helper.signalCode === null) { helper.kill('SIGTERM'); await stopped }
    proxy.closeAllConnections()
    await new Promise<void>(resolve => proxy.close(() => resolve()))
    try { await context.close() } finally {
      try { await collector.stop() } finally { await vendor.stop(); rmSync(dir, { recursive: true, force: true }) }
    }
  }
})
