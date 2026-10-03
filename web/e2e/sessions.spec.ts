// A session in a real browser, against the real binary (plan 4c, frontend
// spec §12): set up and sign in, pair a host whose agent is the scripted
// `hennery-fake-acp`, start a session from New Session, answer the
// permission it asks for, read its streamed Markdown, then crash the host
// and see the session parked when it comes back. At 1280 px and at 390 px,
// each width with a collector and a host of its own, and not one
// Content-Security-Policy violation.
//
// The fake asks before it streams (its `asks` come first in every prompt),
// and echoes the answer it got as the start of its message: the transcript
// shows that the option chosen on the card is the one the agent received.
import { expect, test, type Page } from '@playwright/test'
import { existsSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { startCollector, type Collector } from './collector'
import { FAKE_ACP, fakeAgent, testHost, type TestHost } from './host'
import { scratchEnv } from './spawn'

const PASSWORD = 'correct horse battery staple'
const PROMPT = 'Write the notes, please.'

// Every prompt: one permission ask, then a short Markdown message after the
// echo of its answer (`permission:selected:<option>`, no newline of its own).
const SCRIPT = {
  asks: ['permission'],
  chunks: ['\n\nTwo steps:\n\n', '- first step\n', '- second step\n'],
  chunk_delay_ms: 20,
}

for (const width of [1280, 390]) {
  test.describe(`at ${width} px`, () => {
    test.describe.configure({ mode: 'serial' })

    const narrow = width < 768
    let collector: Collector
    let host: TestHost
    let page: Page
    let project: string
    const violations: string[] = []
    // A log directory in the runner's own environment, as a developer's
    // shell may have: no process started here may write to it.
    let sentinel: string
    let runnerLogDir: string | undefined

    test.beforeAll(async ({ browser }) => {
      if (!existsSync(FAKE_ACP)) {
        throw new Error(`no ${FAKE_ACP}: cargo build -p hennery -p hennery-testkit --bins`)
      }
      sentinel = mkdtempSync(join(tmpdir(), 'hennery-e2e-sentinel-'))
      runnerLogDir = process.env.HENNERY_LOG_DIR
      process.env.HENNERY_LOG_DIR = sentinel
      expect(scratchEnv(sentinel).HENNERY_LOG_DIR).toBeUndefined()
      collector = await startCollector()
      host = testHost(fakeAgent('claude', SCRIPT))
      project = join(realpathSync(host.dir), 'notes')
      mkdirSync(project)
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

    test('the setup link sets hennery up, and the password signs in', async () => {
      await page.goto(collector.setupLink)
      await expect(page).toHaveURL(`${collector.origin}/setup`)
      await page.getByLabel(/^Password at least/).fill(PASSWORD)
      await page.getByLabel('Password again', { exact: true }).fill(PASSWORD)
      await page.getByRole('button', { name: 'Set up', exact: true }).click()
      await expect(page.getByRole('heading', { name: 'hennery is set up', exact: true })).toBeVisible()
      await page.getByRole('button', { name: 'Skip', exact: true }).click()
      await expect(page).toHaveURL(`${collector.origin}/sessions`)
      // Signed out: the rail's button on a desktop; a phone shows none, so
      // the browser drops its session there.
      if (narrow) await page.context().clearCookies()
      else await page.getByRole('button', { name: 'Sign out', exact: true }).click()
      await page.goto('/sessions')
      await expect(page).toHaveURL(`${collector.origin}/login?next=%2Fsessions`)
      await page.getByLabel('Password', { exact: true }).fill(PASSWORD)
      await page.getByRole('button', { name: 'Sign in', exact: true }).click()
      await expect(page).toHaveURL(`${collector.origin}/sessions`)
    })

    test('a host pairs with the command the page shows, and comes online', async () => {
      await page.goto('/hosts')
      await page.getByRole('button', { name: 'Add host', exact: true }).click()
      const command = (await page.getByLabel('Pairing command', { exact: true }).textContent())!
      expect(command).toMatch(new RegExp(`^hennery host join ${collector.origin} [0-9A-Z]{4}-[0-9A-Z]{4}$`))
      expect(await host.join(command, ['--name', 'e2e host', '--no-runtime'])).toBe(0)
      await expect(page.getByText('Paired: e2e host', { exact: true })).toBeVisible({ timeout: 15_000 })
      host.run()
      await expect(async () => {
        await page.reload()
        await expect(
          page.getByRole('listitem', { name: 'e2e host', exact: true }).getByText('Online', { exact: true }),
        ).toBeVisible({ timeout: 1000 })
      }).toPass({ timeout: 20_000 })
    })

    test('New Session starts a session with its first prompt, and opens it', async () => {
      // The rail's link on a desktop, the tab bar's on a phone.
      if (narrow) {
        await page.getByRole('navigation', { name: 'Tabs', exact: true }).getByRole('link', { name: 'New', exact: true }).click()
      } else {
        await page.getByRole('link', { name: 'New session', exact: true }).click()
      }
      // The form's own controls: the session list beside it has a search.
      const form = page.getByRole('form', { name: 'New session', exact: true })
      await expect(form).toBeVisible()
      await expect(form.getByRole('button', { name: 'e2e host', exact: true })).toHaveAttribute('aria-pressed', 'true')
      await expect(form.getByRole('button', { name: 'Claude', exact: true })).toHaveAttribute('aria-pressed', 'true')
      await form.getByRole('combobox').fill(project)
      // The whole line: the directory as resolved is the project itself,
      // and it exists (nothing follows it).
      const at = project.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
      await expect(form.getByRole('status').filter({ hasText: /^Starts in the hat / })).toHaveText(new RegExp(`^Starts in the hat .+ at ${at}$`))
      await form.getByRole('textbox', { name: /^First prompt/ }).fill(PROMPT)
      await form.getByRole('button', { name: 'Start session', exact: true }).click()
      // The start waits for the host to start the agent: on a loaded runner,
      // more than the default wait, and on macOS a fresh binary's first exec
      // waits for its online check (20 s and more has been seen).
      await expect(page).toHaveURL(new RegExp(`^${collector.origin}/sessions/[^/?#]+$`), { timeout: 45_000 })
      await expect(page.locator('.bubble.user')).toHaveText(PROMPT)
    })

    test('the agent’s permission question is answered from its card', async () => {
      const card = page.getByRole('region', { name: 'Question from Claude', exact: true })
      await expect(card.getByText('Write notes.txt', { exact: true })).toBeVisible({ timeout: 15_000 })
      await expect(page.locator('.session-head .badge')).toHaveText('Waiting on a question')
      await card.getByRole('button', { name: 'Allow', exact: true }).click()
      await expect(card.getByText('Answered', { exact: true })).toBeVisible()
      await expect(card.getByRole('button')).toHaveCount(0)
    })

    test('the agent’s message streams in as Markdown, its list markers shown', async () => {
      // The echo of the answer opens the message: `allow` reached the agent.
      const message = page.locator('.bubble', { has: page.getByText('permission:selected:allow', { exact: true }) })
      await expect(message.getByRole('listitem')).toHaveText(['first step', 'second step'])
      await expect(page.locator('.session-head .badge')).toHaveText('Idle')
      // Tailwind's preflight strips list markers: the transcript's own rule
      // must bring them back (client view spec §5.3; jsdom cannot see it).
      const list = message.getByRole('list')
      expect(await list.evaluate((el) => getComputedStyle(el).listStyleType)).toBe('disc')
      const item = message.getByRole('listitem').first()
      // An item draws its marker only as a list item, of its own (inherited) type.
      expect(await item.evaluate((el) => getComputedStyle(el).display)).toBe('list-item')
      expect(await item.evaluate((el) => getComputedStyle(el).listStyleType)).toBe('disc')
    })

    test('the composer’s Send, and a phone’s way back, are on screen', async () => {
      const send = page.getByRole('button', { name: 'Send', exact: true })
      await expect(send).toBeVisible()
      await expect(send).toBeInViewport()
      // Nothing covers it (a tab bar, a sheet): the point at its centre is it.
      const onTop = await send.evaluate((el) => {
        const r = el.getBoundingClientRect()
        const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2)
        return hit !== null && el.contains(hit)
      })
      expect(onTop).toBe(true)
      const back = page.getByRole('link', { name: 'Back to sessions', exact: true })
      if (narrow) {
        await expect(back).toBeVisible()
        await expect(back).toBeInViewport()
        expect((await back.boundingBox())!.height).toBeGreaterThanOrEqual(44)
        await expect(page.getByRole('navigation', { name: 'Tabs', exact: true })).toBeVisible()
      } else {
        // Not shown, so out of the accessibility tree: found by its class.
        await expect(back).toHaveCount(0)
        const hidden = page.locator('.session-head .back-btn')
        expect(await hidden.evaluate((el) => getComputedStyle(el).display)).toBe('none')
      }
    })

    test('a host that crashed and came back leaves the session parked', async () => {
      // Idle first, so no turn is cut short by the crash.
      await expect(page.locator('.session-head .badge')).toHaveText('Idle')
      await host.crash()
      host.run()
      // The restarted host holds no session: the collector parks it.
      const marker = page.locator('.marker-host_restarted .divider > span')
      await expect(marker).toHaveText(/^The host restarted( · .+)?$/, { timeout: 20_000 })
      await expect(page.locator('.session-head .badge')).toHaveText('Parked')
    })

    // TODO(4c T8): the footer's Resume, once T8's footers land. Click
    // `getByRole('button', { name: 'Resume', exact: true })` in the session's
    // footer, see the badge go back to Idle and a "Resumed" marker. The fake
    // asks for permission on EVERY prompt: a prompt sent after the resume
    // raises a second permission card, to answer like the first.
    test.fixme('the session resumes from its footer after the host restart', async () => {})

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
