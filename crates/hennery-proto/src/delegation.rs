//! D1 policy and family metadata. Delegated inputs and plug-in context use
//! `plugin::{InputOrigin, Provenance, PluginContext}`; no parallel types.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct HatSessionTools {
    pub delegation_enabled: bool,
    #[ts(type = "number")]
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SetSessionTools {
    pub delegation_enabled: bool,
}

// Derived struct decoding can accept positional JSON arrays. Authorization
// requests require the schema's object shape, with one exact, nonduplicate key.
impl<'de> Deserialize<'de> for SetSessionTools {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Policy;
        impl<'de> serde::de::Visitor<'de> for Policy {
            type Value = SetSessionTools;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a session-tools policy object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut enabled = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key != "delegation_enabled" {
                        return Err(serde::de::Error::unknown_field(&key, &["delegation_enabled"]));
                    }
                    if enabled.is_some() {
                        return Err(serde::de::Error::duplicate_field("delegation_enabled"));
                    }
                    enabled = Some(map.next_value::<bool>()?);
                }
                Ok(SetSessionTools {
                    delegation_enabled: enabled.ok_or_else(|| serde::de::Error::missing_field("delegation_enabled"))?,
                })
            }
        }
        deserializer.deserialize_map(Policy)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SessionFamily {
    pub child_id: String,
    pub parent_id: String,
    pub root_id: String,
    pub hat_id: String,
    #[ts(type = "number")]
    pub created_event_id: i64,
    pub detached: bool,
    pub reason: Option<String>,
}

// Derived struct decoding accepts positional sequences. Every tool argument is
// map-only, so its JSON Schema and actual parser agree at authorization edges.
macro_rules! tool_args {
    ($(#[$meta:meta])* pub struct $name:ident { $($(#[$field_meta:meta])* pub $field:ident: $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize, JsonSchema, TS)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $($(#[$field_meta])* pub $field: $ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                #[derive(Deserialize, JsonSchema, TS)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($(#[$field_meta])* $field: $ty),* }
                struct Object;
                impl<'de> serde::de::Visitor<'de> for Object {
                    type Value = $name;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(concat!(stringify!($name), " object"))
                    }
                    fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                        let fields = Fields::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                        Ok($name { $($field: fields.$field),* })
                    }
                }
                deserializer.deserialize_map(Object)
            }
        }
    };
}

tool_args! {
    /// Caller-host capabilities, scoped to the caller's hat and validated cwd.
    pub struct CapabilitiesArgs {
        pub agent: Option<String>,
        pub cwd: Option<String>,
    }
}
tool_args! {
    /// Start one independently configured direct child, without provider fallback.
    pub struct SpawnArgs {
        pub operation_key: String,
        pub cwd: String,
        pub brief: String,
        pub name: Option<String>,
        pub agent: Option<String>,
        #[serde(deserialize_with = "strict_config")]
        #[schemars(with = "ConfigFields")]
        pub config: crate::frames::SessionConfig,
    }
}
tool_args! {
    /// Queue an ordinary follow-up for an owned direct child.
    pub struct SendArgs {
        pub operation_key: String,
        pub child_id: String,
        pub text: String,
    }
}
tool_args! {
    /// Answer ordinary elicitation only. Permission requests raised by a child
    /// go to the operator; the parent cannot answer them.
    pub struct AnswerArgs {
        pub operation_key: String,
        pub child_id: String,
        pub pending_id: String,
        pub action: crate::frames::ElicitationAction,
        /// For accept, an object matching the child's requestedSchema. Omit for decline/cancel.
        #[schemars(extend("type" = "object"))]
        #[ts(type = "unknown | null")]
        pub content: Option<serde_json::Value>,
    }
}
tool_args! {
    /// Read a direct child's state, optionally selecting one operation receipt.
    pub struct StatusArgs {
        pub child_id: String,
        pub operation_key: Option<String>,
    }
}
tool_args! {
    /// Page direct children only, never owner-wide sessions or grandchildren.
    pub struct ListArgs {
        pub cursor: Option<String>,
        pub limit: Option<u16>,
    }
}
tool_args! {
    /// Close one direct child. Does not delete sessions or clean files.
    pub struct CloseArgs {
        pub operation_key: String,
        pub child_id: String,
    }
}

tool_args! {
    /// Object-only strict decoding/schema adapter for the shared session config.
    pub struct ConfigFields {
        pub model: Option<String>,
        pub mode: Option<String>,
        #[serde(default)]
        pub axes: std::collections::BTreeMap<String, crate::frames::ConfigValue>,
    }
}
fn strict_config<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<crate::frames::SessionConfig, D::Error> {
    let fields = ConfigFields::deserialize(deserializer)?;
    Ok(crate::frames::SessionConfig {
        cache_retention: Default::default(),
        model: fields.model,
        mode: fields.mode,
        axes: fields.axes,
    })
}

/// Durable acceptance/delivery record. Delivery never implies completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct OperationReceipt {
    pub operation_id: String,
    pub child_id: Option<String>,
    pub turn_id: Option<String>,
    pub pending_id: Option<String>,
    pub state: OperationState,
}

/// A delegation operation's state, independent of the session lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationState {
    Accepted,
    WaitingConfig,
    Queued,
    AwaitingReconciliation,
    Delivered,
    Completed,
    NotDelivered,
    Cancelled,
    Failed { code: String },
}

// Serde's internally tagged unit variants ignore additional fields even with
// deny_unknown_fields. Empty struct variants enforce the advertised shape.
impl<'de> Deserialize<'de> for OperationState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Accepted {},
            WaitingConfig {},
            Queued {},
            AwaitingReconciliation {},
            Delivered {},
            Completed {},
            NotDelivered {},
            Cancelled {},
            Failed { code: String },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Accepted {} => Self::Accepted,
            Wire::WaitingConfig {} => Self::WaitingConfig,
            Wire::Queued {} => Self::Queued,
            Wire::AwaitingReconciliation {} => Self::AwaitingReconciliation,
            Wire::Delivered {} => Self::Delivered,
            Wire::Completed {} => Self::Completed,
            Wire::NotDelivered {} => Self::NotDelivered,
            Wire::Cancelled {} => Self::Cancelled,
            Wire::Failed { code } => Self::Failed { code },
        })
    }
}

/// Self-contained tool argument schemas for MCP discovery.
pub fn argument_schemas() -> [(&'static str, serde_json::Value); 7] {
    macro_rules! schema {
        ($name:literal, $ty:ty) => {
            (
                $name,
                serde_json::to_value(schemars::schema_for!($ty)).expect("argument schema serializes"),
            )
        };
    }
    [
        schema!("session_capabilities", CapabilitiesArgs),
        schema!("session_spawn", SpawnArgs),
        schema!("session_send", SendArgs),
        schema!("session_answer", AnswerArgs),
        schema!("session_status", StatusArgs),
        schema!("session_list", ListArgs),
        schema!("session_close", CloseArgs),
    ]
}
