//! The fold's rules (frontend §6.1, §6.3; client view spec §3), one by
//! one, on events written by hand: the cases the recordings do not hold.

use hennery_proto::frames::{PendingKind, PendingReason};
use hennery_proto::rest::{EventDto, PendingItem, PendingState, StoredBlock};
use hennery_view::cap::{
    GROUP_MAX_BYTES, GROUP_MAX_ITEMS, GROUP_MAX_QUESTIONS, ID_MAX_BYTES, LABEL_MAX_BYTES, PLAN_MAX_ENTRIES,
    RAW_MAX_BYTES, TEXT_MAX_BYTES,
};
use hennery_view::{Body, Fold, Item, Marker, MarkerKind, Pairing, PairingKind, Question, Request, fold};
use serde_json::{Value, json};

fn ev(id: i64, kind: &str, body: Value) -> EventDto {
    EventDto {
        event_id: id,
        session_id: "s".into(),
        host_seq: None,
        kind: kind.into(),
        body,
        ts: format!("2026-10-02T08:00:{:02}.{:03}Z", id / 1000 % 60, id % 1000),
    }
}

fn acp(id: i64, update: Value) -> EventDto {
    ev(
        id,
        "acp_update",
        json!({"kind": "acp_update", "indexed": {}, "payload": {"sessionId": "a", "update": update}}),
    )
}

fn chunk(id: i64, kind: &str, text: &str) -> EventDto {
    acp(
        id,
        json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}),
    )
}

fn turn(id: i64, turn_id: &str) -> EventDto {
    ev(
        id,
        "user_turn",
        json!({"turn_id": turn_id, "content": [{"type": "text", "text": "go"}]}),
    )
}

fn none(_: &str) -> bool {
    false
}

fn kinds(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .map(|i| serde_json::to_value(i).unwrap()["kind"].as_str().unwrap().to_string())
        .collect()
}

fn text(item: &Item) -> &str {
    match &item.body {
        Body::Message(t) | Body::Thinking(t) => &t.text,
        other => panic!("not text: {other:?}"),
    }
}

#[test]
fn chunks_merge_keeping_the_first_timestamp_until_another_item() {
    let events = [
        turn(1, "t1"),
        chunk(2, "agent_message_chunk", "Hel"),
        // Silent updates do not break a run.
        acp(3, json!({"sessionUpdate": "usage_update", "used": 1, "size": 2})),
        ev(
            4,
            "git_state",
            json!({"kind": "git_state", "dirty": false, "worktree": false}),
        ),
        chunk(5, "agent_message_chunk", "lo"),
        // A thought is another run.
        chunk(6, "agent_thought_chunk", "hmm"),
        chunk(7, "agent_thought_chunk", "!"),
        chunk(8, "agent_message_chunk", "a"),
        acp(
            9,
            json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Run"}),
        ),
        chunk(10, "agent_message_chunk", "b"),
    ];
    let items = fold(&events, &none);
    assert_eq!(
        kinds(&items),
        ["user_turn", "message", "thinking", "message", "tool_call", "message"]
    );
    assert_eq!(text(&items[1]), "Hello");
    assert_eq!((items[1].ts.as_str(), items[1].version), (events[1].ts.as_str(), 5));
    assert_eq!(text(&items[2]), "hmm!");
    assert_eq!(text(&items[3]), "a");
    assert_eq!(text(&items[5]), "b");
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, ["t1:0", "t1:1", "t1:2", "t1:3", "t1:tool:c1", "t1:4"]);
    assert!(items.iter().all(|i| i.turn_id.as_deref() == Some("t1")));
}

#[test]
fn an_empty_chunk_changes_nothing_and_one_not_text_is_unrecognised() {
    let events = [
        chunk(1, "agent_message_chunk", "a"),
        chunk(2, "agent_message_chunk", ""),
        acp(
            3,
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "image", "data": "QQ==", "mimeType": "image/png"}}),
        ),
    ];
    let items = fold(&events, &none);
    assert_eq!(kinds(&items), ["message", "unrecognised"]);
    assert_eq!(items[0].version, 1);
    let Body::Unrecognised(u) = &items[1].body else {
        panic!()
    };
    assert_eq!(u.update_kind, "acp_update/agent_message_chunk");
}

#[test]
fn before_the_first_turn_items_belong_to_the_preamble() {
    let items = fold(
        &[
            ev(1, "session_started", json!({"kind": "session_started"})),
            ev(
                2,
                "host_note",
                json!({"kind": "host_note", "note": "replay_unknown_dropped", "text": "3 dropped"}),
            ),
            turn(3, "t1"),
        ],
        &none,
    );
    assert_eq!(items[0].id, "start:0");
    assert_eq!(items[0].turn_id, None);
    assert_eq!(items[1].id, "t1:0");
}

