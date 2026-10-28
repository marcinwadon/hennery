import { expect, test, type Page, type Route } from '@playwright/test'
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { startCollector, type Collector } from './collector'
import { defaultHostDir, fakeAgent, testHost, type TestHost } from './host'
import type { SessionSummary } from '../src/generated/view'

async function setup(page: Page, collector: Collector, host: TestHost) {
  await page.goto(collector.setupLink)
  await page.getByLabel(/^Password at least/).fill('correct horse battery staple')
  await page.getByLabel('Password again', { exact: true }).fill('correct horse battery staple')
  await page.getByRole('button', { name: 'Set up', exact: true }).click()
  await page.getByRole('button', { name: 'Skip', exact: true }).click()
  const request = page.context().request, headers = { origin: collector.origin }
  const code = await (await request.post('/api/hosts/pairing-codes', { headers })).json()
  expect(await host.join(`hennery host join ${collector.origin} ${code.code}`, ['--name', 'Bulk host', '--no-runtime'])).toBe(0)
  const path = join(defaultHostDir(host.dir), 'host.toml')
  writeFileSync(path, `workspace_roots = [${JSON.stringify(realpathSync(host.dir))}]\n${readFileSync(path, 'utf8')}`)
  host.run()
  let hostId = ''
  await expect(async () => {
    const hosts = await (await request.get('/api/hosts')).json()
    const found = hosts.find((h: { name: string; connected: boolean }) => h.name === 'Bulk host' && h.connected)
    expect(found).toBeTruthy(); hostId = found.host_id
  }).toPass({ timeout: 30_000 })
  const cwd = join(realpathSync(host.dir), 'project'); mkdirSync(cwd)
  const ids: Record<string, string> = {}
  for (const label of ['Parent', 'Child', 'Retry', 'Unknown']) {
    const response = await request.post('/api/sessions', { headers, data: { host_id: hostId, agent: 'claude', cwd } })
    expect(response.ok()).toBe(true)
    const id = (await response.json()).session_id; ids[label] = id
    expect((await request.patch(`/api/sessions/${id}`, { headers, data: { title: label } })).ok()).toBe(true)
  }
  expect((await request.post(`/api/sessions/${ids.Parent}/prompt`, { headers, data: { content: [{ type: 'text', text: 'Keep this history.' }] } })).status()).toBe(202)
  await expect(async () => expect(JSON.stringify(await (await request.get(`/api/view/sessions/${ids.Parent}`)).json())).toContain('Retained bulk history.')).toPass()
  return { ids, headers }
}

