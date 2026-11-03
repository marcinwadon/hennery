// Real embedded assets and emitted notification worker, with deterministic
// family API boundaries. The live adapter matrix is a separate release gate.
import { expect, test, type BrowserContext, type Route } from '@playwright/test'
import { startCollector, type Collector } from './collector'
import { workerHarness } from './push-fixture'
import type { FamilyPage, SessionSummary, Item } from '../src/generated/view'

let collector: Collector
test.beforeAll(async () => { collector = await startCollector() })
test.afterAll(async () => { await collector?.stop() })

function summary(id: string): SessionSummary {
  return {
    session_id: id, title: id === 'parent' ? 'Coordinator' : 'Parser worker', host_id: 'host1', agent: id === 'parent' ? 'claude' : 'codex',
    model: id === 'parent' ? 'planner' : 'reasoner', cwd: id === 'parent' ? '/srv/project' : '/srv/worker', hat_id: 'hat1',
    lifecycle: 'active', activity: id === 'parent' ? 'idle' : 'blocked', presumed_parked: false,
    created_at: '2026-10-02T09:00:00Z', last_event_at: '2026-10-02T10:00:00Z', question_waits: id !== 'parent',
    operator_attention: 0, parent_handled_questions: id === 'parent' ? 0 : 1,
    family: { revision: 1, parent: id === 'parent' ? undefined : { session_id: 'parent', title: 'Coordinator', agent: 'claude' },
      children: id === 'parent' ? [{ session_id: 'child', title: 'Parser worker', agent: 'codex' }] : [],
      child_counts: { total: id === 'parent' ? 2 : 0, active: id === 'parent' ? 2 : 0, waiting: id === 'parent' ? 1 : 0 }, attention: 0 },
  }
}

