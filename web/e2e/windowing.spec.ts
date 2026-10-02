// How long a transcript takes to render and to scroll as it grows (client
// view spec §9 OQ1, frontend spec §15): the measurement that found the
// threshold, between 200 and 500 items under load, which set the session
// view's tail window at the newest 200 items (plan 4c decision 20). Kept so
// it can be taken again on a quiet machine, and when the item renderers
// change.
//
// - No server: the built UI (`pnpm build`, `dist/`) is served to Chromium
//   from `page.route` on an origin of its own, and every `/api` route the
//   session view asks for is answered here. An `/api` request with no answer
//   fails the run, so a login screen is never what gets timed.
// - The session is the golden fixtures' items (both pinned adapters),
//   repeated with fresh ids and turn ids until it holds N items, all in its
//   first page. Its stream is held open and never sends.
// - First render: from the page's response to two frames after the Nth item
//   is in the DOM. Scrolling: 240 frames of 400 px each from the end, the
//   gaps between frames and any long task, beside 240 frames standing still
//   (the frame rate the browser keeps on this machine anyway). A row whose
//   still frames are slower than about one frame (p95 > 20 ms) is marked
//   not valid: the machine was busy.
// - Run at the desktop's speed and with the CPU slowed 4× (a phone).
//
// A timing on a shared CI runner says little, so this runs only when asked:
// `HENNERY_WINDOWING=1 pnpm exec playwright test windowing` after
// `pnpm build`, with the flake's browsers.
import { expect, test, type Page, type Route } from '@playwright/test'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { loadavg } from 'node:os'
import { extname, join, resolve } from 'node:path'

test.skip(!process.env.HENNERY_WINDOWING, 'a measurement: set HENNERY_WINDOWING=1 to take it')
test.describe.configure({ mode: 'serial', timeout: 300_000 })

const ORIGIN = 'http://hennery.test'
const ID = 'long-session'
const DIST = resolve(process.cwd(), 'dist')
const FIXTURES = resolve(process.cwd(), '../crates/hennery-view/tests/fixtures')
const SIZES = [200, 500, 1000, 2000, 5000]
const SCROLL_FRAMES = 240
const SCROLL_STEP = 400
const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='

type Raw = Record<string, unknown> & { id: string; turn_id?: string }

const GOLDEN: Raw[] = readdirSync(FIXTURES)
  .filter((f) => f.endsWith('.items.json'))
  .sort()
  .flatMap((f) => JSON.parse(readFileSync(join(FIXTURES, f), 'utf8')) as Raw[])

/** N items: the golden ones again and again, each pass with its own ids and
 *  turns, so the store holds every one in its own group. */
function longSession(n: number): Raw[] {
  const out: Raw[] = []
  for (let pass = 0; out.length < n; pass++) {
    for (const item of GOLDEN) {
      if (out.length === n) break
      out.push({ ...item, id: `p${pass}:${item.id}`, turn_id: `p${pass}:${item.turn_id ?? 'start'}` })
    }
  }
  return out
}

const TYPES: Record<string, string> = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
  '.woff2': 'font/woff2',
}

interface Served {
  unanswered: string[]
  release: () => Promise<void>
}

async function serve(page: Page, items: Raw[]): Promise<Served> {
  const unanswered: string[] = []
  const held: Route[] = []
  const json = (route: Route, body: unknown) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) })
  const detail = {
    session_id: ID,
    host_id: 'h1',
    agent: 'claude',
    cwd: '/srv/work/long',
    hat_id: 'hat1',
    title: 'A long session',
    lifecycle: 'active',
    activity: 'idle',
    presumed_parked: false,
    created_at: '2026-10-02T09:00:00.000Z',
    last_event_at: '2026-10-02T10:00:00.000Z',
    pending: [],
  }
  const page_ = JSON.stringify({ items, older: false, epoch: 'e1', revision: items.length })
  await page.route(`${ORIGIN}/**`, async (route) => {
    const { pathname } = new URL(route.request().url())
    if (pathname.startsWith('/api/')) {
      if (pathname === '/api/capabilities') return json(route, { mode: 'full', features: [] })
      if (pathname === `/api/view/sessions/${ID}`)
        return route.fulfill({ status: 200, contentType: 'application/json', body: page_ })
      if (pathname === `/api/stream/view/sessions/${ID}`) {
        held.push(route)
        return
      }
      if (pathname === `/api/sessions/${ID}`) return json(route, detail)
      if (pathname === `/api/sessions/${ID}/catalog`) return json(route, { session_id: ID, config_options: [], commands: [] })
      if (pathname === '/api/hosts') return json(route, [{ host_id: 'h1', name: 'build-box' }])
      if (pathname === '/api/hats') return json(route, [])
      // A prompt's image: one pixel, whatever its hash.
      if (pathname.startsWith('/api/attachments/'))
        return route.fulfill({ status: 200, contentType: 'image/png', body: Buffer.from(PIXEL, 'base64') })
      unanswered.push(pathname)
      return route.fulfill({ status: 404, contentType: 'application/json', body: '{"code":"not_found","message":"no"}' })
    }
    const file = join(DIST, pathname)
    if (pathname !== '/' && existsSync(file) && !file.endsWith('/'))
      return route.fulfill({ status: 200, contentType: TYPES[extname(file)] ?? 'application/octet-stream', body: readFileSync(file) })
    return route.fulfill({ status: 200, contentType: 'text/html', body: readFileSync(join(DIST, 'index.html')) })
  })
  return {
    unanswered,
    release: async () => {
      for (const route of held.splice(0)) await route.abort().catch(() => {})
    },
  }
}

