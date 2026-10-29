// A session in a real browser, against the real binary (plan 4c, frontend
// spec §12): set up and sign in, pair a host whose agent is the scripted
// `hennery-fake-acp`, start a session from New Session, answer the
// permission it asks for, read its streamed Markdown, then crash the host
// and see the session parked when it comes back, then resume it from its
// footer and answer its second prompt's question. At 1280 px and at 390 px,
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
import type { Item, SessionSummary } from '../src/generated/view'

const PASSWORD = 'correct horse battery staple'
const PROMPT = 'Write the notes, please.'

/** A finished turn is a result to review until its end has been in view (result
 *  inbox): scroll there when an answered card above it took the view. */
async function seeResult(page: Page) {
  const badge = page.locator('.session-head .badge')
  // A queued turn ending meanwhile re-keys the end marker: retry until read.
  await expect(async () => {
    if (await badge.textContent() === 'To review') await page.locator('.result-read-sentinel').last().scrollIntoViewIfNeeded({ timeout: 1000 }).catch(() => {})
    await expect(badge).toHaveText('Idle', { timeout: 1000 })
  }).toPass({ timeout: 15_000 })
}

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
        await page.getByRole('navigation', { name: 'Primary navigation', exact: true }).getByRole('link', { name: 'New', exact: true }).click()
      } else {
        await page.getByRole('link', { name: 'New session', exact: true }).click()
      }
      // The form's own controls: the session list beside it has a search.
      const form = page.getByRole('form', { name: 'New session', exact: true })
      await expect(form).toBeVisible()
      await expect(form.getByRole('radio', { name: 'e2e host', exact: true })).toBeChecked()
      await expect(form.getByRole('radio', { name: 'Claude', exact: true })).toBeChecked()
      await form.getByRole('combobox').fill(project)
      // The whole line: the directory as resolved is the project itself,
      // and it exists (nothing follows it).
      const at = project.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
      await expect(form.getByRole('status').filter({ hasText: /^Starts in the hat / })).toHaveText(new RegExp(`^Starts in the hat .+ at ${at}$`))
      await form.getByRole('textbox', { name: /^First prompt/ }).fill(PROMPT)
      // This delivery/session fixture reports no settings catalog.
      await form.getByRole('button', { name: 'Session settings', exact: true }).click()
      await expect(page.getByText('Agent defaults are unknown. Hat overrides and your choices will still be requested and checked after starting.', { exact: true })).toBeVisible()
      await page.getByRole('button', { name: 'Close session settings', exact: true }).click()
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
      await expect(page.locator('.session-head .badge')).toHaveText('Waiting on you')
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
      const composer = page.locator('.composer')
      // Full-width input and a separate action row; the former one-row 88px
      // bound squeezed the phone textarea between settings and buttons.
      expect((await composer.boundingBox())!.height).toBeLessThanOrEqual(narrow ? 112 : 104)
      const prompt = page.getByRole('textbox', { name: 'Prompt', exact: true })
      expect((await prompt.boundingBox())!.width).toBeGreaterThan((await page.locator('.composer-box').boundingBox())!.width * 0.85)
      const idleHeight = (await prompt.boundingBox())!.height
      await prompt.fill('A line of text\n'.repeat(40))
      expect((await prompt.boundingBox())!.height).toBeGreaterThan(idleHeight)
      expect((await prompt.boundingBox())!.height).toBeLessThanOrEqual(160)
      await prompt.fill('')
      expect((await prompt.boundingBox())!.height).toBe(idleHeight)
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
        await expect(page.locator('.tabbar')).toBeHidden()
      } else {
        await expect(back).toBeVisible()
        await expect(back).toBeInViewport()
        expect((await back.boundingBox())!.height).toBeGreaterThanOrEqual(28)
      }
    })

    test('pins a session to navigation, persists across reload and unpins independently of parking', async () => {
      const trigger = () => page.getByRole('button', { name: 'Session actions', exact: true })
      const sessionPath = new URL(page.url()).pathname
      await trigger().click()
      await page.getByRole('button', { name: 'Pin to sidebar', exact: true }).click()
      await expect(page.getByRole('button', { name: 'Unpin from sidebar', exact: true })).toBeEnabled()
      await expect(page.getByRole('switch', { name: 'Do not park', exact: true })).toHaveAttribute('aria-checked', 'false')
      await page.keyboard.press('Escape')
      await expect(trigger()).toHaveAttribute('aria-expanded', 'false')
      const pins = page.getByRole('group', { name: 'Pinned sessions', exact: true })
      if (!narrow) {
        await expect(pins.getByRole('link', { name: PROMPT, exact: true })).toHaveAttribute('aria-current', 'page')
        await expect(pins.getByRole('link', { name: PROMPT, exact: true })).toHaveAttribute('href', sessionPath)
        await page.screenshot({ path: test.info().outputPath('pinned-expanded.png') })
        await page.getByRole('button', { name: 'Collapse navigation', exact: true }).click()
        await expect(pins.getByRole('link', { name: PROMPT, exact: true })).toBeVisible()
        await page.screenshot({ path: test.info().outputPath('pinned-collapsed.png') })
        await page.getByRole('button', { name: 'Expand navigation', exact: true }).click()
      }
      await page.reload()
      await trigger().click()
      await page.getByRole('button', { name: 'Unpin from sidebar', exact: true }).click()
      await expect(page.getByRole('button', { name: 'Pin to sidebar', exact: true })).toBeEnabled()
      await page.keyboard.press('Escape')
      await expect(pins).toHaveCount(0)
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    })

    test('renames from the title, menu and palette using the server title and source', async () => {
      const path = new URL(page.url()).pathname
      const read = async () => (await page.request.get('/api' + path)).json()
      const initial = await read()
      expect(initial.title_source).toBe('prompt')
      expect(initial.title).toBe(PROMPT)
      await expect(page.locator('.session-head h1 .session-name-provisional')).toHaveText(PROMPT)
      for (const entry of ['header', 'menu', 'palette']) {
        if (entry === 'header') await page.locator('.session-head').getByRole('button', { name: 'Rename session', exact: true }).click()
        else if (entry === 'menu') {
          await page.getByRole('button', { name: 'Session actions', exact: true }).click()
          await expect(page.locator('.session-menu-list > button').first()).toHaveText('Rename')
          await page.getByRole('button', { name: 'Rename', exact: true }).click()
        } else {
          await expect(page.locator('.command-entry').first()).toBeEnabled()
          await page.locator('.session-head').getByRole('button', { name: 'Rename session', exact: true }).focus()
          await expect(page.locator('.session-head').getByRole('button', { name: 'Rename session', exact: true })).toBeFocused()
          const mod = await page.evaluate(() => /Mac|iPhone|iPad/.test(navigator.platform) ? 'Meta' : 'Control')
          await page.keyboard.press(`${mod}+k`)
          const palette = page.getByRole('dialog', { name: 'Command palette', exact: true })
          await expect(palette).toBeVisible()
          await palette.getByRole('combobox', { name: 'Jump to a session or command', exact: true }).fill('Rename session')
          await palette.getByRole('option', { name: 'Rename session', exact: true }).click()
        }
        const form = page.getByRole('form', { name: 'Rename session', exact: true })
        const name = `Operator name from ${entry}`
        await expect(form.getByRole('textbox', { name: 'Session name', exact: true })).toBeFocused()
        await form.getByRole('textbox', { name: 'Session name', exact: true }).fill(name)
        await form.getByRole('button', { name: 'Save', exact: true }).click()
        await expect(page.locator('.session-head h1')).toHaveText(name)
        await expect(page.locator('.session-head h1 .session-name-provisional')).toHaveCount(0)
        expect(await read()).toMatchObject({ title: name, title_source: 'operator' })
        await page.reload()
        await expect(page.locator('.session-head h1')).toHaveText(name)
      }
      await expect(page.getByText('Session name changed', { exact: true })).toHaveCount(3)
      await expect(page.getByText('Unsupported update (session_renamed)', { exact: true })).toHaveCount(0)
      await page.locator('.session-head').getByRole('button', { name: 'Rename session', exact: true }).click()
      const reset = page.getByRole('button', { name: 'Reset to agent title', exact: true })
      await expect(reset).toBeInViewport()
      if (narrow) expect((await reset.boundingBox())!.height).toBeGreaterThanOrEqual(44)
      else {
        const inputBox = (await page.getByRole('textbox', { name: 'Session name', exact: true }).boundingBox())!
        const resetBox = (await reset.boundingBox())!
        const backBox = (await page.getByRole('link', { name: 'Back to sessions', exact: true }).boundingBox())!
        const centre = (box: { y: number; height: number }) => box.y + box.height / 2
        expect(Math.abs(centre(inputBox) - centre(resetBox))).toBeLessThan(2)
        expect(Math.abs(centre(inputBox) - centre(backBox))).toBeLessThan(2)
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      await page.screenshot({ path: test.info().outputPath('rename-reset.png') })
      await reset.click()
      await expect(page.locator('.session-head h1 .session-name-provisional')).toHaveText(PROMPT)
      expect(await read()).toMatchObject({ title: PROMPT, title_source: 'prompt' })
      await page.reload()
      await expect(page.locator('.session-head h1 .session-name-provisional')).toHaveText(PROMPT)
      await expect(page.getByText('Session name changed', { exact: true })).toHaveCount(4)
      await page.locator('.session-head').getByRole('button', { name: 'Rename session', exact: true }).click()
      await expect(page.getByRole('button', { name: 'Reset to agent title', exact: true })).toHaveCount(0)
      await page.getByRole('textbox', { name: 'Session name', exact: true }).press('Escape')
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

    test('Resume and send uses the shortcut and shows the parked notice above the input', async () => {
      const resume = page.getByRole('button', { name: 'Resume', exact: true })
      await expect(resume).toBeVisible()
      if (narrow) expect((await resume.boundingBox())!.height).toBeGreaterThanOrEqual(44)
      const input = page.getByRole('textbox', { name: 'Prompt', exact: true })
      await input.fill('And the second part.')
      const send = page.getByRole('button', { name: 'Resume and send', exact: true })
      await expect(send).toBeEnabled()
      const notice = page.getByText('This session is parked.', { exact: true })
      expect((await notice.boundingBox())!.y).toBeLessThan((await input.boundingBox())!.y)
      for (const theme of ['light', 'dark'] as const) {
        await page.emulateMedia({ colorScheme: theme })
        await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
        const directory = process.env.HENNERY_QUEUE_SCREENSHOTS
        if (directory) mkdirSync(directory, { recursive: true })
        await page.screenshot({ path: directory ? join(directory, `parked-${width}-${theme}.png`) : test.info().outputPath(`parked-${theme}.png`) })
      }
      await page.emulateMedia({ colorScheme: 'light' })
      await input.press(narrow ? 'Control+Enter' : 'Meta+Enter')
      await expect(resume).toHaveCount(0, { timeout: 20000 })
      // The fake asks on every prompt: a second card, answered like the first.
      const cards = page.getByRole('region', { name: 'Question from Claude', exact: true })
      await expect(cards).toHaveCount(2, { timeout: 15_000 })
      const second = cards.nth(1)
      await second.getByRole('button', { name: 'Reject', exact: true }).click()
      await expect(second.getByText('Answered', { exact: true })).toBeVisible()
      // The reply opens with what the agent got this time.
      const reply = page.locator('.bubble', { has: page.getByText('permission:selected:reject', { exact: true }) })
      await expect(reply.getByRole('listitem')).toHaveText(['first step', 'second step'])
      await seeResult(page)
      await expect(page.locator('.session-head .badge')).toHaveText('Idle')
      await expect(page.locator('.bubble.user').getByText('And the second part.', { exact: true })).toHaveCount(1)
    })

    test('durable queued messages edit, remove, sync between windows and drain after permission', async () => {
      await page.getByRole('textbox', { name: 'Prompt', exact: true }).fill('Check the implementation first.')
      await page.getByRole('button', { name: 'Send', exact: true }).click()
      const cards = page.getByRole('region', { name: 'Question from Claude', exact: true })
      await expect(cards).toHaveCount(3, { timeout: 15000 })
      if (narrow) {
        await cards.nth(2).focus()
        await page.getByRole('button', { name: 'Write a prompt', exact: true }).click()
      }
      await page.getByRole('textbox', { name: 'Prompt', exact: true }).fill('Then review the tests.')
      await page.getByTestId('image-input').setInputFiles({ name: 'note.png', mimeType: 'image/png', buffer: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==', 'base64') })
      await page.getByRole('textbox', { name: 'Prompt', exact: true }).press(narrow ? 'Control+Enter' : 'Meta+Enter')
      const queue = page.getByRole('region', { name: 'Queued messages', exact: true })
      await expect(queue.getByText('1 image', { exact: true })).toBeVisible()
      await page.getByRole('textbox', { name: 'Prompt', exact: true }).fill('A removable follow-up.')
      await page.getByRole('button', { name: 'Send', exact: true }).click()
      await expect(queue.getByRole('listitem')).toHaveCount(2)
      const other = await page.context().newPage()
      try {
        await other.goto(page.url())
        if (narrow) {
          await other.getByRole('region', { name: 'Question from Claude', exact: true }).nth(2).focus()
          await other.getByRole('button', { name: 'Write a prompt', exact: true }).click()
        }
        const remote = other.getByRole('region', { name: 'Queued messages', exact: true })
        await expect(remote.getByRole('listitem')).toHaveCount(2)
        await queue.getByRole('button', { name: 'Edit queued message 1', exact: true }).click()
        await queue.getByRole('textbox', { name: 'Edit queued message 1', exact: true }).fill('Then focus on the tests.')
        await queue.getByRole('textbox', { name: 'Edit queued message 1', exact: true }).press('Meta+Enter')
        await expect(remote.getByText('Then focus on the tests.', { exact: true })).toBeVisible()
        await queue.getByRole('button', { name: 'Remove queued message 2', exact: true }).click()
        await expect(remote.getByRole('listitem')).toHaveCount(1)
        await page.reload()
        if (narrow) {
          await cards.nth(2).focus()
          await page.getByRole('button', { name: 'Write a prompt', exact: true }).click()
        }
        await expect(queue.getByText('Then focus on the tests.', { exact: true })).toBeVisible()
        await expect(queue.getByText('1 image', { exact: true })).toBeVisible()
        await expect(cards).toHaveCount(3)
        for (const theme of ['light', 'dark'] as const) {
          await page.emulateMedia({ colorScheme: theme })
          await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
          await page.evaluate(async () => { await document.fonts.ready })
          const directory = process.env.HENNERY_QUEUE_SCREENSHOTS
          if (directory) mkdirSync(directory, { recursive: true })
          await page.screenshot({ path: directory ? join(directory, `queued-${width}-${theme}.png`) : test.info().outputPath(`queued-${theme}.png`) })
          await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeInViewport()
        }
        await page.emulateMedia({ colorScheme: 'light' })
        await cards.nth(2).getByRole('button', { name: 'Allow', exact: true }).click()
        await expect(cards).toHaveCount(4, { timeout: 15000 })
        await expect(page.locator('.bubble.user').last()).toContainText('Then focus on the tests.')
        await cards.nth(3).getByRole('button', { name: 'Allow', exact: true }).click()
        await seeResult(page)
        await expect(page.locator('.session-head .badge')).toHaveText('Idle')
        await expect(remote).toHaveCount(0)
      } finally { await other.close() }
    })

    test('the session menu’s controls are big enough to tap on a phone', async () => {
      test.skip(!narrow, 'the 44 px touch targets are a phone’s')
      const trigger = page.getByRole('button', { name: 'Session actions', exact: true })
      expect((await trigger.boundingBox())!.height).toBeGreaterThanOrEqual(44)
      await trigger.click()
      await expect(trigger).toHaveAttribute('aria-expanded', 'true')
      // An active session: Close, Delete, and Park if its host can park.
      const list = page.locator('.session-menu-list')
      const names = await list.getByRole('button').allTextContents()
      expect(names.filter((n) => n !== 'Park')).toEqual(['Rename', 'New like this', 'Pin to sidebar', 'Close', 'Delete session'])
      expect((await list.getByRole('switch', { name: 'Do not park', exact: true }).boundingBox())!.height).toBeGreaterThanOrEqual(44)
      for (const name of names) {
        const box = await list.getByRole('button', { name, exact: true }).boundingBox()
        expect(box!.height).toBeGreaterThanOrEqual(44)
      }
      await page.keyboard.press('Escape')
      await expect(trigger).toHaveAttribute('aria-expanded', 'false')
    })

    test('explicit Park holds queued input until Resume and send releases it', async () => {
      const input = page.getByRole('textbox', { name: 'Prompt', exact: true })
      await input.fill('Pause before delivering queued work.')
      await page.getByRole('button', { name: 'Send', exact: true }).click()
      const cards = page.getByRole('region', { name: 'Question from Claude', exact: true })
      await expect(cards.last().getByRole('button', { name: 'Allow', exact: true })).toBeVisible()
      if (narrow) { await cards.last().focus(); await page.getByRole('button', { name: 'Write a prompt', exact: true }).click() }
      await input.fill('This queued head waits for Resume.')
      await input.press(narrow ? 'Control+Enter' : 'Meta+Enter')
      const queue = page.getByRole('region', { name: 'Queued messages', exact: true })
      await expect(queue.getByRole('listitem')).toHaveCount(1)
      await page.getByRole('button', { name: 'Session actions', exact: true }).click()
      await page.getByRole('button', { name: 'Park', exact: true }).click()
      await expect(page.locator('.session-head .badge')).toHaveText('Parked')
      const notice = page.getByText('1 queued message will be sent when you resume.', { exact: true })
      await expect(notice).toBeVisible()
      expect((await notice.boundingBox())!.y).toBeLessThan((await input.boundingBox())!.y)
      await page.reload()
      await expect(notice).toBeVisible()
      await expect(queue.getByText('This queued head waits for Resume.', { exact: true })).toBeVisible()
      await expect(page.locator('.bubble.user').getByText('This queued head waits for Resume.', { exact: true })).toHaveCount(0)
      await input.fill('This new send releases the park hold.')
      for (const theme of ['light', 'dark'] as const) {
        await page.emulateMedia({ colorScheme: theme })
        await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
        await page.evaluate(async () => { await document.fonts.ready })
        const directory = process.env.HENNERY_QUEUE_SCREENSHOTS
        if (directory) mkdirSync(directory, { recursive: true })
        await page.screenshot({ path: directory ? join(directory, `park-hold-${width}-${theme}.png`) : test.info().outputPath(`park-hold-${theme}.png`) })
      }
      await page.emulateMedia({ colorScheme: 'light' })
      await page.getByRole('button', { name: 'Resume and send', exact: true }).click()
      await expect(page.locator('.bubble.user').getByText('This queued head waits for Resume.', { exact: true })).toHaveCount(1)
      await expect(cards.last().getByRole('button', { name: 'Allow', exact: true })).toBeVisible()
      await cards.last().getByRole('button', { name: 'Allow', exact: true }).click()
      await expect(page.locator('.bubble.user').getByText('This new send releases the park hold.', { exact: true })).toHaveCount(1)
      await expect(cards.last().getByRole('button', { name: 'Allow', exact: true })).toBeVisible()
      await cards.last().getByRole('button', { name: 'Allow', exact: true }).click()
      await seeResult(page)
      await expect(page.locator('.session-head .badge')).toHaveText('Idle')
      await expect(queue).toHaveCount(0)
      await expect(page.locator('.bubble.user').getByText('This queued head waits for Resume.', { exact: true })).toHaveCount(1)
      await expect(page.locator('.bubble.user').getByText('This new send releases the park hold.', { exact: true })).toHaveCount(1)
    })

    test('closes an active session from its header menu', async () => {
      await expect(page.locator('.session-head .badge')).toHaveText('Idle')
      const trigger = page.getByRole('button', { name: 'Session actions', exact: true })
      if (await trigger.getAttribute('aria-expanded') !== 'true') await trigger.click()
      await expect(page.getByRole('button', { name: 'Park', exact: true })).toBeVisible()
      await page.getByRole('button', { name: 'Close', exact: true }).click()
      await expect(page.locator('.session-head .badge')).toHaveText('Closed')
      await trigger.click()
      await expect(page.getByRole('button', { name: 'Close', exact: true })).toHaveCount(0)
      await expect(page.getByRole('button', { name: 'Park', exact: true })).toHaveCount(0)
      await page.keyboard.press('Escape')
    })

    test('deleting a session warns about unsent messages and scrubs them in every window', async () => {
      await page.getByRole('button', { name: 'Resume', exact: true }).click()
      await expect(page.locator('.session-head .badge')).toHaveText('Idle')
      await page.getByRole('textbox', { name: 'Prompt', exact: true }).fill('Wait before deleting this session.')
      await page.getByRole('button', { name: 'Send', exact: true }).click()
      const cards = page.getByRole('region', { name: 'Question from Claude', exact: true })
      await expect(cards.last().getByRole('button', { name: 'Allow', exact: true })).toBeVisible()
      if (narrow) { await cards.last().focus(); await page.getByRole('button', { name: 'Write a prompt', exact: true }).click() }
      await page.getByRole('textbox', { name: 'Prompt', exact: true }).fill('Erase this unsent message on deletion.')
      await page.getByRole('button', { name: 'Send', exact: true }).click()
      await expect(page.getByRole('region', { name: 'Queued messages', exact: true }).getByRole('listitem')).toHaveCount(1)
      const other = await page.context().newPage()
      try {
        await other.goto(page.url())
        await page.getByRole('button', { name: 'Session actions', exact: true }).click()
        await page.getByRole('button', { name: 'Delete session', exact: true }).click()
        await expect(page.getByRole('dialog', { name: 'Delete this session?', exact: true })).toContainText('1 unsent queued message and its images will also be deleted.')
        await page.getByRole('dialog', { name: 'Delete this session?', exact: true }).getByRole('button', { name: 'Delete', exact: true }).click()
        for (const window of [page, other]) {
          await expect(window.getByRole('heading', { name: 'This session was deleted', exact: true })).toBeVisible()
          await expect(window.getByText('Erase this unsent message on deletion.', { exact: true })).toHaveCount(0)
          await expect(window.getByRole('textbox', { name: 'Prompt', exact: true })).toHaveCount(0)
          await expect(window.getByRole('region', { name: 'Queued messages', exact: true })).toHaveCount(0)
        }
        await page.reload()
        await expect(page.getByText('Erase this unsent message on deletion.', { exact: true })).toHaveCount(0)
        await expect(page.getByRole('heading', { name: 'This session was deleted', exact: true })).toBeVisible()
      } finally { await other.close() }
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

// R3 acceptance uses synthetic, typed view items served over the embedded UI.
// It covers layout/focus and mutation bodies; the real-host cases above keep
// the collector → adapter → delivered-answer proof.
test.describe('redesigned session', () => {
  let app: Collector
  test.beforeAll(async () => { app = await startCollector() })
  test.afterAll(async () => { await app?.stop() })
  for (const width of [1280, 390]) for (const appearance of ['light', 'dark'] as const) {
    test(`details, answers and read-back at ${width}px in ${appearance}`, async ({ browser }) => {
      const context = await browser.newContext({ viewport: { width, height: width === 390 ? 844 : 900 }, colorScheme: appearance, reducedMotion: 'reduce', timezoneId: 'UTC' })
      const held: import('@playwright/test').Route[] = []
      const mutations: { path: string; body: unknown }[] = []
      const now = '2026-10-20T10:00:00Z'
      const info: SessionSummary = { session_id: 'worker', host_id: 'host1', hat_id: 'hat1', title: 'Review the parser', cwd: '/srv/synthetic/parser', agent: 'codex', model: 'model-a', mode: 'review', git_branch: 'parser-checks', git_dirty: true, created_at: now, last_event_at: now, lifecycle: 'active', activity: 'blocked', presumed_parked: false, question_waits: true, operator_attention: 2, parent_handled_questions: 0,
        family: { revision: 1, parent: { session_id: 'coordinator', title: 'Coordinator', agent: 'claude' }, children: [], child_counts: { total: 0, active: 0, waiting: 0 }, attention: 0 } }
      const question: Extract<Item, { kind: 'question' }> = { id: 'question:permission', version: 1, kind: 'question', ts: now, pending_id: 'permission', question_kind: 'permission', request: { type: 'permission', title: 'Run the project checks?', options: [{ option_id: 'allow/once', name: 'Allow once', option_kind: 'allow_once' }, { option_id: 'allow/always', name: 'Always allow', option_kind: 'allow_always' }, { option_id: 'reject/once', name: 'Reject', option_kind: 'reject_once' }] }, state: 'open', answered: false, answerable: true }
      const form: Extract<Item, { kind: 'question' }> = { ...question, id: 'question:question', pending_id: 'question', question_kind: 'elicitation', request: { type: 'elicitation', message: 'Which parser should be checked?', form_supported: true, required: ['parser'], fields: [{ key: 'parser', label: 'Parser', field_kind: 'single', options: [{ value: 'strict', label: 'Strict parser', description: 'Check every input' }, { value: 'compatible', label: 'Compatible parser', description: 'Keep the existing input rules' }] }, { key: 'notes', label: 'Notes', field_kind: 'text' }] } }
      const plan: Extract<Item, { kind: 'plan' }> = { id: 'plan', kind: 'plan', version: 1, ts: now, entries: [{ content: 'Read the parser', status: 'completed' }, { content: 'Check the input rules', status: 'in_progress' }, { content: 'Report the findings', status: 'pending' }] }
      let pinned = false, taken = false
      const catalog = () => ({ session_id: 'worker', commands: [], config_options: [
        { id: 'model', name: 'Model', category: 'model', type: 'select', currentValue: 'model-a', options: [{ value: 'model-a', name: 'Model A' }, { value: 'model-b', name: 'Model B' }] },
        { id: 'mode', name: 'Mode', category: 'mode', type: 'select', currentValue: 'review', options: [{ value: 'review', name: 'Review' }, { value: 'edit', name: 'Edit' }] },
        { id: 'effort', name: 'Effort', category: 'thought_level', type: 'select', currentValue: 'medium', options: ['low', 'medium', 'high'].map(value => ({ value, name: value })) },
      ] })
      const items: Item[] = [
        { id: 'prompt', kind: 'user_turn', version: 1, ts: now, origin: { kind: 'parent', session_id: 'coordinator', operation_id: 'parent-instruction' }, content: [{ type: 'text', text: 'Review the parser and report what needs changing.' }] },
        { id: 'message', kind: 'message', version: 1, ts: now, text: 'I checked the parser.\n\n- Input types are preserved.\n- The remaining checks need permission.\n\n| Check | Result |\n| --- | --- |\n| Input types | Preserved |\n\n```ts\nconst result = parse(input)\n```' },
        { id: 'thinking', kind: 'thinking', version: 1, ts: now, text: 'Consider the boundary cases before changing the parser.' },
        { id: 'tool', kind: 'tool_call', tool_call_id: 'read-parser', version: 1, ts: now, title: 'Read', status: 'completed', input: { path: 'src/parser.ts' }, output: 'Parser input rules inspected.', content: [{ type: 'diff', path: 'src/parser.ts', old_text: 'return input;', new_text: 'return parse(input);' }] }, plan, question, form,
      ]
      await context.route('**/api/**', async route => {
        const path = new URL(route.request().url()).pathname
        const json = (value: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(value) })
        if (path === '/api/capabilities') return json({ mode: 'full', features: [] })
        if (path === '/api/hats') return json([{ id: 'hat1', name: 'Synthetic project', colour: '#8a3cbd' }])
        if (path === '/api/hosts') return json([{ host_id: 'host1', name: 'Test workstation', connected: true, capabilities: ['images', 'park', 'idle_hold'] }])
        if (path === '/api/view/sessions') return json({ sessions: [info], epoch: 'list', revision: 1, waiting: 1 })
        if (path.startsWith('/api/stream/')) { held.push(route); return }
        if (path === '/api/view/sessions/worker') return json({ items, epoch: 'items', revision: 1, older: false })
        if (path === '/api/sessions/worker') return json({ ...info, pending: [], mcp_delivery: { mode: 'unisolated', servers: 2, at: now } })
        if (path.endsWith('/queue')) return json([])
        if (path.endsWith('/catalog')) return json(catalog())
        if (path.endsWith('/family')) return json({ summary: info, children: [], questions: [], routing: taken ? 'detached' : 'active' })
        if (path.includes('/questions/')) return json(path.endsWith('/permission') ? question : form)
        if (route.request().method() !== 'GET') mutations.push({ path, body: route.request().postDataJSON() })
        if (path.endsWith('/takeover')) { taken = true; return route.fulfill({ status: 204 }) }
        if (path.endsWith('/idle-hold')) { if (route.request().method() === 'PATCH') pinned = route.request().postDataJSON().pinned; return json({ pinned, family_active: true, notices_pending: false, held: true }) }
        if (path.endsWith('/config')) return json(catalog(), 202)
        if (path.endsWith('/answer')) return json({ pending_id: path.includes('/permission/') ? 'permission' : 'question', request_id: 'answer' }, 202)
        if (path === '/api/delegation/attention') return json([])
        throw new Error(`Unmatched session acceptance route: ${path}`)
      })
      const page = await context.newPage()
      const shot = async (state: string) => {
        if (!process.env.HENNERY_R3_SCREENSHOTS) return
        mkdirSync(process.env.HENNERY_R3_SCREENSHOTS, { recursive: true })
        await page.evaluate(async () => { await Promise.all([document.fonts.load('400 14px Inter'), document.fonts.load('italic 400 14px Inter'), document.fonts.load('500 18px Roboto')]); await document.fonts.ready })
        await page.screenshot({ path: join(process.env.HENNERY_R3_SCREENSHOTS, `${state}-${width}-${appearance}.png`) })
      }
      try {
        await page.goto(`${app.origin}/sessions/worker?pending=permission`)
        await expect(page.getByRole('heading', { name: 'Review the parser', exact: true, level: 1 })).toBeVisible()
        await expect(page.getByRole('region', { name: 'Parent-driven session', exact: true })).toContainText('A prompt from you takes over this session.')
        await expect(page.locator('html')).toHaveAttribute('data-theme', appearance)
        const permission = page.locator('[data-pending-id="permission"]')
        await permission.focus()
        await expect(permission.getByText('Run the project checks?', { exact: true })).toBeInViewport()
        await expect(permission.getByRole('button', { name: 'Allow once', exact: true })).toBeInViewport()
        if (width === 390) expect((await permission.getByRole('button', { name: 'Allow once', exact: true }).boundingBox())!.height).toBeGreaterThanOrEqual(48)
        await shot('permission')
        if (await page.getByRole('button', { name: 'Session details', exact: true }).getAttribute('aria-pressed') !== 'true') await page.getByRole('button', { name: 'Session details', exact: true }).click()
        const panel = width === 390 ? page.getByRole('dialog', { name: 'Session details', exact: true }) : page.getByRole('complementary', { name: 'Session details', exact: true })
        await expect(panel.getByRole('link', { name: 'Coordinator', exact: true })).toBeVisible()
        await shot('family')
        if (width === 390) await panel.getByRole('tab', { name: 'Plan', exact: true }).click()
        await expect(panel.locator('.steps').getByRole('listitem')).toHaveText(['Read the parser', 'Check the input rules', 'Report the findings'])
        await shot('plan')
        if (width === 390) await panel.getByRole('tab', { name: 'Details', exact: true }).click()
        await expect(panel.getByRole('region', { name: 'MCP delivery', exact: true })).toContainText('2 MCP servers.')
        await shot('details')
        await panel.getByRole('button', { name: 'Close session details', exact: true }).click()
        await expect(page.getByRole('button', { name: 'Session details', exact: true })).toBeFocused()
        if (width === 390) await permission.getByRole('button', { name: 'Write a prompt', exact: true }).click()
        // A direct inline click must not move its button between down and up.
        await permission.getByRole('button', { name: 'Allow once', exact: true }).click()
        await expect(permission.getByText('Sent', { exact: true })).toBeVisible()
        expect(mutations).toContainEqual({ path: '/api/sessions/worker/pending/permission/answer', body: { option_id: 'allow/once' } })
        const elicitation = page.locator('[data-pending-id="question"]')
        await elicitation.focus()
        await elicitation.getByRole('radio', { name: 'Strict parser', exact: true }).check()
        await elicitation.getByRole('textbox', { name: 'Notes', exact: true }).fill('Preserve unknown input types')
        await expect(elicitation.getByRole('button', { name: 'Send', exact: true })).toBeInViewport()
        await shot('elicitation')
        await elicitation.getByRole('button', { name: 'Send', exact: true }).click()
        await expect(elicitation.getByText('Sent', { exact: true })).toBeVisible()
        expect(mutations).toContainEqual({ path: '/api/sessions/worker/pending/question/answer', body: { action: 'accept', content: { parser: 'strict', notes: 'Preserve unknown input types' } } })
        const prompt = page.getByRole('textbox', { name: 'Prompt', exact: true })
        await expect(prompt).toBeInViewport()
        await prompt.fill('Keep this draft')
        await page.getByRole('button', { name: 'Session settings', exact: true }).click()
        await page.getByRole('combobox', { name: 'Model', exact: true }).selectOption('model-b')
        await expect(page.getByRole('alert')).toContainText('the agent kept model-a instead of model-b')
        await expect(page.getByRole('combobox', { name: 'Model', exact: true })).toHaveValue('model-a')
        await shot('composer')
        await page.getByRole('button', { name: 'Close session details', exact: true }).click()
        await expect(prompt).toHaveValue('Keep this draft')
        await page.getByRole('button', { name: 'Session actions', exact: true }).click()
        const pin = page.getByRole('switch', { name: 'Do not park', exact: true })
        await expect(pin).toBeEnabled()
        await expect(pin).not.toBeChecked()
        await expect(page.getByRole('status').filter({ hasText: 'Family work requests a parking hold.' })).toBeVisible()
        await pin.click(); await expect(pin).toBeChecked()
        await shot('parking-menu')
        await page.keyboard.press('Escape')
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
        expect(mutations.filter(m => m.path.endsWith('/prompt'))).toHaveLength(0)
        // The same view with a running child and a retained parked draft.
        question.answerable = false; question.answered = true; question.state = 'delivered'
        form.answerable = false; form.answered = true; form.state = 'delivered'
        info.operator_attention = 0; info.question_waits = false; info.activity = 'running'
        await page.reload()
        await expect(page.locator('.session-head .badge')).toHaveText('Running')
        await page.locator('.transcript').evaluate(el => { el.scrollTop = 0 })
        await expect(page.locator('.bubble.user')).toHaveCount(0)
        const familyMessage = page.getByRole('button', { name: 'Show message', exact: true })
        await expect(familyMessage).toHaveAttribute('aria-expanded', 'false')
        await expect(page.getByRole('link', { name: 'Source session', exact: true })).toBeVisible()
        await familyMessage.click()
        await expect(page.getByRole('button', { name: 'Hide message', exact: true })).toHaveAttribute('aria-expanded', 'true')
        await expect(page.getByText('Review the parser and report what needs changing.', { exact: true })).toBeVisible()
        const userBubble = page.locator('.bubble.user').first(), agentBubble = page.locator('.bubble:not(.user)').first()
        expect(await userBubble.evaluate(el => getComputedStyle(el).backgroundColor)).not.toBe(await agentBubble.evaluate(el => getComputedStyle(el).backgroundColor))
        expect(await agentBubble.evaluate(el => getComputedStyle(el).borderTopWidth)).toBe('1px')
        if (width === 1280) {
          await page.setViewportSize({ width: 1920, height: 900 })
          const inner = (await page.locator('.transcript-inner').boundingBox())!
          expect(inner.width).toBeGreaterThan(1000)
          expect((await page.getByRole('button', { name: 'Read Done', exact: true }).boundingBox())!.width).toBeGreaterThan(inner.width - 70)
          expect((await page.locator('.bubble table').boundingBox())!.width).toBeGreaterThan(inner.width - 100)
          expect((await page.locator('.bubble pre').boundingBox())!.width).toBeGreaterThan(inner.width - 100)
          await page.setViewportSize({ width, height: 900 })
        }
        await shot('running')
        await page.getByRole('button', { name: 'Read Done', exact: true }).click()
        await expect(page.locator('.tool-diff')).toContainText('return parse(input);')
        await shot('tool-expanded')
        info.lifecycle = 'parked'; info.activity = 'idle'
        await page.reload()
        await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeVisible()
        await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toHaveValue('Keep this draft')
        await shot('parked')
        await page.getByRole('button', { name: 'Session actions', exact: true }).click()
        await page.getByRole('button', { name: 'Delete session', exact: true }).click()
        const confirmation = page.getByRole('dialog', { name: 'Delete this session?', exact: true })
        await expect(confirmation).toContainText('Copies already delivered to related sessions remain')
        await shot('delete-confirmation')
        await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click()
        await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toHaveValue('Keep this draft')
      } finally { for (const route of held) await route.abort().catch(() => {}); await context.close() }
    })
  }
})
