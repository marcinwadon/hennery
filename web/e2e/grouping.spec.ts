// Grouping/header regressions against the embedded app; synthetic API fixtures only.
import { expect, test, type Page, type Route } from '@playwright/test'
import { mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { startCollector, type Collector } from './collector'
import { deriveAccent } from '../src/lib/accent'
import type { SessionSummary } from '../src/generated/view'
let collector: Collector
const captures = process.env.HENNERY_GROUPING_CAPTURES
const ts = '2026-10-21T10:00:00Z'
const base: SessionSummary = { session_id: 'coordinator', title: 'Review parser boundaries', host_id: 'one', hat_id: 'personal', agent: 'claude', cwd: '/srv/projects/parser/src', project_root: '/srv/projects/parser', lifecycle: 'active', activity: 'idle', presumed_parked: false, question_waits: false, operator_attention: 0, parent_handled_questions: 0, created_at: ts, last_event_at: ts }
const rows: SessionSummary[] = [base, { ...base, session_id: 'worker', title: 'Check worktree behavior', host_id: 'two', cwd: '/scratch/parser-fix', created_at: '2026-10-21T11:00:00Z', operator_attention: 1, family: { revision: 1, parent: { session_id: base.session_id, agent: 'claude' }, children: [], child_counts: { total: 0, active: 0, waiting: 0 }, attention: 0 } }, ...Array.from({ length: 10 }, (_, i) => ({ ...base, session_id: `session-${i}`, title: `Synthetic task ${i + 1}`, host_id: ['one', 'two', 'three', 'four'][i % 4], cwd: i % 2 ? '/srv/projects/parser/test' : '/srv/projects/console', project_root: i % 2 ? '/srv/projects/parser' : '/srv/projects/console', created_at: `2026-10-21T09:${String(i).padStart(2, '0')}:00Z`, activity: i % 3 ? 'idle' : 'running' }))]
async function capture(page: Page, name: string, width: number, theme: string) {
  await page.evaluate(async () => { await document.fonts.load('500 16px Roboto'); await document.fonts.load('400 12px Inter'); await document.fonts.ready })
  if (captures) { mkdirSync(captures, { recursive: true }); await page.screenshot({ path: resolve(captures, `ours-${name}-${width}-${theme}.png`), fullPage: true }) }
}
test.beforeAll(async () => { collector = await startCollector() })
test.afterAll(async () => { await collector?.stop() })
for (const width of [1280, 1024, 390]) for (const theme of ['light', 'dark'] as const) test(`two-level groups and responsive header at ${width}px ${theme}`, async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width, height: width === 390 ? 844 : 900 }, colorScheme: theme, reducedMotion: 'reduce' })
  const streams: Route[] = [], unmatched: string[] = []
  await context.addInitScript(({ theme, palette }) => { localStorage.setItem('hennery.theme', JSON.stringify({ version: 1, preference: theme, ...palette })) }, { theme, palette: deriveAccent('#8050d0') })
  await context.route('**/api/**', route => {
    const path = new URL(route.request().url()).pathname
    const json = (value: unknown) => route.fulfill({ contentType: 'application/json', body: JSON.stringify(value) })
    if (path === '/api/capabilities') return json({ mode: 'full', features: [] })
    if (path === '/api/hosts') return json(['one', 'two', 'three', 'four'].map(host_id => ({ host_id, name: `dev-${host_id}`, connected: true, capabilities: [] })))
    if (path === '/api/hats') return json([{ id: 'personal', name: 'Personal', colour: '#8050d0', created_at: ts, purging: false, default_for_new_hosts: false }])
    if (path === '/api/view/sessions') return json({ sessions: rows, epoch: 'list', revision: 1, waiting: 1 })
    if (path.startsWith('/api/stream/')) { streams.push(route); return }
    if (path === '/api/delegation/attention' || path === '/api/push/subscriptions') return json([])
    const session = rows.find(row => path === `/api/sessions/${row.session_id}`)
    if (session) return json({ ...session, pending: [] })
    if (rows.some(row => path === `/api/sessions/${row.session_id}/usage`)) return route.fulfill({ status: 404, body: '{"code":"not_found"}' })
    const catalog = rows.find(row => path === `/api/sessions/${row.session_id}/catalog`)
    if (catalog) return json({ session_id: catalog.session_id, config_options: [], commands: [] })
    const family = rows.find(row => path === `/api/view/sessions/${row.session_id}/family`)
    if (family) return json({ summary: family, children: [], questions: [] })
    if (rows.some(row => path === `/api/sessions/${row.session_id}/queue`)) return json([])
    if (rows.some(row => path === `/api/view/sessions/${row.session_id}`)) return json({ items: [], epoch: 'items', revision: 1, older: false })
    unmatched.push(path); return route.fulfill({ status: 404, contentType: 'application/json', body: '{"code":"not_found"}' })
  })
  try {
    const page = await context.newPage()
    await page.clock.setFixedTime(new Date('2026-10-21T12:00:00Z'))
    await page.goto(collector.origin + '/sessions')
    await expect(page.getByRole('heading', { name: 'Sessions', exact: true })).toBeVisible()
    await expect(page.locator('.project-toggle')).toHaveCount(2)
    await expect(page.locator('.overview-counter')).toHaveText('12 sessions · 4 hosts')
    await expect(page.locator('.grp-path[title="/srv/projects/parser"]')).toHaveCount(1)
    await expect(page.locator('.project-waiting')).toHaveAttribute('aria-label', '1 session needs you')
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
    if (width >= 768) {
      await expect(page.locator('.attention-preview .preview-note')).toHaveCount(0)
      await expect(page.locator('.attention-preview .preview-error')).toHaveCount(0)
    }
    await capture(page, 'sessions', width, theme)
    if (width >= 768) {
      expect((await page.getByRole('searchbox', { name: 'Search sessions', exact: true }).boundingBox())!.width).toBeGreaterThanOrEqual(120)
      expect((await page.getByRole('combobox', { name: 'Host', exact: true }).boundingBox())!.width).toBeGreaterThanOrEqual(110)
      expect((await page.getByRole('combobox', { name: 'Lifecycle', exact: true }).boundingBox())!.width).toBeGreaterThanOrEqual(140)
      for (const label of ['Host', 'Lifecycle']) expect(await page.getByRole('combobox', { name: label, exact: true }).evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true)
      await expect(page.locator('.session-columns')).toHaveCount(1)
      const columns = await page.locator('.session-columns').boundingBox(), firstGroup = await page.locator('.grp').first().boundingBox()
      expect(Math.abs(columns!.x - firstGroup!.x)).toBeLessThanOrEqual(1)
      const nameEdges = await page.evaluate(() => {
        const range = document.createRange(); range.selectNodeContents(document.querySelector('.session-columns th')!)
        return [range.getBoundingClientRect().left, document.querySelector('.session-table:not(.session-columns) .sess-name')!.getBoundingClientRect().left]
      })
      expect(Math.abs(nameEdges[0] - nameEdges[1])).toBeLessThanOrEqual(2)
      // Rows under a project heading are indented: status under the folder icon, name under the project name.
      const indent = await page.evaluate(() => {
        const left = (selector: string) => document.querySelector(selector)!.getBoundingClientRect().left
        return [left('.project-toggle > svg'), left('.project-rows .sess-led'), left('.project-toggle .project-name'), left('.project-rows .sess-name')]
      })
      expect(Math.abs(indent[0] - indent[1])).toBeLessThanOrEqual(2); expect(Math.abs(indent[2] - indent[3])).toBeLessThanOrEqual(2)
      const metrics = await page.locator('.overview-head').first().evaluate(header => {
        const parts = [header.querySelector('h1')!, header.querySelector('.overview-counter')!, header.querySelector('.grouping-controls')!, header.querySelector('.btn-primary')!]
        return { centers: parts.map(el => { const rect = el.getBoundingClientRect(); return rect.top + rect.height / 2 }), height: header.getBoundingClientRect().height, overflow: header.scrollWidth > header.clientWidth }
      })
      expect(Math.max(...metrics.centers) - Math.min(...metrics.centers)).toBeLessThanOrEqual(1)
      expect(metrics.height).toBe(52); expect(metrics.overflow).toBe(false)
      if (width === 1024) expect((await page.locator('.rail').boundingBox())!.width).toBe(240)
      const counter = page.locator('.overview-counter')
      await expect(counter).toHaveCSS('text-overflow', 'ellipsis')
      // Stress the same counter with a larger inventory; its siblings retain one line.
      await counter.evaluate(el => { el.textContent = '123456 sessions · 4321 hosts '.repeat(4) })
      expect(await counter.evaluate(el => el.scrollWidth > el.clientWidth)).toBe(true)
      expect(await page.locator('.overview-head').first().evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true)
      await capture(page, 'counter-ellipsis', width, theme)
      await counter.evaluate(el => { el.textContent = '12 sessions · 4 hosts' })
    } else {
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
      for (const label of ['Hat', 'Host', 'Project', 'Nothing']) expect((await page.getByRole('radio', { name: label, exact: true }).locator('..').boundingBox())!.height).toBeGreaterThanOrEqual(44)
      await page.getByRole('button', { name: 'Filters', exact: true }).click()
      await expect(page.getByRole('dialog', { name: 'Session filters', exact: true })).toBeVisible()
      await capture(page, 'filters', width, theme)
    }
    await expect(page.locator('.grp-count')).toHaveText('1 need you · 4 running · 12 total')
    await expect(page.locator('.project-summary').filter({ hasText: /^\d+ running · \d+ sessions$/ })).toHaveCount(2)
    await page.getByRole('radio', { name: 'Started', exact: true }).check()
    await page.getByRole('radio', { name: 'Host', exact: true }).check()
    await page.getByRole('radio', { name: 'Nothing', exact: true }).check()
    await expect(page.locator('.project-toggle')).toHaveCount(0)
    expect(await page.evaluate(() => ['hennery.group', 'hennery.subgroup', 'hennery.sort'].map(key => localStorage.getItem(key)))).toEqual(['host', 'none', 'started'])
    await page.getByRole('radio', { name: 'Hat', exact: true }).check()
    await page.getByRole('radio', { name: 'Project', exact: true }).check()
    if (width === 390) await page.getByRole('button', { name: 'Close filters', exact: true }).click()
    const parser = page.locator('.project-toggle').filter({ has: page.locator('.grp-path[title="/srv/projects/parser"]') })
    await parser.click()
    await expect(parser).toHaveAttribute('aria-expanded', 'false')
    await expect(page.locator('[data-session-id="coordinator"]')).toHaveCount(0)
    await capture(page, 'project-collapsed', width, theme)
    await page.locator('[data-session-id="session-0"] [data-session-link]').click()
    await expect(page).toHaveURL(/\/sessions\/session-0$/)
    await page.goto(collector.origin + '/sessions')
    await expect(parser).toHaveAttribute('aria-expanded', 'false')
    await parser.click()
    await expect(page.locator('[data-session-id="coordinator"]')).toHaveCount(1)
    // Remount uses the persisted preferences, without another scope or list stream.
    await page.reload()
    await expect(page.getByRole('radio', { name: 'Hat', exact: true })).toBeChecked()
    await expect(page.getByRole('radio', { name: 'Project', exact: true })).toBeChecked()
    if (width === 390) await page.getByRole('button', { name: 'Filters', exact: true }).click()
    await expect(page.getByRole('radio', { name: 'Started', exact: true })).toBeChecked()
    expect(unmatched).toEqual([])
  } finally { for (const stream of streams) await stream.abort().catch(() => {}); await context.close() }
})