for (const width of [390, 1280]) test(`bulk Close exact IDs, resync, partial retry and retained history at ${width}px`, async ({ browser }) => {
  test.setTimeout(150_000)
  const collector = await startCollector(), host = testHost(fakeAgent('claude', { chunks: ['Retained bulk history.'] }))
  const context = await browser.newContext({ baseURL: collector.origin, viewport: { width, height: 844 }, reducedMotion: 'reduce' })
  const page = await context.newPage(), request = context.request
  const errors: string[] = [], closes: string[] = [], reads: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('request', r => { if (r.method() === 'POST' && r.url().endsWith('/close')) closes.push(new URL(r.url()).pathname.split('/')[3]) })
  page.on('request', r => { if (r.method() === 'GET' && /^\/api\/sessions\/[^/]+$/.test(new URL(r.url()).pathname)) reads.push(new URL(r.url()).pathname.split('/')[3]) })
  let firstStream: Route | undefined, heldRead: Route | undefined
  try {
    const { ids } = await setup(page, collector, host)
    await context.addInitScript(() => { localStorage.setItem('hennery.subgroup', 'none'); localStorage.setItem('hennery.sort', 'started') })
    let omitParent = false
    // Synthetic family metadata over real sessions tests only UI target scope;
    // all successful closes/history/lifecycles use the real collector + host.
    await page.route(/\/api\/view\/sessions(?:\?.*)?$/, async route => {
      const response = await route.fetch(), body = await response.json()
      body.sessions = body.sessions.filter((s: SessionSummary) => !omitParent || s.session_id !== ids.Parent).map((s: SessionSummary) => s.session_id === ids.Parent
        ? { ...s, family: { revision: 1, children: [{ session_id: ids.Child, agent: 'claude' }], child_counts: { total: 1, active: 1, waiting: 0 }, attention: 0 } }
        : s.session_id === ids.Child ? { ...s, family: { revision: 1, parent: { session_id: ids.Parent, agent: 'claude' }, children: [], child_counts: { total: 0, active: 0, waiting: 0 }, attention: 0 } } : s)
      await route.fulfill({ response, json: body })
    })
    let held = false
    await page.route(/\/api\/stream\/sessions(?:\?.*)?$/, route => { if (!held) { held = true; firstStream = route; return } return route.continue() })
    let failRetry = true
    await page.route(`**/api/sessions/${ids.Retry}/close`, route => failRetry ? route.fulfill({ status: 409, json: { code: 'starting' } }) : route.continue())
    await page.route(`**/api/sessions/${ids.Unknown}/close`, route => route.fulfill({ status: 503, json: { code: 'delivery_unknown' } }))
    await page.goto('/sessions')
    await page.getByRole('button', { name: 'Select sessions', exact: true }).click()
    const box = (label: string) => page.getByRole('checkbox', { name: `Select ${label}`, exact: true })
    await box('Parent').check(); await expect(page).toHaveURL(collector.origin + '/sessions')
    expect(await box('Parent').evaluate(node => !node.closest('a'))).toBe(true)
    expect(await box('Parent').locator('..').evaluate(node => node.getBoundingClientRect().height)).toBeGreaterThanOrEqual(44)
    await expect(page.getByRole('button', { name: 'Session actions', exact: true })).toHaveCount(0)
    if (width === 390) await page.getByRole('button', { name: 'Filters', exact: true }).click()
    const search = page.getByRole('searchbox', { name: 'Search sessions', exact: true })
    await search.fill('Parent')
    await expect(page.getByRole('group', { name: 'Selected session actions', exact: true })).toContainText('0 selected')
    await search.clear()
    if (width === 390) await page.getByRole('button', { name: 'Close filters', exact: true }).click()
    await box('Parent').check()
    const expand = page.getByRole('button', { name: '↳ 1 child', exact: true })
    await expand.click(); await box('Child').check(); await expand.click()
    await expect(page.getByRole('region', { name: 'Selected sessions', exact: true })).toContainText('1 selected outside current rows')
    await page.getByRole('button', { name: 'Remove Child from selection', exact: true }).click()
    await box('Retry').focus(); await page.keyboard.press('Space'); await box('Unknown').check()
    await expect.poll(() => !!firstStream).toBe(true)
    omitParent = true
    await firstStream!.fulfill({ contentType: 'text/event-stream', body: 'event: resync_required\ndata: {}\n\n' }); firstStream = undefined
    await expect(box('Parent')).toHaveCount(0)
    await expect(page.getByRole('region', { name: 'Selected sessions', exact: true })).toContainText('1 selected outside current rows')
    const selectedClose = page.getByRole('button', { name: 'Close selected', exact: true })
    await selectedClose.click()
    const dialog = page.getByRole('dialog', { name: 'Close 3 selected sessions?', exact: true })
    await expect(dialog).toContainText('Transcripts are kept.')
    await expect(dialog.getByRole('list', { name: 'Sessions to close', exact: true }).locator('li')).toHaveCount(3)
    await expect(dialog.getByRole('button', { name: 'Cancel', exact: true })).toBeFocused()
    const artifact = join(process.cwd(), '../.superpowers/bulk-close'); mkdirSync(artifact, { recursive: true })
    await page.screenshot({ path: join(artifact, `confirmation-${width}.png`) })
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click(); await expect(selectedClose).toBeFocused()
    await page.screenshot({ path: join(artifact, `selection-${width}.png`) })
    await selectedClose.click(); await dialog.getByRole('button', { name: 'Close sessions', exact: true }).click()
    const results = page.getByRole('region', { name: 'Close results', exact: true })
    await expect(results).toContainText('Confirmation pending'); await expect(results).toContainText('Failed')
    expect([...closes].sort()).toEqual([ids.Parent, ids.Retry, ids.Unknown].sort())
    expect((await (await request.get(`/api/sessions/${ids.Child}`)).json()).lifecycle).toBe('active')
    expect(JSON.stringify(await (await request.get(`/api/view/sessions/${ids.Parent}`)).json())).toContain('Retained bulk history.')
    await results.getByRole('button', { name: 'Check status', exact: true }).click(); await expect(results).toContainText('current run')
    expect(closes).toHaveLength(3)
    await results.getByRole('button', { name: 'Review retry', exact: true }).click()
    const retry = page.getByRole('dialog', { name: 'Close 1 selected session?', exact: true })
    await expect(retry.getByRole('list', { name: 'Sessions to close', exact: true })).toContainText('Retry')
    failRetry = false
    await retry.getByRole('button', { name: 'Close sessions', exact: true }).click()
    await expect(results).toContainText('2 / 3 closed in Hennery')
    expect(closes.filter(id => id === ids.Retry)).toHaveLength(2)
    expect(closes.filter(id => id === ids.Parent)).toHaveLength(1)
    expect(closes.filter(id => id === ids.Unknown)).toHaveLength(1)
    await page.screenshot({ path: join(artifact, `results-${width}.png`) })
    // The toolbar must not bypass GET-only resolution of a delivery_unknown,
    // even though that session remains checked and has a live list summary.
    const beforeRead = reads.filter(id => id === ids.Unknown).length
    await selectedClose.click()
    const unknownConfirm = page.getByRole('dialog', { name: 'Close 1 selected session?', exact: true })
    await expect(unknownConfirm).toBeVisible()
    expect(reads.filter(id => id === ids.Unknown)).toHaveLength(beforeRead + 1)
    await expect(unknownConfirm.getByRole('list', { name: 'Sessions to close', exact: true })).toContainText('Unknown')
    expect(closes.filter(id => id === ids.Unknown)).toHaveLength(1)
    await unknownConfirm.getByRole('button', { name: 'Cancel', exact: true }).click()
    await expect(selectedClose).toBeFocused()
    // A held detail read followed by closed must never offer or send a POST.
    await page.route(`**/api/sessions/${ids.Unknown}`, route => { heldRead = route })
    await selectedClose.click()
    await expect.poll(() => !!heldRead).toBe(true)
    await expect(selectedClose).toBeDisabled()
    await expect(page.getByRole('dialog')).toHaveCount(0)
    expect(closes.filter(id => id === ids.Unknown)).toHaveLength(1)
    await heldRead!.fulfill({ json: { session_id: ids.Unknown, lifecycle: 'closed' } }); heldRead = undefined
    await expect(page.getByRole('list', { name: 'Selection check results', exact: true })).toContainText('Already closed in Hennery; no Close sent.')
    await expect(page.getByRole('dialog')).toHaveCount(0)
    expect(closes.filter(id => id === ids.Unknown)).toHaveLength(1)
    await page.getByRole('button', { name: 'Done', exact: true }).click()
    await expect(page.getByRole('button', { name: 'Select sessions', exact: true })).toBeFocused()
    await expect(page.getByRole('checkbox', { name: /^Select / })).toHaveCount(0)
    expect(errors).toEqual([])
  } finally { await heldRead?.abort().catch(() => {}); await firstStream?.abort().catch(() => {}); await context.close(); await host.stop(); await collector.stop() }
})
