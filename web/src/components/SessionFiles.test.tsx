import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ClientContext, browserClient } from '../app-client'
import SessionFiles, { ArtifactCard, ArtifactVersions, artifactHref } from './SessionFiles'
import type { ArtifactRevision } from '../generated/protocol'

const a: ArtifactRevision = { artifact_id: 'artifact', session_id: 'session', revision: 1, title: 'Analysis', name: 'report.md', mime_type: 'text/markdown', sha256: 'a'.repeat(64), size: 10, created_at: '2026-11-18T01:00:00Z' }
const second = { ...a, revision: 2 }
function json(body: unknown) { return new Response(JSON.stringify(body), { headers: { 'Content-Type': 'application/json' } }) }
function page(artifacts = [second]) { return { artifacts, next_cursor: null, attachments: [{ type: 'file', name: 'data.csv', mimeType: 'text/csv', sha256: 'b'.repeat(64), size: 4 }], attachments_truncated: false } }
function show(fetch: typeof globalThis.fetch) {
  const client = browserClient(async () => {}, fetch)
  return render(<ClientContext.Provider value={client}><SessionFiles sessionId="session" /></ClientContext.Provider>)
}

describe('session files', () => {
  it('pins a transcript download to its immutable revision and indicates a newer result', () => {
    render(<ArtifactVersions.Provider value={new Map([['artifact', 2]])}><ArtifactCard artifact={a} /></ArtifactVersions.Provider>)
    expect(screen.getByRole('link', { name: 'Download version 1' })).toHaveAttribute('href', artifactHref(a))
    expect(screen.getByText('Version 2 is available in Files.', { exact: true })).toBeVisible()
  })
  it('shows one latest artifact, historical downloads and the original input filename', async () => {
    const fetch = vi.fn<typeof globalThis.fetch>(async input => String(input).endsWith('/files') ? json(page()) : json({ revisions: [second, a], next_before: null }))
    show(fetch)
    expect(await screen.findByRole('link', { name: 'Download version 2' })).toHaveAttribute('href', artifactHref(second))
    expect(screen.getByRole('link', { name: /data.csv/ })).toHaveAttribute('download', 'data.csv')
    await userEvent.click(screen.getByRole('button', { name: 'Version history' }))
    expect(await screen.findByRole('link', { name: 'Version 1' })).toHaveAttribute('href', artifactHref(a))
  })
  it('updates the selected artifact with its observed revision', async () => {
    const posts: unknown[] = []
    const fetch = vi.fn<typeof globalThis.fetch>(async (_input, init) => {
      if (init?.method === 'POST') { posts.push(JSON.parse(String(init.body))); return json({ ...a, revision: 3 }) }
      return json(page())
    })
    show(fetch)
    await screen.findByRole('link', { name: 'Download version 2' })
    const user = userEvent.setup()
    await user.type(screen.getByRole('textbox', { name: 'File path' }), 'reports/report.md')
    await user.selectOptions(screen.getByRole('combobox', { name: 'Publish as' }), 'artifact')
    await user.click(screen.getByRole('button', { name: 'Publish new version' }))
    await waitFor(() => expect(posts).toHaveLength(1))
    expect(posts[0]).toMatchObject({ path: 'reports/report.md', artifact_id: 'artifact', expected_revision: 2, title: 'Analysis' })
  })
  it('reuses the exact publication request after an uncertain network result', async () => {
    const posts: unknown[] = []
    const fetch = vi.fn<typeof globalThis.fetch>(async (_input, init) => {
      if (init?.method === 'POST') { posts.push(JSON.parse(String(init.body))); if (posts.length === 1) throw new TypeError('connection lost'); return json(a) }
      return json(page([]))
    })
    show(fetch)
    await screen.findByText('No files have been published.', { exact: true })
    const user = userEvent.setup()
    await user.type(screen.getByRole('textbox', { name: 'File path' }), 'report.md')
    await user.click(screen.getByRole('button', { name: 'Publish file' }))
    await user.click(await screen.findByRole('button', { name: 'Retry publication' }))
    await waitFor(() => expect(posts).toHaveLength(2))
    expect(posts[0]).toEqual(posts[1])
  })
})
