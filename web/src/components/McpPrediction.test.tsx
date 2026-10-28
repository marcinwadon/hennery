import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { host, hat } from '../test-fixtures'
import McpPrediction from './McpPrediction'
import { predictionHat } from '../lib/mcpDelivery'

describe('delivery prediction', () => {
  it('uses the correct ACP override when a hat mixes Claude-only and Codex defaults', () => {
    const h = host({ capabilities: ['mcp_servers'], mcp_delivery: { codex: 'default_hat_only', claude: 'isolated' }, mcp_source_policies: { claude: ['managed_only', 'managed_plus_native'] } })
    const selected = hat({ id: 'hat-a', mcp_source_overrides: { claude: { policy: 'managed_only', revision: 2 }, codex: { policy: 'legacy', revision: 1 } } })
    const view = render(<McpPrediction host={h} agent="claude" hatId="hat-a" hat={selected} />)
    expect(screen.getByRole('region', { name: 'MCP prediction' })).toHaveTextContent('Only MCP configured in Hennery')
    view.rerender(<McpPrediction host={h} agent="codex" hatId="hat-a" hat={selected} />)
    expect(screen.getByRole('region', { name: 'MCP prediction' })).toHaveTextContent('Codex also loads your own ~/.codex MCP servers.')
    expect(screen.getByRole('region', { name: 'MCP prediction' })).not.toHaveTextContent('Starting will be refused')
  })
  const h = host({ capabilities: ['mcp_servers'], mcp_delivery: { codex: 'default_hat_only', claude: 'isolated' } })
  it('shows default-hat Codex as not isolated, including its own configuration', () => {
    render(<McpPrediction host={h} agent="codex" hatId="hat-a" />)
    const note = screen.getByRole('region', { name: 'MCP prediction' })
    expect(note).toHaveTextContent('Prediction from the host’s last report: The MCP servers are not isolated per hat')
    expect(note).toHaveTextContent('Codex also loads your own ~/.codex MCP servers.')
  })
  it('shows fallback in another hat', () => {
    render(<McpPrediction host={h} agent="codex" hatId="hat-b" />)
    expect(screen.getByRole('region', { name: 'MCP prediction' })).toHaveTextContent('this hat is not the host’s default')
    expect(screen.getByRole('region', { name: 'MCP prediction' })).not.toHaveTextContent('~/.codex')
  })
  it('keeps a missing report unknown', () => {
    render(<McpPrediction host={{ ...h, mcp_delivery: undefined }} agent="codex" hatId="hat-a" />)
    expect(screen.getByRole('region', { name: 'MCP prediction' })).toHaveTextContent('MCP delivery is unknown.')
    expect(screen.getByRole('region', { name: 'MCP prediction' })).toHaveTextContent('Codex may also load your own ~/.codex MCP servers.')
  })
  it('does not show a Codex note for Claude', () => {
    render(<McpPrediction host={h} agent="claude" hatId="hat-a" />)
    expect(screen.getByRole('region', { name: 'MCP prediction' })).toHaveTextContent("This agent was given only this hat's MCP servers.")
    expect(screen.getByRole('region', { name: 'MCP prediction' })).not.toHaveTextContent('Codex also loads your own ~/.codex MCP servers.')
  })
  it('uses only a resolution for the current host and path', () => {
    const resolved = { hostId: 'h1', path: '/project', hat: 'hat-a' }
    expect(predictionHat(resolved, 'h1', '/project')).toBe('hat-a')
    expect(predictionHat(resolved, 'h2', '/project')).toBeUndefined()
    expect(predictionHat(resolved, 'h1', '/different')).toBeUndefined()
    expect(predictionHat(undefined, 'h1', '/project')).toBeUndefined()
  })
})

it.each([
  ['isolated', host({ capabilities: ['mcp_servers'], mcp_delivery: { codex: 'isolated' } })],
  ['unsupported', host()],
])('omits the own-config warning for %s Codex', (_mode, h) => {
  render(<McpPrediction host={h} agent="codex" hatId="hat-a" />)
  expect(screen.getByRole('region', { name: 'MCP prediction' })).not.toHaveTextContent('~/.codex')
})
