import { useState } from 'react'
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it } from 'vitest'
import { hat, host } from '../test-fixtures'
import type { SourcePolicyPatch } from '../lib/mcpSourcePolicy'
import AgentSourcePolicies from './AgentSourcePolicies'

const hosts = [host({ mcp_source_policies: { claude: ['managed_only', 'managed_plus_native'] } })]

it('treats a valid adapter ID matching an object property as adapter defaults', () => {
  render(<AgentSourcePolicies hosts={[host({ mcp_delivery: { constructor: 'default_hat_only' as const } })]} patch={{}} onChange={() => {}} />)
  const adapter = screen.getByRole('combobox', { name: 'constructor MCP sources' })
  expect(adapter).toHaveValue('legacy')
  expect(within(adapter).getAllByRole('option')).toHaveLength(1)
  expect(within(screen.getByRole('group', { name: 'constructor MCP sources' })).queryByRole('status')).toBeNull()
})

it('keeps adapter defaults selectable and only offers qualified explicit choices for each ACP', async () => {
  function Form() {
    const [patch, setPatch] = useState<SourcePolicyPatch>({ claude: 'managed_only' })
    return <AgentSourcePolicies hosts={hosts} patch={patch} onChange={setPatch} />
  }
  render(<Form />)
  const claude = screen.getByRole('combobox', { name: 'Claude MCP sources' })
  const codex = screen.getByRole('combobox', { name: 'Codex MCP sources' })
  expect(claude).toHaveValue('managed_only')
  expect(codex).toHaveValue('legacy')
  expect(within(codex).getAllByRole('option')).toHaveLength(1)
  await userEvent.selectOptions(claude, 'managed_plus_native')
  await userEvent.selectOptions(claude, 'legacy')
  expect(claude).toHaveValue('legacy')
  expect(codex).toHaveValue('legacy')
})

it('resets one ACP to hat defaults without changing another ACP draft', async () => {
  const h = hat({ mcp_source_overrides: { claude: { policy: 'managed_only', revision: 1 }, codex: { policy: 'legacy', revision: 1 } } })
  function Form() {
    const [patch, setPatch] = useState<SourcePolicyPatch>({})
    return <><AgentSourcePolicies hat={h} hosts={hosts} patch={patch} onChange={setPatch} /><output>{JSON.stringify(patch)}</output></>
  }
  render(<Form />)
  await userEvent.click(screen.getByRole('button', { name: 'Reset Claude MCP sources' }))
  expect(screen.getByRole('combobox', { name: 'Claude MCP sources' })).toHaveValue('legacy')
  expect(screen.getByRole('combobox', { name: 'Codex MCP sources' })).toHaveValue('legacy')
  expect(screen.getByRole('status')).toHaveTextContent('{"claude":null}')
})
