import type { HatItem, McpSourcePolicy, McpSourceState } from '../generated/protocol'

/** Revisions belong to the actual hat/ACP pair, never the model or executable. */
export function sourcePolicyFor(hat: HatItem | undefined, agent: string): Pick<McpSourceState, 'policy' | 'revision'> {
  const fallback = hat?.mcp_source_policy ?? 'legacy'
  const revision = hat?.mcp_policy_revision ?? 0
  const override = hat?.mcp_source_overrides?.[agent]
  return override?.policy != null
    ? { policy: override.policy, revision: override.revision }
    : { policy: fallback, revision: Math.max(revision, override?.revision ?? 0) }
}

export type SourcePolicyPatch = Record<string, McpSourcePolicy | null>
