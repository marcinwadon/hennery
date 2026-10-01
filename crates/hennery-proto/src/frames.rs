use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

/// A session that a host still has an adapter for, reported in `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AttachedSession {
    pub session_id: String,
    #[ts(type = "number")]
    pub last_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_turn_id: Option<String>,
}

/// A feature a host implements, announced in `hello` (ACP core §3.3). The
/// collector never sends a frame that needs a capability to a host that
/// lacks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Project enumeration and browsing.
    Projects,
    /// Image content blocks in prompts.
    Images,
    /// Explicit park (`park_session`).
    Park,
}

/// `hello.capabilities`. Deserialized leniently: a capability this build
/// does not know (a newer host, a minor protocol bump) is skipped, never a
/// reason to refuse the whole `hello`. The generated schema still lists
/// `Capability` as a closed `oneOf` (there is no open-ended JSON Schema
/// equivalent), but that is a description of the known values, not a
/// constraint hennery itself enforces: an entry outside it is ignored, not
/// rejected, so a schema-validating client or proxy must not reject a
/// `hello` on an unknown capability either.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema, TS)]
pub struct Capabilities(pub Vec<Capability>);

impl Capabilities {
    pub fn has(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }
}

impl<'de> Deserialize<'de> for Capabilities {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw: Vec<Value> = Deserialize::deserialize(deserializer)?;
        Ok(Self(
            raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
        ))
    }
}

/// How a turn ended. Exactly one `turn_ended` per accepted turn (ACP core §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}

/// Why a session was parked (ACP core §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ParkReason {
    Idle,
    AdapterExited,
    Operator,
}

/// What a pending request asks the operator (ACP core §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingKind {
    /// `session/request_permission`: pick one of the offered options.
    Permission,
    /// `elicitation/create`: fill in a form, or decline.
    Elicitation,
}

/// How a pending request ended (ACP core §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingResolution {
    /// The operator's answer reached the waiting adapter.
    Delivered,
    /// The adapter was told the question is off (see `PendingReason`).
    Cancelled,
}

/// Why a pending request was cancelled (ACP core §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingReason {
    TurnCancelled,
    SessionClosed,
    SessionParked,
    AdapterLost,
    HostRestarted,
    /// The adapter withdrew its own question (`$/cancel_request`).
    AgentWithdrew,
    /// The operator revoked the session's host (kernel spec §4.3): it will
    /// never connect again to deliver an answer.
    HostRevoked,
}

/// The operator's answer to an elicitation (ACP `elicitation/create`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

/// The `pending` extract (ACP core §3.2): what the collector needs to hold
/// a pending request and validate its answer, without reading the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PendingExtract {
    pub id: String,
    pub kind: PendingKind,
    /// The permission's option ids. Absent for an elicitation, and for a
    /// permission request whose options hennery could not parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_ids: Option<Vec<String>>,
}

/// The value of one config option (ACP `session/set_config_option`): a
/// select's value id, or a boolean toggle's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum ConfigValue {
    Bool(bool),
    Id(String),
}

/// Model, mode and the other config axes of a session (ACP core §3.3,
/// §4.3): what a start asks for, and what a resume re-applies. `model` and
/// `mode` are the values of the adapter's model and mode options; `axes`
/// holds every other option, by config id. Flattened into the frames and
/// the start request, so the wire stays `model?, mode?, axes{}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub axes: BTreeMap<String, ConfigValue>,
}

impl SessionConfig {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.mode.is_none() && self.axes.is_empty()
    }
}

/// Fields the collector may read from a session event. Closed set (ACP core §3.2).
///
/// The catalogue extracts (`config_options`, `current_model`,
/// `current_mode`, `current_axes`) travel together: when `config_options`
/// is present and not empty, the four are one snapshot of the adapter's
/// config. An absent or empty `config_options` means "no read-back", never
/// "the adapter has no config" (a response that failed to parse looks
/// empty).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Indexed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The full config catalogue: the adapter's ACP `SessionConfigOption`
    /// objects, for the UI. The collector stores it and never reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown[] | undefined", optional)]
    pub config_options: Option<Vec<Value>>,
    /// The current value of the model option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_model: Option<String>,
    /// The current value of the mode option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_mode: Option<String>,
    /// The current value of every other option, by config id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_axes: Option<BTreeMap<String, ConfigValue>>,
    /// On `pending_opened`: the request's id, kind and option ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingExtract>,
}