#[test]
fn a_tool_call_merges_at_its_first_place_and_keeps_its_first_timestamp() {
    let events = [
        turn(1, "t1"),
        acp(
            2,
            json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Preparing", "rawInput": {}, "status": "pending"}),
        ),
        chunk(3, "agent_message_chunk", "x"),
        acp(
            4,
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "title": "Write a", "rawInput": {"p": 1}}),
        ),
        // Sparser: no title, no input; the output arrives.
        acp(
            5,
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed", "rawOutput": "ok"}),
        ),
    ];
    let items = fold(&events, &none);
    assert_eq!(kinds(&items), ["user_turn", "tool_call", "message"]);
    let tool = &items[1];
    assert_eq!((tool.ts.as_str(), tool.version), (events[1].ts.as_str(), 5));
    let Body::ToolCall(call) = &tool.body else { panic!() };
    assert_eq!(call.title.as_deref(), Some("Write a"));
    assert_eq!(call.input, Some(json!({"p": 1})));
    assert_eq!(
        (call.status.as_deref(), call.output.as_deref()),
        (Some("completed"), Some("ok"))
    );
}

#[test]
fn a_tool_update_in_a_later_turn_is_an_item_of_that_turn() {
    let items = fold(
        &[
            turn(1, "t1"),
            acp(
                2,
                json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Run", "status": "in_progress"}),
            ),
            turn(3, "t2"),
            acp(
                4,
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed"}),
            ),
        ],
        &none,
    );
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, ["t1:0", "t1:tool:c1", "t2:0", "t2:tool:c1"]);
    let Body::ToolCall(first) = &items[1].body else {
        panic!()
    };
    assert_eq!(first.status.as_deref(), Some("in_progress"));
}

#[test]
fn a_tool_update_that_changes_nothing_keeps_its_version() {
    let mut fold_state = Fold::new();
    fold_state.apply(&turn(1, "t1"), &none);
    fold_state.apply(
        &acp(
            2,
            json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Run"}),
        ),
        &none,
    );
    fold_state.take();
    assert!(!fold_state.apply(
        &acp(
            3,
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "title": "Run"})
        ),
        &none
    ));
    assert!(fold_state.take().is_empty());
}

#[test]
fn the_plan_is_the_latest_snapshot_of_its_turn() {
    let plan = |id, steps: &[&str]| {
        let entries: Vec<Value> = steps
            .iter()
            .map(|s| json!({"content": s, "priority": "high", "status": "pending"}))
            .collect();
        acp(id, json!({"sessionUpdate": "plan", "entries": entries}))
    };
    let items = fold(
        &[
            turn(1, "t1"),
            plan(2, &["a", "b"]),
            chunk(3, "agent_message_chunk", "x"),
            plan(4, &["a", "b", "c"]),
            turn(5, "t2"),
            plan(6, &["d"]),
            acp(7, json!({"sessionUpdate": "plan", "entries": "not a list"})),
        ],
        &none,
    );
    assert_eq!(
        kinds(&items),
        ["user_turn", "plan", "message", "user_turn", "plan", "unrecognised"]
    );
    let steps = |item: &Item| match &item.body {
        Body::Plan { entries, .. } => entries.iter().map(|e| e.content.clone()).collect::<Vec<_>>(),
        _ => panic!(),
    };
    assert_eq!(items[1].id, "t1:plan");
    assert_eq!(steps(&items[1]), ["a", "b", "c"]);
    assert_eq!(steps(&items[4]), ["d"]);
}

#[test]
fn a_plan_keeps_every_step_and_says_when_it_was_cut() {
    let plan = |entries: Vec<Value>| {
        let items = fold(&[acp(1, json!({"sessionUpdate": "plan", "entries": entries}))], &none);
        match &items[0].body {
            Body::Plan { entries, truncated } => (entries.clone(), *truncated),
            other => panic!("{other:?}"),
        }
    };
    // A step with no text keeps its place.
    let (entries, truncated) = plan(vec![
        json!({"content": "a"}),
        json!({"status": "pending"}),
        json!({"content": 3}),
        json!({"content": "d"}),
    ]);
    let steps: Vec<&str> = entries.iter().map(|e| e.content.as_str()).collect();
    assert_eq!(steps, ["a", "", "", "d"]);
    assert_eq!(entries[1].status.as_deref(), Some("pending"));
    assert!(!truncated);
    // Past the step count, or a step past its cap: cut, and said so.
    let (entries, truncated) = plan(vec![json!({"content": "s"}); PLAN_MAX_ENTRIES + 1]);
    assert_eq!((entries.len(), truncated), (PLAN_MAX_ENTRIES, true));
    let (entries, truncated) = plan(vec![json!({"content": "s".repeat(LABEL_MAX_BYTES + 1)})]);
    assert_eq!((entries[0].content.len(), truncated), (LABEL_MAX_BYTES, true));
    let (_, truncated) = plan(vec![json!({"content": "s"}); PLAN_MAX_ENTRIES]);
    assert!(!truncated);
}

