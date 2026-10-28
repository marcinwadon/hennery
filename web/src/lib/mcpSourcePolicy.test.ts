import { expect, it } from 'vitest'
import { hat } from '../test-fixtures'
import { sourcePolicyFor } from './mcpSourcePolicy'

it('selects each actual ACP independently in the same hat', () => {
  const h = hat({ mcp_source_overrides: {
    claude: { policy: 'managed_only', revision: 9 }, codex: { policy: 'legacy', revision: 1 },
  } })
  expect(sourcePolicyFor(h, 'claude')).toEqual({ policy: 'managed_only', revision: 9 })
  expect(sourcePolicyFor(h, 'codex')).toEqual({ policy: 'legacy', revision: 1 })
  expect(sourcePolicyFor(h, 'claude-model-name')).toEqual({ policy: 'legacy', revision: 0 })
  const changed = { ...h, mcp_source_overrides: { ...h.mcp_source_overrides, claude: { policy: 'managed_plus_native' as const, revision: 10 } } }
  expect(sourcePolicyFor(changed, 'codex')).toEqual(sourcePolicyFor(h, 'codex'))
})

it('preserves old scalar fallback and a monotonic reset revision', () => {
  const h = hat({ mcp_source_policy: 'managed_only', mcp_policy_revision: 7,
    mcp_source_overrides: { codex: { policy: null, revision: 9 }, claude: { policy: 'managed_plus_native', revision: 2 } } })
  expect(sourcePolicyFor(h, 'codex')).toEqual({ policy: 'managed_only', revision: 9 })
  expect(sourcePolicyFor({ ...h, mcp_source_policy: 'legacy', mcp_policy_revision: 10 }, 'codex'))
    .toEqual({ policy: 'legacy', revision: 10 })
  expect(sourcePolicyFor(h, 'claude')).toEqual({ policy: 'managed_plus_native', revision: 2 })
  expect(sourcePolicyFor(h, 'other')).toEqual({ policy: 'managed_only', revision: 7 })
})
