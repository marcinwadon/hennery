import { expect, test, type Page } from '@playwright/test'
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, unlinkSync } from 'node:fs'
import { homedir, tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { startCollector } from './collector'
import { defaultHostDir, FAKE_ACP, testHost } from './host'
import { BIN, spawnHennery, type ChildProcess } from './spawn'

const PASSWORD = 'correct horse battery staple'
test.use({ actionTimeout: 15_000 })
let harness: string, compilerScratch: string

async function end(child: ChildProcess | undefined) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return
  const exited = new Promise<void>(done => child.once('exit', () => done()))
  child.kill('SIGTERM')
  const bound = setTimeout(() => child.kill('SIGKILL'), 5000)
  await exited; clearTimeout(bound)
}

test.beforeAll(async () => {
  test.setTimeout(180_000)
  compilerScratch = mkdtempSync(join(tmpdir(), 'hennery-source-policy-compiler-'))
  const cargoHome = join(dirname(BIN), '..', 'source-policy-compiler-home')
  const cache = process.env.CARGO_HOME ?? join(homedir(), '.cargo')
  mkdirSync(cargoHome, { recursive: true })
  for (const part of ['registry', 'git']) if (existsSync(join(cache, part)) && !existsSync(join(cargoHome, part))) {
    const target = join(cargoHome, part)
    if (lstatSync(target, { throwIfNoEntry: false })?.isSymbolicLink()) unlinkSync(target)
    symlinkSync(join(cache, part), target, 'dir')
  }
  const toolchain = Object.fromEntries(Object.entries(process.env).filter(([key]) => /^(?:NIX_|SDKROOT$|CC$|CXX$|AR$|PKG_CONFIG_PATH$|PKG_CONFIG_LIBDIR$|OPENSSL_DIR$|LIBCLANG_PATH$|RUSTFLAGS$|CARGO_ENCODED_RUSTFLAGS$)/.test(key)))
  const build = spawnHennery([resolve('e2e/build-delegation-host.sh')], compilerScratch, { ...toolchain, CARGO_HOME: cargoHome, CARGO_BUILD_JOBS: '2' }, ['ignore', 'ignore', 'pipe'], '/bin/sh')
  let output = ''; build.stderr?.on('data', chunk => { output += chunk })
  const code = await new Promise<number | null>((done, fail) => { build.once('error', fail); build.once('exit', done) })
  expect(code, output).toBe(0)
  harness = join(dirname(BIN), '..', 'hennery-delegation-host')
})
test.afterAll(() => { if (compilerScratch) rmSync(compilerScratch, { recursive: true, force: true }) })

async function report(page: Page, width: number) {
  const toggle = page.getByRole('button', { name: 'Session details', exact: true })
  if (await toggle.getAttribute('aria-pressed') !== 'true') await toggle.click()
  if (width === 390) await page.getByRole('tab', { name: 'Details', exact: true }).click()
  return page.getByRole('region', { name: 'MCP sources', exact: true })
}

