//! Independent same-hat session access policy.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum PeerAccess {
    Off,
    Read,
    ReadWrite,
}
impl PeerAccess {
    pub fn enabled(self) -> bool {
        self != Self::Off
    }
    pub fn writable(self) -> bool {
        self == Self::ReadWrite
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Read => "read",
            Self::ReadWrite => "read_write",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct PeerPolicy {
    pub access: PeerAccess,
    #[ts(type = "number")]
    pub revision: i64,
}
