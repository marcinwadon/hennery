//! Shared observations, deliberately independent of presentation status labels.
use crate::item::PlanEntry;
use hennery_proto::delegation_status::ChildSummary;
use hennery_proto::frames::TurnOutcome;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionWork {
    /// Greatest applied event included in this read. Reading never acknowledges it.
    #[ts(type = "number")]
    pub revision: i64,
    pub lifecycle: String,
    pub activity: Option<String>,
    pub presumed_parked: bool,
    pub close_requested: bool,
    pub configuration_error: Option<String>,
    pub current_turn_id: Option<String>,
    pub last_activity_at: Option<String>,
    /// Latest assistant text update, not the timestamp of a UI acknowledgement.
    pub last_message_at: Option<String>,
    pub last_turn: Option<WorkTurn>,
    pub waiting: WorkWaiting,
    pub queue: WorkQueue,
    /// ACP v1 does not provide an authoritative inventory of background work.
    pub background: BackgroundWork,
    /// Omitted without transcript access, including the plan's contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub content: Option<WorkContent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkTurn {
    pub turn_id: String,
    pub outcome: TurnOutcome,
    pub ended_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkWaiting {
    pub permissions: u32,
    pub questions: u32,
    /// Open requests whose accepted answer is still awaiting reconciliation.
    pub answered: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkQueue {
    pub held: bool,
    pub operator: u32,
    pub delegation: u32,
    /// All queued external prompts in the authorized session; external clients
    /// see only their own prompts.
    pub external: u32,
    pub external_scope: ExternalQueueScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ExternalQueueScope {
    Session,
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundWork {
    /// Missing telemetry is not an empty task list and cannot establish quiet.
    NotReported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkContent {
    pub last_message: Option<WorkMessage>,
    /// A reported plan is advisory, never evidence that a process is running.
    pub plan: Option<WorkPlan>,
    /// Earlier events were outside the bounded observation window.
    pub history_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkMessage {
    pub turn_id: Option<String>,
    pub text: String,
    pub updated_at: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkPlan {
    pub turn_id: Option<String>,
    pub entries: Vec<PlanEntry>,
    pub updated_at: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkChildSummary {
    #[serde(flatten)]
    pub summary: ChildSummary,
    pub work: Option<SessionWork>,
}

impl std::ops::Deref for WorkChildSummary {
    type Target = ChildSummary;
    fn deref(&self) -> &Self::Target {
        &self.summary
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkChildList {
    pub children: Vec<WorkChildSummary>,
    pub next_cursor: Option<String>,
}
