import { expect, test } from '@playwright/test'
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { startCollector } from './collector'
import { fakeAgent, testHost } from './host'

for (const width of [1280, 390]) test(`session files upload, explicit versions and offline downloads at ${width}px`, async ({ browser }) => {
  test.setTimeout(120_000)
  const scratch = mkdtempSync(join(tmpdir(), 'hennery-files-ui-'))
  const receipt = join(scratch, 'adapter.jsonl')
  const collector = await startCollector()
  const host = testHost(fakeAgent('claude', { chunks: ['Received the file.'], session_log: receipt, log_prompts: true }))
  const context = await browser.newContext({ baseURL: collector.origin, viewport: { width, height: 900 }, acceptDownloads: true })
  try {
    const page = await context.newPage(), origin = { Origin: collector.origin }
    const records = () => existsSync(receipt) ? readFileSync(receipt, 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line)) : []
    expect((await page.request.post('/api/setup', { headers: origin, data: { token: new URL(collector.setupLink).hash.slice(1), password: 'correct horse battery staple', public_url: collector.origin } })).status()).toBe(201)
    const pairing = await (await page.request.post('/api/hosts/pairing-codes', { headers: origin })).json()
    expect(await host.join(`hennery host join ${collector.origin} ${pairing.code}`, ['--name', 'Files workstation', '--no-runtime'])).toBe(0)
    host.run()
    let hostId = ''
    await expect(async () => { const rows = await (await page.request.get('/api/hosts')).json(); expect(rows[0].connected).toBe(true); hostId = rows[0].host_id }).toPass({ timeout: 30_000 })
    const project = realpathSync(host.dir)
    const start = await page.request.post('/api/sessions', { headers: origin, data: { host_id: hostId, agent: 'claude', cwd: project } })
    expect(start.status()).toBe(202)
    const session = (await start.json()).session_id
    await page.goto(`/sessions/${session}`)
    await page.getByTestId('image-input').setInputFiles({ name: 'data.csv', mimeType: 'text/csv', buffer: Buffer.from('name,value\na,42\n') })
    await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toHaveValue('[File #1] ')
    await page.getByRole('button', { name: 'Send', exact: true }).click()
    await expect(async () => expect(records().some(row => row.method === 'session/prompt')).toBe(true)).toPass()
    const sent = records().find(row => row.method === 'session/prompt').params.prompt
    const fileContext = sent.find((block: { text?: string }) => block.text?.startsWith('Attached file ')).text as string
    const localPath = JSON.parse(fileContext.match(/available at ("(?:\\.|[^"])*")\./)![1])
    expect(readFileSync(localPath, 'utf8')).toBe('name,value\na,42\n')
    await expect(page.getByRole('link', { name: 'data.csv · Download', exact: true })).toBeVisible()
    const details = page.getByRole('button', { name: 'Session details', exact: true })
    if (await details.getAttribute('aria-pressed') !== 'true') await details.click()
    if (width < 1024) await page.getByRole('tab', { name: 'Files', exact: true }).click()
    const files = page.getByRole('region', { name: 'Session files', exact: true })
    writeFileSync(join(project, 'report.md'), '# First report\n')
    await files.getByRole('textbox', { name: 'File path', exact: true }).fill('report.md')
    await files.getByRole('button', { name: 'Publish file', exact: true }).click()
    await expect(files.getByRole('link', { name: 'Download version 1', exact: true })).toBeVisible()
    const metadata = await (await page.request.get(`/api/sessions/${session}/files`)).json()
    const artifact = metadata.artifacts[0]
    const descriptor = records().find(row => row.method === 'session/new').params.mcpServers.find((s: { name: string }) => s.name === 'hennery_files')
    expect(descriptor).toBeTruthy()
    const token = descriptor.headers.find((h: { name: string }) => h.name.toLowerCase() === 'authorization').value
    writeFileSync(join(project, 'report.md'), '# Second report\n')
    const updated = await page.request.post(`${collector.origin}/mcp/builtin/files`, { headers: { Authorization: token, Accept: 'application/json' }, data: { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'artifact_publish', arguments: { path: 'report.md', request_key: 'version-two', artifact_id: artifact.artifact_id, expected_revision: 1 } } } })
    expect(updated.status()).toBe(200)
    const result = await updated.json()
    expect(result.result.isError).toBe(false)
    expect(result.result.structuredContent.revision).toBe(2)
    await expect(files.getByRole('link', { name: 'Download version 2', exact: true })).toBeVisible()
    await files.getByRole('button', { name: 'Preview', exact: true }).click()
    await expect(files.getByRole('heading', { name: 'Second report', exact: true })).toBeVisible()
    await host.crash()
    writeFileSync(join(project, 'report.md'), 'Changed after publication')
    await files.getByRole('button', { name: 'Version history', exact: true }).click()
    for (const [version, text] of [[1, '# First report\n'], [2, '# Second report\n']] as const) {
      const download = page.waitForEvent('download')
      await files.getByRole('link', { name: `Version ${version}`, exact: true }).click()
      const saved = await download
      expect(saved.suggestedFilename()).toBe('report.md')
      expect(readFileSync((await saved.path())!, 'utf8')).toBe(text)
    }
    await page.screenshot({ path: test.info().outputPath(`files-${width}.png`), fullPage: true })
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  } finally {
    await context.close(); await host.stop(); await collector.stop(); rmSync(scratch, { recursive: true, force: true })
  }
})
