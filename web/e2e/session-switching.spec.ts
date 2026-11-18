// Redesign 2: real embedded UI and routing with synthetic, owned summaries.
import { expect, test } from '@playwright/test'
import { mkdir } from 'node:fs/promises'
import { resolve } from 'node:path'
import { startCollector, type Collector } from './collector'

const sessionId = (n: number) => `019a0180-1234-7000-8000-${String(n).padStart(12, '0')}`
let collector: Collector
test.beforeAll(async () => { collector = await startCollector() })
test.afterAll(async () => { await collector?.stop() })

for (const width of [1280, 390]) for (const theme of ['light', 'dark'] as const) test(`visited sessions and hat scope at ${width}px ${theme}`, async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width, height: width === 390 ? 844 : 900 }, colorScheme: theme, reducedMotion: 'reduce' })
  const page = await context.newPage(), held: (() => Promise<void>)[] = []
  const ts = '2026-10-20T10:00:00Z'
  const rows = Array.from({ length: 8 }, (_, n) => ({ session_id: sessionId(n), title: `Plan ${n}`, hat_id: n % 2 ? 'home' : 'work', host_id: 'fixture-host', agent: 'claude', cwd: '/srv/project', lifecycle: n === 2 ? 'failed' : [3, 7].includes(n) ? 'parked' : 'active', activity: [0, 5].includes(n) ? 'running' : 'idle', presumed_parked: n === 7, question_waits: n === 1, operator_attention: n < 2 ? 1 : 0, parent_handled_questions: 0, created_at: ts, last_event_at: n === 0 ? '2099-01-01T00:00:00Z' : ts }))
  await context.route('**/api/**', async route => {
    const url = new URL(route.request().url()), path = url.pathname
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) })
    if (path === '/api/auth/logout') return route.fulfill({ status: 204 })
    if (/^\/api\/sessions\/[^/]+\/files$/.test(path) && route.request().method() === 'GET') return json({ artifacts: [], next_cursor: null, attachments: [], attachments_truncated: false })
    if (path === '/api/capabilities') return json({ mode: 'full', features: [] })
    if (path === '/api/hats') return json([{ id: 'work', name: 'Work', colour: '#8050d0', created_at: ts, default_for_new_hosts: true }, { id: 'home', name: 'Home', colour: '#087c70', created_at: ts, default_for_new_hosts: false }])
    if (path === '/api/hosts') return json([{ host_id: 'fixture-host', name: 'Development host', platform: 'linux', host_version: '0.2.2', capabilities: [], default_hat_id: 'work', workspace_roots: [], connected: true, created_at: ts, last_seen_at: ts }])
    if (path === '/api/view/sessions') {
      const matching = rows.filter(row => (!url.searchParams.has('hat') || row.hat_id === url.searchParams.get('hat')) && (!url.searchParams.has('q') || row.session_id === url.searchParams.get('q')))
      return json({ sessions: matching, epoch: 'fixture', revision: 1, waiting: matching.filter(row => row.operator_attention).length })
    }
    if (path.startsWith('/api/stream/')) { held.push(() => route.abort().catch(() => {})); return }
    const id = path.split('/')[3], row = rows.find(row => row.session_id === id)
    if (path.startsWith('/api/sessions/') && row) return path.endsWith('/catalog') ? json({ session_id: id, config_options: [], commands: [] }) : json({ ...row, pending: [] })
    if (/^\/api\/view\/sessions\/[^/]+$/.test(path)) return json({ items: [], older: false, epoch: 'fixture', revision: 1 })
    return json({ code: 'not_found', message: 'Unavailable in synthetic fixture.' }, 404)
  })
  const captures = process.env.HENNERY_REDESIGN2_NAV_CAPTURES
  const capture = async (name: string) => {
    await page.evaluate(async () => {
      for (const family of ['Inter', 'Roboto']) for (const weight of [400, 500, 600, 700]) await document.fonts.load(`${weight} 16px "${family}"`)
      await document.fonts.ready
    })
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    if (captures) { await mkdir(captures, { recursive: true }); await page.screenshot({ path: resolve(captures, `ours-${name}-${width}-${theme}.png`), fullPage: true }) }
  }
  const mod = await page.evaluate(() => /Mac|iPhone|iPad/.test(navigator.platform) ? 'Meta' : 'Control')
  const list = page.getByRole('region', { name: 'Session list', exact: true })
  const recent = page.getByRole('region', { name: 'Recent sessions', exact: true })
  const returnToList = async () => {
    // Use the app's own link on desktop, and session Back on phone.
    if (width === 1280) await page.getByRole('navigation', { name: 'Main navigation', exact: true }).getByRole('link', { name: 'Sessions', exact: true }).click()
    else await page.getByRole('link', { name: 'Back to sessions', exact: true }).click()
    await expect(list).toBeVisible()
  }
  const routeIs = async (n: number) => {
    await expect(page).toHaveURL(collector.origin + `/sessions/${sessionId(n)}`)
    await expect(page.getByRole('heading', { level: 1, name: `Plan ${n}`, exact: true })).toBeVisible()
    await expect.poll(() => page.evaluate(() => JSON.parse(sessionStorage.getItem('hennery.windowSessions') ?? '[]')[0])).toBe(sessionId(n))
  }
  try {
    await page.goto(collector.origin + '/sessions')
    await expect(list).toBeVisible()
    for (const n of [0, 1, 2, 3, 4, 5, 6]) {
      await list.locator('.session-table').getByRole('link', { name: new RegExp(`Plan ${n}(?: |$)`) }).click()
      await routeIs(n)
      if (n !== 6) await returnToList()
    }
    if (width === 1280) {
      await expect(recent.getByRole('link')).toHaveCount(6)
      expect(await recent.locator('.recent-title').allTextContents()).toEqual(['Plan 6', 'Plan 5', 'Plan 4', 'Plan 3', 'Plan 2', 'Plan 1'])
      await expect(recent.locator('[aria-current="page"]')).toContainText('Plan 6')
      await expect(page.getByRole('navigation', { name: 'Main navigation', exact: true }).locator('[aria-current="page"]')).toHaveCount(1)
      await expect(recent.locator('.recent-hat').first()).toHaveAttribute('title', 'Work')
      await expect(recent.locator('.recent-hat').first()).toHaveCSS('width', '8px')
      const original: string[] = await page.evaluate(() => JSON.parse(localStorage.getItem('hennery.visitedSessions')!))
      await recent.getByRole('link', { name: /^Plan 4 ·/ }).click()
      await routeIs(4)
      await expect(recent.locator('.recent-title')).toHaveText(['Plan 6', 'Plan 5', 'Plan 4', 'Plan 3', 'Plan 2', 'Plan 1'])
      await expect(recent.getByRole('link').nth(2)).toHaveAttribute('aria-current', 'page')
      await expect(recent.getByRole('link').nth(2).locator('kbd')).toHaveText(/(?:⌥|Alt\+)3/)
      await page.keyboard.press('Alt+Digit3'); await routeIs(4)
      await page.keyboard.press('Backquote'); await routeIs(6)
      expect(await page.evaluate(() => JSON.parse(localStorage.getItem('hennery.visitedSessions')!))).toEqual(original)
      const otherWindow = await context.newPage()
      try {
        await otherWindow.goto(collector.origin + `/sessions/${sessionId(7)}`)
        await expect.poll(() => otherWindow.evaluate(() => JSON.parse(sessionStorage.getItem('hennery.windowSessions') ?? '[]')[0])).toBe(sessionId(7))
        await otherWindow.goto(collector.origin + `/sessions/${sessionId(0)}`)
        await expect(otherWindow.locator('.rail .command-entry')).toBeEnabled()
        await expect.poll(() => otherWindow.evaluate(() => JSON.parse(sessionStorage.getItem('hennery.windowSessions') ?? '[]')[0])).toBe(sessionId(0))
        await expect(recent.locator('.recent-title')).toHaveText(['Plan 0', 'Plan 7', 'Plan 6', 'Plan 5', 'Plan 4', 'Plan 3'])
        await expect(recent.locator('[aria-current="page"]')).toContainText('Plan 6')
        await page.bringToFront()
        await page.locator('body').click({ position: { x: width - 2, y: 2 } })
        await page.keyboard.press('Alt+Digit2'); await routeIs(7)
        await page.keyboard.press('Backquote'); await routeIs(6)
        // The other window keeps its own 0 -> 7 history despite our visits.
        await otherWindow.bringToFront()
        expect(await otherWindow.evaluate(() => JSON.parse(sessionStorage.getItem('hennery.windowSessions') ?? '[]'))).toEqual([sessionId(0), sessionId(7)])
        const otherBack = otherWindow.getByRole('link', { name: 'Back to sessions', exact: true })
        await otherBack.focus()
        await expect(otherBack).toBeFocused()
        await otherWindow.keyboard.press('?')
        const otherHelp = otherWindow.getByRole('dialog', { name: 'Keyboard shortcuts', exact: true })
        await expect(otherHelp.getByText('Back to the previous session (backtick: US layouts)', { exact: true })).toBeVisible()
        await otherHelp.getByRole('button', { name: 'Close shortcuts', exact: true }).click()
        await otherWindow.keyboard.press('Backquote')
        await expect(otherWindow).toHaveURL(collector.origin + `/sessions/${sessionId(7)}`)
        await otherWindow.keyboard.press(`${mod}+k`)
        await expect(otherWindow.getByRole('dialog', { name: 'Command palette', exact: true }).getByRole('option', { selected: true })).toContainText('Plan 0')
        await otherWindow.keyboard.press('Enter')
        await expect(otherWindow).toHaveURL(collector.origin + `/sessions/${sessionId(0)}`)
        await otherWindow.evaluate(ids => localStorage.setItem('hennery.visitedSessions', JSON.stringify(ids)), original)
        await expect(recent.locator('.recent-title')).toHaveText(['Plan 6', 'Plan 5', 'Plan 4', 'Plan 3', 'Plan 2', 'Plan 1'])
      } finally { await otherWindow.close() }
    }
    await page.locator('body').click({ position: { x: width - 2, y: 2 } })
    await page.keyboard.press('Alt+Digit3'); await routeIs(4)
    await page.keyboard.press('Backquote'); await routeIs(6)
    await page.keyboard.press(`${mod}+k`)
    const palette = page.getByRole('dialog', { name: 'Command palette', exact: true })
    const visits = palette.getByRole('group', { name: 'Recent', exact: true })
    await expect(visits.getByRole('option', { selected: true })).toContainText('Plan 4')
    await capture('palette')
    await page.keyboard.press('Enter'); await routeIs(4)
    // Focused composer, IME, and contenteditable leave switching keys alone.
    const composer = page.getByRole('textbox', { name: 'Prompt', exact: true })
    await composer.fill('draft')
    await composer.press('Alt+Digit2'); await composer.press('Backquote')
    await expect(page).toHaveURL(collector.origin + `/sessions/${sessionId(4)}`)
    await expect(composer).toHaveValue(/^draft.*`$/)
    await page.evaluate(() => {
      const editable = document.createElement('div')
      editable.id = 'fixture-editable'; editable.contentEditable = 'true'; editable.textContent = 'Editing'
      document.querySelector('main')!.append(editable)
      editable.focus()
    })
    await page.keyboard.press('Alt+Digit2'); await page.keyboard.press('Backquote')
    await expect(page).toHaveURL(collector.origin + `/sessions/${sessionId(4)}`)
    await page.locator('#fixture-editable').evaluate(element => element.remove())
    await page.locator('body').click({ position: { x: width - 2, y: 2 } })
    await page.evaluate(() => window.dispatchEvent(new KeyboardEvent('keydown', { key: '`', isComposing: true, bubbles: true })))
    await expect(page).toHaveURL(collector.origin + `/sessions/${sessionId(4)}`)
    await capture('session')
    if (width === 1280) {
      await page.getByRole('button', { name: 'Collapse navigation', exact: true }).click()
      await expect(recent.locator('.recent-title').first()).toBeHidden()
      await expect(recent.locator('.recent-hat').first()).toBeHidden()
      await expect(recent.locator('.recent-hat-badge').first()).toBeVisible()
      await expect(recent.locator('.recent-status svg').first()).toHaveAttribute('width', '18')
      await expect(recent.getByRole('img').first()).toBeVisible()
      await expect(recent.locator('[aria-current="page"]')).toContainText('Plan 4')
      const footer = page.locator('.rail-foot')
      await expect(footer.locator('.version-info')).toBeHidden()
      for (const button of [footer.getByRole('button', { name: /^Appearance:/ }), footer.getByRole('button', { name: 'Sign out', exact: true })]) {
        await expect(button).toBeVisible()
        await expect(button).toHaveText('')
        await expect(button).toHaveAttribute('title', /.+/)
        await expect(button.locator('svg')).toHaveCount(1)
        await expect(button).toHaveCSS('width', '32px')
      }
      await capture('collapsed')
      await page.keyboard.press('Alt+Digit2'); await routeIs(5)
      await page.keyboard.press('Backquote'); await routeIs(4)
      await page.getByRole('button', { name: 'Expand navigation', exact: true }).click()
    }
    await returnToList()
    await page.getByRole('group', { name: 'Hat', exact: true }).getByRole('button', { name: 'Work', exact: true }).click()
    await expect(list.locator('.session-table [data-session-link]')).toHaveCount(4)
    const queue = page.getByRole('region', { name: 'Attention queue', exact: true })
    if (width === 1280) {
      await expect(queue.getByRole('article')).toHaveCount(1)
      await expect(recent.locator('.recent-title')).toHaveText(['Plan 6', 'Plan 4', 'Plan 2', 'Plan 0'])
    }
    await expect(page).toHaveTitle('(1) hennery')
    await page.locator('body').click({ position: { x: width - 2, y: 2 } })
    await page.keyboard.press(`${mod}+k`)
    await expect(visits.getByRole('option')).toHaveCount(4)
    await expect(visits.getByRole('option').filter({ hasText: /^Plan [1357]/ })).toHaveCount(0)
    await page.keyboard.press('Escape')
    await capture('work-scope')
    await page.getByRole('navigation', { name: width === 1280 ? 'Main navigation' : 'Primary navigation', exact: true }).getByRole('link', { name: 'Needs you', exact: true }).click()
    await expect(queue.getByRole('article')).toHaveCount(1)
    await page.getByRole('group', { name: 'Hat', exact: true }).getByRole('button', { name: 'All hats', exact: true }).click()
    await expect(queue.getByRole('article')).toHaveCount(2)
    await expect(page).toHaveTitle('(2) hennery')
    await page.getByRole('navigation', { name: width === 1280 ? 'Main navigation' : 'Primary navigation', exact: true }).getByRole('link', { name: 'Sessions', exact: true }).click()
    await expect(list.locator('.session-table [data-session-link]')).toHaveCount(8)
    if (width === 1280) await expect(recent.locator('.recent-title')).toHaveText(['Plan 6', 'Plan 5', 'Plan 4', 'Plan 3', 'Plan 2', 'Plan 1'])
    await capture('overview')
    await page.reload()
    await expect(list).toBeVisible()
    await expect(page.locator('.rail .command-entry')).toBeEnabled()
    await page.locator('body').click({ position: { x: width - 2, y: 2 } })
    await page.keyboard.press(`${mod}+k`)
    await expect(visits.getByRole('option', { selected: true })).toContainText('Plan 4')
    await expect(visits.getByRole('option', { name: 'Plan 5 /srv/project', exact: true })).toBeVisible()
    await page.keyboard.press('Escape')
    if (width === 1280) {
      await page.getByRole('button', { name: 'Sign out', exact: true }).click()
      await expect(page).toHaveURL(collector.origin + '/login')
      expect(await page.evaluate(() => localStorage.getItem('hennery.visitedSessions'))).toBeNull()
    }
  } finally { for (const release of held) await release(); await context.close() }
})