for (const width of [1280, 390]) test(`MCP source setting and confirmed attach stay distinct at ${width}px`, async ({ browser }, testInfo) => {
  test.setTimeout(180_000)
  const scratch = mkdtempSync(join(tmpdir(), 'hennery-source-policy-ui-'))
  const collector = await startCollector(), host = testHost()
  const context = await browser.newContext({ baseURL: collector.origin, viewport: { width, height: 900 } })
  let runner: ChildProcess | undefined
  try {
    const page = await context.newPage(), headers = { Origin: collector.origin }
    expect((await page.request.post('/api/setup', { headers, data: { token: new URL(collector.setupLink).hash.slice(1), password: PASSWORD, public_url: collector.origin } })).status()).toBe(201)
    const minted = await (await page.request.post('/api/hosts/pairing-codes', { headers })).json()
    expect(await host.join(`hennery host join ${collector.origin} ${minted.code}`, ['--name', 'Policy fixture', '--no-runtime'])).toBe(0)
    const receipt = join(scratch, 'sessions.jsonl'), project = realpathSync(host.dir)
    runner = spawnHennery([defaultHostDir(host.dir), project, FAKE_ACP, JSON.stringify({ chunks: ['Completed.'], session_log: receipt })], host.dir, {}, undefined, harness)
    let hostId = '', hatId = ''
    await expect(async () => {
      const rows = await (await page.request.get('/api/hosts')).json()
      expect(rows[0].connected).toBe(true)
      expect(rows[0].mcp_source_policies.claude).toContain('managed_only')
      hostId = rows[0].host_id; hatId = rows[0].default_hat_id
    }).toPass({ timeout: 30_000 })
    await page.goto('/hats')
    const personal = page.getByRole('listitem', { name: 'Personal', exact: true })
    await expect(personal).toContainText('Existing / adapter defaults')
    await personal.getByRole('button', { name: 'Edit', exact: true }).click()
    await personal.getByRole('combobox', { name: 'Claude MCP sources', exact: true }).selectOption('managed_only')
    await expect(personal.getByRole('combobox', { name: 'Codex MCP sources', exact: true })).toHaveValue('legacy')
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
    await page.screenshot({ path: testInfo.outputPath('per-acp-hat-form.png'), fullPage: true })
    await personal.getByRole('button', { name: 'Save', exact: true }).click()
    await expect(personal).toContainText('Only MCP configured in Hennery')
    const created = await page.request.post('/api/sessions', { headers, data: { host_id: hostId, agent: 'claude', cwd: project } })
    expect(created.status()).toBe(202)
    const { session_id: id } = await created.json()
    const codex = await page.request.post('/api/sessions', { headers, data: { host_id: hostId, agent: 'codex', cwd: project } })
    expect(codex.status()).toBe(202)
    const codexId = (await codex.json()).session_id
    await expect(async () => {
      const state = (await (await page.request.get(`/api/sessions/${codexId}`)).json()).mcp_source_policy
      expect(state.policy).toBe('legacy')
      expect(state.revision).toBe(0)
    }).toPass()
    await page.goto(`/sessions/${id}`)
    await expect(await report(page, width)).toContainText('Last confirmed attach: Only MCP configured in Hennery')

    await page.goto('/hats')
    await personal.getByRole('button', { name: 'Edit', exact: true }).click()
    await personal.getByRole('combobox', { name: 'Claude MCP sources', exact: true }).selectOption('managed_plus_native')
    await personal.getByRole('button', { name: 'Save', exact: true }).click()
    await expect(personal).toContainText('Hennery and runtime-native MCP')
    await page.goto(`/sessions/${id}`)
    const pending = await report(page, width)
    await expect(pending).toContainText('Hat setting: Hennery and runtime-native MCP')
    await expect(pending).toContainText('Last confirmed attach: Only MCP configured in Hennery')
    await expect(pending).toContainText('next start or resume')
    await page.goto(`/sessions/${codexId}`)
    const codexReport = await report(page, width)
    await expect(codexReport).toContainText('Hat setting: Existing / adapter defaults')
    await expect(codexReport).not.toContainText('next start or resume')
    await page.goto(`/sessions/${id}`)
    await report(page, width)
    expect((await page.request.post(`/api/sessions/${id}/park`, { headers })).status()).toBe(202)
    expect((await page.request.post(`/api/sessions/${id}/resume`, { headers })).status()).toBe(202)
    await expect(pending).toContainText('Last confirmed attach: Hennery and runtime-native MCP')
    const detail = await (await page.request.get(`/api/sessions/${id}`)).json()
    expect(detail.mcp_source_policy.applied).toEqual(detail.mcp_source_policy.requested)
    expect(detail.mcp_delivery.mode).toBe('managed_plus_native')
    expect(detail.mcp_delivery.servers).toBe(0)

    // Same host, another hat, different source permission while the first runs.
    await page.goto('/hats')
    await page.getByRole('button', { name: '+ New hat', exact: true }).click()
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill('Restricted')
    await expect(page.getByRole('combobox', { name: 'Claude MCP sources', exact: true })).toHaveValue('managed_only')
    await expect(page.getByRole('combobox', { name: 'Codex MCP sources', exact: true })).toHaveValue('legacy')
    // Build an equal-revision destination through public API transitions below.
    if (width === 390) await page.getByRole('combobox', { name: 'Claude MCP sources', exact: true }).selectOption('legacy')
    await page.getByRole('button', { name: 'Create', exact: true }).click()
    const hats = await (await page.request.get('/api/hats')).json()
    let restricted = hats.find((hat: { name: string }) => hat.name === 'Restricted')
    expect(restricted.mcp_source_policy).toBe('legacy')
    expect(restricted.mcp_source_overrides.claude.policy).toBe(width === 390 ? 'legacy' : 'managed_only')
    expect(restricted.mcp_source_overrides.codex).toBeUndefined()
    if (width === 390) {
      const changed = await page.request.patch(`/api/hats/${restricted.id}`, { headers, data: { mcp_source_overrides: { claude: 'managed_only' } } })
      expect(changed.status()).toBe(200)
      restricted = await changed.json()
    }
    const other = join(project, 'restricted'); mkdirSync(other)
    expect((await page.request.put(`/api/hosts/${hostId}/path-rules`, { headers, data: { rules: [{ prefix: other, hat_id: restricted.id }] } })).status()).toBe(200)
    const second = await page.request.post('/api/sessions', { headers, data: { host_id: hostId, agent: 'claude', cwd: other } })
    expect(second.status()).toBe(202)
    const secondId = (await second.json()).session_id
    expect((await (await page.request.get(`/api/sessions/${secondId}`)).json()).mcp_source_policy.applied.policy).toBe('managed_only')
    const otherCodex = await page.request.post('/api/sessions', { headers, data: { host_id: hostId, agent: 'codex', cwd: other } })
    expect(otherCodex.status()).toBe(202)
    const otherCodexId = (await otherCodex.json()).session_id
    expect((await (await page.request.get(`/api/sessions/${otherCodexId}`)).json()).mcp_source_policy.policy).toBe('legacy')
    // Unqualified adapters inherit legacy until explicitly configured otherwise.
    expect((await page.request.patch(`/api/hats/${restricted.id}`, { headers, data: { mcp_source_overrides: { custom: 'managed_only' } } })).status()).toBe(200)
    const refused = await page.request.post('/api/sessions', { headers, data: { host_id: hostId, agent: 'custom', cwd: other } })
    expect(refused.status()).toBe(409)
    expect((await refused.json()).code).toBe('mcp_source_policy_unsupported')
    const records = readFileSync(receipt, 'utf8').trim().split('\n').map(line => JSON.parse(line))
    expect(records.some(row => row.method === 'session/load' && row.params._meta?.claudeCode.options.strictMcpConfig === false)).toBe(true)
    expect(hatId).not.toBe(restricted.id)

    // Revisions are local to each hat/ACP: exercise both a lower and an equal
    // destination revision without navigating away from the selected session.
    const destination = (await (await page.request.get('/api/hats')).json()).find((hat: { id: string }) => hat.id === restricted.id)
    expect(width === 390 ? destination.mcp_source_overrides.claude.revision === detail.mcp_source_policy.revision
      : destination.mcp_source_overrides.claude.revision < detail.mcp_source_policy.revision).toBe(true)
    await page.goto(`/sessions/${id}`)
    const reassigned = await report(page, width)
    await expect(reassigned).toContainText('Hat setting: Hennery and runtime-native MCP')
    expect((await page.request.post(`/api/sessions/${id}/park`, { headers })).status()).toBe(202)
    await expect(async () => {
      expect((await (await page.request.get(`/api/sessions/${id}`)).json()).lifecycle).toBe('parked')
    }).toPass()
    const changed = await page.request.patch(`/api/sessions/${id}`, { headers, data: { hat_id: restricted.id } })
    expect(changed.status()).toBe(200)
    await expect(reassigned).toContainText('Hat setting: Only MCP configured in Hennery')
    await expect(reassigned).toContainText('Last confirmed attach: Hennery and runtime-native MCP')
    await expect(reassigned).toContainText('next start or resume')
  } finally {
    await Promise.allSettled([end(runner), host.stop(), collector.stop(), context.close()])
    rmSync(scratch, { recursive: true, force: true })
  }
})
