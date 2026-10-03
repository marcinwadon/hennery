// The MCP gateway's connections in a real browser, against the real binary
// (plan 4e-i): a connection created with a step-up, its token set with a
// step-up and gone from the page at once, and mounted on a host paired with
// the command the Hosts screen shows, at 1280 px and at 390 px, each width
// with a collector and a host of its own.
import { expect, test, type Page } from '@playwright/test'
import { mkdtempSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { startCollector, type Collector } from './collector'
import { scratchEnv } from './spawn'
import { testHost, type TestHost } from './host'

const PASSWORD = 'correct horse battery staple'
const TOKEN = 'e2e-token-0123456789abcdef'

/** Answers the first `method` request to a URL matching `pattern` with 403
 *  `step_up_required`, as the server would five minutes after a sign-in,
 *  and lets every other request through. Resolves the count of refusals. */
async function refuseOnce(page: Page, pattern: string, method: string): Promise<() => number> {
  let refused = 0
  await page.route(pattern, async (route) => {
    if (route.request().method() === method && refused === 0) {
      refused++
      await route.fulfill({
        status: 403,
        contentType: 'application/json',
        body: JSON.stringify({ code: 'step_up_required', message: 'confirm it is you' }),
      })
    } else {
      await route.continue()
    }
  })
  return () => refused
}

async function stepUp(page: Page) {
  const dialog = page.getByRole('dialog', { name: 'This needs a fresh confirmation' })
  await expect(dialog).toBeVisible()
  await dialog.getByLabel('Your password').fill(PASSWORD)
  await dialog.getByRole('button', { name: 'Confirm' }).click()
  await expect(dialog).toBeHidden()
}

for (const width of [1280, 390]) {
  test.describe(`at ${width} px`, () => {
    test.describe.configure({ mode: 'serial' })

    let collector: Collector
    let host: TestHost
    let page: Page
    const violations: string[] = []
    let sentinel: string
    let runnerLogDir: string | undefined

    test.beforeAll(async ({ browser }) => {
      sentinel = mkdtempSync(join(tmpdir(), 'hennery-e2e-sentinel-'))
      runnerLogDir = process.env.HENNERY_LOG_DIR
      process.env.HENNERY_LOG_DIR = sentinel
      expect(scratchEnv(sentinel).HENNERY_LOG_DIR).toBeUndefined()
      collector = await startCollector()
      host = testHost()
      const context = await browser.newContext({ baseURL: collector.origin, viewport: { width, height: 844 } })
      page = await context.newPage()
      await page.addInitScript(() => {
        document.addEventListener('securitypolicyviolation', (e) => {
          const seen = ((window as unknown as { __csp?: string[] }).__csp ??= [])
          seen.push(`${e.violatedDirective} ${e.blockedURI}`)
        })
      })
      page.on('console', (m) => {
        if (m.text().includes('Content Security Policy')) violations.push(m.text())
      })
      const token = new URL(collector.setupLink).hash.slice(1)
      const setup = await page.request.post('/api/setup', {
        headers: { Origin: collector.origin },
        data: { token, password: PASSWORD, public_url: collector.origin },
      })
      expect(setup.status()).toBe(201)
    })

    test.afterAll(async () => {
      try {
        await page?.context().close()
      } finally {
        try {
          await host?.stop()
        } finally {
          try {
            await collector?.stop()
          } finally {
            if (runnerLogDir === undefined) delete process.env.HENNERY_LOG_DIR
            else process.env.HENNERY_LOG_DIR = runnerLogDir
            if (sentinel) rmSync(sentinel, { recursive: true, force: true })
          }
        }
      }
    })

    test('a host is paired to mount connections on', async () => {
      await page.goto('/hosts')
      await page.getByRole('button', { name: 'Add host' }).click()
      const command = (await page.getByLabel('Pairing command').textContent())!
      expect(await host.join(command, ['--name', 'e2e host', '--no-runtime'])).toBe(0)
      await expect(page.getByText('Paired: e2e host')).toBeVisible({ timeout: 15_000 })
    })

    test('a connection is created after a fresh confirmation', async () => {
      await page.goto('/mcp')
      await expect(page.getByRole('region', { name: 'How sessions get these' })).toContainText(
        'Changes apply to new and resumed sessions.',
      )
      const refused = await refuseOnce(page, '**/api/mcp/connections', 'POST')
      await page.getByRole('button', { name: 'Add connection' }).click()
      const form = page.getByRole('region', { name: 'Add a connection' })
      await form.getByRole('textbox', { name: 'Name', exact: true }).fill('Docs')
      await form.getByRole('textbox', { name: 'Slug', exact: true }).fill('docs')
      await form.getByRole('textbox', { name: 'Server URL', exact: true }).fill('https://mcp.example.com/mcp')
      await form.getByRole('button', { name: 'Create', exact: true }).click()
      await stepUp(page)
      const card = page.getByRole('listitem', { name: 'Docs' })
      await expect(card.getByRole('heading', { name: 'Docs', exact: true })).toBeFocused()
      await expect(card.getByText('hennery-docs', { exact: true })).toBeVisible()
      await expect(card.getByText('Not used yet', { exact: true })).toBeVisible()
      await expect(card.getByText('Not set yet', { exact: true })).toBeVisible()
      expect(refused()).toBe(1)
      await page.unroute('**/api/mcp/connections')
    })

    test('its token is set after a fresh confirmation, and is gone from the page at once', async () => {
      const card = page.getByRole('listitem', { name: 'Docs' })
      const refused = await refuseOnce(page, '**/api/mcp/connections/*/credential', 'PUT')
      const field = card.getByLabel('Token', { exact: true })
      await field.fill(TOKEN)
      await card.getByRole('button', { name: 'Save token' }).click()
      await expect(field).toHaveValue('')
      await stepUp(page)
      await expect(card.getByRole('status')).toHaveText('Token saved. It is never shown again.')
      await expect(card.getByText('Set', { exact: true })).toBeVisible()
      // Focus is back where the operator left it, not on the page's body.
      await expect(card.getByRole('button', { name: 'Save token' })).toBeFocused()
      expect(refused()).toBe(1)
      await page.unroute('**/api/mcp/connections/*/credential')
      expect(await page.content()).not.toContain(TOKEN)
      expect(page.url()).not.toContain(TOKEN)
      expect(await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))).not.toContain(TOKEN)
      // The server answers whether one is stored, never the token.
      const listed = await page.request.get('/api/mcp/connections', { headers: { Origin: collector.origin } })
      expect(await listed.text()).not.toContain(TOKEN)
    })

    test('it is mounted on the host, and stays so after a reload', async () => {
      const card = page.getByRole('listitem', { name: 'Docs' })
      const tick = card.getByRole('checkbox', { name: 'e2e host', exact: true })
      await expect(tick).not.toBeChecked()
      // Ticked once the server's answer lands, not before: a click, not
      // check(), which wants the box ticked at once.
      await tick.click()
      await expect(tick).toBeChecked()
      await expect(tick).toBeFocused()
      await expect(page.getByRole('dialog')).toHaveCount(0)
      await page.reload()
      await expect(page.getByRole('listitem', { name: 'Docs' }).getByRole('checkbox', { name: 'e2e host', exact: true })).toBeChecked()
    })

    test('broke no Content-Security-Policy rule', async () => {
      expect(violations).toEqual([])
      const seen = await page.evaluate(() => (window as unknown as { __csp?: string[] }).__csp ?? [])
      expect(seen).toEqual([])
    })

    test('wrote nothing where the runner’s own environment pointed', async () => {
      expect(readdirSync(sentinel)).toEqual([])
    })
  })
}