async function fixture(context: BrowserContext, statusOnly = false) {
  let statusOutstanding = statusOnly, attentionReads = 0
  let detached = false, familyReads = 0, itemReads = 0, answers = 0
  const listStreams: Route[] = [], itemStreams: Route[] = [], unmatched: string[] = [], held: Route[] = []
  const current = (id: string) => {
    const s = summary(id)
    if (statusOnly) {
      s.family!.attention = statusOutstanding ? 1 : 0
      if (id === 'child') { s.lifecycle = 'failed'; s.activity = undefined; s.question_waits = false; s.operator_attention = statusOutstanding ? 1 : 0; s.parent_handled_questions = 0 }
    }
    if (detached && id === 'child') { s.operator_attention = 1; s.parent_handled_questions = 0 }
    return s
  }
  const questions = () => [{ pending_id: 'question1', kind: 'elicitation', handling: detached ? 'operator_required' : 'parent_delivered', reason: detached ? 'operator_takeover' : undefined }]
  const family = (id: string, more: boolean): FamilyPage => ({
    summary: current(id), questions: id === 'child' ? questions() : [], routing: id === 'child' ? detached ? 'detached' : 'active' : undefined,
    reason: detached && id === 'child' ? 'operator_takeover' : undefined,
    children: id === 'parent' ? [{ summary: { ...current('child'), session_id: more ? 'child2' : 'child', title: more ? 'Second worker' : 'Parser worker' },
      routing: detached ? 'detached' : 'active', reason: detached ? 'operator_takeover' : undefined,
      operations: [{ operation_id: 'operation1', tool: 'session_spawn', state: 'accepted' }], questions: more ? [] : questions() }] : [],
    next_cursor: id === 'parent' && !more ? 'child' : undefined,
  })
  const question: Item = { id: 'question:question1', version: 1, ts: '2026-10-02T10:00:00Z', kind: 'question', pending_id: 'question1', question_kind: 'elicitation',
    request: { type: 'elicitation', message: 'Which parser?', fields: [{ key: 'parser', label: 'Parser', field_kind: 'text' }], form_supported: true }, answerable: true, state: 'open', answered: false }
  const json = (route: Route, value: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(value) })
  await context.route('**/api/**', async route => {
    const url = new URL(route.request().url()), path = url.pathname
    if (path === '/api/capabilities') return json(route, { mode: 'full', features: [] })
    if (path === '/api/hosts') return json(route, [{ host_id: 'host1', name: 'worker host', connected: true, capabilities: [] }])
    if (path === '/api/hats') return json(route, [])
    if (path === '/api/view/sessions') return json(route, { sessions: [current('parent'), current('child')], epoch: 'list1', revision: 1, waiting: statusOutstanding || detached ? 1 : 0 })
    if (path === '/api/stream/sessions') { listStreams.push(route); held.push(route); return }
    if (path === '/api/delegation/attention') {
      attentionReads++
      return json(route, statusOutstanding ? [{ id: 'attention1', child_id: 'child', hat_id: 'hat1', reason: 'child_failed', pending_id: null }] : detached ? [{ id: 'question-attention', child_id: 'child', hat_id: 'hat1', reason: 'operator_takeover', pending_id: 'question1' }] : [])
    }
    if (path === '/api/delegation/attention/attention1/acknowledge') {
      expect(route.request().method()).toBe('POST'); statusOutstanding = false
      const stream = listStreams.pop()
      if (stream) await stream.fulfill({ contentType: 'text/event-stream', body: `event: session_upsert\ndata: ${JSON.stringify(current('parent'))}\nid: list1:2\n\nevent: session_upsert\ndata: ${JSON.stringify(current('child'))}\nid: list1:3\n\nevent: waiting_changed\ndata: {"count":0}\nid: list1:4\n\n` })
      return route.fulfill({ status: 204 })
    }
    const match = /^\/api\/(?:view\/)?sessions\/(parent|child|child2)(.*)$/.exec(path)
    if (match) {
      const [, id, suffix] = match
      if (suffix === '/usage') return json(route, { code: 'not_found', message: 'No usage observations' }, 404)
      if (suffix === '/family') { familyReads++; return json(route, family(id, url.searchParams.has('after'))) }
      if (suffix === '/takeover') {
        expect(route.request().postDataJSON()).toEqual({}); detached = true
        const stream = listStreams.pop()
        if (stream) await stream.fulfill({ contentType: 'text/event-stream', body: `event: session_upsert\ndata: ${JSON.stringify(current('child'))}\nid: list1:2\n\nevent: waiting_changed\ndata: {"count":1}\nid: list1:3\n\n` })
        return route.fulfill({ status: 204 })
      }
      if (suffix === '/queue') return json(route, [])
      if (suffix === '/catalog') return json(route, { session_id: id, config_options: [], commands: [] })
      if (suffix === '/questions/question1') return json(route, question)
      if (suffix === '/pending/question1/answer') { answers++; return json(route, { pending_id: 'question1', request_id: 'answer1' }, 202) }
      if (suffix === '' && path.startsWith('/api/view/')) { itemReads++; return json(route, { items: id === 'child' ? [question] : [], older: false, epoch: 'items1', revision: 1 }) }
      if (suffix === '') return json(route, { ...current(id), pending: [] })
    }
    if (path.startsWith('/api/stream/view/sessions/')) { itemStreams.push(route); held.push(route); return }
    // PWA foreground subscription checks may run on browsers allowing workers.
    if (path === '/api/push/subscriptions') return json(route, [])
    if (path === '/api/push/vapid') return json(route, { public_key: '' })
    unmatched.push(path); return json(route, { code: 'not_found', message: 'unhandled fixture route' }, 404)
  })
  return { unmatched, attentionReads: () => attentionReads, familyReads: () => familyReads, itemReads: () => itemReads, answers: () => answers,
    resync: async () => { const stream = itemStreams.pop(); expect(stream).toBeDefined(); await stream!.fulfill({ contentType: 'text/event-stream', body: 'event: resync_required\ndata: {}\n\n' }) },
    release: async () => { for (const route of held) await route.abort().catch(() => {}) },
  }
}