#[test]
fn every_collector_and_host_divider_is_a_marker() {
    let marker = |marker, reason: Option<&str>, text: Option<&str>| Marker {
        marker,
        reason: reason.map(str::to_string),
        text: text.map(str::to_string),
        from: None,
        to: None,
        about_turn: None,
    };
    let cases = [
        (
            "session_parked",
            json!({"kind": "session_parked", "reason": "idle"}),
            marker(MarkerKind::Parked, Some("idle"), None),
        ),
        ("operator_resumed", json!({}), marker(MarkerKind::Resumed, None, None)),
        ("operator_closed", json!({}), marker(MarkerKind::Closed, None, None)),
        (
            "host_restarted",
            json!({}),
            marker(MarkerKind::HostRestarted, None, None),
        ),
        (
            "presumed_parked",
            json!({"reason": "host_revoked"}),
            marker(MarkerKind::HostOffline, Some("host_revoked"), None),
        ),
        ("reattached", json!({}), marker(MarkerKind::HostBack, None, None)),
        (
            "turn_ended_synthesized",
            json!({"turn_id": "t", "outcome": "interrupted"}),
            marker(MarkerKind::TurnInterrupted, None, None),
        ),
        (
            "turn_ended",
            json!({"kind": "turn_ended", "turn_id": "t", "outcome": "interrupted"}),
            marker(MarkerKind::TurnInterrupted, None, None),
        ),
        (
            "turn_ended",
            json!({"kind": "turn_ended", "turn_id": "t", "outcome": "cancelled"}),
            marker(MarkerKind::TurnCancelled, None, None),
        ),
        (
            "turn_ended",
            json!({"kind": "turn_ended", "turn_id": "t", "outcome": "failed", "error": "boom"}),
            marker(MarkerKind::TurnFailed, None, Some("boom")),
        ),
        (
            "turn_not_delivered",
            json!({"turn_id": "t"}),
            Marker {
                about_turn: Some("t".into()),
                ..marker(MarkerKind::TurnNotDelivered, None, None)
            },
        ),
        // A turn id past its cap names no turn.
        (
            "turn_not_delivered",
            json!({"turn_id": "t".repeat(ID_MAX_BYTES + 1)}),
            marker(MarkerKind::TurnNotDelivered, None, None),
        ),
        (
            "start_not_delivered",
            json!({}),
            marker(MarkerKind::StartNotDelivered, None, None),
        ),
        (
            "start_failed",
            json!({"kind": "start_failed", "request_id": "r", "code": "spawn_failed", "message": "no node"}),
            marker(MarkerKind::StartFailed, Some("spawn_failed"), Some("no node")),
        ),
        (
            "adapter_exited",
            json!({"kind": "adapter_exited", "code": 1, "stderr_tail": "panic"}),
            marker(MarkerKind::AdapterExited, Some("code 1"), Some("panic")),
        ),
        (
            "adapter_exited",
            json!({"kind": "adapter_exited", "signal": 9, "stderr_tail": ""}),
            marker(MarkerKind::AdapterExited, Some("signal 9"), Some("")),
        ),
        (
            "transcript_gap",
            json!({"kind": "transcript_gap", "from_seq": 4, "to_seq": 9}),
            marker(MarkerKind::TranscriptGap, Some("4..9"), None),
        ),
        (
            "host_note",
            json!({"kind": "host_note", "note": "config_failed", "text": "model not applied"}),
            marker(MarkerKind::HostNote, Some("config_failed"), Some("model not applied")),
        ),
        (
            "conflict",
            json!({"seq": 7, "received": {"kind": "session_closed"}}),
            marker(MarkerKind::Conflict, Some("7"), Some(r#"{"kind":"session_closed"}"#)),
        ),
        (
            "hat_reassigned",
            json!({"from": "hat-a", "to": "hat-b"}),
            Marker {
                from: Some("hat-a".into()),
                to: Some("hat-b".into()),
                ..marker(MarkerKind::HatReassigned, None, None)
            },
        ),
    ];
    for (kind, body, expected) in cases {
        let items = fold(&[turn(1, "t1"), ev(2, kind, body.clone())], &none);
        assert_eq!(items.len(), 2, "{kind} {body}");
        assert_eq!(items[1].body, Body::Marker(expected), "{kind} {body}");
    }
}

#[test]
fn the_sessions_own_state_makes_no_item() {
    for (kind, body) in [
        (
            "session_started",
            json!({"kind": "session_started", "request_id": "r", "agent_session_id": "a"}),
        ),
        (
            "turn_started",
            json!({"kind": "turn_started", "request_id": "r", "turn_id": "t"}),
        ),
        ("config_applied", json!({"kind": "config_applied", "request_id": "r"})),
        (
            "git_state",
            json!({"kind": "git_state", "dirty": true, "worktree": false}),
        ),
        ("session_closed", json!({"kind": "session_closed"})),
        ("operator_parked", json!({})),
        (
            "turn_ended",
            json!({"kind": "turn_ended", "turn_id": "t", "outcome": "completed"}),
        ),
    ] {
        assert_eq!(fold(&[ev(1, kind, body)], &none), [], "{kind}");
    }
    for update in [
        "available_commands_update",
        "usage_update",
        "session_info_update",
        "current_mode_update",
        "config_option_update",
    ] {
        assert_eq!(fold(&[acp(1, json!({"sessionUpdate": update}))], &none), [], "{update}");
    }
}

#[test]
fn a_user_turn_carries_its_images_as_attachments() {
    let sha = "a".repeat(64);
    let items = fold(
        &[ev(
            1,
            "user_turn",
            json!({"turn_id": "t1", "content": [
                {"type": "text", "text": "what is this?"},
                {"type": "image", "mimeType": "image/png", "sha256": sha, "size": 75},
            ]}),
        )],
        &none,
    );
    let Body::UserTurn { content, truncated } = &items[0].body else {
        panic!()
    };
    assert!(!truncated);
    assert_eq!(
        content[1],
        StoredBlock::Image {
            mime_type: "image/png".into(),
            sha256: sha,
            size: 75
        }
    );
}

#[test]
fn a_user_turn_it_cannot_read_still_starts_its_turn() {
    let items = fold(
        &[
            chunk(1, "agent_message_chunk", "a"),
            ev(2, "user_turn", json!({"turn_id": "t1", "content": "not blocks"})),
            chunk(3, "agent_message_chunk", "b"),
        ],
        &none,
    );
    assert_eq!(kinds(&items), ["message", "unrecognised", "message"]);
    assert_eq!(items[1].id, "t1:0");
    assert_eq!(items[2].id, "t1:1");
}

fn opened(id: i64, pending: &str, kind: &str) -> EventDto {
    ev(
        id,
        "pending_opened",
        json!({"kind": "pending_opened", "pending_id": pending,
               "indexed": {"pending": {"id": pending, "kind": kind, "option_ids": ["yes", "no"]}},
               "payload": {"toolCall": {"toolCallId": "c1", "title": "Write"},
                           "options": [{"optionId": "yes", "name": "Yes", "kind": "allow_once"},
                                       {"optionId": "no", "name": "No", "kind": "reject_once"}]}}),
    )
}

fn question(item: &Item) -> &Question {
    match &item.body {
        Body::Question(q) => q,
        other => panic!("not a question: {other:?}"),
    }
}

#[test]
fn a_questions_verdict_is_monotonic_and_answerable_comes_from_the_pending_set() {
    let open = |id: &str| id == "p1";
    let events = [
        turn(1, "t1"),
        opened(2, "p1", "permission"),
        ev(
            3,
            "answer_submitted",
            json!({"pending_id": "p1", "request_id": "r", "answer": {"option_id": "yes"}}),
        ),
        ev(
            4,
            "answer_result",
            json!({"kind": "answer_result", "pending_id": "p1", "request_id": "r", "delivered": true}),
        ),
        ev(
            5,
            "answer_result",
            json!({"kind": "answer_result", "pending_id": "p1", "request_id": "r", "delivered": false}),
        ),
        ev(
            6,
            "pending_resolved",
            json!({"kind": "pending_resolved", "pending_id": "p1", "resolution": "delivered"}),
        ),
        ev(
            7,
            "pending_cancelled",
            json!({"pending_id": "p1", "reason": "session_parked"}),
        ),
        // A verdict on a question asked before this fold: not its item.
        ev(
            8,
            "answer_result",
            json!({"kind": "answer_result", "pending_id": "elsewhere", "request_id": "r", "delivered": true}),
        ),
    ];
    let mut fold_state = Fold::new();
    let mut versions = Vec::new();
    for event in &events {
        fold_state.apply(event, &open);
        versions.extend(
            fold_state
                .take()
                .into_iter()
                .filter(|i| i.id == "question:p1")
                .map(|i| i.version),
        );
    }
    // The second verdict and the late cancel change nothing.
    assert_eq!(versions, [2, 3, 4, 6]);
    let items = fold(&events, &open);
    assert_eq!(items.len(), 2);
    let q = question(&items[1]);
    assert_eq!(items[1].id, "question:p1");
    assert_eq!(items[1].turn_id.as_deref(), Some("t1"));
    assert!(q.answerable, "from the set, not from the events");
    assert_eq!(
        (q.state, q.reason, q.answered, q.delivered),
        (PendingState::Delivered, None, true, Some(true))
    );
    let Request::Permission {
        options, tool_call_id, ..
    } = &q.request
    else {
        panic!()
    };
    assert_eq!(tool_call_id.as_deref(), Some("c1"));
    assert_eq!(options.len(), 2);
    // Withdrawn: cancelled with its reason.
    let items = fold(
        &[
            opened(1, "p2", "elicitation"),
            ev(
                2,
                "pending_resolved",
                json!({"kind": "pending_resolved", "pending_id": "p2", "resolution": "cancelled", "reason": "agent_withdrew"}),
            ),
        ],
        &none,
    );
    let q = question(&items[0]);
    assert_eq!(
        (q.state, q.reason, q.answerable),
        (PendingState::Cancelled, Some(PendingReason::AgentWithdrew), false)
    );
}

#[test]
fn a_question_from_the_collectors_record_matches_the_fold() {
    let events = [opened(1, "p1", "permission")];
    let folded = question(&fold(&events, &none)[0]).clone();
    let pending = PendingItem {
        pending_id: "p1".into(),
        session_id: "s".into(),
        kind: PendingKind::Permission,
        state: PendingState::Open,
        reason: None,
        turn_id: None,
        option_ids: Some(vec!["yes".into(), "no".into()]),
        payload: events[0].body["payload"].clone(),
        answered: false,
        delivered: None,
    };
    let mut from_record = Question::from_pending(&pending);
    assert!(from_record.answerable, "open and unanswered");
    from_record.answerable = false;
    assert_eq!(from_record, folded);
    let answered = PendingItem {
        answered: true,
        delivered: Some(true),
        ..pending.clone()
    };
    assert!(!Question::from_pending(&answered).answerable);
    // Answered, then cancelled before the host took the answer: the
    // collector marks it undelivered with no event, and the fold agrees.
    let events = [
        opened(1, "p1", "permission"),
        ev(
            2,
            "answer_submitted",
            json!({"pending_id": "p1", "request_id": "r", "answer": {"option_id": "yes"}}),
        ),
        ev(
            3,
            "pending_cancelled",
            json!({"pending_id": "p1", "reason": "session_parked"}),
        ),
    ];
    let folded = question(&fold(&events, &none)[0]).clone();
    let cancelled = PendingItem {
        state: PendingState::Cancelled,
        reason: Some(PendingReason::SessionParked),
        answered: true,
        delivered: Some(false),
        ..pending
    };
    assert_eq!(Question::from_pending(&cancelled), folded);
}

#[test]
fn what_the_fold_does_not_know_is_shown_never_dropped() {
    let hostile =
        json!({"sessionUpdate": "brand_new", "html": "<script>alert(1)</script>", "pad": "x".repeat(RAW_MAX_BYTES)});
    let events = [
        acp(
            1,
            json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "hi"}}),
        ),
        acp(2, hostile),
        ev(3, "operator_renamed", json!({"title": "new"})),
        acp(4, json!({"sessionUpdate": "tool_call"})),
        ev(5, "acp_update", json!({"kind": "acp_update"})),
        ev(6, "pending_opened", json!({"kind": "pending_opened"})),
        // An id past its cap is refused, not cut (decision 10, N-4).
        acp(
            7,
            json!({"sessionUpdate": "tool_call", "toolCallId": "c".repeat(ID_MAX_BYTES + 1)}),
        ),
        opened(8, &"p".repeat(ID_MAX_BYTES + 1), "permission"),
    ];
    let items = fold(&events, &none);
    let unknown: Vec<(&str, bool)> = items
        .iter()
        .map(|i| match &i.body {
            Body::Unrecognised(u) => (u.update_kind.as_str(), u.truncated),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        unknown,
        [
            ("acp_update/user_message_chunk", false),
            ("acp_update/brand_new", true),
            ("operator_renamed", false),
            ("acp_update/tool_call", false),
            ("acp_update", false),
            ("pending_opened", false),
            ("acp_update/tool_call", false),
            ("pending_opened", false),
        ]
    );
    // The body as text, never a value; cut at its cap.
    let json = serde_json::to_value(&items[1]).unwrap();
    let raw = json["raw"].as_str().expect("raw is a string");
    assert_eq!(raw.len(), RAW_MAX_BYTES);
    assert!(raw.contains("<script>"));
}

