import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { startCollector, type Collector } from './collector'
import { spawnHennery } from './spawn'
import { defaultHostDir, fakeAgent, testHost, type TestHost } from './host'

const select = (id: string, value: string, choices: string[]) => ({ id, name: id, category: id === 'effort' ? 'thought_level' : id, type: 'select', currentValue: value, options: choices.map(value => ({ value, name: value })) })
const options = [select('model', 'opus', ['opus', 'sonnet']), select('mode', 'default', ['default', 'bypass']), select('effort', 'low', ['low', 'high']), { id: 'fast', name: 'Fast', type: 'boolean', currentValue: false }]

async function setup(page: Page, collector: Collector) {
  await page.goto(collector.setupLink)
  await page.getByLabel(/^Password at least/).fill('correct horse battery staple')
  await page.getByLabel('Password again', { exact: true }).fill('correct horse battery staple')
  await page.getByRole('button', { name: 'Set up', exact: true }).click()
  await page.getByRole('button', { name: 'Skip', exact: true }).click()
  await expect(page).toHaveURL(`${collector.origin}/sessions`)
}
async function pair(request: APIRequestContext, collector: Collector, host: TestHost) {
  const code = await (await request.post('/api/hosts/pairing-codes', { headers: { origin: collector.origin } })).json()
  expect(await host.join(`hennery host join ${collector.origin} ${code.code}`, ['--name', 'Clone host', '--no-runtime'])).toBe(0)
  const path = join(defaultHostDir(host.dir), 'host.toml')
  writeFileSync(path, `workspace_roots = [${JSON.stringify(realpathSync(host.dir))}]\n${readFileSync(path, 'utf8')}`)
  host.run()
  let id = ''
  await expect(async () => {
    const hosts = await (await request.get('/api/hosts')).json()
    const host = hosts.find((h: { name: string; connected: boolean }) => h.name === 'Clone host' && h.connected)
    expect(host).toBeTruthy(); id = host.host_id
  }).toPass({ timeout: 30_000 })
  return id
}

// Keep Git under the same isolated environment as every browser-test process.
async function git(home: string, cwd: string, args: string[]): Promise<string> {
  const child = spawnHennery(['-C', cwd, ...args], home, {}, ['ignore', 'pipe', 'pipe'], 'git')
  let output = '', errors = ''
  child.stdout?.on('data', (chunk: Buffer) => { output += chunk.toString() })
  child.stderr?.on('data', (chunk: Buffer) => { errors += chunk.toString() })
  await new Promise<void>((resolve, reject) => {
    child.once('error', reject)
    child.once('close', code => code === 0 ? resolve() : reject(new Error(`scratch Git failed: ${errors}`)))
  })
  return output.trim()
}

