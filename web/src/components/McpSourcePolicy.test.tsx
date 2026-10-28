import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import McpSourcePolicy, { SourcePolicyPrediction } from './McpSourcePolicy'

it('keeps an active applied setting distinct from the changed hat policy', () => {
  render(<McpSourcePolicy state={{ policy: 'managed_only', revision: 3,
    requested: { policy: 'managed_plus_native', revision: 2, request_id: 'old' },
    applied: { policy: 'managed_plus_native', revision: 2, request_id: 'old' } }} />)
  const section=screen.getByRole('region',{name:'MCP sources'})
  expect(section).toHaveTextContent('Hat setting: Only MCP configured in Hennery')
  expect(section).toHaveTextContent('Last confirmed attach: Hennery and runtime-native MCP')
  expect(section).toHaveTextContent('next start or resume')
  expect(section).not.toHaveTextContent('Awaiting host confirmation')
})

it('does not label a requested attach as confirmed', () => {
  render(<McpSourcePolicy state={{policy:'managed_only',revision:1,
    requested:{policy:'managed_only',revision:1,request_id:'new'}}} />)
  expect(screen.getByRole('region',{name:'MCP sources'})).toHaveTextContent('Last confirmed attach: Not reported')
  expect(screen.getByText(/Awaiting host confirmation/)).toBeVisible()
})

it('predicts an unsupported explicit mode without silently selecting another runtime', () => {
  render(<SourcePolicyPrediction policy="managed_only" agent="codex" />)
  expect(screen.getByRole('status')).toHaveTextContent('Starting will be refused')
})