#[test]
fn a_message_past_its_cap_is_cut_once_and_then_changes_nothing() {
    let half = "x".repeat(TEXT_MAX_BYTES / 2 + 1);
    let mut fold_state = Fold::new();
    fold_state.apply(&chunk(1, "agent_message_chunk", &half), &none);
    fold_state.apply(&chunk(2, "agent_message_chunk", &half), &none);
    let cut = fold_state.take();
    let Body::Message(m) = &cut[0].body else { panic!() };
    assert!(m.truncated);
    assert_eq!((m.text.len(), cut[0].version), (TEXT_MAX_BYTES, 2));
    assert!(!fold_state.apply(&chunk(3, "agent_message_chunk", "more"), &none));
    assert!(fold_state.take().is_empty());
}

#[test]
fn a_turn_past_its_item_budget_is_elided_once_but_its_questions_are_not() {
    let mut events = vec![turn(1, "t1")];
    let mut id = 2;
    for _ in 0..GROUP_MAX_ITEMS + 5 {
        events.push(ev(id, "brand_new_kind", json!({})));
        id += 1;
    }
    events.push(opened(id, "p1", "permission"));
    events.push(turn(id + 1, "t2"));
    events.push(ev(id + 2, "brand_new_kind", json!({})));
    let items = fold(&events, &none);
    let elided: Vec<&Item> = items
        .iter()
        .filter(|i| matches!(&i.body, Body::Marker(m) if m.marker == MarkerKind::Elided))
        .collect();
    assert_eq!(elided.len(), 1);
    let Body::Marker(m) = &elided[0].body else { panic!() };
    assert_eq!(m.reason.as_deref(), Some("items"));
    let in_t1 = items.iter().filter(|i| i.turn_id.as_deref() == Some("t1")).count();
    // The turn's prompt and its budget's items, the marker, the question.
    assert_eq!(in_t1, GROUP_MAX_ITEMS + 2);
    assert!(items.iter().any(|i| i.id == "question:p1"));
    // The next turn starts afresh.
    assert_eq!(items.iter().filter(|i| i.turn_id.as_deref() == Some("t2")).count(), 2);
}

