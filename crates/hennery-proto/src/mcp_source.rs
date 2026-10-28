//! Per-hat MCP source permission, separate from managed mount delivery.
use crate::frames::{Capabilities, Capability, McpIsolation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