impl Indexed {
    /// The config this event reports as current, if it carries a catalogue
    /// snapshot (see the type's doc).
    pub fn current_config(&self) -> Option<SessionConfig> {
        let options = self.config_options.as_ref()?;
        if options.is_empty() {
            return None;
        }
        Some(SessionConfig {
            model: self.current_model.clone(),
            mode: self.current_mode.clone(),
            axes: self.current_axes.clone().unwrap_or_default(),
        })
    }
}

/// The body of a sequenced, outboxed session frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionBody {
    /// The adapter session exists. Resolves the collector's start waiter.
    /// `indexed` carries the catalogue after the start's config switches
    /// (ACP core §4.3, P-13).
    SessionStarted {
        request_id: String,
        agent_session_id: String,
        #[serde(default)]
        indexed: Indexed,
    },
    /// The host accepted a start but could not create the adapter session
    /// (spawn, `initialize` or `session/new` failed). Rejects the start waiter.
    StartFailed {
        request_id: String,
        code: String,
        message: String,
    },
    /// The prompt reached the adapter. Resolves the collector's prompt waiter.
    TurnStarted { request_id: String, turn_id: String },
    /// An ACP message from the adapter, verbatim in `payload`.
    AcpUpdate {
        #[serde(default)]
        indexed: Indexed,
        #[ts(type = "unknown")]
        payload: Value,
    },
    TurnEnded {
        turn_id: String,
        outcome: TurnOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// The adapter process is gone and the session is detached (idle reap,
    /// adapter exit, or an operator park). Completes `park_session`.
    SessionParked { reason: ParkReason },
    /// The operator closed an attached session. Completes `close_session`.
    SessionClosed,
    /// The adapter exited without being asked to (ACP core §2.3). Followed by
    /// `session_parked{adapter_exited}`. The stderr tail is scrubbed.
    AdapterExited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        stderr_tail: String,
    },
    /// hennery's own diagnostic that is not state (ACP core §3.2): a failed
    /// re-apply, or update kinds dropped during `session/load` (§4.5).
    /// `note` is a machine code; `text` is scrubbed.
    HostNote { note: String, text: String },
    /// A `set_config` took effect: the catalogue the adapter answered with,
    /// the authoritative read-back (ACP core §3.2). Resolves the collector's
    /// config waiter.
    ConfigApplied {
        request_id: String,
        #[serde(default)]
        indexed: Indexed,
    },
    /// The adapter asked the operator something (ACP core §4.6): its ACP
    /// request verbatim in `payload`, with `indexed.pending`. It waits, with
    /// no timeout, until it is answered or cancelled. Its kind is
    /// `indexed.pending.kind` (the body's own `kind` is its tag).
    PendingOpened {
        pending_id: String,
        #[serde(default)]
        indexed: Indexed,
        #[ts(type = "unknown")]
        payload: Value,
    },
    /// A pending request is over: answered (`delivered`), or cancelled for
    /// `reason`.
    PendingResolved {
        pending_id: String,
        resolution: PendingResolution,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<PendingReason>,
    },
    /// The host's verdict on one answer (umbrella §6.8): `delivered` if the
    /// adapter was still waiting for it. A delivered answer is followed by
    /// `pending_resolved{delivered}`.
    AnswerResult {
        pending_id: String,
        request_id: String,
        delivered: bool,
    },
}

impl SessionBody {
    /// A `session_started` with nothing but its ids, for tests and for
    /// callers that have no catalogue to announce.
    pub fn session_started(request_id: impl Into<String>, agent_session_id: impl Into<String>) -> Self {
        Self::SessionStarted {
            request_id: request_id.into(),
            agent_session_id: agent_session_id.into(),
            indexed: Indexed::default(),
        }
    }
}

