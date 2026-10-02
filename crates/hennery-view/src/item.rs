//! Display items (client view spec §3): what every client renders.

use hennery_proto::frames::{PendingKind, PendingReason};
use hennery_proto::rest::{PendingState, StoredBlock};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// One display item of a session. Its `id` is stable for the session's
/// life; `version` is the `event_id` of the last event that changed it, so
/// a client keeps the item with the higher version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Item {
    pub id: String,
    /// The `event_id` of the last event that changed the item.
    #[ts(type = "number")]
    pub version: i64,
    /// The turn whose group the item is in: the events from its
    /// `user_turn` up to the next one. Absent before the first turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub turn_id: Option<String>,
    /// When the item's first event was stored (RFC 3339, UTC): a merged
    /// message keeps its first chunk's.
    pub ts: String,
    #[serde(flatten)]
    pub body: Body,
}

/// What an item is, by its `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// The operator's prompt, as the collector stored it: text, and images
    /// as attachments (`GET /api/attachments/{sha256}`).
    UserTurn {
        content: Vec<StoredBlock>,
        /// A text block went past `TEXT_MAX_BYTES` and was cut there.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[ts(optional, as = "Option<bool>")]
        truncated: bool,
    },
    /// The agent's reply: consecutive message chunks, merged.
    Message(Text),
    /// The agent's reasoning: consecutive thought chunks, merged.
    Thinking(Text),
    ToolCall(ToolCall),
    /// The agent's plan, as its latest update in the group left it.
    Plan {
        entries: Vec<PlanEntry>,
        /// Steps past `PLAN_MAX_ENTRIES` were left out, or a step was cut.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[ts(optional, as = "Option<bool>")]
        truncated: bool,
    },
    /// A permission or elicitation the agent asked.
    Question(Question),
    /// A collector or host event, shown as a divider.
    Marker(Marker),
    /// An update the fold does not know, or could not read: never dropped
    /// (client view spec D3). Shown collapsed, its JSON as text.
    Unrecognised(Unrecognised),
}

/// Merged text, cut at `TEXT_MAX_BYTES`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Text {
    pub text: String,
    /// The text went past `TEXT_MAX_BYTES` and was cut there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(optional, as = "Option<bool>")]
    pub truncated: bool,
}

/// A tool call, merged from its `tool_call` and `tool_call_update`s.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ToolCall {
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// ACP's tool kind (`read`, `edit`, `execute`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tool_kind: Option<String>,
    /// `pending`, `in_progress`, `completed` or `failed`, as the agent sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<String>,
    /// The tool's raw input. Over `INPUT_MAX_BYTES` it is a string: the
    /// start of its JSON text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(optional, as = "Option<Vec<ToolContent>>")]
    pub content: Vec<ToolContent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(optional, as = "Option<Vec<Location>>")]
    pub locations: Vec<Location>,
    /// The tool's text output, from whichever shape the adapter sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output: Option<String>,
    /// The tool-invocation syntax found in the output: a sign the agent
    /// wrote a tool call and its result instead of making one (F-13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fabricated: Option<String>,
    /// A field went past its cap and was cut or left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(optional, as = "Option<bool>")]
    pub truncated: bool,
}

/// One block of a tool call's `content`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    Text {
        text: String,
    },
    Diff {
        path: String,
        /// Absent for a new file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        old_text: Option<String>,
        new_text: String,
    },
    Image {
        mime_type: String,
        /// Base64; absent past `IMAGE_MAX_BYTES`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        data: Option<String>,
    },
    Terminal {
        terminal_id: String,
    },
    /// A block of a type the fold does not know: its JSON as text.
    Other {
        raw: String,
    },
}

/// A file a tool call touches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Location {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub line: Option<u64>,
}

/// One step of the agent's plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PlanEntry {
    /// Empty when the agent sent none: the step is still shown.
    pub content: String,
    /// `high`, `medium` or `low`, as the agent sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub priority: Option<String>,
    /// `pending`, `in_progress` or `completed`, as the agent sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<String>,
}

/// A question the agent asked (ACP core §4.6), with where its answer
/// stands. The state fields mirror `PendingItem`'s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Question {
    pub pending_id: String,
    pub question_kind: PendingKind,
    pub request: Request,
    /// The server's pending set holds it open with no answer queued: only
    /// then can a card answer it (F-15). Never derived from position.
    pub answerable: bool,
    pub state: PendingState,
    /// Why it was cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<PendingReason>,
    /// An answer was accepted for it.
    pub answered: bool,
    /// The host's verdict on that answer; `true` sticks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub delivered: Option<bool>,
}

/// A question's request, parsed for its card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Permission {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        title: Option<String>,
        /// The tool call it asks about: the `tool_call_id` of a `tool_call`
        /// item.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        tool_call_id: Option<String>,
        /// In the adapter's order. Empty: nothing here can answer it.
        options: Vec<PermissionOption>,
    },
    Elicitation {
        message: String,
        fields: Vec<Field>,
        /// The keys the form requires, as the schema lists them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(optional, as = "Option<Vec<String>>")]
        required: Vec<String>,
        /// Every field is one the card can fill; otherwise it can only
        /// decline or cancel, and never invents a value.
        form_supported: bool,
    },
}

