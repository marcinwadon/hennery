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
        /// Walking skeleton only: a shared development token. Replaced by an
        /// Ed25519 proof of possession (ACP core §3.5).
        token: String,
        /// What this host implements (a closed list, ACP core §3.3). Absent
        /// means none.
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
