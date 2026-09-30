use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::frames::{ConfigValue, ElicitationAction, Indexed, PendingKind, PendingReason, SessionConfig};

/// `POST /api/sessions` (ACP core §9): `{host_id, agent, cwd, model?, mode?, axes?}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StartSessionRequest {
    pub host_id: String,
    pub agent: String,
    pub cwd: String,
    #[serde(flatten)]
    pub config: SessionConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StartSessionResponse {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PromptRequest {
    #[ts(type = "unknown[]")]
    pub content: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PromptResponse {
    pub turn_id: String,
}

/// One stored timeline event, as served by REST and SSE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EventDto {
    #[ts(type = "number")]
    pub event_id: i64,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | undefined", optional)]
    pub host_seq: Option<u64>,
    pub kind: String,
    #[ts(type = "unknown")]
    pub body: Value,
    pub ts: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    /// Set when the error leaves a session reachable by id (e.g. a session
    /// start whose delivery is unknown: the session was created and may
    /// still start, but the caller has no other way to learn its id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub session_id: Option<String>,
}

/// Result of a park or close: the session's lifecycle once the request took
/// effect (`parked` or `closed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LifecycleResponse {
    pub session_id: String,
    pub lifecycle: String,
}

/// A session's open turn, for the detail view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct OpenTurn {
    pub turn_id: String,
    /// `sent` (not yet acknowledged by the adapter) or `started`.
    pub state: String,
}

/// `GET /api/sessions/{id}` (ACP core §9): the list item, the open turn and
/// the pending requests still open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionDetail {
    pub session_id: String,
    pub host_id: String,
    pub agent: String,
    pub cwd: String,
    pub lifecycle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub failure_reason: Option<String>,
    /// Parked only because its host has been offline past the threshold
    /// (ACP core §5.3); the host may still be running it.
    pub presumed_parked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "OpenTurn | undefined", optional)]
    pub open_turn: Option<OpenTurn>,
    /// Open pending requests, oldest first: what the operator can answer.
    #[serde(default)]
    pub pending: Vec<PendingItem>,
}

/// Where a pending request stands (ACP core §4.6): `open`, then
/// `delivered` or `cancelled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingState {
    Open,
    Delivered,
    Cancelled,
}

/// One pending request, as the collector holds it: an entry of
/// `SessionDetail.pending`, and the data of SSE `pending_changed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PendingItem {
    pub pending_id: String,
    pub session_id: String,
    pub kind: PendingKind,
    pub state: PendingState,
    /// Why it was cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "PendingReason | undefined", optional)]
    pub reason: Option<PendingReason>,
    /// The turn it was asked in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub turn_id: Option<String>,
    /// A permission's option ids: the only valid answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string[] | undefined", optional)]
    pub option_ids: Option<Vec<String>>,
    /// The adapter's ACP request, verbatim.
    #[ts(type = "unknown")]
    pub payload: Value,
    /// An answer has been accepted for it (at most one is).
    pub answered: bool,
    /// The host's verdict on that answer, once one arrived. `true` sticks
    /// (umbrella §6.8): a card shows "answered" only then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub delivered: Option<bool>,
}

/// `POST /api/sessions/{id}/pending/{pending_id}/answer` (ACP core §9): an
/// option for a permission request, or an action for an elicitation, with
/// the form's content when it accepts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum AnswerRequest {
    Permission {
        option_id: String,
    },
    Elicitation {
        action: ElicitationAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown", optional)]
        content: Option<Value>,
    },
}

/// 202 to an answer: it is queued for the session's host, and delivered now
/// or on the host's next connection (ACP core §4.6). The verdict follows on
/// the session stream as `pending_changed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AnswerResponse {
    pub pending_id: String,
    /// Carried by the host's `answer_result` for this answer.
    pub request_id: String,
}

/// `POST /api/sessions/{id}/cancel`: how the open turn ended. Usually
/// `cancelled`, but `completed` or `failed` if the turn ended before the
/// cancel reached the agent, and `interrupted` if the session was parked or
/// closed while the cancel was outstanding, or its adapter exited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CancelResponse {
    pub turn_id: String,
    pub outcome: crate::frames::TurnOutcome,
}

/// `POST /api/sessions/{id}/config`: switch one config option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ConfigRequest {
    pub config_id: String,
    pub value: ConfigValue,
}

/// A session's config catalogue and its current values: `GET
/// /api/sessions/{id}/catalog`, the answer to `POST …/config`, and the data
/// of the SSE `catalog_changed` message (ACP core §9). Commands, plan and
/// usage join it with the plans that produce them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionCatalog {
    pub session_id: String,
    /// The adapter's ACP `SessionConfigOption` objects, as last reported.
    #[ts(type = "unknown[]")]
    pub config_options: Vec<Value>,
    #[serde(flatten)]
    pub current: SessionConfig,
}

impl SessionCatalog {
    /// The catalogue an event's extracts report, if they carry a snapshot.
    pub fn from_indexed(session_id: &str, indexed: &Indexed) -> Option<Self> {
        let current = indexed.current_config()?;
        Some(Self {
            session_id: session_id.to_string(),
            config_options: indexed.config_options.clone().unwrap_or_default(),
            current,
        })
    }
}

/// 201 to `POST /api/hosts/pairing-codes` (kernel spec §4.1): a single-use
/// code for `hennery host join`, shown once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PairingCodeResponse {
    /// `XXXX-XXXX`, Crockford base32.
    pub code: String,
    /// RFC 3339.
    pub expires_at: String,
}

/// `POST /api/hosts/enroll` (kernel spec §4.1): a host pairs itself with a
/// code and the public half of the key it generated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EnrollRequest {
    pub code: String,
    /// Ed25519 public key, 64 hex characters.
    pub public_key: String,
    pub name: String,
    pub host_version: String,
    pub platform: String,
}

/// 201 to an enrollment: the id the host names itself by in `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EnrollResponse {
    pub host_id: String,
}

/// One entry of `GET /api/hosts` (kernel spec §8): a paired host, revoked
/// ones included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HostItem {
    pub host_id: String,
    pub name: String,
    pub platform: String,
    pub host_version: String,
    /// From its latest accepted `hello`.
    pub capabilities: crate::frames::Capabilities,
    /// Connected and reconciled: requests reach it now.
    pub connected: bool,
    /// RFC 3339.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub last_seen_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub revoked_at: Option<String>,
}