#[test]
fn a_turn_past_its_byte_budget_is_elided() {
    // Items made big: each unknown update's text at its cap.
    let mut events = vec![turn(1, "t1")];
    let pad = "x".repeat(RAW_MAX_BYTES);
    let made = GROUP_MAX_BYTES / RAW_MAX_BYTES + 2;
    for i in 0..made as i64 {
        events.push(ev(i + 2, "brand_new_kind", json!({"pad": pad})));
    }
    let items = fold(&events, &none);
    assert!(made < GROUP_MAX_ITEMS);
    let last = items.last().unwrap();
    assert!(
        matches!(&last.body, Body::Marker(m) if m.marker == MarkerKind::Elided && m.reason.as_deref() == Some("bytes"))
    );
    // An item weighs 64, its id, its timestamp and its turn id, and its
    // kind's strings: the prompt its text and 16, an unknown update its
    // kind and its text. Each is made while the turn stays within budget.
    let ts = events[0].ts.len();
    let mut bytes = 64 + "t1:0".len() + ts + "t1".len() + "go".len() + 16;
    let mut kept = 0;
    loop {
        let id = format!("t1:{}", kept + 1);
        let weight = 64 + id.len() + ts + "t1".len() + "brand_new_kind".len() + RAW_MAX_BYTES;
        if bytes + weight > GROUP_MAX_BYTES {
            break;
        }
        bytes += weight;
        kept += 1;
    }
    assert!(kept < made);
    // The prompt, the updates within budget, the marker.
    assert_eq!(items.len(), 1 + kept + 1);
}

