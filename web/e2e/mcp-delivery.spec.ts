import { captureManagement } from './management-captures'
// Real dispatch receipts, not MCP tool execution: the Codex-named fake
// records session/new and session/load under its own scratch directory.
import { expect, test, type Page } from '@playwright/test'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { startCollector, type Collector } from './collector'
import { fakeAgent, testHost, type TestHost } from './host'
import { scratchEnv } from './spawn'

const PASSWORD = 'correct horse battery staple'
const ENV_VALUE = 'e2e-environment-value-0123456789'
async function stepUp(page: Page) {
  const dialog = page.getByRole('dialog', { name: 'This needs a fresh confirmation', exact: true })
  await expect(dialog).toBeVisible()
  await dialog.getByLabel('Your password', { exact: true }).fill(PASSWORD)
  await dialog.getByRole('button', { name: 'Confirm', exact: true }).click()
  await expect(dialog).toBeHidden()
}

for (const width of [1280, 390]) test.describe(`MCP delivery at ${width} px`, () => {
  test.describe.configure({ mode: 'serial' })
  let collector: Collector, host: TestHost, page: Page
  let project: string, receipt: string, sessionUrl: string, hostId: string, hatId: string, sentinel: string
  let runnerLog: string | undefined
  const violations: string[] = []
  const records = () => existsSync(receipt) ? readFileSync(receipt, 'utf8').trim().split('\n').map(line => JSON.parse(line)) : []
  const local = () => page.getByRole('region', { name: 'Local MCP servers', exact: true })
  const report = () => page.getByRole('region', { name: 'MCP delivery', exact: true })
  // R3 moves the last-start snapshot into the session's Details tab.
  const openReport = async () => {
    const toggle = page.getByRole('button', { name: 'Session details', exact: true })
    if (await toggle.getAttribute('aria-pressed') !== 'true') await toggle.click()
    if (width === 390) await page.getByRole('tab', { name: 'Details', exact: true }).click()
  }
  const closeReportOnPhone = async () => {
    if (width === 390) await page.getByRole('button', { name: 'Close session details', exact: true }).click()
  }
  test.beforeAll(async ({ browser }) => {
    sentinel = mkdtempSync(join(tmpdir(), 'hennery-e2e-sentinel-'))
    runnerLog = process.env.HENNERY_LOG_DIR; process.env.HENNERY_LOG_DIR = sentinel
    expect(scratchEnv(sentinel).HENNERY_LOG_DIR).toBeUndefined()
    collector = await startCollector()
    // The log is scratch too, and never printed: it can contain env values.
    const agentScratch = mkdtempSync(join(tmpdir(), 'hennery-e2e-receipts-'))
    receipt = join(agentScratch, 'session.jsonl')
    host = testHost(fakeAgent('codex', { chunks: ['Done.'], session_log: receipt }))
    project = join(realpathSync(host.dir), 'notes'); mkdirSync(project)
    const context = await browser.newContext({ baseURL: collector.origin, viewport: { width, height: 844 } })
    page = await context.newPage()
    await page.addInitScript(() => document.addEventListener('securitypolicyviolation', e => {
      const seen = ((window as unknown as { __csp?: string[] }).__csp ??= [])
      seen.push(`${e.violatedDirective} ${e.blockedURI}`)
    }))
    page.on('console', m => { if (m.text().includes('Content Security Policy')) violations.push(m.text()) })
    const setup = await page.request.post('/api/setup', { headers: { Origin: collector.origin }, data: { token: new URL(collector.setupLink).hash.slice(1), password: PASSWORD, public_url: collector.origin } })
    expect(setup.status()).toBe(201)
  })
  test.afterAll(async () => {
    try { await page?.context().close() } finally {
      try { await host?.stop() } finally {
        try { await collector?.stop() } finally {
          if (runnerLog === undefined) delete process.env.HENNERY_LOG_DIR; else process.env.HENNERY_LOG_DIR = runnerLog
          if (receipt) rmSync(join(receipt, '..'), { recursive: true, force: true })
          if (sentinel) rmSync(sentinel, { recursive: true, force: true })
        }
      }
    }
  })
  test('pairs a real host running a Codex-named fake', async () => {
    await page.goto('/hosts'); await page.getByRole('button', { name: 'Add host', exact: true }).click()
    const command = (await page.getByLabel('Pairing command', { exact: true }).textContent())!
    expect(await host.join(command, ['--name', 'e2e host', '--no-runtime'])).toBe(0)
    await expect(page.getByText('Paired: e2e host', { exact: true })).toBeVisible({ timeout: 15_000 })
    host.run()
    await expect(async () => {
      const response = await page.request.get('/api/hosts'); expect(response.status()).toBe(200)
      const hosts = await response.json(); expect(hosts[0].connected).toBe(true)
      hostId = hosts[0].host_id; hatId = hosts[0].default_hat_id
    }).toPass({ timeout: 20_000 })
  })
  test('saves a full local set through step-up, reads no values back and starts with its actual delivery', async () => {
    await page.goto('/mcp'); await expect(local()).toBeVisible()
    // The existing connection form creates and mounts the HTTP entry.
    await page.getByRole('button', { name: 'Add connection', exact: true }).click()
    const connection = page.getByRole('region', { name: 'Add a connection', exact: true })
    await connection.getByRole('textbox', { name: 'Name', exact: true }).fill('Docs')
    await connection.getByRole('textbox', { name: 'Slug', exact: true }).fill('docs')
    await connection.getByRole('textbox', { name: 'Server URL', exact: true }).fill('https://mcp.example.com/mcp')
    await connection.getByRole('combobox', { name: 'Authentication', exact: true }).selectOption('none')
    await connection.getByRole('button', { name: 'Create', exact: true }).click()
    const tick = page.getByRole('listitem', { name: 'Docs', exact: true }).getByRole('checkbox', { name: 'e2e host', exact: true })
    await tick.click(); await expect(tick).toBeChecked()
    await local().getByRole('button', { name: 'Add local server', exact: true }).click()
    await local().getByRole('textbox', { name: 'Server name', exact: true }).fill('local')
    await local().getByRole('textbox', { name: 'Command', exact: true }).fill('tool')
    await local().getByRole('button', { name: 'Add environment variable', exact: true }).click()
    await local().getByRole('textbox', { name: 'Environment name', exact: true }).fill('TOKEN')
    await local().getByRole('textbox', { name: 'Value for TOKEN', exact: true }).fill(ENV_VALUE)
    await captureManagement(page, 'mcp-stdio', 'Desktop mcp / local stdio')
    await local().getByRole('button', { name: 'Review local servers', exact: true }).click()
    await page.getByRole('dialog', { name: 'Save these local MCP servers?', exact: true }).getByRole('button', { name: 'Cancel', exact: true }).click()
    await expect(local().getByRole('textbox', { name: 'Value for TOKEN', exact: true })).toHaveValue('')
    await local().getByRole('textbox', { name: 'Value for TOKEN', exact: true }).fill(ENV_VALUE)
    let refused = 0
    await page.route('**/api/mcp/stdio-servers?*', async route => {
      if (route.request().method() === 'PUT' && refused === 0) { refused++; await route.fulfill({ status: 403, contentType: 'application/json', body: JSON.stringify({ code: 'step_up_required' }) }) }
      else await route.continue()
    })
    await local().getByRole('button', { name: 'Review local servers', exact: true }).click()
    await page.getByRole('dialog', { name: 'Save these local MCP servers?', exact: true }).getByRole('button', { name: 'Save local servers', exact: true }).click()
    await expect(local().getByRole('textbox', { name: 'Value for TOKEN', exact: true })).toHaveValue('')
    await stepUp(page)
    await local().locator('.stdio-server-disclosure > summary').click()
    await expect(local().getByRole('checkbox', { name: 'Keep stored value for TOKEN', exact: true })).toBeChecked()
    expect(refused).toBe(1); await page.unroute('**/api/mcp/stdio-servers?*')
    const listed = await page.request.get(`/api/mcp/stdio-servers?host_id=${hostId}&hat_id=${hatId}`)
    expect(listed.status()).toBe(200)
    const body = await listed.text(); expect(body.includes(ENV_VALUE)).toBe(false)
    expect(JSON.parse(body).servers[0].env).toEqual([{ name: 'TOKEN', has_value: true }])
    expect(await page.locator('textarea').evaluateAll((fields, value) => fields.some(field => field instanceof HTMLTextAreaElement && field.value.includes(value)), ENV_VALUE)).toBe(false); expect(page.url().includes(ENV_VALUE)).toBe(false)
    expect(await page.evaluate(value => document.body.innerText.includes(value), ENV_VALUE)).toBe(false)
    expect(await page.evaluate(value => JSON.stringify({ ...localStorage, ...sessionStorage }).includes(value), ENV_VALUE)).toBe(false)
    if (width === 390) expect((await local().getByRole('button', { name: 'Review local servers', exact: true }).boundingBox())!.height).toBeGreaterThanOrEqual(44)
    await page.goto('/new')
    const form = page.getByRole('form', { name: 'New session', exact: true }); await form.getByRole('radio', { name: 'Codex', exact: true }).check()
    await form.getByRole('combobox').fill(project)
    await expect(form.getByRole('region', { name: 'MCP prediction', exact: true })).toHaveCount(0)
    await form.getByRole('textbox', { name: 'First prompt optional', exact: true }).fill('Show delivery.')
    // This delivery/session fixture reports no settings catalog.
    await form.getByRole('button', { name: 'Session settings', exact: true }).click()
    await expect(page.getByText('Agent defaults are unknown. Hat overrides and your choices will still be requested and checked after starting.', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Close session settings', exact: true }).click()
    // Reading a missing preview starts no disposable adapter. Count every
    // actual launch, including empty delivery for other hats below.
    const beforeStart = records().length
    await form.getByRole('button', { name: 'Start session', exact: true }).click()
    await expect(page).toHaveURL(/\/sessions\/[^/?#]+$/, { timeout: 45_000 }); sessionUrl = page.url()
    await openReport()
    await expect(report()).toContainText('At the last start/resume:')
    await expect(report().getByText('3 MCP servers.', { exact: true })).toBeVisible()
    await expect(report()).toContainText('Codex also loads your own ~/.codex MCP servers.')
    const started = records()[beforeStart]
    expect(started.method).toBe('session/new')
    expect(started.params.mcpServers.map((s: { name: string }) => s.name).sort()).toEqual(['hennery-docs', 'hennery-local', 'hennery_files'])
    expect(JSON.stringify(started.params.mcpServers).includes(ENV_VALUE)).toBe(true)
    await closeReportOnPhone()
    await page.reload(); await openReport(); await expect(report().getByText('3 MCP servers.', { exact: true })).toBeVisible()
  })
  test('keeps the last-start snapshot until resume, and delivers none for a different hat', async () => {
    await page.goto('/mcp'); await local().locator('.stdio-server-disclosure > summary').click(); await local().getByRole('button', { name: 'Add local server', exact: true }).click()
    await local().getByRole('textbox', { name: 'Server name', exact: true }).nth(1).fill('second')
    await local().getByRole('textbox', { name: 'Command', exact: true }).nth(1).fill('runner')
    await local().getByRole('button', { name: 'Review local servers', exact: true }).click()
    await page.getByRole('dialog', { name: 'Save these local MCP servers?', exact: true }).getByRole('button', { name: 'Save local servers', exact: true }).click()
    await expect(page.getByRole('dialog')).toHaveCount(0)
    await page.goto(sessionUrl); await openReport(); await expect(report().getByText('3 MCP servers.', { exact: true })).toBeVisible()
    await expect(page.locator('.session-head .badge')).toHaveText('Idle')
    await host.crash(); host.run()
    await expect(page.locator('.session-head .badge')).toHaveText('Parked', { timeout: 20_000 })
    await closeReportOnPhone()
    await page.getByRole('button', { name: 'Resume', exact: true }).click()
    if (width === 390) await openReport()
    await expect(report()).toContainText('4 MCP servers.', { timeout: 20_000 })
    expect(records().at(-1).method).toBe('session/load'); expect(records().at(-1).params.mcpServers.length).toBe(4)
    const made = await page.request.post('/api/hats', { headers: { Origin: collector.origin }, data: { name: 'Other hat' } }); expect(made.status()).toBe(201)
    const other = await made.json()
    const path = join(project, 'other'); mkdirSync(path)
    const rules = await page.request.put(`/api/hosts/${hostId}/path-rules`, { headers: { Origin: collector.origin }, data: { rules: [{ prefix: path, hat_id: other.id }] } }); expect(rules.status()).toBe(200)
    await page.goto('/new')
    const form = page.getByRole('form', { name: 'New session', exact: true }); await form.getByRole('radio', { name: 'Codex', exact: true }).check(); await form.getByRole('combobox').fill(path)
    await expect(form.getByRole('region', { name: 'MCP prediction', exact: true })).toHaveCount(0)
    // This delivery/session fixture reports no settings catalog.
    await form.getByRole('button', { name: 'Session settings', exact: true }).click()
    await expect(page.getByText('Agent defaults are unknown. Hat overrides and your choices will still be requested and checked after starting.', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Close session settings', exact: true }).click()
    await form.getByRole('button', { name: 'Start session', exact: true }).click()
    await expect(page).toHaveURL(/\/sessions\/[^/?#]+$/, { timeout: 45_000 })
    await openReport()
    await expect(report()).toContainText('this hat is not the host’s default', { timeout: 45_000 })
    await expect(report()).toContainText('0 MCP servers.')
    expect(records().at(-1).params.mcpServers).toEqual([])
  })
  test('obeys CSP and leaves the runner’s log directory untouched', async () => {
    expect(violations).toEqual([])
    expect(await page.evaluate(() => (window as unknown as { __csp?: string[] }).__csp ?? [])).toEqual([])
    expect(readdirSync(sentinel)).toEqual([])
  })
})
