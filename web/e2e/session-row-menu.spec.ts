import { expect, test, type Page } from '@playwright/test'
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { startCollector, type Collector } from './collector'
import { defaultHostDir, fakeAgent, testHost, type TestHost } from './host'

async function setup(page: Page, collector: Collector, host: TestHost) {
  await page.goto(collector.setupLink)
  await page.getByLabel(/^Password at least/).fill('correct horse battery staple')
  await page.getByLabel('Password again', { exact: true }).fill('correct horse battery staple')
  await page.getByRole('button', { name: 'Set up', exact: true }).click()
  await page.getByRole('button', { name: 'Skip', exact: true }).click()
  const request = page.context().request, headers = { origin: collector.origin }
  const code = await (await request.post('/api/hosts/pairing-codes', { headers })).json()
  expect(await host.join(`hennery host join ${collector.origin} ${code.code}`, ['--name', 'Menu host', '--no-runtime'])).toBe(0)
  const path = join(defaultHostDir(host.dir), 'host.toml')
  writeFileSync(path, `workspace_roots = [${JSON.stringify(realpathSync(host.dir))}]\n${readFileSync(path, 'utf8')}`)
  host.run()
  let id = ''
  await expect(async () => {
    const hosts = await (await request.get('/api/hosts')).json()
    const found = hosts.find((host: { name: string; connected: boolean }) => host.name === 'Menu host' && host.connected)
    expect(found).toBeTruthy(); id = found.host_id
  }).toPass({ timeout: 30_000 })
  const cwd = join(realpathSync(host.dir), 'project'); mkdirSync(cwd)
  const started = await request.post('/api/sessions', { headers, data: { host_id: id, agent: 'claude', cwd } })
  expect(started.ok()).toBe(true)
  const sessionId = (await started.json()).session_id
  expect((await request.patch(`/api/sessions/${sessionId}`, { headers, data: { title: 'Menu source' } })).ok()).toBe(true)
  return { sessionId, headers }
}

