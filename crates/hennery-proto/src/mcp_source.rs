//! Per-hat, per-ACP MCP source permission, separate from managed mount delivery.
use crate::frames::{Capabilities, Capability, McpIsolation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

/// Sparse per-ACP setting. A reset retains its revision with a null policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct McpSourceOverride {
    pub policy: Option<McpSourcePolicy>,
    #[ts(type = "number")]
    pub revision: u64,
}

pub fn source_policy_for(
    fallback: McpSourcePolicy,
    fallback_revision: u64,
    overrides: &BTreeMap<String, McpSourceOverride>,
    agent: &str,
) -> (McpSourcePolicy, u64) {
    match overrides.get(agent) {
        Some(setting) => match setting.policy {
            Some(policy) => (policy, setting.revision),
            None => (fallback, setting.revision.max(fallback_revision)),
        },
        None => (fallback, fallback_revision),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpSourcePolicy {
    #[default]
    Legacy,
    ManagedOnly,
    ManagedPlusNative,
}

impl McpSourcePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::ManagedOnly => "managed_only",
            Self::ManagedPlusNative => "managed_plus_native",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Legacy, Self::ManagedOnly, Self::ManagedPlusNative]
            .into_iter()
            .find(|policy| policy.as_str() == value)
    }

    /// This version qualifies only the bundled Claude mechanism. Unknown and
    /// own-CLI profiles never acquire support merely from their agent name.
    pub fn supported(self, capabilities: &Capabilities, isolation: McpIsolation) -> bool {
        self == Self::Legacy
            || (capabilities.has(Capability::McpServers)
                && capabilities.has(Capability::McpSourcePolicy)
                && isolation == McpIsolation::ClaudeStrict
                && (self != Self::ManagedPlusNative || capabilities.has(Capability::McpNativeSources)))
    }
}

/// Echoed only after new/load used this snapshot. Request identity is the
/// existing start request, not a second lifecycle or proof namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct McpSourceSnapshot {
    pub policy: McpSourcePolicy,
    #[ts(type = "number")]
    pub revision: u64,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct McpSourceState {
    pub policy: McpSourcePolicy,
    #[ts(type = "number")]
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub requested: Option<McpSourceSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub applied: Option<McpSourceSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn per_agent_source_patch_distinguishes_omit_reset_and_existing_behavior() {
        use crate::rest::UpdateHatRequest;
        let omitted: UpdateHatRequest = serde_json::from_str("{}").unwrap();
        assert!(omitted.mcp_source_overrides.is_none());
        let patch: UpdateHatRequest = serde_json::from_str(
            r#"{"mcp_source_overrides":{"claude":"managed_only","codex":"legacy","custom":null}}"#,
        )
        .unwrap();
        let patch = patch.mcp_source_overrides.unwrap();
        assert_eq!(patch["claude"], Some(McpSourcePolicy::ManagedOnly));
        assert_eq!(patch["codex"], Some(McpSourcePolicy::Legacy));
        assert_eq!(patch["custom"], None);
        assert!(serde_json::from_str::<UpdateHatRequest>(r#"{"mcp_source_overrides":{"claude":"unknown"}}"#).is_err());
    }
}
