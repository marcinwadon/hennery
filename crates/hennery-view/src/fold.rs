//! The display fold (frontend §6.1; client view spec §3): stored events in,
//! items out.
//!
//! A session's events are cut into groups at each `user_turn` event; the
//! events before the first form the preamble. Every merge stays inside its
//! group: a chunk run, a tool call's updates, the plan, a question's
//! verdict. So folding from any group's start gives the same items, with
//! the same ids and versions, for the groups it covers as folding the whole
//! session: a page folds only its own turns, and the item stream keeps only
//! the current group in memory. Groups are cut on the `kind` column, which
//! only the collector writes as `user_turn`: no host or agent can forge
//! one (the review's O-3).
//!
//! `apply` changes the state; `take` hands out what changed since the last
//! `take`, each item once and whole, so a page or a resume writes each item
//! out once, not once per chunk (the review's A-6).

use crate::cap::{
    CODE_MAX_BYTES, GROUP_MAX_BYTES, GROUP_MAX_ITEMS, GROUP_MAX_QUESTIONS, ID_MAX_BYTES, LABEL_MAX_BYTES,
    NOTE_MAX_BYTES, PLAN_MAX_ENTRIES, RAW_MAX_BYTES, TEXT_MAX_BYTES, append, clip, cut, json_text,
};
use crate::item::{Body, Item, Marker, MarkerKind, PlanEntry, Question, Text, ToolCall, Unrecognised};
use crate::{question, tool};
use hennery_proto::frames::{PendingKind, PendingReason};
use hennery_proto::rest::{EventDto, PendingState, StoredBlock};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// The key of the preamble group in item ids.
pub const PREAMBLE: &str = "start";

/// ACP updates that make no item: each is served elsewhere (the catalogue,
/// the session list) and none breaks a chunk run.
pub const SILENT_UPDATES: [&str; 5] = [
    "available_commands_update",
    "usage_update",
    "session_info_update",
    "current_mode_update",
    "config_option_update",
];

/// Event kinds that make no item: the session's own state, shown by the
/// session list and the header.
pub const SILENT_EVENTS: [&str; 6] = [
    "session_started",
    "turn_started",
    "config_applied",
    "git_state",
    // Its `operator_closed` is the divider.
    "session_closed",
    // Its `session_parked` is the divider.
    "operator_parked",
];

/// Who may answer a question now: the server's pending set (F-15).
pub trait Answerable {
    /// Open, with no answer queued.
    fn answerable(&self, pending_id: &str) -> bool;
}

impl<F: Fn(&str) -> bool> Answerable for F {
    fn answerable(&self, pending_id: &str) -> bool {
        self(pending_id)
    }
}

/// Items in the order they first appeared, each as it last stood.
#[derive(Debug, Default)]
pub struct Items {
    items: Vec<Item>,
    at: HashMap<String, usize>,
}