#[test]
fn a_question_past_its_turns_count_is_elided() {
    let mut events = vec![turn(1, "t1")];
    for i in 0..GROUP_MAX_QUESTIONS as i64 + 1 {
        events.push(opened(i + 2, &format!("p{i}"), "permission"));
    }
    let items = fold(&events, &none);
    let questions = items.iter().filter(|i| matches!(i.body, Body::Question(_))).count();
    assert_eq!(questions, GROUP_MAX_QUESTIONS);
    assert!(matches!(&items.last().unwrap().body, Body::Marker(m) if m.reason.as_deref() == Some("questions")));
}

#[test]
fn a_new_turn_keeps_only_its_own_items_in_memory() {
    let mut fold_state = Fold::new();
    for event in [
        turn(1, "t1"),
        chunk(2, "agent_message_chunk", "a"),
        acp(3, json!({"sessionUpdate": "tool_call", "toolCallId": "c1"})),
        turn(4, "t2"),
    ] {
        fold_state.apply(&event, &none);
    }
    assert_eq!(fold_state.current().len(), 1);
    assert_eq!(fold_state.turn(), Some("t2"));
    // What changed in the ended turn is still handed out once.
    assert_eq!(fold_state.take().len(), 4);
}

#[test]
fn an_empty_chunk_starts_no_message() {
    assert_eq!(
        fold(&[turn(1, "t1"), chunk(2, "agent_message_chunk", "")], &none).len(),
        1
    );
}

