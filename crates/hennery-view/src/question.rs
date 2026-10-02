//! Questions (frontend §6.3): the request parsed for its card, and where
//! its answer stands, folded monotonically.

use crate::cap::{
    CODE_MAX_BYTES, FIELD_OPTIONS_MAX, FIELDS_MAX, ID_MAX_BYTES, LABEL_MAX_BYTES, PERMISSION_OPTIONS_MAX,
    QUESTION_MAX_BYTES, clip,
};
use crate::item::{Field, FieldKind, FieldOption, Pairing, PairingKind, PermissionOption, Question, Request};
use hennery_proto::frames::{PendingKind, PendingReason};
use hennery_proto::rest::{PendingItem, PendingState};
use serde_json::Value;

/// A permission's request: its title, the tool call it asks about, and the
/// options among `option_ids`, the only answers the collector accepts
/// (ACP core §4.6). Without option ids nothing here can answer it.
pub fn permission(payload: &Value, option_ids: Option<&[String]>) -> Request {
    let title = payload
        .pointer("/toolCall/title")
        .or_else(|| payload.pointer("/_meta/permission/title"))
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(|t| clip(t, LABEL_MAX_BYTES));
    let tool_call_id = payload
        .pointer("/toolCall/toolCallId")
        .and_then(Value::as_str)
        .filter(|id| id.len() <= ID_MAX_BYTES)
        .map(str::to_string);
    let offered = payload.get("options").and_then(Value::as_array);
    let options: Vec<PermissionOption> = offered
        .map(|options| {
            options
                .iter()
                .filter_map(|o| {
                    let option_id = o.get("optionId")?.as_str()?;
                    option_ids?.iter().any(|id| id == option_id).then(|| PermissionOption {
                        option_id: option_id.to_string(),
                        name: clip(
                            o.get("name").and_then(Value::as_str).unwrap_or(option_id),
                            LABEL_MAX_BYTES,
                        ),
                        option_kind: clip(
                            o.get("kind").and_then(Value::as_str).unwrap_or_default(),
                            CODE_MAX_BYTES,
                        ),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // Past the cap none is offered, so no option (a reject among them) is
    // ever hidden from the others; an id past its cap is none the
    // collector accepts.
    let too_many = offered.is_some_and(|o| o.len() > PERMISSION_OPTIONS_MAX);
    let options = if too_many || options.iter().any(|o| o.option_id.len() > ID_MAX_BYTES) {
        Vec::new()
    } else {
        options
    };
    Request::Permission {
        title,
        tool_call_id,
        options,
    }
}

/// The options of a `oneOf` or `anyOf`: each `const` string, the value an
/// answer carries, with its title as its label. `None` when they cannot be
/// offered as they are: not a list, none, past the count, an entry with no
/// string `const` (the card would hide a choice), or a value past its cap
/// (the card never sends a value cut short).
fn options(raw: &Value) -> Option<Vec<FieldOption>> {
    let raw = raw
        .as_array()
        .filter(|o| !o.is_empty() && o.len() <= FIELD_OPTIONS_MAX)?;
    let mut out = Vec::new();
    for o in raw {
        let value = o.get("const")?.as_str()?;
        if value.len() > LABEL_MAX_BYTES {
            return None;
        }
        out.push(FieldOption {
            value: value.to_string(),
            label: o.get("title").and_then(Value::as_str).map(|t| clip(t, LABEL_MAX_BYTES)),
            description: o
                .get("description")
                .and_then(Value::as_str)
                .map(|d| clip(d, LABEL_MAX_BYTES)),
        });
    }
    Some(out)
}

/// One property of an elicitation's form: a single select (`oneOf` of
/// `const`s), a multi select (an array whose `items.anyOf` holds them), or
/// text; anything else is unsupported, and the card then offers only
/// decline and cancel.
fn field(key: &str, prop: &Value) -> Field {
    let label = clip(
        prop.get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .unwrap_or(key),
        LABEL_MAX_BYTES,
    );
    let hint = prop
        .get("description")
        .and_then(Value::as_str)
        .map(|h| clip(h, LABEL_MAX_BYTES));
    let pairing = pairing(prop);
    let unsupported = (FieldKind::Unsupported, Vec::new());
    let kind = prop.get("type").and_then(Value::as_str);
    let (field_kind, options) = if let Some(one_of) = prop.get("oneOf") {
        options(one_of).map_or(unsupported, |single| (FieldKind::Single, single))
    } else if kind == Some("array") {
        prop.pointer("/items/anyOf")
            .and_then(options)
            .map_or(unsupported, |multi| (FieldKind::Multi, multi))
    } else if kind == Some("string") && ["anyOf", "enum", "const"].iter().all(|k| prop.get(k).is_none()) {
        // Text only when nothing restricts the value: a card that typed
        // freely into an `enum` could send one the agent refuses.
        (FieldKind::Text, Vec::new())
    } else {
        unsupported
    };
    // A key past its cap is not one an answer can name.
    let field_kind = if key.len() > ID_MAX_BYTES {
        FieldKind::Unsupported
    } else {
        field_kind
    };
    Field {
        key: clip(key, ID_MAX_BYTES),
        label,
        hint,
        field_kind,
        options,
        pairing,
    }
}

/// The adapters' free-text companion of a select: Claude's
/// `_askUserQuestionCustomAnswer` answers instead of the select; Codex's
/// field with the `user_note` role adds to it, and any other role answers
/// instead.
fn pairing(prop: &Value) -> Option<Pairing> {
    let (with, kind) = match prop.pointer("/_meta/_askUserQuestionCustomAnswer/questionId") {
        Some(with) => (with, PairingKind::Exclusive),
        None => {
            let with = prop.pointer("/_meta/codex/questionId")?;
            let note = prop.pointer("/_meta/codex/role").and_then(Value::as_str) == Some("user_note");
            (
                with,
                if note {
                    PairingKind::Note
                } else {
                    PairingKind::Exclusive
                },
            )
        }
    };
    let with = with
        .as_str()
        // Past its cap it could equal another, real key cut short.
        .filter(|id| id.len() <= ID_MAX_BYTES)?;
    Some(Pairing {
        with: with.to_string(),
        kind,
    })
}

/// An elicitation's request: its message and its form's fields, in the
/// adapter's order. Only a form-mode request with every field supported can
/// be filled in.
pub fn elicitation(payload: &Value) -> Request {
    let message = clip(
        payload.get("message").and_then(Value::as_str).unwrap_or_default(),
        LABEL_MAX_BYTES,
    );
    if payload.get("mode").and_then(Value::as_str) != Some("form") {
        return Request::Elicitation {
            message,
            fields: Vec::new(),
            required: Vec::new(),
            form_supported: false,
        };
    }
    let props = payload
        .pointer("/requestedSchema/properties")
        .and_then(Value::as_object);
    // An id past its cap names no field an answer can fill.
    let required: Vec<String> = payload
        .pointer("/requestedSchema/required")
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .filter_map(Value::as_str)
                .filter(|k| k.len() <= ID_MAX_BYTES)
                .take(FIELDS_MAX)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let fields: Vec<Field> = props
        .map(|props| {
            props
                .iter()
                .take(FIELDS_MAX)
                .map(|(key, prop)| field(key, prop))
                .collect()
        })
        .unwrap_or_default();
    // A form past the cap is shown in part and never filled in part.
    let form_supported =
        props.is_none_or(|p| p.len() <= FIELDS_MAX) && fields.iter().all(|f| f.field_kind != FieldKind::Unsupported);
    Request::Elicitation {
        message,
        fields,
        required,
        form_supported,
    }
}

/// What a parsed request weighs held in memory, near enough: its strings'
/// bytes.
pub fn weight(request: &Request) -> usize {
    let opt = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    match request {
        Request::Permission {
            title,
            tool_call_id,
            options,
        } => {
            opt(title)
                + opt(tool_call_id)
                + options
                    .iter()
                    .map(|o| o.option_id.len() + o.name.len() + o.option_kind.len() + 16)
                    .sum::<usize>()
        }
        Request::Elicitation {
            message,
            fields,
            required,
            ..
        } => {
            message.len()
                + fields.iter().map(field_weight).sum::<usize>()
                + required.iter().map(|k| k.len() + 16).sum::<usize>()
        }
    }
}

fn field_weight(field: &Field) -> usize {
    let opt = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    field.key.len()
        + field.label.len()
        + opt(&field.hint)
        + field.pairing.as_ref().map_or(0, |p| p.with.len() + 16)
        + field
            .options
            .iter()
            .map(|o| o.value.len() + opt(&o.label) + opt(&o.description) + 16)
            .sum::<usize>()
        + 16
}

impl Question {
    /// A question just asked: open, unanswered.
    pub fn asked(pending_id: &str, kind: PendingKind, payload: &Value, option_ids: Option<&[String]>) -> Self {
        let mut request = match kind {
            PendingKind::Permission => permission(payload, option_ids),
            PendingKind::Elicitation => elicitation(payload),
        };
        if let Request::Elicitation {
            fields, form_supported, ..
        } = &mut request
            && fields.iter().map(field_weight).sum::<usize>() > QUESTION_MAX_BYTES
        {
            fields.clear();
            *form_supported = false;
        }
        Question {
            pending_id: pending_id.to_string(),
            question_kind: kind,
            request,
            answerable: false,
            state: PendingState::Open,
            reason: None,
            answered: false,
            delivered: None,
        }
    }

    /// The question as the collector holds it (`PendingItem`): the item
    /// stream and the page use it for a question asked before the events
    /// they fold. `answerable` is the server's: open with no answer queued.
    pub fn from_pending(pending: &PendingItem) -> Self {
        let mut question = Self::asked(
            &pending.pending_id,
            pending.kind,
            &pending.payload,
            pending.option_ids.as_deref(),
        );
        question.absorb(pending.state, pending.reason, pending.answered, pending.delivered);
        question.answerable = pending.state == PendingState::Open && !pending.answered;
        question
    }

    /// Take in what another account of this question says, never going
    /// back (frontend §6.3): the state leaves `open` once, with its reason;
    /// `answered` only becomes true; a delivered verdict sticks over a
    /// later undelivered one. An answer still waiting when the question is
    /// cancelled is never delivered: the collector marks it so with no
    /// event of its own. Whether anything changed.
    pub fn absorb(
        &mut self,
        state: PendingState,
        reason: Option<PendingReason>,
        answered: bool,
        delivered: Option<bool>,
    ) -> bool {
        let before = (self.state, self.reason, self.answered, self.delivered);
        let cancelled = self.state == PendingState::Open && state == PendingState::Cancelled;
        if self.state == PendingState::Open && state != PendingState::Open {
            self.state = state;
            self.reason = reason;
        }
        self.answered |= answered;
        self.delivered = match (self.delivered, delivered) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), _) | (_, Some(false)) => Some(false),
            (None, None) => None,
        };
        if cancelled && self.answered && self.delivered.is_none() {
            self.delivered = Some(false);
        }
        before != (self.state, self.reason, self.answered, self.delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_permission_offers_only_the_options_the_collector_accepts() {
        let payload = json!({
            "toolCall": {"toolCallId": "c1", "title": "Write a.txt"},
            "options": [
                {"optionId": "allow", "name": "Yes", "kind": "allow_once"},
                {"optionId": "sneaky", "name": "Also yes", "kind": "allow_always"},
                {"optionId": "reject", "name": "No", "kind": "reject_once"},
            ],
        });
        let ids = ["allow".to_string(), "reject".to_string()];
        let Request::Permission {
            title,
            tool_call_id,
            options,
        } = permission(&payload, Some(&ids))
        else {
            panic!("a permission");
        };
        assert_eq!(title.as_deref(), Some("Write a.txt"));
        assert_eq!(tool_call_id.as_deref(), Some("c1"));
        let ids: Vec<&str> = options.iter().map(|o| o.option_id.as_str()).collect();
        assert_eq!(ids, ["allow", "reject"]);
        // No option ids: nothing to answer with.
        let Request::Permission { options, .. } = permission(&payload, None) else {
            panic!("a permission");
        };
        assert!(options.is_empty());
    }

    #[test]
    fn an_elicitation_reads_single_multi_and_text_fields_in_order() {
        let payload = json!({
            "mode": "form",
            "message": "Pick",
            "requestedSchema": {"type": "object", "properties": {
                "zeta": {"type": "string", "title": "Colour", "oneOf": [{"const": "Red", "title": "Rouge", "description": "r"}, {"const": "Blue"}]},
                "zeta_custom": {"type": "string", "title": "Other", "_meta": {"_askUserQuestionCustomAnswer": {"questionId": "zeta"}}},
                "alpha": {"type": "array", "items": {"anyOf": [{"const": "A"}, {"const": "B"}]}},
                "note": {"type": "string", "_meta": {"codex": {"questionId": "alpha", "role": "user_note"}}},
                "other": {"type": "string", "_meta": {"codex": {"questionId": "alpha", "role": "answer"}}},
            }, "required": ["zeta", 7, "k".repeat(ID_MAX_BYTES + 1), "alpha"]},
        });
        let Request::Elicitation {
            message,
            fields,
            required,
            form_supported,
        } = elicitation(&payload)
        else {
            panic!("an elicitation");
        };
        assert_eq!(message, "Pick");
        assert!(form_supported);
        // Strings only, none past the id cap.
        assert_eq!(required, ["zeta", "alpha"]);
        let shape: Vec<_> = fields
            .iter()
            .map(|f| {
                let pairing = f.pairing.as_ref().map(|p| (p.with.as_str(), p.kind));
                (f.key.as_str(), f.field_kind, f.options.len(), pairing)
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("zeta", FieldKind::Single, 2, None),
                (
                    "zeta_custom",
                    FieldKind::Text,
                    0,
                    Some(("zeta", PairingKind::Exclusive))
                ),
                ("alpha", FieldKind::Multi, 2, None),
                ("note", FieldKind::Text, 0, Some(("alpha", PairingKind::Note))),
                ("other", FieldKind::Text, 0, Some(("alpha", PairingKind::Exclusive))),
            ]
        );
        assert_eq!(fields[0].label, "Colour");
        assert_eq!(fields[2].label, "alpha");
        // An option's title is its label.
        let labels: Vec<Option<&str>> = fields[0].options.iter().map(|o| o.label.as_deref()).collect();
        assert_eq!(labels, [Some("Rouge"), None]);
    }

    #[test]
    fn an_unsupported_field_or_mode_leaves_decline_only() {
        let form =
            |prop: Value| json!({"mode": "form", "message": "m", "requestedSchema": {"properties": {"f": prop}}});
        for payload in [
            form(json!({"type": "number"})),
            form(json!({"type": "array", "items": {}})),
            form(json!({"type": "array", "items": {"anyOf": []}})),
            // A select it cannot offer whole is not text either.
            form(json!({"type": "string", "oneOf": []})),
            form(json!({"type": "string", "oneOf": "Red"})),
            form(json!({"type": "string", "oneOf": [{"const": "Red"}, {"title": "no const"}]})),
            form(json!({"type": "string", "oneOf": [{"const": "Red"}, {"const": 2}]})),
            form(json!({"type": "array", "items": {"anyOf": [{"const": "A"}, {"title": "no const"}]}})),
            // A string restricted otherwise is not free text.
            form(json!({"type": "string", "enum": ["a", "b"]})),
            form(json!({"type": "string", "const": "a"})),
            form(json!({"type": "string", "anyOf": [{"const": "a"}]})),
            json!({"mode": "url", "message": "m", "url": "https://example.com"}),
        ] {
            let Request::Elicitation { form_supported, .. } = elicitation(&payload) else {
                panic!("an elicitation");
            };
            assert!(!form_supported, "{payload}");
        }
    }

    #[test]
    fn past_a_cap_nothing_is_answered_in_part() {
        // Too many permission options: none offered.
        let many: Vec<Value> = (0..PERMISSION_OPTIONS_MAX + 1)
            .map(|i| json!({"optionId": i.to_string(), "name": "n", "kind": "allow_once"}))
            .collect();
        let ids: Vec<String> = (0..PERMISSION_OPTIONS_MAX + 1).map(|i| i.to_string()).collect();
        let Request::Permission { options, .. } = permission(&json!({"options": many}), Some(&ids)) else {
            panic!("a permission");
        };
        assert!(options.is_empty());
        // Too many fields, a field with too many options, a value past its
        // cap: the form cannot be filled.
        let mut props = serde_json::Map::new();
        for i in 0..FIELDS_MAX + 1 {
            props.insert(format!("f{i}"), json!({"type": "string"}));
        }
        let many_options: Vec<Value> = (0..FIELD_OPTIONS_MAX + 1)
            .map(|i| json!({"const": i.to_string()}))
            .collect();
        let long = "v".repeat(LABEL_MAX_BYTES + 1);
        for (payload, fields_shown) in [
            (
                json!({"mode": "form", "requestedSchema": {"properties": props}}),
                FIELDS_MAX,
            ),
            (
                json!({"mode": "form", "requestedSchema": {"properties": {"c": {"type": "string", "oneOf": many_options}}}}),
                1,
            ),
            (
                json!({"mode": "form", "requestedSchema": {"properties": {"c": {"type": "string", "oneOf": [{"const": long}]}}}}),
                1,
            ),
        ] {
            let Request::Elicitation {
                fields, form_supported, ..
            } = elicitation(&payload)
            else {
                panic!("an elicitation");
            };
            assert!(!form_supported);
            assert_eq!(fields.len(), fields_shown);
        }
    }

    #[test]
    fn the_verdict_never_goes_back() {
        let mut q = Question::asked("p", PendingKind::Permission, &json!({}), None);
        assert!(q.absorb(PendingState::Open, None, true, None));
        assert!(q.absorb(PendingState::Open, None, false, Some(true)));
        // A later "not delivered" does not undo a delivery.
        assert!(!q.absorb(PendingState::Open, None, false, Some(false)));
        assert_eq!(q.delivered, Some(true));
        assert!(q.absorb(PendingState::Cancelled, Some(PendingReason::SessionParked), false, None));
        // Once over, it stays as it ended.
        assert!(!q.absorb(PendingState::Delivered, None, false, None));
        assert_eq!(
            (q.state, q.reason),
            (PendingState::Cancelled, Some(PendingReason::SessionParked))
        );
        assert!(q.answered);
        // Undelivered can still become delivered.
        let mut q = Question::asked("p", PendingKind::Permission, &json!({}), None);
        q.absorb(PendingState::Open, None, true, Some(false));
        q.absorb(PendingState::Open, None, false, Some(true));
        assert_eq!(q.delivered, Some(true));
    }

    #[test]
    fn an_answer_waiting_when_its_question_is_cancelled_is_not_delivered() {
        let mut q = Question::asked("p", PendingKind::Permission, &json!({}), None);
        q.absorb(PendingState::Open, None, true, None);
        assert!(q.absorb(PendingState::Cancelled, Some(PendingReason::SessionParked), false, None));
        assert_eq!(q.delivered, Some(false));
        // Unanswered, or resolved as delivered: no verdict is made up.
        let mut q = Question::asked("p", PendingKind::Permission, &json!({}), None);
        q.absorb(PendingState::Cancelled, Some(PendingReason::AgentWithdrew), false, None);
        assert_eq!(q.delivered, None);
        let mut q = Question::asked("p", PendingKind::Permission, &json!({}), None);
        q.absorb(PendingState::Open, None, true, None);
        q.absorb(PendingState::Delivered, None, false, None);
        assert_eq!(q.delivered, None);
    }
}
