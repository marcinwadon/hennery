//! Session file contract. Bytes travel only in bounded host frames, not events.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const MAX_ARTIFACT_BYTES: usize = 16 << 20;
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ArtifactListArgs {
    pub cursor: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ArtifactPage {
    pub artifacts: Vec<ArtifactRevision>,
    pub next_cursor: Option<String>,
}
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\'))
}
pub fn valid_mime(mime: &str) -> bool {
    mime.len() <= 128
        && mime
            .split_once('/')
            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
        && mime
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/!#$&^_.+-".contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ArtifactRevision {
    pub artifact_id: String,
    pub session_id: String,
    pub revision: u32,
    pub title: String,
    pub name: String,
    pub mime_type: String,
    pub sha256: String,
    #[ts(type = "number")]
    pub size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct PublishArtifact {
    pub path: String,
    pub request_key: String,
    pub title: Option<String>,
    pub mime_type: Option<String>,
    pub artifact_id: Option<String>,
    pub expected_revision: Option<u32>,
}

impl PublishArtifact {
    pub fn valid(&self) -> bool {
        !self.path.is_empty()
            && self.path.len() <= 4096
            && !self.path.contains('\0')
            && !self.request_key.is_empty()
            && self.request_key.len() <= 128
            && self
                .request_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
            && self
                .title
                .as_ref()
                .is_none_or(|t| !t.trim().is_empty() && t.len() <= 256 && !t.chars().any(char::is_control))
            && self.mime_type.as_deref().is_none_or(valid_mime)
            && self.artifact_id.as_ref().is_none_or(|id| uuid_shape(id))
            && self.artifact_id.is_some() == self.expected_revision.is_some()
            && self.expected_revision.is_none_or(|v| v > 0 && v < u32::MAX)
    }
}
pub fn publish_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(PublishArtifact)).expect("typed schema")
}
pub fn list_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(ArtifactListArgs)).expect("typed schema")
}
fn uuid_shape(id: &str) -> bool {
    id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}
