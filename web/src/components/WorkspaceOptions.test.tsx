import { useState } from 'react'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it } from 'vitest'
import { routed, json } from '../test-stream'
import WorkspaceOptionsEditor, { type WorkspaceOptions } from './WorkspaceOptions'

it('loads an explicit project profile, resolves the base and saves the selected preparation', async () => {
  const commit = 'a'.repeat(40)
  const server = routed(call => {
    if (call.method === 'GET') return json({ files: ['AGENTS.md'], command: 'npm ci' })
    if (call.path === '/api/workspace-preview') return json({ base_commit: commit, branch: 'main' })
    return json({ saved: true })
  })
  function Form() {
    const [value, setValue] = useState<WorkspaceOptions | null>({ base: 'main', files: [] })
    return <WorkspaceOptionsEditor source={{ host_id: 'host', cwd: '/repo' }} value={value} onChange={setValue} />
  }
  render(<Form />, { wrapper: server.wrapper })
  await waitFor(() => expect(screen.getByLabelText('Preparation files (one relative path per line)')).toHaveValue('AGENTS.md'))
  await waitFor(() => expect(screen.getByLabelText('Base branch or commit')).toHaveValue(commit))
  expect(screen.getByLabelText('Preparation command (optional)')).toHaveValue('npm ci')
  fireEvent.click(screen.getByRole('button', { name: 'Save preparation profile' }))
  await screen.findByText('Preparation profile saved for this project.')
  expect(JSON.parse(String(server.fetch.mock.calls.find(call => call[1]?.method === 'PUT')?.[1]?.body))).toEqual({ files: ['AGENTS.md'], command: 'npm ci' })
})