/// One answer a permission offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once` or `reject_always`.
    pub option_kind: String,
}

/// One field of an elicitation's form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Field {
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hint: Option<String>,
    pub field_kind: FieldKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(optional, as = "Option<Vec<FieldOption>>")]
    pub options: Vec<FieldOption>,
    /// This free-text field belongs to another field of the form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pairing: Option<Pairing>,
}

/// A free-text field's tie to the select it goes with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Pairing {
    /// The key of the field it goes with.
    pub with: String,
    pub kind: PairingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PairingKind {
    /// An answer instead of the other field's: choosing an option clears
    /// the text, and typing clears the option (frontend §6.3).
    Exclusive,
    /// An addition to the other field's answer, which stays as chosen.
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Single,
    Multi,
    Text,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FieldOption {
    /// What the answer carries.
    pub value: String,
    /// What the card shows for it: the option's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
}

/// A divider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Marker {
    pub marker: MarkerKind,
    /// A machine code: the park's reason, the host note's code, the start
    /// failure's code, the exit code or signal, the conflicting seq.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
    /// Text to show behind a disclosure: an error, a stderr tail, a note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub to: Option<String>,
    /// The turn the marker is about: for `turn_not_delivered`, the prompt
    /// that was not delivered; a client offers to send it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub about_turn: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    /// `session_parked`; `reason`: idle, adapter_exited or operator.
    Parked,
    /// `operator_resumed`.
    Resumed,
    /// `operator_closed`.
    Closed,
    HostRestarted,
    /// `presumed_parked`; `reason`: host_offline or host_revoked.
    HostOffline,
    /// `reattached`.
    HostBack,
    /// `turn_ended{interrupted}` or `turn_ended_synthesized`.
    TurnInterrupted,
    /// `turn_ended{failed}`; `text`: the error.
    TurnFailed,
    TurnCancelled,
    /// `about_turn`: the prompt that was not delivered.
    TurnNotDelivered,
    StartNotDelivered,
    /// `reason`: the code; `text`: the message.
    StartFailed,
    /// `reason`: `code <n>` or `signal <n>`; `text`: the stderr tail.
    AdapterExited,
    /// `reason`: `<from_seq>..<to_seq>`.
    TranscriptGap,
    /// `reason`: the note's code (`config_failed` and `reapply_failed`: a
    /// config switch did not take); `text`: the note.
    HostNote,
    /// `reason`: the seq; `text`: the body received, as JSON text.
    Conflict,
    /// `from` and `to`: hat ids.
    HatReassigned,
    /// The turn made more than the view holds (`GROUP_MAX_*`): nothing
    /// after this in the turn is shown; the raw events route has it all.
    /// `reason`: `items`, `questions` or `bytes`.
    Elided,
}

/// An update the fold could not place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Unrecognised {
    /// The event's kind, and for an ACP update its `sessionUpdate`
    /// (`acp_update/<sessionUpdate>`).
    pub update_kind: String,
    /// The event's body as JSON text, at most `RAW_MAX_BYTES`. Text, never
    /// a value: a cut one is not JSON, and a client shows it as it is.
    pub raw: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(optional, as = "Option<bool>")]
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The lane's envelope: `id`, `version`, `turn_id?` and `ts`, then the
    /// kind's fields, all on one level, `kind` naming it.
    #[test]
    fn an_item_is_one_flat_object_tagged_by_kind() {
        let item = Item {
            id: "t1:tool:c1".into(),
            version: 7,
            turn_id: Some("t1".into()),
            ts: "2026-10-02T08:00:00.000Z".into(),
            body: Body::ToolCall(ToolCall {
                tool_call_id: "c1".into(),
                tool_kind: Some("edit".into()),
                ..ToolCall::default()
            }),
        };
        let value = serde_json::to_value(&item).unwrap();
        assert_eq!(
            value,
            json!({"id": "t1:tool:c1", "version": 7, "turn_id": "t1", "ts": "2026-10-02T08:00:00.000Z",
                   "kind": "tool_call", "tool_call_id": "c1", "tool_kind": "edit"})
        );
        assert_eq!(serde_json::from_value::<Item>(value).unwrap(), item);
        // Before the first turn: no `turn_id` at all.
        let marker = Item {
            turn_id: None,
            body: Body::Marker(Marker {
                marker: MarkerKind::HostBack,
                reason: None,
                text: None,
                from: None,
                to: None,
                about_turn: None,
            }),
            ..item
        };
        assert_eq!(
            serde_json::to_value(&marker).unwrap(),
            json!({"id": "t1:tool:c1", "version": 7, "ts": "2026-10-02T08:00:00.000Z", "kind": "marker", "marker": "host_back"})
        );
    }
}
