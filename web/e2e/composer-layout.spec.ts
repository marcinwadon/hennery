// Embedded UI + synthetic API data. Real keyboard/device acceptance is separate.
import { expect, test, type Route } from '@playwright/test'
import { startCollector, type Collector } from './collector'
import type { QueuedPrompt, SessionCatalog } from '../src/generated/protocol'
import type { SessionSummary } from '../src/generated/view'

let app: Collector
test.beforeAll(async () => { app = await startCollector() })
test.afterAll(async () => { await app?.stop() })

for (const viewport of [{ width: 320, height: 568 }, { width: 390, height: 844 }, { width: 844, height: 390 }, { width: 1280, height: 900 }]) {
  test(`composer fits ${viewport.width}x${viewport.height} with long settings, queue and pending read-back`, async ({ browser }) => {
    const context = await browser.newContext({ viewport, reducedMotion: 'reduce' })
    const held: Route[] = []
    let configRequest: Route | undefined
    let promptCalls = 0
    const now = '2026-10-20T10:00:00Z'
    const info: SessionSummary = { session_id: 'worker', host_id: 'host1', hat_id: 'hat1', title: 'Synthetic composer', cwd: '/srv/synthetic/composer', agent: 'codex', model: 'model-a', mode: 'review', created_at: now, last_event_at: now, lifecycle: 'active', activity: 'running', presumed_parked: false, question_waits: false, operator_attention: 0, parent_handled_questions: 0 }
    const longName = 'A very long model name with an extended reasoning and context window description'
    const catalog: SessionCatalog = { session_id: 'worker', commands: [{ name: 'review', description: 'Review the current changes' }], config_options: [
      { id: 'model', name: 'Model', category: 'model', type: 'select', currentValue: 'model-a', options: [{ value: 'model-a', name: longName }, { value: 'model-b', name: 'Another long model name for the next prompt' }] },
      { id: 'mode', name: 'Mode', category: 'mode', type: 'select', currentValue: 'review', options: [{ value: 'review', name: 'A long mode name for reviewing changes before execution' }] },
      { id: 'effort', name: 'Effort', category: 'thought_level', type: 'select', currentValue: 'medium', options: ['low', 'medium', 'high'].map(value => ({ value, name: value })) },
      { id: 'extra', name: 'An additional setting with a long descriptive label', type: 'boolean', currentValue: false },
    ] }
    let queue: QueuedPrompt[] = Array.from({ length: 4 }, (_, i) => ({ id: `q${i}`, state: i === 3 ? 'failed' : 'queued', reason: i === 3 ? 'The previous message could not be delivered.' : null, content: [{ type: 'text', text: `Queued message ${i + 1}: ${'long-content-'.repeat(30)}` }] }))
    await context.route('**/api/**', async route => {
      const path = new URL(route.request().url()).pathname
      const json = (value: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(value) })
      if (path === '/api/capabilities') return json({ mode: 'full', features: [] })
      if (path === '/api/hats') return json([{ id: 'hat1', name: 'Synthetic project', colour: '#8a3cbd' }])
      if (path === '/api/hosts') return json([{ host_id: 'host1', name: 'Synthetic host', connected: true, capabilities: ['images'] }])
      if (path === '/api/view/sessions') return json({ sessions: [info], epoch: 'list', revision: 1, waiting: 0 })
      if (path.startsWith('/api/stream/')) { held.push(route); return }
      if (path === '/api/view/sessions/worker') return json({ items: [{ id: 'm', kind: 'message', version: 1, ts: now, text: 'Ready for another prompt.' }], epoch: 'items', revision: 1, older: false })
      if (path === '/api/sessions/worker') return json({ ...info, pending: [] })
      if (path.endsWith('/catalog')) return json(catalog)
      if (path.endsWith('/family')) return json({ summary: info, children: [], questions: [], routing: 'standalone' })
      if (path.endsWith('/queue')) return json(queue)
      if (path.endsWith('/queue/q0')) { queue = queue.filter(item => item.id !== 'q0'); return json(queue) }
      if (path.endsWith('/config')) { configRequest = route; return }
      if (path.endsWith('/prompt')) { promptCalls++; return json({ queue_id: 'next' }, 202) }
      if (path.endsWith('/cancel')) return json({ outcome: 'cancelled' }, 202)
      if (path === '/api/delegation/attention') return json([])
      throw new Error(`Unmatched composer route: ${path}`)
    })
    const page = await context.newPage()
    try {
      await page.goto(`${app.origin}/sessions/worker`)
      const input = page.getByRole('textbox', { name: 'Prompt', exact: true })
      await input.fill('A multiline draft\nthat keeps the full phone width.')
      const composer = page.locator('.composer')
      const box = page.locator('.composer-box')
      const send = composer.getByRole('button', { name: 'Send', exact: true })
      const stop = composer.getByRole('button', { name: 'Cancel', exact: true })
      const settings = composer.getByRole('button', { name: 'Session settings', exact: true })
      await expect(stop).toBeVisible()
      const geometry = await box.evaluate(element => {
        const input = element.querySelector('textarea')!.getBoundingClientRect()
        const outer = element.getBoundingClientRect()
        return { inputWidth: input.width, boxWidth: outer.width, inputBottom: input.bottom, outerBottom: outer.bottom, scrollWidth: element.scrollWidth, clientWidth: element.clientWidth }
      })
      await test.info().attach('composer-geometry', { body: JSON.stringify(geometry), contentType: 'application/json' })
      console.info('Composer geometry', viewport, geometry)
      await page.screenshot({ path: test.info().outputPath('composer.png') })
      // A hidden global overflow must not mask a squeezed or overflowing field.
      expect(geometry.inputWidth).toBeGreaterThan(geometry.boxWidth * 0.85)
      expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.clientWidth + 1)
      await expect(input).toBeInViewport()
      await expect(send).toBeInViewport()
      await expect(stop).toBeInViewport()
      if (viewport.width < 1024) {
        for (const control of [send, stop, settings, composer.getByRole('button', { name: 'Attach images', exact: true })]) {
          const bounds = (await control.boundingBox())!
          expect(bounds.width).toBeGreaterThanOrEqual(44)
          expect(bounds.height).toBeGreaterThanOrEqual(44)
        }
      }
      const queued = page.getByRole('region', { name: 'Queued messages', exact: true })
      await expect(queued.getByRole('heading', { name: /^Queued/ })).toBeVisible()
      await settings.click()
      const panel = viewport.width < 1024 ? page.getByRole('dialog', { name: 'Session details', exact: true }) : page.getByRole('complementary', { name: 'Session details', exact: true })
      const model = panel.getByRole('combobox', { name: 'Model', exact: true })
      await expect(model).toBeFocused()
      await expect(panel.getByText(longName, { exact: false }).last()).toBeVisible()
      expect(await panel.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true)
      await model.selectOption('model-b')
      await expect(panel.getByText('Applying session settings…', { exact: true })).toBeVisible()
      await expect(model).toBeDisabled()
      await panel.getByRole('button', { name: 'Close session details', exact: true }).click()
      await expect(settings).toBeFocused()
      await expect(send).toBeDisabled()
      await input.fill('My draft stays editable while applying.')
      await input.press('Control+Enter')
      expect(promptCalls).toBe(0)
      await expect(stop).toBeEnabled()
      await settings.click()
      await expect(model).toBeDisabled()
      await page.setViewportSize(viewport.width < 1024 ? { width: 1280, height: 900 } : { width: 390, height: 844 })
      await expect(send).toBeDisabled()
      await page.setViewportSize(viewport)
      await expect(model).toHaveValue('model-b')
      await expect(model).toBeDisabled()
      await expect.poll(() => !!configRequest).toBe(true)
      await configRequest!.fulfill({ status: 202, contentType: 'application/json', body: JSON.stringify(catalog) })
      configRequest = undefined
      await expect(panel.getByRole('alert')).toContainText('the agent kept model-a instead of model-b')
      await expect(model).toHaveValue('model-a')
      await model.focus()
      await page.keyboard.press('Escape')
      await expect(settings).toBeFocused()
      await expect(composer.getByRole('button', { name: 'Check settings', exact: true })).toBeVisible()
      await expect(send).toBeEnabled()
      await expect(input).toHaveValue('My draft stays editable while applying.')
      expect(promptCalls).toBe(0)
      // Twenty images occupy one bounded strip rather than twenty wrapped chips.
      const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=', 'base64')
      await composer.getByTestId('image-input').setInputFiles(Array.from({ length: 20 }, (_, i) => ({ name: `image-${i}.png`, mimeType: 'image/png', buffer: png })))
      const images = composer.getByRole('list', { name: 'Images', exact: true })
      await expect(images.getByRole('listitem')).toHaveCount(20)
      if (viewport.width < 1024) {
        const remove = (await images.getByRole('button', { name: 'Remove image #1', exact: true }).boundingBox())!
        expect(remove.width).toBeGreaterThanOrEqual(44)
        expect(remove.height).toBeGreaterThanOrEqual(44)
      }
      expect((await images.boundingBox())!.height).toBeLessThanOrEqual(65)
      expect((await images.boundingBox())!.y + (await images.boundingBox())!.height).toBeLessThanOrEqual(viewport.height)
      const composerBounds = (await composer.boundingBox())!
      expect(composerBounds.y + composerBounds.height).toBeLessThanOrEqual(viewport.height)
      // Composer sizing must not shrink the independent queued-message editor.
      const draftWithImages = await input.inputValue()
      await queued.getByRole('button', { name: 'Edit queued message 1', exact: true }).click()
      const queueEditor = queued.getByRole('textbox', { name: 'Edit queued message 1', exact: true })
      await queueEditor.fill('An independent queue edit')
      expect(await queueEditor.evaluate(element => parseFloat(getComputedStyle(element).minHeight))).toBeGreaterThanOrEqual(72)
      await expect(input).toHaveValue(draftWithImages)
      await queued.getByRole('button', { name: 'Cancel edit', exact: true }).click()
      await expect(send).toBeInViewport()
      expect(await composer.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true)
      await page.emulateMedia({ colorScheme: 'dark' })
      await page.screenshot({ path: test.info().outputPath('composer-warning-dark.png') })
    } finally {
      if (configRequest) await configRequest.abort().catch(() => {})
      for (const route of held) await route.fulfill({ status: 200, contentType: 'text/event-stream', body: '' }).catch(() => {})
      await context.close()
    }
  })
}