for (const width of [1280, 390]) test(`family navigation, takeover, attention and resync at ${width}px`, async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width, height: 844 } })
  const f = await fixture(context)
  try {
    const page = await context.newPage(); await page.goto(`${collector.origin}/sessions/parent`)
    await expect(page.locator('.session-head .badge')).toHaveText('Waiting on children')
    const family = page.getByRole('region', { name: 'Session family', exact: true })
    if (await page.getByRole('button', { name: 'Session details', exact: true }).getAttribute('aria-pressed') !== 'true') await page.getByRole('button', { name: 'Session details', exact: true }).click()
    const child = family.getByRole('article', { name: 'Parser worker', exact: true })
    for (const text of ['Codex', 'reasoner', 'worker host', '/srv/worker', 'active', 'blocked', 'Start child: Accepted', 'Parent is handling']) {
      await expect(child.getByText(text, { exact: true })).toBeVisible()
    }
    await family.getByRole('button', { name: 'Load more children', exact: true }).click()
    await expect(family.getByRole('link', { name: 'Second worker', exact: true })).toBeVisible()
    await child.getByRole('link', { name: 'Open question', exact: true }).click()
    await expect(page).toHaveURL(`${collector.origin}/sessions/child?pending=question1`)
    const card = page.getByRole('region', { name: 'Question from Codex', exact: true })
    await expect(card.getByText('Parent is handling', { exact: true })).toBeVisible()
    await expect(card.getByRole('button', { name: 'Decline', exact: true })).toBeEnabled()
    await expect(page.getByRole('status', { name: '1 waiting on you', exact: true })).toHaveCount(0)
    if (await page.getByRole('button', { name: 'Session details', exact: true }).getAttribute('aria-pressed') !== 'true') await page.getByRole('button', { name: 'Session details', exact: true }).click()
    const take = family.getByRole('button', { name: 'Take over', exact: true })
    if (width === 390) expect((await take.boundingBox())!.height).toBeGreaterThanOrEqual(44)
    await take.click()
    await page.getByRole('button', { name: 'Close session details', exact: true }).click()
    await expect(card.getByText('Operator answer required', { exact: true })).toBeVisible()
    await expect(card.getByText('You took over this session', { exact: true })).toBeVisible()
    await expect(page.locator('.session-head .badge')).toHaveText('Waiting on you')
    await expect(page.getByRole('status', { name: '1 waiting on you', exact: true }).first()).toBeVisible()
    const before = f.familyReads()
    await f.resync()
    await expect.poll(f.familyReads).toBeGreaterThan(before)
    await expect(card.getByText('Operator answer required', { exact: true })).toBeVisible()
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    if (width === 390) await card.getByRole('button', { name: 'Write a prompt', exact: true }).click()
    const prompt = page.getByRole('textbox', { name: 'Prompt', exact: true })
    await expect(prompt).toBeInViewport()
    expect(f.answers()).toBe(0)
    expect(f.unmatched).toEqual([])
  } finally { await f.release(); await context.close() }
})

test('phone notification opens the child card and never sends a lock-screen answer', async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 } })
  const f = await fixture(context)
  try {
    const page = await context.newPage(); await page.goto(`${collector.origin}/sessions/parent`)
    await expect(page.locator('.session-head .badge')).toHaveText('Waiting on children')
    const code = await (await context.request.get(`${collector.origin}/sw.js`)).text()
    const harness = workerHarness(code, undefined, { origin: collector.origin, clients: {
      matchAll: async () => [], openWindow: async target => { await page.goto(collector.origin + target); return null },
    } })
    const target = '/sessions/child?pending=question1'
    await harness.push(JSON.stringify({ title: 'Needs your answer', body: 'Open the child card', tag: 'child', url: target }))
    expect(harness.notifications).toHaveLength(1)
    expect(harness.notifications[0].options).not.toHaveProperty('actions')
    expect(f.answers()).toBe(0)
    await harness.click(target)
    await expect(page).toHaveURL(collector.origin + target)
    const card = page.getByRole('region', { name: 'Question from Codex', exact: true })
    await expect(card.getByText('Which parser?', { exact: true })).toBeVisible()
    await expect(card.getByText('Parent is handling', { exact: true })).toBeVisible()
    await expect(card.getByRole('textbox', { name: 'Parser', exact: true })).toBeInViewport()
    expect(f.answers()).toBe(0)
    expect(f.unmatched).toEqual([])
  } finally { await f.release(); await context.close() }
})


test('phone status-only child attention can be acknowledged without an answer card', async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 } })
  const f = await fixture(context, true)
  try {
    const page = await context.newPage(); await page.goto(`${collector.origin}/sessions/parent`)
    const family = page.getByRole('region', { name: 'Session family', exact: true })
    if (await page.getByRole('button', { name: 'Session details', exact: true }).getAttribute('aria-pressed') !== 'true') await page.getByRole('button', { name: 'Session details', exact: true }).click()
    const attention = family.getByRole('region', { name: 'Delegation attention', exact: true })
    await expect(attention.getByText('Child session failed', { exact: true })).toBeVisible()
    await expect(attention.getByRole('link', { name: 'Parser worker', exact: true })).toHaveAttribute('href', '/sessions/child')
    await expect(page.getByRole('status', { name: '1 waiting on you', exact: true }).first()).toBeVisible()
    const before = f.familyReads()
    const acknowledge = attention.getByRole('button', { name: 'Acknowledge Child session failed for Parser worker', exact: true })
    expect((await acknowledge.boundingBox())!.height).toBeGreaterThanOrEqual(44)
    await acknowledge.click()
    await expect(attention.getByText('Child session failed', { exact: true })).toHaveCount(0)
    await expect(page.getByRole('status', { name: '1 waiting on you', exact: true })).toHaveCount(0)
    await expect.poll(f.familyReads).toBeGreaterThan(before)
    expect(f.attentionReads()).toBeGreaterThan(0)
    expect(f.answers()).toBe(0)
    expect(f.unmatched).toEqual([])
  } finally { await f.release(); await context.close() }
})