for (const [width, appearance] of [[390, 'light'], [1280, 'dark']] as const) {
  test(`New like this copies current settings from header and list at ${width}px ${appearance}`, async ({ browser }) => {
    test.setTimeout(120_000)
    const collector = await startCollector()
    const host = testHost(fakeAgent('claude', { config_options: options, chunks: ['Ready.'] }))
    const context = await browser.newContext({ baseURL: collector.origin, viewport: { width, height: width < 768 ? 844 : 900 }, reducedMotion: 'reduce', colorScheme: appearance })
    const page = await context.newPage()
    const prompts: string[] = [], clones: string[] = []
    page.on('request', request => {
      if (request.method() !== 'POST') return
      if (/\/prompt$/.test(request.url())) prompts.push(request.url())
      if (/\/new-like-this$/.test(request.url())) clones.push(request.url())
    })
    try {
      await setup(page, collector)
      const hostId = await pair(context.request, collector, host)
      const cwd = join(realpathSync(host.dir), 'synthetic-project'); mkdirSync(cwd)
      await git(host.dir, cwd, ['init', '-q'])
      writeFileSync(join(cwd, 'base.txt'), 'COMMITTED')
      await git(host.dir, cwd, ['add', 'base.txt'])
      await git(host.dir, cwd, ['-c', 'user.name=Browser test', '-c', 'user.email=browser@example.invalid', 'commit', '-qm', 'base'])
      const base = await git(host.dir, cwd, ['rev-parse', 'HEAD'])
      writeFileSync(join(cwd, 'base.txt'), 'SOURCE_LOCAL_CHANGES')
      const headers = { origin: collector.origin }
      const started = await context.request.post('/api/sessions', { headers, data: { host_id: hostId, agent: 'claude', cwd } })
      expect(started.ok()).toBe(true)
      const source = (await started.json()).session_id
      // Change the live session after starting; these values must beat defaults.
      for (const [config_id, value] of [['model', 'sonnet'], ['mode', 'bypass'], ['effort', 'high'], ['fast', true], ['fast', false]] as const) {
        expect((await context.request.post(`/api/sessions/${source}/config`, { headers, data: { config_id, value } })).ok()).toBe(true)
      }
      expect((await context.request.patch(`/api/sessions/${source}`, { headers, data: { title: 'Source session' } })).ok()).toBe(true)
      const sourceDetail = await (await context.request.get(`/api/sessions/${source}`)).json()
      const saved = await (await context.request.get(`/api/hats/${sourceDetail.hat_id}/session-defaults?agent=claude`)).json()
      expect((await context.request.put(`/api/hats/${sourceDetail.hat_id}/session-defaults?agent=claude`, { headers, data: { revision: saved.revision, config: { model: 'opus', mode: 'default', axes: { effort: 'low' } } } })).ok()).toBe(true)

      for (const entry of ['header', 'list'] as const) {
        if (entry === 'header') {
          await page.goto(`/sessions/${source}`)
          await page.getByRole('button', { name: 'Session actions', exact: true }).click()
        } else {
          expect((await context.request.post(`/api/sessions/${source}/close`, { headers })).ok()).toBe(true)
          await page.evaluate(() => localStorage.setItem('hennery.hideClosed', 'false'))
          await page.goto('/sessions')
          const row = page.locator('.sess-item').filter({ has: page.locator(`a[href="/sessions/${source}"]`) })
          await row.getByRole('button', { name: 'Session actions', exact: true }).click()
        }
        const action = page.getByRole('button', { name: 'New like this', exact: true })
        await expect(action).toBeVisible()
        await expect(action).toBeInViewport({ ratio: 1 })
        const artifact = join(process.cwd(), '../.superpowers/sdd/new-like-this/screenshots'); mkdirSync(artifact, { recursive: true })
        await page.screenshot({ path: join(artifact, `${entry}-menu-${width}-${appearance}.png`), animations: 'disabled' })
        const before = clones.length
        await action.click()
        const workspace = page.getByRole('dialog', { name: 'New like this', exact: true })
        await expect(workspace).toBeVisible()
        expect(clones).toHaveLength(before)
        const createWorktree = workspace.getByRole('checkbox', { name: 'Create a new worktree' })
        const isolated = entry === 'list'
        // Header: override the initial isolated default to share cwd. List:
        // verify the saved shared default and explicitly request isolation.
        await expect(createWorktree).toBeChecked({ checked: entry === 'header' })
        await createWorktree.setChecked(isolated)
        if (isolated) {
          await expect(workspace.getByLabel('Base branch or commit', { exact: true })).toHaveValue(base)
          await expect(workspace.getByText(`Resolved base commit: ${base}`, { exact: true })).toBeVisible()
        } else {
          await expect(workspace.getByText('Uses the existing directory, including its uncommitted changes.', { exact: true })).toBeVisible()
        }
        const submit = workspace.getByRole('button', { name: 'Start session', exact: true })
        await expect(submit).toBeEnabled()
        await expect(submit).toBeInViewport({ ratio: 1 })
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
        await page.screenshot({ path: join(artifact, `${entry}-workspace-${width}-${appearance}.png`), animations: 'disabled' })
        const response = page.waitForResponse(response => response.request().method() === 'POST' && response.url().endsWith('/new-like-this'))
        await submit.click()
        const created = await response
        expect(created.ok(), await created.text()).toBe(true)
        const id = (await created.json()).session_id
        await expect(page).toHaveURL(`${collector.origin}/sessions/${id}`, { timeout: 45_000 })
        expect(id).not.toBe(source)
        const prompt = page.getByRole('textbox', { name: 'Prompt', exact: true })
        await expect(prompt).toBeFocused(); await expect(prompt).toHaveValue('')
        const detail = await (await context.request.get(`/api/sessions/${id}`)).json()
        expect(detail).toMatchObject({ host_id: hostId, agent: 'claude', hat_id: sourceDetail.hat_id, lifecycle: 'active', model: 'sonnet', mode: 'bypass' })
        if (isolated) {
          expect(detail.cwd).not.toBe(cwd)
          expect(readFileSync(join(detail.cwd, 'base.txt'), 'utf8')).toBe('COMMITTED')
          const managed = await (await context.request.get(`/api/sessions/${id}/workspace`)).json()
          expect(managed).toMatchObject({ root: detail.cwd, base_commit: base, users: 1, state: 'ready' })
        } else {
          expect(detail.cwd).toBe(cwd)
          expect(readFileSync(join(detail.cwd, 'base.txt'), 'utf8')).toBe('SOURCE_LOCAL_CHANGES')
        }
        expect(readFileSync(join(cwd, 'base.txt'), 'utf8')).toBe('SOURCE_LOCAL_CHANGES')
        expect(detail.title).not.toBe('Source session')
        expect(detail.family?.parent).toBeFalsy()
        const catalog = await (await context.request.get(`/api/sessions/${id}/catalog`)).json()
        expect(catalog).toMatchObject({ model: 'sonnet', mode: 'bypass', axes: { effort: 'high', fast: false } })
        const view = await (await context.request.get(`/api/view/sessions/${id}`)).json()
        expect(view.items.filter((item: { kind: string }) => item.kind === 'user_turn')).toHaveLength(0)
        expect(prompts).toHaveLength(0)
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
        await page.screenshot({ path: join(artifact, `${entry}-session-${width}-${appearance}.png`), animations: 'disabled' })
      }
      expect(clones).toHaveLength(2)
    } finally { await context.close(); await host.stop(); await collector.stop() }
  })
}
