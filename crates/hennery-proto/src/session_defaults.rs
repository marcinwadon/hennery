//! Sparse operator launcher preferences, independent of delegation grants.
use crate::frames::{ConfigValue, SessionConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct HatSessionDefaults {
    pub hat_id: String,
    pub agent: String,
    #[ts(type = "number")]
    pub revision: i64,
    #[serde(deserialize_with = "strict_config")]
    pub config: SessionConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct PutHatSessionDefaults {
    #[ts(type = "number")]
    pub revision: i64,
    #[serde(deserialize_with = "strict_config")]
    pub config: SessionConfig,
}

fn strict_config<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<SessionConfig, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Strict {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        axes: BTreeMap<String, ConfigValue>,
    }
    let value = Strict::deserialize(deserializer)?;
    Ok(SessionConfig {
        cache_retention: Default::default(),
        model: value.model,
        mode: value.mode,
        axes: value.axes,
    })
}