#[test]
fn a_question_asked_again_in_its_turn_changes_nothing() {
    let mut fold_state = Fold::new();
    fold_state.apply(&opened(1, "p1", "permission"), &none);
    fold_state.apply(
        &ev(
            2,
            "answer_submitted",
            json!({"pending_id": "p1", "request_id": "r", "answer": {"option_id": "yes"}}),
        ),
        &none,
    );
    fold_state.take();
    assert!(!fold_state.apply(&opened(3, "p1", "permission"), &none));
    let items = fold(
        &[
            opened(1, "p1", "permission"),
            ev(
                2,
                "answer_submitted",
                json!({"pending_id": "p1", "request_id": "r", "answer": {"option_id": "yes"}}),
            ),
            opened(3, "p1", "permission"),
        ],
        &none,
    );
    assert_eq!(items.len(), 1);
    assert!(question(&items[0]).answered);
}

#[test]
fn answerable_is_read_again_on_every_verdict() {
    let answered = std::cell::Cell::new(false);
    let open = |id: &str| id == "p1" && !answered.get();
    let mut fold_state = Fold::new();
    fold_state.apply(&opened(1, "p1", "permission"), &open);
    assert!(question(&fold_state.take()[0]).answerable);
    // Another device answered: the set no longer holds it.
    answered.set(true);
    fold_state.apply(
        &ev(
            2,
            "answer_submitted",
            json!({"pending_id": "p1", "request_id": "r", "answer": {"option_id": "yes"}}),
        ),
        &open,
    );
    let item = fold_state.take();
    assert!(!question(&item[0]).answerable);
    // A verdict that changes nothing but whether it can be answered still
    // raises the version.
    let still = std::cell::Cell::new(true);
    let flips = |_: &str| still.get();
    let mut fold_state = Fold::new();
    fold_state.apply(&opened(1, "p1", "permission"), &flips);
    fold_state.take();
    still.set(false);
    fold_state.apply(
        &ev(
            2,
            "answer_result",
            json!({"kind": "answer_result", "pending_id": "p1", "request_id": "r"}),
        ),
        &flips,
    );
    let item = fold_state.take();
    assert_eq!((item[0].version, question(&item[0]).answerable), (2, false));
}

#[test]
fn an_elided_turn_changes_its_items_no_more() {
    let mut events = vec![
        turn(1, "t1"),
        acp(
            2,
            json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "status": "in_progress"}),
        ),
    ];
    let mut id = 3;
    for _ in 0..GROUP_MAX_ITEMS {
        events.push(ev(id, "brand_new_kind", json!({})));
        id += 1;
    }
    events.push(acp(
        id,
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed"}),
    ));
    let items = fold(&events, &none);
    let tool = items.iter().find(|i| i.id == "t1:tool:c1").unwrap();
    assert_eq!(tool.version, 2);
}

#[test]
fn growing_items_past_the_byte_budget_elides_the_turn() {
    let mut events = vec![turn(1, "t1")];
    let calls = GROUP_MAX_BYTES / TEXT_MAX_BYTES + 1;
    for i in 0..calls as i64 {
        events.push(acp(
            i + 2,
            json!({"sessionUpdate": "tool_call", "toolCallId": format!("c{i}")}),
        ));
    }
    let output = "o".repeat(TEXT_MAX_BYTES);
    for i in 0..calls as i64 {
        events.push(acp(
            1000 + i,
            json!({"sessionUpdate": "tool_call_update", "toolCallId": format!("c{i}"), "rawOutput": output}),
        ));
    }
    let items = fold(&events, &none);
    let marker = items.last().unwrap();
    assert!(
        matches!(&marker.body, Body::Marker(m) if m.marker == MarkerKind::Elided && m.reason.as_deref() == Some("bytes"))
    );
    // The update that went past is the last one that changed anything.
    let changed = items
        .iter()
        .filter(|i| i.version >= 1000 && matches!(i.body, Body::ToolCall(_)))
        .count();
    // Before the outputs: the prompt (64, id, timestamp, turn id, its text
    // and 16) and each call (64, id, timestamp, turn id, its tool call id).
    // Each output then adds its text, and the update that goes past the
    // budget still lands.
    let ts = events[0].ts.len();
    let base = 64
        + "t1:0".len()
        + ts
        + "t1".len()
        + "go".len()
        + 16
        + (0..calls)
            .map(|i| 64 + format!("t1:tool:c{i}").len() + ts + "t1".len() + format!("c{i}").len())
            .sum::<usize>();
    let expected = (GROUP_MAX_BYTES - base) / TEXT_MAX_BYTES + 1;
    assert!(expected < calls);
    assert_eq!(changed, expected);
}