for (const width of [390, 1280]) {
  test(`list actions match the header and retain dialogs across row removal at ${width}px`, async ({ browser }) => {
    test.setTimeout(120_000)
    const collector = await startCollector(), host = testHost(fakeAgent('claude', { asks: ['permission'], chunks: ['Ready.'] }))
    const context = await browser.newContext({ baseURL: collector.origin, viewport: { width, height: 844 }, reducedMotion: 'reduce' })
    const page = await context.newPage(), request = context.request
    const errors: string[] = []
    page.on('pageerror', error => errors.push(error.message))
    let releaseDelete = () => {}
    try {
      const { sessionId, headers } = await setup(page, collector, host)
      const path = `/api/sessions/${sessionId}`
      await page.goto(`/sessions/${sessionId}`)
      await page.getByRole('button', { name: 'Session actions', exact: true }).click()
      const names = await page.locator('.session-menu-list button').evaluateAll(buttons => buttons.map(button => button.getAttribute('aria-label') || button.textContent))
      await page.goto('/sessions')
      const row = page.locator(`tr[data-session-id="${sessionId}"]`)
      const trigger = row.getByRole('button', { name: 'Session actions', exact: true })
      await trigger.focus(); await page.keyboard.press('Enter')
      const menu = page.locator('.session-row-menu')
      await expect(menu).toBeVisible()
      expect(await menu.locator('button').evaluateAll(buttons => buttons.map(button => button.getAttribute('aria-label') || button.textContent))).toEqual(names)
      await expect(menu.getByRole('button', { name: 'Rename', exact: true })).toBeFocused()
      expect(await menu.evaluate(element => element.parentElement === document.body)).toBe(true)
      await page.keyboard.press('Escape'); await expect(trigger).toBeFocused()
      await row.locator('.sess-item').click({ button: 'right' }); await expect(menu).toBeVisible()
      await menu.getByRole('button', { name: 'Rename', exact: true }).click()
      const rename = page.getByRole('dialog', { name: 'Rename session', exact: true })
      await expect(rename.getByRole('textbox', { name: 'Session name', exact: true })).toBeFocused()
      await rename.getByRole('textbox', { name: 'Session name', exact: true }).fill('Renamed locally')
      await rename.getByRole('button', { name: 'Save', exact: true }).click()
      await expect(rename).toHaveCount(0); await expect(trigger).toBeFocused()
      await expect(row.getByRole('link')).toContainText('Renamed locally')
      await expect(page).toHaveURL(`${collector.origin}/sessions`)

      await trigger.click()
      const pin = menu.getByRole('switch', { name: 'Pin to sidebar', exact: true })
      await expect(pin).toHaveAttribute('aria-checked', 'false')
      await pin.click()
      await expect(pin).toHaveAttribute('aria-checked', 'true')
      await expect(pin).toHaveText('Pin to sidebar✓ On')
      const parking = menu.getByRole('switch', { name: 'Do not park', exact: true })
      await expect(parking).toHaveAttribute('aria-checked', 'false')
      await parking.click(); await expect(parking).toHaveAttribute('aria-checked', 'true')
      expect((await (await request.get(path)).json()).sidebar_pinned).toBe(true)
      expect((await (await request.get(`${path}/idle-hold`)).json()).pinned).toBe(true)

      // The complete menu, including its asynchronous hold note, stays inside
      // a short viewport and can scroll without dismissing itself.
      await page.setViewportSize({ width, height: 420 })
      await expect(menu).toHaveCount(0)
      await trigger.click(); await expect(menu).toBeVisible()
      await expect(menu).toBeInViewport({ ratio: 1 })
      const box = await menu.boundingBox()
      expect(box).not.toBeNull(); expect(box!.y).toBeGreaterThanOrEqual(0); expect(box!.y + box!.height).toBeLessThanOrEqual(420)
      await menu.evaluate(element => { element.scrollTop = element.scrollHeight; element.dispatchEvent(new Event('scroll')) })
      await expect(menu).toBeVisible()
      await expect(menu.getByRole('button', { name: 'Delete session', exact: true })).toBeInViewport({ ratio: 1 })
      if (width === 390) {
        const buttons = await menu.locator('button').evaluateAll(buttons => buttons.map(button => button.getBoundingClientRect().height))
        expect(buttons.every(height => height >= 44)).toBe(true)
      }
      const artifact = join(process.cwd(), '../.superpowers/menu'); mkdirSync(artifact, { recursive: true })
      await page.screenshot({ path: join(artifact, `menu-${width}.png`) })
      await page.keyboard.press('Escape')
      await page.setViewportSize({ width, height: 844 })

      await page.route('**' + path + '/park', route => route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ code: 'not_attached' }) }))
      await trigger.click(); await menu.getByRole('button', { name: 'Park', exact: true }).click()
      await expect(page.getByRole('alert')).toContainText('there is nothing to park')
      await expect(page.getByRole('alert')).toBeInViewport({ ratio: 1 })
      await page.unroute('**' + path + '/park')
      // Queue behind a blocked turn before parking. A new prompt after Park
      // intentionally releases the hold and resumes, racing Close with starting.
      const running = await request.post(`${path}/prompt`, { headers, data: { content: [{ type: 'text', text: 'Wait for permission before continuing.' }] } })
      expect(running.status()).toBe(202)
      await expect(async () => expect((await (await request.get(path)).json()).activity).toBe('blocked')).toPass()
      const queued = await request.post(`${path}/prompt`, { headers, data: { content: [{ type: 'text', text: 'Retained queued message' }] } })
      expect(queued.status()).toBe(202)
      const queueId = (await queued.json()).queue_id
      expect(typeof queueId).toBe('string')
      await trigger.click(); await menu.getByRole('button', { name: 'Park', exact: true }).click()
      await expect(async () => expect((await (await request.get(path)).json()).lifecycle).toBe('parked')).toPass()
      expect(await (await request.get(`${path}/queue`)).json()).toEqual([expect.objectContaining({ id: queueId, state: 'queued' })])
      await trigger.click(); await menu.getByRole('button', { name: 'Close', exact: true }).click()
      await expect(row).toHaveCount(0)
      expect((await (await request.get(path)).json()).lifecycle).toBe('closed')
      expect(await (await request.get(`${path}/queue`)).json()).toEqual([expect.objectContaining({ id: queueId, state: 'failed', content: [{ type: 'text', text: 'Retained queued message' }] })])
      // Include the closed source again; Close kept the session and its queue.
      await page.evaluate(() => localStorage.setItem('hennery.hideClosed', 'false'))
      await page.reload(); await expect(row).toBeVisible()
      await trigger.click()
      await expect(menu.getByRole('button', { name: 'Close', exact: true })).toHaveCount(0)
      await menu.getByRole('button', { name: 'Delete session', exact: true }).click()
      const dialog = page.getByRole('dialog', { name: 'Delete this session?', exact: true })
      await expect(dialog).toContainText('1 unsent queued message and its images will also be deleted.')
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
      await expect(trigger).toBeFocused()
      await trigger.click(); await menu.getByRole('button', { name: 'Delete session', exact: true }).click()
      // Let real deletion/SSE finish before delivering the HTTP result to UI.
      const gate = new Promise<void>(resolve => { releaseDelete = resolve })
      await page.route(`**${path}`, async route => {
        if (route.request().method() !== 'DELETE') return route.continue()
        const response = await route.fetch()
        await gate
        await route.fulfill({ response })
      })
      await dialog.getByRole('button', { name: 'Delete', exact: true }).click()
      await expect(row).toHaveCount(0)
      await expect(dialog).toBeVisible()
      releaseDelete()
      await expect(dialog).toHaveCount(0)
      await expect(page.getByRole('status').filter({ hasText: 'This session was deleted.' })).toBeVisible()
      await expect(page.getByRole('heading', { name: 'Sessions', exact: true })).toBeFocused()
      expect((await request.get(path)).status()).toBe(404)
      expect(errors).toEqual([])
    } finally { releaseDelete(); await context.close(); await host.stop(); await collector.stop() }
  })
}