/** Installed before the app: the time the page's response came, the time
 *  the Nth item was painted, and every long task. */
function probe(want: number) {
  const m = { longtasks: [] as [number, number][], t0: 0, t1: 0 }
  ;(window as unknown as { __m: typeof m }).__m = m
  new PerformanceObserver((list) => {
    for (const e of list.getEntries()) m.longtasks.push([e.startTime, e.duration])
  }).observe({ type: 'longtask', buffered: true })
  const original = window.fetch
  window.fetch = async (...args: Parameters<typeof fetch>) => {
    const response = await original(...args)
    const url = String(args[0] instanceof Request ? args[0].url : args[0])
    if (!m.t0 && /\/api\/view\/sessions\/[^/?]+(\?|$)/.test(url)) m.t0 = performance.now()
    return response
  }
  const tick = () => {
    const inner = document.querySelector('.transcript-inner')
    if (m.t0 && inner && inner.children.length >= want) {
      requestAnimationFrame(() => requestAnimationFrame(() => (m.t1 = performance.now())))
      return
    }
    requestAnimationFrame(tick)
  }
  requestAnimationFrame(tick)
}

async function scrollCost(page: Page) {
  return page.evaluate(
    async ({ frames, step }) => {
      const m = (window as unknown as { __m: { longtasks: [number, number][] } }).__m
      const frame = () => new Promise<number>((r) => requestAnimationFrame(r))
      const el = document.querySelector('.transcript') as HTMLElement
      el.scrollTop = el.scrollHeight
      await frame()
      await frame()
      // The same number of frames standing still first: the browser's own
      // frame rate on this machine, which the scrolling is read against.
      const run = async (move: boolean) => {
        const from = performance.now()
        const gaps: number[] = []
        let last = await frame()
        for (let i = 0; i < frames; i++) {
          if (move) el.scrollTop -= step
          const now = await frame()
          gaps.push(now - last)
          last = now
        }
        gaps.sort((a, b) => a - b)
        const long = m.longtasks.filter(([start]) => start >= from)
        return {
          mean: gaps.reduce((a, b) => a + b, 0) / gaps.length,
          p95: gaps[Math.floor(gaps.length * 0.95)],
          max: gaps[gaps.length - 1],
          longTasks: long.length,
          longestTask: Math.max(0, ...long.map(([, d]) => d)),
        }
      }
      const still = await run(false)
      const moving = await run(true)
      return { still, moving, height: el.scrollHeight }
    },
    { frames: SCROLL_FRAMES, step: SCROLL_STEP },
  )
}

for (const slowdown of [1, 4]) {
  for (const n of SIZES) {
    test(`${n} items, CPU ×${slowdown}`, async ({ browser }) => {
      const context = await browser.newContext({ viewport: { width: 1280, height: 800 } })
      const page = await context.newPage()
      const served = await serve(page, longSession(n))
      try {
        if (slowdown > 1) {
          const cdp = await context.newCDPSession(page)
          await cdp.send('Emulation.setCPUThrottlingRate', { rate: slowdown })
        }
        await page.addInitScript(probe, n)
        await page.goto(`${ORIGIN}/sessions/${ID}`)
        await page.waitForFunction(() => (window as unknown as { __m: { t1: number } }).__m.t1 > 0, undefined, {
          timeout: 240_000,
          polling: 100,
        })
        const first = await page.evaluate(() => {
          const m = (window as unknown as { __m: { t0: number; t1: number; longtasks: [number, number][] } }).__m
          const during = m.longtasks.filter(([start]) => start >= m.t0 && start <= m.t1)
          return {
            firstRenderMs: m.t1 - m.t0,
            longestTaskMs: Math.max(0, ...during.map(([, d]) => d)),
            domNodes: document.getElementsByTagName('*').length,
            heapMB: ((performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ?? 0) / 2 ** 20,
          }
        })
        const scroll = await scrollCost(page)
        expect(served.unanswered).toEqual([])
        expect(await page.locator('.transcript-inner > *').count()).toBe(n)
        // A row counts only if the browser kept its frame rate standing
        // still: else it measured the machine, not the transcript.
        const row = { n, slowdown, load1: loadavg()[0], valid: scroll.still.p95 <= 20, ...first, scroll }
        console.log(`WINDOWING ${JSON.stringify(row)}`)
        test.info().annotations.push({ type: 'windowing', description: JSON.stringify(row) })
      } finally {
        await served.release()
        await context.close()
      }
    })
  }
}