/// Host -> collector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostFrame {
    Hello {
        protocol_version: String,
        host_version: String,
        host_id: String,
        /// Proof of possession of the host's key (ACP core §3.5): its
        /// Ed25519 signature, hex, over `hello_proof_message(nonce, host_id,
        /// protocol_version)`, where the nonce is the one the collector sent
        /// in the upgrade response's `hennery-hello-nonce` header.
        proof: String,
        /// What this host implements (ACP core §3.3). `Capability` lists
        /// the values this version knows; an unknown one is ignored, not
        /// rejected (see `Capabilities`). Absent means none.
        #[serde(default)]
        capabilities: Capabilities,
        attached_sessions: Vec<AttachedSession>,
    },
    /// Every state-bearing fact is a sequenced frame: it goes through the host
    /// outbox and is acked (ACP core §3.3).
    Session {
        session_id: String,
        #[ts(type = "number")]
        seq: u64,
        body: SessionBody,
    },
    /// A rejected request. Not outboxed: a rejection means nothing happened,
    /// so losing it only costs the collector a timeout.
    Error {
        request_id: String,
        code: String,
        message: String,
    },
    /// Sent once per connection after the unacked outbox has been resent.
    /// The collector reconciles `hello.attached_sessions` only after this
    /// frame, so a resent `turn_ended` is never duplicated by a synthesised
    /// one (ACP core §5.2).
    ResendComplete,
}

/// Collector -> host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CollectorFrame {
    HelloAck {
        protocol_version: String,
        collector_version: String,
        /// Highest committed seq per session listed in `hello`; the host
        /// fast-forwards its counters if they are lower (lost outbox).
        #[ts(type = "Record<string, number>")]
        committed: BTreeMap<String, u64>,
    },
    HelloError {
        code: String,
        message: String,
    },
    /// Start a new session: spawn the adapter, `session/new`, then apply the
    /// requested config (ACP core §3.3, §4.3). Completed by
    /// `session_started` | `start_failed`.
    StartSession {
        request_id: String,
        session_id: String,
        /// The collector's highest committed seq for this session; the host
        /// continues from the larger of this and its own counter (§5.1).
        #[ts(type = "number")]
        committed_seq: u64,
        agent: String,
        cwd: String,
        /// Applied after `session/new`: model, then the other axes, then
        /// mode (ACP core §4.3).
        #[serde(flatten)]
        config: SessionConfig,
    },
    /// Attach a parked, closed or failed session again: spawn the adapter and
    /// `session/load` it with replay suppression (ACP core §4.3, §4.5).
    /// Completed by `session_started` | `start_failed`, like a start.
    ResumeSession {
        request_id: String,
        session_id: String,
        /// Fast-forward the host's counter before the first frame (§5.1).
        #[ts(type = "number")]
        committed_seq: u64,
        agent: String,
        cwd: String,
        /// The adapter's own session id, from the stored `session_started`:
        /// the host keeps no copy across restarts.
        agent_session_id: String,
        /// The stored config, re-applied after `session/load` (ACP core
        /// §4.3). A switch that fails is a `host_note`, not a failed resume.
        #[serde(flatten)]
        config: SessionConfig,
    },
    Prompt {
        request_id: String,
        session_id: String,
        turn_id: String,
        /// ACP ContentBlocks, built by the frontend.
        #[ts(type = "unknown[]")]
        content: Vec<Value>,
    },
    /// Stop the turn in flight: `session/cancel` to the adapter. Completed
    /// by that turn's `turn_ended` (ACP core §3.3, §4.4), whatever its
    /// outcome: a turn that finished first is not cancelled.
    CancelTurn {
        request_id: String,
        session_id: String,
        turn_id: String,
    },
    /// Switch one config option of an attached session (`session/set_config_option`).
    /// Completed by `config_applied` | `error` (ACP core §3.3).
    SetConfig {
        request_id: String,
        session_id: String,
        config_id: String,
        value: ConfigValue,
    },
    /// The operator's choice for a permission request. Completed by
    /// `answer_result` (ACP core §4.6); it has no collector waiter.
    AnswerPermission {
        request_id: String,
        session_id: String,
        pending_id: String,
        option_id: String,
    },
    /// The operator's answer to an elicitation; `content` only with
    /// `accept`. Completed by `answer_result`, like a permission's.
    AnswerElicitation {
        request_id: String,
        session_id: String,
        pending_id: String,
        action: ElicitationAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown")]
        content: Option<Value>,
    },
    Ack {
        session_id: String,
        #[ts(type = "number")]
        ack_seq: u64,
    },
    /// Completed by `session_parked{operator}` (ACP core §4.8).
    ParkSession {
        request_id: String,
        session_id: String,
    },
    /// Completed by `session_closed` (ACP core §4.8).
    CloseSession {
        request_id: String,
        session_id: String,
    },
}