impl Items {
    /// Put in an item: replace it where it is, or add it at the end.
    pub fn upsert(&mut self, item: Item) {
        match self.at.get(&item.id) {
            Some(&at) => self.items[at] = item,
            None => {
                self.at.insert(item.id.clone(), self.items.len());
                self.items.push(item);
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Item> {
        self.items.get(*self.at.get(id)?)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Item> {
        self.items.get_mut(*self.at.get(id)?)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn into_vec(self) -> Vec<Item> {
        self.items
    }
}

/// What an item weighs held in memory, near enough: its strings' bytes,
/// and a little for the rest. The group's budget sums it.
pub fn weight(item: &Item) -> usize {
    let opt = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    64 + item.id.len()
        + item.ts.len()
        + opt(&item.turn_id)
        + match &item.body {
            Body::UserTurn { content, .. } => content
                .iter()
                .map(|block| match block {
                    StoredBlock::Text { text } => text.len() + 16,
                    StoredBlock::Image { mime_type, sha256, .. } => mime_type.len() + sha256.len() + 24,
                })
                .sum(),
            Body::Message(text) | Body::Thinking(text) => text.text.len(),
            Body::ToolCall(call) => tool::weight(call),
            Body::Plan { entries, .. } => entries
                .iter()
                .map(|e| e.content.len() + opt(&e.priority) + opt(&e.status) + 16)
                .sum(),
            Body::Question(question) => question.pending_id.len() + question::weight(&question.request),
            Body::Marker(marker) => {
                opt(&marker.reason) + opt(&marker.text) + opt(&marker.from) + opt(&marker.to) + opt(&marker.about_turn)
            }
            Body::Unrecognised(u) => u.update_kind.len() + u.raw.len(),
        }
}

/// The fold's state: the current group's items, and what changed since the
/// last `take`.
#[derive(Debug)]
pub struct Fold {
    /// The current group's turn id; `None` in the preamble.
    turn: Option<String>,
    /// The current group's key in ids: its turn id, or `start`.
    group: String,
    /// Items made so far in the group that take an ordinal id.
    ordinal: u64,
    /// The current group's items.
    items: Items,
    /// Their weight together, and how many there are, questions apart.
    bytes: usize,
    count: usize,
    questions: usize,
    /// The group went past its budget: it makes and changes nothing more,
    /// but its questions.
    elided: bool,
    /// The chunk run being merged into: its item's id.
    run: Option<String>,
    /// The group's tool calls: `toolCallId` to item id.
    tools: HashMap<String, String>,
    /// The group's plan's item id.
    plan: Option<String>,
    /// Ids changed since the last `take`, in the order each first changed.
    dirty: Vec<String>,
    dirty_set: HashSet<String>,
    /// What changed in groups that have ended since the last `take`.
    flushed: Vec<Item>,
    /// Ids the event being applied changed.
    touched: Vec<String>,
}

/// The session update inside an `acp_update`'s payload.
fn update_of(body: &Value) -> Option<&Value> {
    body.pointer("/payload/update")
}

fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

impl Default for Fold {
    fn default() -> Self {
        Self::new()
    }
}

impl Fold {
    /// A fold at the start of a session, or of any group: the next event
    /// it gets is its first.
    pub fn new() -> Self {
        Self {
            turn: None,
            group: PREAMBLE.to_string(),
            ordinal: 0,
            items: Items::default(),
            bytes: 0,
            count: 0,
            questions: 0,
            elided: false,
            run: None,
            tools: HashMap::new(),
            plan: None,
            dirty: Vec::new(),
            dirty_set: HashSet::new(),
            flushed: Vec::new(),
            touched: Vec::new(),
        }
    }

    /// The current group's turn id; `None` in the preamble.
    pub fn turn(&self) -> Option<&str> {
        self.turn.as_deref()
    }

    /// The current group's items, as they stand.
    pub fn current(&self) -> &Items {
        &self.items
    }

    /// Apply one stored event, in `event_id` order. Whether it made or
    /// changed any item.
    pub fn apply(&mut self, event: &EventDto, answerable: &dyn Answerable) -> bool {
        self.touched.clear();
        self.event(event, answerable);
        // Only an event that makes or changes an item breaks a run: one
        // that changes the run's own item continues it.
        if self.touched.iter().any(|id| Some(id) != self.run.as_ref()) {
            self.run = None;
        }
        !self.touched.is_empty()
    }

    /// Every item made or changed since the last `take`, each once, as it
    /// stands, in the order each first changed.
    pub fn take(&mut self) -> Vec<Item> {
        let mut out = std::mem::take(&mut self.flushed);
        for id in self.dirty.drain(..) {
            if let Some(item) = self.items.get(&id) {
                out.push(item.clone());
            }
        }
        self.dirty_set.clear();
        out
    }

    fn event(&mut self, event: &EventDto, answerable: &dyn Answerable) {
        let body = &event.body;
        let kind = event.kind.as_str();
        if kind == "user_turn" {
            return self.user_turn(event);
        }
        match kind {
            "pending_opened" => return self.question_opened(event, answerable),
            "answer_submitted" | "answer_result" | "pending_resolved" | "pending_cancelled" => {
                return self.question_changed(event, answerable);
            }
            _ => {}
        }
        if self.elided {
            return;
        }
        match kind {
            "acp_update" => self.acp_update(event),
            kind if SILENT_EVENTS.contains(&kind) => {}
            "turn_ended" => match str_at(body, "outcome") {
                Some("completed") => {}
                Some("interrupted") => self.marker(event, MarkerKind::TurnInterrupted, None, None),
                Some("cancelled") => self.marker(event, MarkerKind::TurnCancelled, None, None),
                Some("failed") => {
                    let text = str_at(body, "error").map(str::to_string);
                    self.marker(event, MarkerKind::TurnFailed, None, text);
                }
                _ => self.unrecognised(event, kind),
            },
            _ => {
                if !self.marker_for(event) {
                    self.unrecognised(event, kind);
                }
            }
        }
    }

    /// A new group begins at a `user_turn`: nothing before it merges with
    /// anything after.
    fn user_turn(&mut self, event: &EventDto) {
        for id in self.dirty.drain(..) {
            if let Some(item) = self.items.get(&id) {
                self.flushed.push(item.clone());
            }
        }
        self.dirty_set.clear();
        let turn = str_at(&event.body, "turn_id")
            .filter(|t| !t.is_empty() && t.len() <= ID_MAX_BYTES)
            .map(str::to_string);
        self.group = turn.clone().unwrap_or_else(|| format!("event-{}", event.event_id));
        self.turn = Some(self.group.clone());
        self.ordinal = 0;
        self.items = Items::default();
        self.bytes = 0;
        self.count = 0;
        self.questions = 0;
        self.elided = false;
        self.run = None;
        self.tools.clear();
        self.plan = None;
        let content = event.body.get("content").cloned().unwrap_or(Value::Null);
        match serde_json::from_value::<Vec<StoredBlock>>(content) {
            Ok(mut content) => {
                let mut truncated = false;
                for block in &mut content {
                    if let StoredBlock::Text { text } = block {
                        let (kept, cut_off) = cut(text, TEXT_MAX_BYTES);
                        *text = kept;
                        truncated |= cut_off;
                    }
                }
                self.create_ordinal(event, Body::UserTurn { content, truncated });
            }
            Err(_) => self.unrecognised(event, "user_turn"),
        }
    }

    /// The divider an event shows as, if it is one.
    fn marker_for(&mut self, event: &EventDto) -> bool {
        let body = &event.body;
        let text = |key: &str| str_at(body, key).map(str::to_string);
        let mut about_turn = None;
        let (kind, reason, shown, from, to) = match event.kind.as_str() {
            "session_parked" => (MarkerKind::Parked, text("reason"), None, None, None),
            "operator_resumed" => (MarkerKind::Resumed, None, None, None, None),
            "operator_closed" => (MarkerKind::Closed, None, None, None, None),
            "host_restarted" => (MarkerKind::HostRestarted, None, None, None, None),
            "presumed_parked" => (MarkerKind::HostOffline, text("reason"), None, None, None),
            "reattached" => (MarkerKind::HostBack, None, None, None, None),
            "turn_ended_synthesized" => (MarkerKind::TurnInterrupted, None, None, None, None),
            "turn_not_delivered" => {
                // Past its cap it names no turn a client could send again.
                about_turn = text("turn_id").filter(|t| !t.is_empty() && t.len() <= ID_MAX_BYTES);
                (MarkerKind::TurnNotDelivered, None, None, None, None)
            }
            "start_not_delivered" => (MarkerKind::StartNotDelivered, None, None, None, None),
            "start_failed" => (MarkerKind::StartFailed, text("code"), text("message"), None, None),
            "adapter_exited" => {
                let number = |key| body.get(key).and_then(Value::as_i64);
                let reason = match (number("code"), number("signal")) {
                    (Some(code), _) => Some(format!("code {code}")),
                    (None, Some(signal)) => Some(format!("signal {signal}")),
                    (None, None) => None,
                };
                (MarkerKind::AdapterExited, reason, text("stderr_tail"), None, None)
            }
            "transcript_gap" => {
                let seq = |key| body.get(key).and_then(Value::as_u64);
                let reason = seq("from_seq")
                    .zip(seq("to_seq"))
                    .map(|(from, to)| format!("{from}..{to}"));
                (MarkerKind::TranscriptGap, reason, None, None, None)
            }
            "host_note" => (MarkerKind::HostNote, text("note"), text("text"), None, None),
            "conflict" => {
                let seq = body.get("seq").and_then(Value::as_u64).map(|s| s.to_string());
                let received = body.get("received").map(|r| json_text(r, NOTE_MAX_BYTES).0);
                (MarkerKind::Conflict, seq, received, None, None)
            }
            "hat_reassigned" => (MarkerKind::HatReassigned, None, None, text("from"), text("to")),
            _ => return false,
        };
        let marker = Marker {
            marker: kind,
            reason: reason.map(|r| clip(&r, CODE_MAX_BYTES)),
            text: shown.map(|t| clip(&t, NOTE_MAX_BYTES)),
            from: from.map(|f| clip(&f, ID_MAX_BYTES)),
            to: to.map(|t| clip(&t, ID_MAX_BYTES)),
            about_turn,
        };
        self.create_ordinal(event, Body::Marker(marker));
        true
    }

    fn marker(&mut self, event: &EventDto, marker: MarkerKind, reason: Option<String>, text: Option<String>) {
        let marker = Marker {
            marker,
            reason,
            text: text.map(|t| clip(&t, NOTE_MAX_BYTES)),
            from: None,
            to: None,
            about_turn: None,
        };
        self.create_ordinal(event, Body::Marker(marker));
    }

    fn acp_update(&mut self, event: &EventDto) {
        let Some(update) = update_of(&event.body) else {
            return self.unrecognised(event, "acp_update");
        };
        let kind = str_at(update, "sessionUpdate").unwrap_or_default();
        let label = format!("acp_update/{kind}");
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => {
                let content = update.get("content");
                let text = content
                    .filter(|c| str_at(c, "type") == Some("text"))
                    .and_then(|c| str_at(c, "text"));
                match text {
                    None => self.unrecognised(event, &label),
                    // An empty chunk changes nothing.
                    Some("") => {}
                    Some(text) => self.chunk(event, kind == "agent_thought_chunk", text),
                }
            }
            "tool_call" | "tool_call_update" => {
                let Some(call_id) =
                    str_at(update, "toolCallId").filter(|id| !id.is_empty() && id.len() <= ID_MAX_BYTES)
                else {
                    return self.unrecognised(event, &label);
                };
                let id = match self.tools.get(call_id) {
                    Some(id) => id.clone(),
                    None => {
                        let id = format!("{}:tool:{call_id}", self.group);
                        let body = Body::ToolCall(ToolCall {
                            tool_call_id: call_id.to_string(),
                            ..ToolCall::default()
                        });
                        if !self.create(event, id.clone(), body, false) {
                            return;
                        }
                        self.tools.insert(call_id.to_string(), id.clone());
                        id
                    }
                };
                self.change(event, &id, |body| match body {
                    Body::ToolCall(call) => tool::merge(call, update),
                    _ => false,
                });
            }
            "plan" => {
                let Some(entries) = update.get("entries").and_then(Value::as_array) else {
                    return self.unrecognised(event, &label);
                };
                // A step with no text is still a step: dropping it would
                // renumber the rest.
                let mut truncated = entries.len() > PLAN_MAX_ENTRIES;
                let entries: Vec<PlanEntry> = entries
                    .iter()
                    .take(PLAN_MAX_ENTRIES)
                    .map(|e| {
                        let (content, cut_off) = cut(str_at(e, "content").unwrap_or_default(), LABEL_MAX_BYTES);
                        truncated |= cut_off;
                        PlanEntry {
                            content,
                            priority: str_at(e, "priority").map(|p| clip(p, CODE_MAX_BYTES)),
                            status: str_at(e, "status").map(|s| clip(s, CODE_MAX_BYTES)),
                        }
                    })
                    .collect();
                let id = match &self.plan {
                    Some(id) => id.clone(),
                    None => {
                        let id = format!("{}:plan", self.group);
                        let body = Body::Plan {
                            entries: Vec::new(),
                            truncated: false,
                        };
                        if !self.create(event, id.clone(), body, false) {
                            return;
                        }
                        self.plan = Some(id.clone());
                        id
                    }
                };
                self.change(event, &id, |body| {
                    let new = Body::Plan { entries, truncated };
                    let changed = *body != new;
                    *body = new;
                    changed
                });
            }
            kind if SILENT_UPDATES.contains(&kind) => {}
            _ => self.unrecognised(event, &label),
        }
    }

    /// Merge a chunk into the run of its kind, or start one. A chunk past
    /// the cap changes nothing: no new version, no upsert.
    fn chunk(&mut self, event: &EventDto, thinking: bool, text: &str) {
        let same_run = self
            .run
            .as_deref()
            .and_then(|id| self.items.get(id))
            .is_some_and(|item| {
                matches!(
                    (&item.body, thinking),
                    (Body::Thinking(_), true) | (Body::Message(_), false)
                )
            });
        if same_run {
            let id = self.run.clone().expect("a run");
            self.change(event, &id, |body| match body {
                Body::Message(merged) | Body::Thinking(merged) if !merged.truncated => {
                    let (appended, cut_off) = append(&mut merged.text, text, TEXT_MAX_BYTES);
                    merged.truncated |= cut_off;
                    appended > 0 || cut_off
                }
                _ => false,
            });
            return;
        }
        let (text, truncated) = cut(text, TEXT_MAX_BYTES);
        let text = Text { text, truncated };
        let body = if thinking {
            Body::Thinking(text)
        } else {
            Body::Message(text)
        };
        if let Some(id) = self.create_ordinal(event, body) {
            self.run = Some(id);
        }
    }

    fn question_opened(&mut self, event: &EventDto, answerable: &dyn Answerable) {
        let body = &event.body;
        let pending = body.pointer("/indexed/pending");
        let kind = pending
            .and_then(|p| p.get("kind"))
            .and_then(|k| serde_json::from_value::<PendingKind>(k.clone()).ok());
        let pending_id = str_at(body, "pending_id").filter(|id| !id.is_empty() && id.len() <= ID_MAX_BYTES);
        let (Some(pending_id), Some(kind)) = (pending_id, kind) else {
            if !self.elided {
                self.unrecognised(event, "pending_opened");
            }
            return;
        };
        let id = format!("question:{pending_id}");
        // Asked again in its group (a resent fact): nothing changes, so the
        // verdict never goes back to open (the re-confirmation's N-2).
        if self.items.get(&id).is_some() {
            return;
        }
        let option_ids: Option<Vec<String>> = pending
            .and_then(|p| p.get("option_ids"))
            .and_then(|ids| serde_json::from_value(ids.clone()).ok());
        let payload = body.get("payload").unwrap_or(&Value::Null);
        let mut question = Question::asked(pending_id, kind, payload, option_ids.as_deref());
        question.answerable = answerable.answerable(pending_id);
        self.create(event, id, Body::Question(question), true);
    }

    /// A verdict event for a question of this group. One asked in an
    /// earlier group is not this fold's: its item is the collector's
    /// record's (`Question::from_pending`).
    fn question_changed(&mut self, event: &EventDto, answerable: &dyn Answerable) {
        let body = &event.body;
        let Some(pending_id) = str_at(body, "pending_id") else {
            if !self.elided {
                self.unrecognised(event, &event.kind);
            }
            return;
        };
        let reason = body
            .get("reason")
            .and_then(|r| serde_json::from_value::<PendingReason>(r.clone()).ok());
        let (state, answered, delivered) = match event.kind.as_str() {
            "answer_submitted" => (PendingState::Open, true, None),
            "answer_result" => (
                PendingState::Open,
                false,
                body.get("delivered").and_then(Value::as_bool),
            ),
            "pending_resolved" => match str_at(body, "resolution") {
                Some("delivered") => (PendingState::Delivered, false, None),
                Some("cancelled") => (PendingState::Cancelled, false, None),
                _ => (PendingState::Open, false, None),
            },
            _ => (PendingState::Cancelled, false, None),
        };
        let now = answerable.answerable(pending_id);
        self.change(event, &format!("question:{pending_id}"), |body| match body {
            Body::Question(question) => {
                let mut changed = question.absorb(state, reason, answered, delivered);
                changed |= question.answerable != now;
                question.answerable = now;
                changed
            }
            _ => false,
        });
    }

    fn unrecognised(&mut self, event: &EventDto, update_kind: &str) {
        let (raw, truncated) = json_text(&event.body, RAW_MAX_BYTES);
        let body = Body::Unrecognised(Unrecognised {
            update_kind: clip(update_kind, CODE_MAX_BYTES),
            raw,
            truncated,
        });
        self.create_ordinal(event, body);
    }

    /// Make an item whose id is its group and its place among the group's
    /// ordinal items.
    fn create_ordinal(&mut self, event: &EventDto, body: Body) -> Option<String> {
        let id = format!("{}:{}", self.group, self.ordinal);
        if !self.create(event, id.clone(), body, false) {
            return None;
        }
        self.ordinal += 1;
        Some(id)
    }

    /// Make an item, within the group's budget; past it, the group is
    /// elided instead (once, with a marker) and nothing is made.
    fn create(&mut self, event: &EventDto, id: String, body: Body, question: bool) -> bool {
        let item = Item {
            id: id.clone(),
            version: event.event_id,
            turn_id: self.turn.clone(),
            ts: event.ts.clone(),
            body,
        };
        let weight = weight(&item);
        // A question is made past the byte budget, and after the group is
        // elided, up to its own count: the operator must see what is asked.
        // Each weighs at most `QUESTION_MAX_BYTES`.
        let over = if question {
            (self.questions >= GROUP_MAX_QUESTIONS).then_some("questions")
        } else if self.elided {
            return false;
        } else if self.count >= GROUP_MAX_ITEMS {
            Some("items")
        } else {
            (self.bytes + weight > GROUP_MAX_BYTES).then_some("bytes")
        };
        if let Some(reason) = over {
            self.elide(event, reason);
            return false;
        }
        if question {
            self.questions += 1;
        } else {
            self.count += 1;
        }
        self.bytes += weight;
        self.items.upsert(item);
        self.touch(id);
        true
    }

    /// Change an item of the group with `f`, which says whether it changed
    /// anything; only a change raises its version.
    fn change(&mut self, event: &EventDto, id: &str, f: impl FnOnce(&mut Body) -> bool) {
        let Some(item) = self.items.get_mut(id) else {
            return;
        };
        let before = weight(item);
        if !f(&mut item.body) {
            return;
        }
        item.version = event.event_id;
        let after = weight(item);
        self.bytes = self.bytes.saturating_sub(before) + after;
        self.touch(id.to_string());
        if self.bytes > GROUP_MAX_BYTES {
            self.elide(event, "bytes");
        }
    }

    /// The group made more than the view holds: say so, once, and make or
    /// change nothing more in it but its questions.
    fn elide(&mut self, event: &EventDto, reason: &str) {
        if self.elided {
            return;
        }
        self.elided = true;
        let id = format!("{}:{}", self.group, self.ordinal);
        self.ordinal += 1;
        let marker = Marker {
            marker: MarkerKind::Elided,
            reason: Some(reason.to_string()),
            text: None,
            from: None,
            to: None,
            about_turn: None,
        };
        self.items.upsert(Item {
            id: id.clone(),
            version: event.event_id,
            turn_id: self.turn.clone(),
            ts: event.ts.clone(),
            body: Body::Marker(marker),
        });
        self.touch(id);
    }

    fn touch(&mut self, id: String) {
        if self.dirty_set.insert(id.clone()) {
            self.dirty.push(id.clone());
        }
        self.touched.push(id);
    }
}

/// Fold `events` (one session's, in `event_id` order, starting at a group's
/// start) into its items, in the order they first appeared.
pub fn fold<'a>(events: impl IntoIterator<Item = &'a EventDto>, answerable: &dyn Answerable) -> Vec<Item> {
    let mut fold = Fold::new();
    for event in events {
        fold.apply(event, answerable);
    }
    fold.take()
}
