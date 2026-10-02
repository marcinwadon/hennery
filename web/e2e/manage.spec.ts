// Hosts and hats in a real browser, against the real binary (plan 4d): a
// host paired with the command the page shows, a revoke that steps up and
// goes through, and the path tester asking that host, at 1280 px and at
// 390 px, each width with a collector and a host of its own.
import { expect, test, type Page } from '@playwright/test'
import { mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { startCollector, type Collector } from './collector'
import { scratchEnv } from './spawn'
import { testHost, type TestHost } from './host'

const PASSWORD = 'correct horse battery staple'

for (const width of [1280, 390]) {
  test.describe(`at ${width} px`, () => {
    test.describe.configure({ mode: 'serial' })

    let collector: Collector
    let host: TestHost
    let page: Page
    const violations: string[] = []
    // A log directory in the runner's own environment, as a developer's
    // shell may have: no binary started here may write to it.
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
      // Every CSP violation the page sees, whatever its source.
      await page.addInitScript(() => {
        document.addEventListener('securitypolicyviolation', (e) => {
          const seen = ((window as unknown as { __csp?: string[] }).__csp ??= [])
          seen.push(`${e.violatedDirective} ${e.blockedURI}`)
        })
      })
      page.on('console', (m) => {
        if (m.text().includes('Content Security Policy')) violations.push(m.text())
      })
      // Set up through the API, with the token from the setup link: the
      // session it opens is stepped up for five minutes.
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

    test('a host pairs with the command the page shows, and the code goes', async () => {
      await page.goto('/hosts')
      await page.getByRole('button', { name: 'Add host' }).click()
      const command = (await page.getByLabel('Pairing command').textContent())!
      expect(command).toMatch(new RegExp(`^hennery host join ${collector.origin} [0-9A-Z]{4}-[0-9A-Z]{4}$`))
      const code = command.split(' ').at(-1)!
      expect(await host.join(command, ['--name', 'e2e host', '--no-runtime'])).toBe(0)
      await expect(page.getByText('Paired: e2e host')).toBeVisible({ timeout: 15_000 })
      const card = page.getByRole('listitem', { name: 'e2e host' })
      await expect(card.getByText('Offline')).toBeVisible()
      // The code is spent, and gone from the page, its storage and its URL.
      expect(await page.content()).not.toContain(code)
      expect(await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))).not.toContain(code)
      expect(page.url()).not.toContain(code)
      host.run()
      await expect(async () => {
        await page.reload()
        await expect(page.getByRole('listitem', { name: 'e2e host' }).getByText('Online')).toBeVisible({ timeout: 1000 })
      }).toPass({ timeout: 20_000 })
    })

    test('the path tester asks the host, and a saved rule changes its answer', async () => {
      const project = realpathSync(host.dir)
      mkdirSync(join(project, 'work'), { recursive: true })
      await page.goto('/hats')
      await page.getByRole('textbox', { name: 'Name' }).fill('Work')
      await page.getByRole('button', { name: 'Create' }).click()
      await expect(page.getByRole('listitem', { name: 'Work' })).toBeVisible()
      const tester = page.getByLabel('Test a path')
      await tester.fill(join(project, 'work'))
      const resolution = page.getByLabel('Resolution')
      await expect(resolution).toContainText(join(project, 'work'))
      await expect(resolution).toContainText('Personal')
      await expect(resolution).toContainText('the host’s default hat')
      await page.getByRole('button', { name: 'Add rule' }).click()
      await page.getByLabel('Path 1').fill(join(project, 'work'))
      await page.getByLabel('Hat 1').selectOption({ label: 'Work' })
      await page.getByRole('button', { name: 'Save rules' }).click()
      await expect(page.getByText('Saved.')).toBeVisible()
      await tester.fill(join(project, 'work', 'app'))
      await expect(resolution).toContainText(join(project, 'work', 'app'))
      await expect(resolution).toContainText('Work')
      await expect(resolution).toContainText('a path rule')
      await expect(resolution).toContainText('does not exist')
    })

    test('a revoke asks for a fresh confirmation, then goes through', async () => {
      await page.goto('/hosts')
      // The session is still stepped up from setup; the server's refusal is
      // what the browser would get five minutes on. The first DELETE is
      // answered so, and the retry reaches the server. That the server
      // refuses without a step-up is the Rust tests'.
      let refused = 0
      await page.route('**/api/hosts/*', async (route) => {
        if (route.request().method() === 'DELETE' && refused === 0) {
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
      const card = page.getByRole('listitem', { name: 'e2e host' })
      await card.getByRole('button', { name: 'Revoke' }).click()
      const confirm = page.getByRole('dialog', { name: 'Revoke this host?' })
      await expect(confirm).toContainText('stop only when it next connects')
      await confirm.getByRole('button', { name: 'Revoke' }).click()
      const stepUp = page.getByRole('dialog', { name: 'This needs a fresh confirmation' })
      await expect(stepUp).toBeVisible()
      // The page under it is inert while it is open: the confirmation
      // beneath can take no focus, by script or by keyboard.
      expect(await page.locator('.page').getAttribute('inert')).not.toBeNull()
      await expect(stepUp.getByLabel('Your password')).toBeFocused()
      // The confirmation itself, not its buttons: they are disabled while
      // its action waits, and a disabled button takes no focus anyway.
      await confirm.evaluate((d: HTMLElement) => d.focus())
      await expect(stepUp.getByLabel('Your password')).toBeFocused()
      // Shift+Tab from the dialog's first field would land on the page
      // before it, were the page not inert. The password is its first
      // field only while no passkey is offered.
      await expect(stepUp.getByRole('button', { name: 'Confirm with passkey' })).toHaveCount(0)
      await page.keyboard.press('Shift+Tab')
      expect(await page.evaluate(() => !!document.activeElement?.closest('.page'))).toBe(false)
      await stepUp.getByLabel('Your password').focus()
      await stepUp.getByLabel('Your password').fill(PASSWORD)
      await stepUp.getByRole('button', { name: 'Confirm' }).click()
      await expect(card.getByText('Revoked', { exact: true })).toBeVisible()
      expect(refused).toBe(1)
      expect(await page.locator('.page').getAttribute('inert')).toBeNull()
      await page.unroute('**/api/hosts/*')
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