/// Locality with elision (the re-confirmation's N-3): a session whose
/// middle turns are elided three ways folds the same from any turn's start.
#[test]
fn a_fold_from_any_turn_is_the_same_when_turns_are_elided() {
    let mut events = vec![turn(1, "t0"), chunk(2, "agent_message_chunk", "hi")];
    let mut id = 3;
    let mut push = |events: &mut Vec<EventDto>, kind: &str, body: Value| {
        events.push(ev(id, kind, body));
        id += 1;
    };
    // Elided by item count.
    push(&mut events, "user_turn", json!({"turn_id": "t1", "content": []}));
    for _ in 0..GROUP_MAX_ITEMS + 3 {
        push(&mut events, "brand_new_kind", json!({}));
    }
    // Elided by bytes, grown by changes.
    push(&mut events, "user_turn", json!({"turn_id": "t2", "content": []}));
    let calls = GROUP_MAX_BYTES / TEXT_MAX_BYTES + 1;
    let update = |call: String, output: Option<&str>| {
        let mut update = json!({"sessionUpdate": "tool_call_update", "toolCallId": call});
        if let Some(output) = output {
            update["rawOutput"] = json!(output);
        }
        json!({"kind": "acp_update", "indexed": {}, "payload": {"sessionId": "a", "update": update}})
    };
    for i in 0..calls {
        push(&mut events, "acp_update", update(format!("c{i}"), None));
    }
    let output = "o".repeat(TEXT_MAX_BYTES);
    for i in 0..calls {
        push(&mut events, "acp_update", update(format!("c{i}"), Some(&output)));
    }
    // Elided by question count.
    push(&mut events, "user_turn", json!({"turn_id": "t3", "content": []}));
    for i in 0..GROUP_MAX_QUESTIONS + 2 {
        let pending = format!("p{i}");
        push(
            &mut events,
            "pending_opened",
            json!({"pending_id": pending, "indexed": {"pending": {"id": pending, "kind": "elicitation"}}, "payload": {"mode": "form", "message": "?"}}),
        );
    }
    push(&mut events, "user_turn", json!({"turn_id": "t4", "content": []}));
    push(&mut events, "operator_resumed", json!({}));
    let whole = fold(&events, &none);
    let elided = whole
        .iter()
        .filter(|i| matches!(&i.body, Body::Marker(m) if m.marker == MarkerKind::Elided))
        .count();
    assert_eq!(elided, 3);
    for start in (0..events.len()).filter(|&i| events[i].kind == "user_turn") {
        let window = fold(&events[start..], &none);
        let turns: Vec<Option<&str>> = window.iter().map(|i| i.turn_id.as_deref()).collect();
        let expected: Vec<&Item> = whole.iter().filter(|i| turns.contains(&i.turn_id.as_deref())).collect();
        assert_eq!(window.iter().collect::<Vec<_>>(), expected, "from event {start}");
    }
}

#[test]
fn a_free_text_pairing_past_its_cap_pairs_with_nothing() {
    let key = "k".repeat(ID_MAX_BYTES + 1);
    let payload = json!({"mode": "form", "message": "m", "requestedSchema": {"properties": {
        "note": {"type": "string", "_meta": {"codex": {"questionId": key}}},
    }}});
    let Request::Elicitation { fields, .. } = hennery_view::question::elicitation(&payload) else {
        panic!()
    };
    assert_eq!(fields[0].pairing, None);
    // At its cap it still pairs.
    let key = "k".repeat(ID_MAX_BYTES);
    let payload = json!({"mode": "form", "message": "m", "requestedSchema": {"properties": {
        "note": {"type": "string", "_meta": {"codex": {"questionId": key, "role": "user_note"}}},
    }}});
    let Request::Elicitation { fields, .. } = hennery_view::question::elicitation(&payload) else {
        panic!()
    };
    assert_eq!(
        fields[0].pairing,
        Some(Pairing {
            with: key,
            kind: PairingKind::Note
        })
    );
}
