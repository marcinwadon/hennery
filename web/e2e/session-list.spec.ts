import { expect, test, type Route } from '@playwright/test'
import { startCollector, type Collector } from './collector'
import type { SessionSummary } from '../src/generated/view'

let collector: Collector
test.beforeAll(async () => { collector = await startCollector() })
test.afterAll(async () => { await collector?.stop() })

for (const width of [1280, 390]) for (const count of [1, 100]) test(`${count} children remain compact and stable during streaming at ${width}px`, async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width, height: 900 } })
  await context.addInitScript(() => { localStorage.setItem('hennery.sort', 'started'); localStorage.setItem('hennery.subgroup', 'none') })
  const ts = '2026-10-01T09:00:00Z', hostname = 'a-very-long-development-hostname-that-must-not-wrap'
  const base: SessionSummary = { session_id: 'parent', title: 'Coordinator', host_id: 'host', hat_id: 'hat', agent: 'claude', model: 'claude-haiku-4-5-20251001', cwd: '/srv/project', lifecycle: 'active', activity: 'idle', presumed_parked: false, question_waits: false, operator_attention: 0, parent_handled_questions: 0, created_at: ts, last_event_at: ts }
  const parent = { ...base, family: { revision: 1, children: [{ session_id: 'child', agent: 'claude' }, { session_id: 'child', agent: 'claude' }, { session_id: 'closed', agent: 'claude' }], child_counts: { total: count + 1, active: 1, waiting: 0 }, attention: 0 } }
  const child = { ...base, session_id: 'child', title: 'Worker', created_at: '2026-10-01T11:00:00Z', family: { revision: 1, parent: { session_id: 'parent', agent: 'claude' }, children: [], child_counts: { total: 0, active: 0, waiting: 0 }, attention: 0 } }
  const extra = Array.from({ length: count - 1 }, (_, i) => ({ ...child, session_id: `child-${i}`, title: `Worker ${i}`, created_at: new Date(Date.UTC(2026, 9, 1, 10, 30, i)).toISOString() }))
  const rows: SessionSummary[] = [parent, child, ...extra, { ...base, session_id: 'other', title: 'Other project', created_at: '2026-10-01T10:00:00Z' }, { ...child, session_id: 'closed', title: 'Closed worker', lifecycle: 'closed' }]
  const streams: Route[] = [], unmatched: string[] = []
  await context.route('**/api/**', route => {
    const url = new URL(route.request().url()), path = url.pathname
    const json = (value: unknown) => route.fulfill({ contentType: 'application/json', body: JSON.stringify(value) })
    if (/^\/api\/sessions\/[^/]+\/queue$/.test(path)) return json([])
    if (path === '/api/capabilities') return json({ mode: 'full', features: [] })
    if (path === '/api/hosts') return json([{ host_id: 'host', name: hostname, connected: true, capabilities: [] }])
    if (path === '/api/hats') return json([{ id: 'hat', name: 'Personal', colour: '#8050d0', created_at: ts, purging: false, default_for_new_hosts: false }])
    if (path === '/api/view/sessions') return json({ sessions: rows.filter(row => !url.searchParams.has('lifecycle') || url.searchParams.get('lifecycle')!.split(',').includes(row.lifecycle)), epoch: 'list', revision: 1, waiting: 0 })
    if (path === '/api/stream/sessions') { streams.push(route); return }
    if (path === '/api/sessions/parent') return json({ ...parent, pending: [] })
    if (path === '/api/view/sessions/parent') return json({ items: [], epoch: 'items', revision: 1, older: false })
    if (path === '/api/sessions/parent/catalog') return json({ session_id: 'parent', config_options: [], commands: [] })
    if (path === '/api/sessions/parent/usage') return route.fulfill({ status: 404, body: '{"code":"not_found"}' })
    if (path === '/api/view/sessions/parent/family') {
      const offset = Number(url.searchParams.get('after') ?? 0)
      const all = rows.filter(row => row.family?.parent?.session_id === 'parent')
      return json({ summary: parent, children: all.slice(offset, offset + 20).map(summary => ({ summary, operations: [], questions: [], routing: 'active' })), next_cursor: all.length > offset + 20 ? String(offset + 20) : undefined, questions: [] })
    }
    if (path === '/api/stream/view/sessions/parent') { streams.push(route); return }
    if (path === '/api/delegation/attention' || path === '/api/push/subscriptions') return json([])
    unmatched.push(path); return route.fulfill({ status: 404, contentType: 'application/json', body: '{"code":"not_found"}' })
  })
  try {
    const page = await context.newPage(); await page.goto(collector.origin + '/sessions')
    const links = page.locator('[data-session-link]')
    await expect(links.locator('.sess-name')).toHaveText(['Other project', 'Coordinator'])
    const chip = page.getByRole('button', { name: `↳ ${count} ${count === 1 ? 'child' : 'children'}`, exact: true })
    await expect(chip).toHaveAttribute('aria-expanded', 'false')
    const rowHeight = (await page.locator('[data-session-id="parent"]').boundingBox())!.height
    expect(rowHeight).toBeLessThanOrEqual(width === 1280 ? 64 : 100)
    const chipBox = (await chip.boundingBox())!
    const menuBox = (await page.locator('[data-session-id="parent"]').getByRole('button', { name: 'Session actions', exact: true }).boundingBox())!
    const rowBox = (await page.locator('[data-session-id="parent"]').boundingBox())!
    expect(chipBox.x + chipBox.width).toBeLessThanOrEqual(menuBox.x)
    expect(menuBox.x + menuBox.width).toBeLessThanOrEqual(rowBox.x + rowBox.width)
    await chip.click()
    const visible = ['Other project', 'Coordinator', 'Worker', ...(count === 100 ? ['Worker 98', 'Worker 97', 'Worker 96', 'Worker 95'] : [])]
    await expect(links.locator('.sess-name')).toHaveText(visible)
    await expect(page.locator('.sess-child')).toHaveCount(Math.min(5, count))
    await expect(page.getByRole('link', { name: `Show all ${count} ${count === 1 ? 'child' : 'children'}`, exact: true })).toHaveAttribute('href', '/sessions/parent?family=1')
    await expect(page.getByText(/^(Parent:|Child:)/)).toHaveCount(0)
    await expect(page.locator('[data-session-id="child"] .sess-branch')).toHaveText('↳')
    await expect(page.locator('[data-session-id="child"] [data-session-link]')).toHaveAttribute('href', '/sessions/child')
    const before = await page.locator('.session-table tbody').evaluateHandle(body => {
      const state = { rows: [...body.children], changes: [] as string[] }
      const observer = new MutationObserver(records => { for (const record of records) if (record.target === body) state.changes.push(record.type) })
      return { ...state, observer }
    })
    // Observe the tbody itself: streaming may update row text, never move rows.
    await before.evaluate(state => state.observer.observe(state.rows[0].parentElement!, { childList: true }))
    await expect.poll(() => streams.length).toBe(1)
    const events = Array.from({ length: 60 }, (_, i) => `event: session_upsert\ndata: ${JSON.stringify({ ...child, activity: i % 2 ? 'running' : 'idle', last_event_at: new Date(Date.UTC(2026, 9, 2, 11, 0, i)).toISOString() })}\nid: list:${i + 2}\n\n`).join('')
    await streams[0].fulfill({ contentType: 'text/event-stream', body: events })
    await expect(page.locator('[data-session-id="child"] .row-status')).toHaveText('Running')
    await expect(links.locator('.sess-name')).toHaveText(visible)
    expect(await before.evaluate(state => ({ same: state.rows.every((row, i) => row === row.parentElement?.children[i]), changes: state.changes }))).toEqual({ same: true, changes: [] })
    await before.evaluate(state => state.observer.disconnect())
    for (const grouping of ['host', 'hat']) {
      await page.getByRole('radio', { name: grouping === 'host' ? 'Host' : 'Hat', exact: true }).check()
      await expect(links.locator('.sess-name')).toHaveText(visible)
      expect(await page.evaluate(() => localStorage.getItem('hennery.group'))).toBe(grouping)
    }
    await page.getByRole('radio', { name: 'Project', exact: true }).check()
    await expect(page.locator('.grp-path')).toHaveAttribute('title', '/srv/project')
    await expect(links.locator('.sess-name')).toHaveText(visible)
    await page.getByRole('radio', { name: 'Nothing', exact: true }).check()
    await expect(page.locator('.project-toggle')).toHaveCount(0)
    const host = page.locator('[data-session-id="parent"] [data-label="Host"] bdi')
    const model = page.locator('[data-session-id="parent"] [data-label="Agent / model"] bdi')
    await expect(host).toHaveAttribute('title', hostname)
    await expect(model).toHaveAttribute('title', base.model!)
    await expect(model).toHaveText('Claude · Haiku 4.5')
    if (width === 1280) expect(await host.evaluate(el => { const css = getComputedStyle(el); return css.whiteSpace === 'nowrap' && css.textOverflow === 'ellipsis' && el.scrollWidth > el.clientWidth })).toBe(true)
    if (width === 390) await page.getByRole('button', { name: 'Filters', exact: true }).click()
    await page.getByRole('button', { name: 'Hide closed', exact: true }).click()
    await expect(page.getByText(`↳ ${count + 1} children`, { exact: true })).toBeVisible()
    await expect(page.locator('[data-session-id="closed"]')).toHaveCount(1)
    if (width === 390) await page.getByRole('button', { name: 'Close filters', exact: true }).click()
    await page.getByRole('link', { name: `Show all ${count + 1} children`, exact: true }).click()
    await expect(page).toHaveURL(collector.origin + '/sessions/parent?family=1')
    const family = page.getByRole('region', { name: 'Session family', exact: true })
    await expect(family).toBeVisible()
    if (count === 100) {
      const children = family.locator('.family-child:not(.family-current)')
      await expect(children).toHaveCount(20)
      for (const size of [40, 60, 80, 100, 101]) {
        await family.getByRole('button', { name: 'Load more children', exact: true }).click()
        await expect(children).toHaveCount(size)
      }
    }
    expect(unmatched).toEqual([])
  } finally { for (const stream of streams) await stream.abort().catch(() => {}); await context.close() }
})
