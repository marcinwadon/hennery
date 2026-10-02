use hennery_proto::frames::{
    CollectorFrame, ConfigValue, HostFrame, Indexed, ParkReason, SessionBody, SessionConfig, TurnOutcome,
};
use serde_json::json;

#[test]
fn host_frames_use_snake_case_type_tags() {
    let frame = HostFrame::Session {
        session_id: "s1".into(),
        seq: 7,
        body: SessionBody::TurnEnded {
            turn_id: "t1".into(),
            outcome: TurnOutcome::Completed,
            stop_reason: Some("end_turn".into()),
            error: None,
        },
    };
    let value = serde_json::to_value(&frame).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "session",
            "session_id": "s1",
            "seq": 7,
            "body": {"kind": "turn_ended", "turn_id": "t1", "outcome": "completed", "stop_reason": "end_turn"}
        })
    );
}

#[test]
fn acp_payload_round_trips_unknown_fields_verbatim() {
    let payload = json!({
        "sessionId": "a1",
        "update": {"sessionUpdate": "some_future_kind", "_meta": {"x": [1, 2]}, "novel": true}
    });
    let frame = HostFrame::Session {
        session_id: "s1".into(),
        seq: 1,
        body: SessionBody::AcpUpdate {
            indexed: Indexed::default(),
            payload: payload.clone(),
        },
    };
    let text = serde_json::to_string(&frame).unwrap();
    let back: HostFrame = serde_json::from_str(&text).unwrap();
    match back {
        HostFrame::Session {
            body: SessionBody::AcpUpdate { payload: got, .. },
            ..
        } => assert_eq!(got, payload),
        other => panic!("unexpected frame {other:?}"),
    }
}

#[test]
fn unknown_frame_type_is_an_error_not_a_panic() {
    let err = serde_json::from_value::<CollectorFrame>(json!({"type": "from_the_future"}));
    assert!(err.is_err());
}

#[test]
fn every_collector_frame_round_trips() {
    let frames = vec![
        CollectorFrame::HelloAck {
            protocol_version: "1.0".into(),
            collector_version: "0.0.0".into(),
            committed: [("s".to_string(), 4u64)].into_iter().collect(),
        },
        CollectorFrame::HelloError {
            code: "bad_token".into(),
            message: "no".into(),
        },
        CollectorFrame::StartSession {
            request_id: "r".into(),
            session_id: "s".into(),
            committed_seq: 0,
            agent: "claude".into(),
            cwd: "/tmp".into(),
            config: Default::default(),
        },
        CollectorFrame::ResumeSession {
            request_id: "r".into(),
            session_id: "s".into(),
            committed_seq: 41,
            agent: "claude".into(),
            cwd: "/tmp".into(),
            agent_session_id: "a1".into(),
            config: Default::default(),
        },
        CollectorFrame::Prompt {
            request_id: "r".into(),
            session_id: "s".into(),
            turn_id: "t".into(),
            content: vec![json!({"type": "text", "text": "hi"})],
        },
        CollectorFrame::Ack {
            session_id: "s".into(),
            ack_seq: 3,
        },
        CollectorFrame::ParkSession {
            request_id: "r".into(),
            session_id: "s".into(),
        },
        CollectorFrame::CloseSession {
            request_id: "r".into(),
            session_id: "s".into(),
        },
        CollectorFrame::CancelTurn {
            request_id: "r".into(),
            session_id: "s".into(),
            turn_id: "t".into(),
        },
        CollectorFrame::SetConfig {
            request_id: "r".into(),
            session_id: "s".into(),
            config_id: "model".into(),
            value: ConfigValue::Id("large".into()),
        },
        CollectorFrame::AnswerPermission {
            request_id: "r".into(),
            session_id: "s".into(),
            pending_id: "p".into(),
            option_id: "allow".into(),
        },
        CollectorFrame::AnswerElicitation {
            request_id: "r".into(),
            session_id: "s".into(),
            pending_id: "p".into(),
            action: hennery_proto::frames::ElicitationAction::Cancel,
            content: None,
        },
        CollectorFrame::ResolvePath {
            request_id: "r".into(),
            path: "~/Projects".into(),
        },
        CollectorFrame::ForgetHat {
            hat_id: "hat-0123456789abcdef".into(),
        },
    ];
    for f in frames {
        let back: CollectorFrame = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }
}

#[test]
fn teardown_bodies_use_the_spec_field_names() {
    let cases = [
        (
            SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
            json!({"kind": "session_parked", "reason": "adapter_exited"}),
        ),
        (SessionBody::SessionClosed, json!({"kind": "session_closed"})),
        (
            SessionBody::AdapterExited {
                code: None,
                signal: Some(9),
                stderr_tail: "boom".into(),
            },
            json!({"kind": "adapter_exited", "signal": 9, "stderr_tail": "boom"}),
        ),
    ];
    for (body, expected) in cases {
        assert_eq!(serde_json::to_value(&body).unwrap(), expected);
        assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), body);
    }
}

#[test]
fn resume_frames_and_host_notes_use_the_spec_field_names() {
    let resume = CollectorFrame::ResumeSession {
        request_id: "r".into(),
        session_id: "s".into(),
        committed_seq: 41,
        agent: "claude".into(),
        cwd: "/tmp".into(),
        agent_session_id: "a1".into(),
        config: Default::default(),
    };
    assert_eq!(
        serde_json::to_value(&resume).unwrap(),
        json!({
            "type": "resume_session", "request_id": "r", "session_id": "s", "committed_seq": 41,
            "agent": "claude", "cwd": "/tmp", "agent_session_id": "a1"
        })
    );
    let note = SessionBody::HostNote {
        note: "replay_dropped".into(),
        text: "dropped 2 unknown update kinds during session/load".into(),
    };
    let expected = json!({
        "kind": "host_note",
        "note": "replay_dropped",
        "text": "dropped 2 unknown update kinds during session/load"
    });
    assert_eq!(serde_json::to_value(&note).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), note);
}

/// A list item with only what every session has.
fn bare_item() -> hennery_proto::rest::SessionItem {
    hennery_proto::rest::SessionItem {
        session_id: "s".into(),
        host_id: "h".into(),
        agent: "claude".into(),
        cwd: "/tmp".into(),
        hat_id: "hat-1".into(),
        title: None,
        lifecycle: "active".into(),
        activity: Some("running".into()),
        failure_reason: None,
        presumed_parked: false,
        git_branch: None,
        git_dirty: None,
        model: None,
        mode: None,
        created_at: "2026-10-07T12:00:00.000Z".into(),
        last_event_at: "2026-10-07T12:00:01.000Z".into(),
    }
}

#[test]
fn session_detail_leaves_out_absent_optionals() {
    use hennery_proto::rest::{OpenTurn, SessionDetail};
    let detail = SessionDetail {
        session: bare_item(),
        open_turn: Some(OpenTurn {
            turn_id: "t".into(),
            state: "started".into(),
        }),
        pending: vec![],
    };
    assert_eq!(
        serde_json::to_value(&detail).unwrap(),
        json!({
            "session_id": "s", "host_id": "h", "agent": "claude", "cwd": "/tmp", "hat_id": "hat-1",
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "created_at": "2026-10-07T12:00:00.000Z", "last_event_at": "2026-10-07T12:00:01.000Z",
            "open_turn": {"turn_id": "t", "state": "started"}, "pending": []
        })
    );
}

/// Plan 6b, the review's A1: as the list serves it, a model, mode or
/// failure reason past its cap is left out (the stored value is kept for a
/// resume), a title or branch past its cap is cut, and a long cwd keeps
/// its end, after `…`; within the caps nothing changes.
#[test]
fn a_listed_item_is_bounded_field_by_field() {
    use hennery_proto::rest::{
        AGENT_MAX_JSON_BYTES, CWD_MAX_JSON_BYTES, FAILURE_REASON_MAX_JSON_BYTES, HOST_ID_MAX_JSON_BYTES,
        MODE_MAX_JSON_BYTES, MODEL_MAX_JSON_BYTES,
    };
    let within = hennery_proto::rest::SessionItem {
        title: Some("Fix the login bug".into()),
        git_branch: Some("fix/login".into()),
        model: Some("m".repeat(MODEL_MAX_JSON_BYTES)),
        mode: Some("\"".repeat(MODE_MAX_JSON_BYTES / 2)),
        failure_reason: Some("f".repeat(FAILURE_REASON_MAX_JSON_BYTES)),
        cwd: "c".repeat(CWD_MAX_JSON_BYTES),
        ..bare_item()
    };
    assert_eq!(within.clone().bounded(), within);
    assert_eq!(AGENT_MAX_JSON_BYTES, 32);

    let past = hennery_proto::rest::SessionItem {
        title: Some("t".repeat(500)),
        git_branch: Some("b".repeat(500)),
        model: Some("m".repeat(MODEL_MAX_JSON_BYTES + 1)),
        mode: Some("\"".repeat(MODE_MAX_JSON_BYTES / 2 + 1)),
        failure_reason: Some("f".repeat(FAILURE_REASON_MAX_JSON_BYTES + 1)),
        cwd: format!("/home/someone/{}webapp", "deep/".repeat(40)),
        ..bare_item()
    }
    .bounded();
    assert_eq!(past.title, Some("t".repeat(120)));
    assert_eq!(past.git_branch, Some("b".repeat(120)));
    assert_eq!((past.model, past.mode, past.failure_reason), (None, None, None));
    // The second review's P1: within its cap, but with a control or hidden
    // character, a model, mode or failure reason is left out too; an agent
    // or host id from before the start checked them is cut.
    let odd = hennery_proto::rest::SessionItem {
        model: Some("op\u{202E}us".into()),
        mode: Some("pl\u{200B}an".into()),
        failure_reason: Some("bad\nreason".into()),
        agent: "a".repeat(40),
        host_id: "h".repeat(100),
        ..bare_item()
    }
    .bounded();
    assert_eq!((odd.model, odd.mode, odd.failure_reason), (None, None, None));
    assert_eq!(
        (odd.agent, odd.host_id),
        ("a".repeat(AGENT_MAX_JSON_BYTES), "h".repeat(HOST_ID_MAX_JSON_BYTES))
    );
    assert!(
        past.cwd.starts_with('…') && past.cwd.ends_with("/deep/webapp"),
        "{}",
        past.cwd
    );
    assert!(serde_json::to_string(&past.cwd).unwrap().len() - 2 <= CWD_MAX_JSON_BYTES);
    // Never inside a character: four bytes each.
    let emoji = hennery_proto::rest::SessionItem {
        cwd: "😀".repeat(100),
        ..bare_item()
    }
    .bounded();
    assert_eq!(emoji.cwd, format!("…{}", "😀".repeat((CWD_MAX_JSON_BYTES - 3) / 4)));
}

/// P-23 (ACP core §8): a list item stays under 1 KiB, here with every field
/// at its cap or past it, the worst content for each (quotes, backslashes,
/// control and four-byte characters), a cwd of exactly its cap (kept
/// whole), a 30-character `created_at` from before stamps had one width,
/// and the `hat_id` hats added: `hat-` and 16 hex digits, as the kernel
/// mints them (plan 6b decision 10, the reviews' A1 and P2; plan 5c).
#[test]
fn a_listed_item_stays_under_1_kib_with_every_field_at_its_worst() {
    let item = hennery_proto::rest::SessionItem {
        session_id: "0199a4c2-7e1f-7c3a-9b2d-4f6e8a0c1d2e".into(),
        // Past their caps (an older row's): cut to 32 bytes each.
        host_id: "\"".repeat(40),
        agent: "\"".repeat(40),
        // Exactly 128 bytes as JSON writes it: kept whole.
        cwd: format!("/{}", "\"\\😀".repeat(15)) + &"x".repeat(7),
        hat_id: "hat-0123456789abcdef".into(),
        title: Some("\"".repeat(500)),
        lifecycle: "starting".into(),
        activity: Some("blocked".into()),
        failure_reason: Some("\\".repeat(16)),
        presumed_parked: false,
        git_branch: Some("\\😀".repeat(200)),
        git_dirty: Some(false),
        model: Some("\"".repeat(24)),
        mode: Some("\\".repeat(24)),
        created_at: "2026-10-07T12:34:56.123456789Z".into(),
        last_event_at: "2026-10-07T12:34:56.789Z".into(),
    }
    .bounded();
    assert!(!item.cwd.starts_with('…'), "{}", item.cwd);
    assert_eq!(serde_json::to_string(&item.cwd).unwrap().len() - 2, 128);
    let json = serde_json::to_string(&item).unwrap();
    assert!(json.len() <= 1024, "{} bytes: {json}", json.len());
    assert!(item.model.is_some() && item.mode.is_some() && item.failure_reason.is_some());
}

#[test]
fn protocol_major_parses_only_well_formed_versions() {
    use hennery_proto::protocol_major;
    assert_eq!(protocol_major("1.0"), Some(1));
    assert_eq!(protocol_major("2.13"), Some(2));
    assert_eq!(protocol_major("1"), None);
    assert_eq!(protocol_major("x.0"), None);
    assert_eq!(protocol_major("1.x"), None);
}

#[test]
fn cancel_turn_and_its_answer_use_the_spec_field_names() {
    let cancel = CollectorFrame::CancelTurn {
        request_id: "r".into(),
        session_id: "s".into(),
        turn_id: "t".into(),
    };
    assert_eq!(
        serde_json::to_value(&cancel).unwrap(),
        json!({"type": "cancel_turn", "request_id": "r", "session_id": "s", "turn_id": "t"})
    );
    let answer = hennery_proto::rest::CancelResponse {
        turn_id: "t".into(),
        outcome: TurnOutcome::Cancelled,
    };
    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        json!({"turn_id": "t", "outcome": "cancelled"})
    );
}

#[test]
fn hello_capabilities_skip_unknown_entries_and_default_to_none() {
    use hennery_proto::frames::{Capabilities, Capability};
    let hello = |extra: serde_json::Value| {
        let mut v = json!({
            "type": "hello", "protocol_version": "1.0", "host_version": "0", "host_id": "h",
            "proof": "p", "attached_sessions": []
        });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        match serde_json::from_value::<HostFrame>(v).unwrap() {
            HostFrame::Hello { capabilities, .. } => capabilities,
            other => panic!("expected hello, got {other:?}"),
        }
    };
    let newer = hello(json!({"capabilities": ["park", "teleport", "images"]}));
    assert_eq!(newer, Capabilities(vec![Capability::Park, Capability::Images]));
    assert!(newer.has(Capability::Park) && !newer.has(Capability::Projects));
    assert_eq!(hello(json!({})), Capabilities::default());
    let sent = HostFrame::Hello {
        protocol_version: "1.0".into(),
        host_version: "0".into(),
        host_id: "h".into(),
        proof: "p".into(),
        capabilities: Capabilities(vec![Capability::Park]),
        workspace_roots: vec![],
        attached_sessions: vec![],
    };
    assert_eq!(serde_json::to_value(&sent).unwrap()["capabilities"], json!(["park"]));
}

#[test]
fn session_started_names_the_request_and_the_agents_session() {
    let body = serde_json::to_value(SessionBody::session_started("r", "a")).unwrap();
    assert_eq!(
        (&body["kind"], &body["request_id"], &body["agent_session_id"]),
        (&json!("session_started"), &json!("r"), &json!("a"))
    );
}

// Plan B2b: model, axes and mode (ACP core §3.2, §3.3).

fn config() -> SessionConfig {
    SessionConfig {
        model: Some("large".into()),
        mode: Some("plan".into()),
        axes: [
            ("effort".to_string(), ConfigValue::Id("high".into())),
            ("fast".to_string(), ConfigValue::Bool(true)),
        ]
        .into_iter()
        .collect(),
    }
}

#[test]
fn start_and_resume_carry_model_mode_and_axes_as_flat_fields() {
    let start = CollectorFrame::StartSession {
        request_id: "r".into(),
        session_id: "s".into(),
        committed_seq: 0,
        agent: "claude".into(),
        cwd: "/tmp".into(),
        config: config(),
    };
    let expected = json!({
        "type": "start_session", "request_id": "r", "session_id": "s", "committed_seq": 0,
        "agent": "claude", "cwd": "/tmp",
        "model": "large", "mode": "plan", "axes": {"effort": "high", "fast": true}
    });
    assert_eq!(serde_json::to_value(&start).unwrap(), expected);
    assert_eq!(serde_json::from_value::<CollectorFrame>(expected).unwrap(), start);
    // Absent fields are an empty config, for a collector that sends none.
    let bare: CollectorFrame = serde_json::from_value(json!({
        "type": "resume_session", "request_id": "r", "session_id": "s", "committed_seq": 3,
        "agent": "claude", "cwd": "/tmp", "agent_session_id": "a1"
    }))
    .unwrap();
    let CollectorFrame::ResumeSession { config, .. } = bare else {
        panic!("{bare:?}");
    };
    assert!(config.is_empty());
}

#[test]
fn config_frames_use_the_spec_field_names() {
    let set = CollectorFrame::SetConfig {
        request_id: "r".into(),
        session_id: "s".into(),
        config_id: "fast".into(),
        value: ConfigValue::Bool(false),
    };
    assert_eq!(
        serde_json::to_value(&set).unwrap(),
        json!({"type": "set_config", "request_id": "r", "session_id": "s", "config_id": "fast", "value": false})
    );
    let applied = SessionBody::ConfigApplied {
        request_id: "r".into(),
        indexed: Indexed {
            config_options: Some(vec![json!({"id": "model"})]),
            current_model: Some("large".into()),
            current_mode: Some("plan".into()),
            current_axes: Some(config().axes),
            ..Indexed::default()
        },
    };
    let expected = json!({
        "kind": "config_applied", "request_id": "r",
        "indexed": {
            "config_options": [{"id": "model"}], "current_model": "large", "current_mode": "plan",
            "current_axes": {"effort": "high", "fast": true}
        }
    });
    assert_eq!(serde_json::to_value(&applied).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), applied);
    // A `session_started` from an older host has no extracts.
    let old: SessionBody =
        serde_json::from_value(json!({"kind": "session_started", "request_id": "r", "agent_session_id": "a"})).unwrap();
    assert_eq!(old, SessionBody::session_started("r", "a"));
}

#[test]
fn only_a_non_empty_catalogue_is_a_snapshot_of_the_current_config() {
    let mut indexed = Indexed {
        current_model: Some("large".into()),
        ..Indexed::default()
    };
    assert_eq!(indexed.current_config(), None, "no catalogue");
    indexed.config_options = Some(vec![]);
    assert_eq!(indexed.current_config(), None, "an empty read-back");
    indexed.config_options = Some(vec![json!({"id": "model"})]);
    assert_eq!(
        indexed.current_config(),
        Some(SessionConfig {
            model: Some("large".into()),
            ..SessionConfig::default()
        })
    );
}

#[test]
fn rest_config_requests_take_a_value_id_or_a_boolean() {
    use hennery_proto::rest::{ConfigRequest, StartSessionRequest};
    let start: StartSessionRequest =
        serde_json::from_value(json!({"host_id": "h", "agent": "claude", "cwd": "/tmp", "mode": "plan"})).unwrap();
    assert_eq!(start.config.mode.as_deref(), Some("plan"));
    let plain: StartSessionRequest =
        serde_json::from_value(json!({"host_id": "h", "agent": "claude", "cwd": "/tmp"})).unwrap();
    assert!(plain.config.is_empty());
    let id: ConfigRequest = serde_json::from_value(json!({"config_id": "model", "value": "large"})).unwrap();
    assert_eq!(id.value, ConfigValue::Id("large".into()));
    let toggle: ConfigRequest = serde_json::from_value(json!({"config_id": "fast", "value": true})).unwrap();
    assert_eq!(toggle.value, ConfigValue::Bool(true));
    assert!(serde_json::from_value::<ConfigRequest>(json!({"config_id": "fast", "value": 3})).is_err());
}

// Plan (2): permission and elicitation (ACP core §3.2, §3.3, §4.6).

#[test]
fn pending_bodies_use_the_spec_field_names() {
    use hennery_proto::frames::{PendingExtract, PendingKind, PendingReason, PendingResolution};
    let opened = SessionBody::PendingOpened {
        pending_id: "p1".into(),
        indexed: Indexed {
            turn_id: Some("t1".into()),
            pending: Some(Box::new(PendingExtract {
                id: "p1".into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into(), "reject".into()]),
                title: None,
            })),
            ..Indexed::default()
        },
        payload: json!({"sessionId": "a1", "options": [], "_meta": {"x": 1}}),
    };
    // `kind` is the body's tag, so the request's own kind rides in the
    // `pending` extract.
    let expected = json!({
        "kind": "pending_opened", "pending_id": "p1",
        "indexed": {"turn_id": "t1", "pending": {"id": "p1", "kind": "permission", "option_ids": ["allow", "reject"]}},
        "payload": {"sessionId": "a1", "options": [], "_meta": {"x": 1}}
    });
    assert_eq!(serde_json::to_value(&opened).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), opened);
    let cancelled = SessionBody::PendingResolved {
        pending_id: "p1".into(),
        resolution: PendingResolution::Cancelled,
        reason: Some(PendingReason::TurnCancelled),
    };
    let expected =
        json!({"kind": "pending_resolved", "pending_id": "p1", "resolution": "cancelled", "reason": "turn_cancelled"});
    assert_eq!(serde_json::to_value(&cancelled).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), cancelled);
    let result = SessionBody::AnswerResult {
        pending_id: "p1".into(),
        request_id: "r9".into(),
        delivered: true,
    };
    let expected = json!({"kind": "answer_result", "pending_id": "p1", "request_id": "r9", "delivered": true});
    assert_eq!(serde_json::to_value(&result).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), result);
}

#[test]
fn answer_frames_use_the_spec_field_names() {
    use hennery_proto::frames::ElicitationAction;
    let permission = CollectorFrame::AnswerPermission {
        request_id: "r".into(),
        session_id: "s".into(),
        pending_id: "p".into(),
        option_id: "allow".into(),
    };
    let expected = json!({"type": "answer_permission", "request_id": "r", "session_id": "s", "pending_id": "p", "option_id": "allow"});
    assert_eq!(serde_json::to_value(&permission).unwrap(), expected);
    assert_eq!(serde_json::from_value::<CollectorFrame>(expected).unwrap(), permission);
    let elicitation = CollectorFrame::AnswerElicitation {
        request_id: "r".into(),
        session_id: "s".into(),
        pending_id: "p".into(),
        action: ElicitationAction::Accept,
        content: Some(json!({"name": "hennery"})),
    };
    let expected = json!({
        "type": "answer_elicitation", "request_id": "r", "session_id": "s", "pending_id": "p",
        "action": "accept", "content": {"name": "hennery"}
    });
    assert_eq!(serde_json::to_value(&elicitation).unwrap(), expected);
    assert_eq!(serde_json::from_value::<CollectorFrame>(expected).unwrap(), elicitation);
    let declined: CollectorFrame = serde_json::from_value(json!({
        "type": "answer_elicitation", "request_id": "r", "session_id": "s", "pending_id": "p", "action": "decline"
    }))
    .unwrap();
    assert!(matches!(
        declined,
        CollectorFrame::AnswerElicitation { content: None, .. }
    ));
}

#[test]
fn a_rest_answer_is_an_option_or_an_elicitation_action() {
    use hennery_proto::frames::ElicitationAction;
    use hennery_proto::rest::AnswerRequest;
    let option: AnswerRequest = serde_json::from_value(json!({"option_id": "allow"})).unwrap();
    assert_eq!(
        option,
        AnswerRequest::Permission {
            option_id: "allow".into()
        }
    );
    let accept: AnswerRequest = serde_json::from_value(json!({"action": "accept", "content": {"n": 1}})).unwrap();
    assert_eq!(
        accept,
        AnswerRequest::Elicitation {
            action: ElicitationAction::Accept,
            content: Some(json!({"n": 1}))
        }
    );
    let decline: AnswerRequest = serde_json::from_value(json!({"action": "decline"})).unwrap();
    assert!(matches!(decline, AnswerRequest::Elicitation { content: None, .. }));
    for bad in [json!({}), json!({"action": "maybe"}), json!({"option_id": 3})] {
        assert!(serde_json::from_value::<AnswerRequest>(bad.clone()).is_err(), "{bad}");
    }
}

/// Plan 6a: a stored prompt names its images by the attachment they were
/// stored as, and never carries their bytes (ACP core §7).
#[test]
fn stored_blocks_name_the_attachment_and_never_carry_its_bytes() {
    use hennery_proto::rest::{AttachmentUsage, StoredBlock};
    let blocks = vec![
        StoredBlock::Text { text: "look".into() },
        StoredBlock::Image {
            mime_type: "image/png".into(),
            sha256: "ab".repeat(32),
            size: 3,
        },
    ];
    let value = serde_json::to_value(&blocks).unwrap();
    assert_eq!(
        value,
        json!([
            {"type": "text", "text": "look"},
            {"type": "image", "mimeType": "image/png", "sha256": "ab".repeat(32), "size": 3}
        ])
    );
    assert_eq!(serde_json::from_value::<Vec<StoredBlock>>(value).unwrap(), blocks);
    // An ACP image block, with its bytes and no attachment, is not one.
    let acp = json!({"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgo="});
    assert!(serde_json::from_value::<StoredBlock>(acp).is_err());
    assert_eq!(
        serde_json::to_value(AttachmentUsage { count: 2, bytes: 10 }).unwrap(),
        json!({"count": 2, "bytes": 10})
    );
}

// Plan 6c: the project picker's probes (ACP core §3.3, §7).

fn projects_reply() -> HostFrame {
    HostFrame::Projects {
        request_id: "r".into(),
        items: vec![hennery_proto::frames::Project { path: "/p/a".into() }],
        partial: true,
        home: None,
    }
}

fn directory_reply() -> HostFrame {
    HostFrame::Directory {
        request_id: "r".into(),
        path: "/p".into(),
        parent: Some("/".into()),
        entries: vec![hennery_proto::frames::DirEntry {
            name: "a".into(),
            git: true,
        }],
        truncated: false,
    }
}

#[test]
fn probe_frames_use_the_spec_field_names() {
    for (frame, expected) in [
        (
            CollectorFrame::ListProjects { request_id: "r".into() },
            json!({"type": "list_projects", "request_id": "r"}),
        ),
        (
            CollectorFrame::BrowseDirectory {
                request_id: "r".into(),
                path: "/p".into(),
            },
            json!({"type": "browse_directory", "request_id": "r", "path": "/p"}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&frame).unwrap(), expected);
        assert_eq!(serde_json::from_value::<CollectorFrame>(expected).unwrap(), frame);
    }
    for (frame, expected) in [
        (
            projects_reply(),
            json!({"type": "projects", "request_id": "r", "items": [{"path": "/p/a"}], "partial": true}),
        ),
        (
            directory_reply(),
            json!({
                "type": "directory", "request_id": "r", "path": "/p", "parent": "/",
                "entries": [{"name": "a", "git": true}], "truncated": false
            }),
        ),
    ] {
        assert_eq!(serde_json::to_value(&frame).unwrap(), expected);
        assert_eq!(serde_json::from_value::<HostFrame>(expected).unwrap(), frame);
    }
}

#[test]
fn only_probe_replies_name_a_probe() {
    assert_eq!(projects_reply().probe_request_id(), Some("r"));
    assert_eq!(directory_reply().probe_request_id(), Some("r"));
    let error = HostFrame::Error {
        request_id: "r".into(),
        code: "invalid".into(),
        message: "no".into(),
    };
    assert_eq!(error.probe_request_id(), None);
    assert_eq!(HostFrame::ResendComplete.probe_request_id(), None);
}

#[test]
fn a_hello_without_workspace_roots_has_none() {
    let hello = |extra: serde_json::Value| {
        let mut v = json!({
            "type": "hello", "protocol_version": "1.0", "host_version": "0", "host_id": "h",
            "proof": "p", "attached_sessions": []
        });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        match serde_json::from_value::<HostFrame>(v).unwrap() {
            HostFrame::Hello { workspace_roots, .. } => workspace_roots,
            other => panic!("expected hello, got {other:?}"),
        }
    };
    assert!(hello(json!({})).is_empty());
    assert_eq!(hello(json!({"workspace_roots": ["/p", "~/src"]})), ["/p", "~/src"]);
}

/// Plan 6b-ii: `git_state` on the wire, with its optionals left out.
#[test]
fn git_state_round_trips_and_leaves_out_absent_optionals() {
    let detached = SessionBody::GitState {
        branch: None,
        dirty: true,
        worktree: false,
        head: None,
        base_commit: None,
    };
    let expected = json!({"kind": "git_state", "dirty": true, "worktree": false});
    assert_eq!(serde_json::to_value(&detached).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), detached);
    let full = json!({
        "kind": "git_state", "branch": "main", "dirty": false, "worktree": true,
        "head": "c0ffee", "base_commit": "c0ffee"
    });
    let body: SessionBody = serde_json::from_value(full.clone()).unwrap();
    assert_eq!(serde_json::to_value(&body).unwrap(), full);
}

/// Kernel spec §5.4: `resolve_path{path}` → `resolved_path{canonical,
/// exists, is_dir}`, each with its request id.
#[test]
fn resolve_path_and_its_answer_use_the_spec_field_names() {
    let request = CollectorFrame::ResolvePath {
        request_id: "r".into(),
        path: "~/p".into(),
    };
    let wire = json!({"type": "resolve_path", "request_id": "r", "path": "~/p"});
    assert_eq!(serde_json::to_value(&request).unwrap(), wire);
    assert_eq!(serde_json::from_value::<CollectorFrame>(wire).unwrap(), request);
    let answer = HostFrame::ResolvedPath {
        request_id: "r".into(),
        canonical: "/home/me/p".into(),
        exists: true,
        is_dir: false,
    };
    let wire = json!({
        "type": "resolved_path", "request_id": "r", "canonical": "/home/me/p", "exists": true, "is_dir": false
    });
    assert_eq!(serde_json::to_value(&answer).unwrap(), wire);
    assert_eq!(serde_json::from_value::<HostFrame>(wire).unwrap(), answer);
}

/// Plan 9c decision 12 (ACP core §3.3): the collector tells a host after
/// each reconciled handshake which hats are purged; it is no probe.
#[test]
fn forget_hat_uses_the_spec_field_names_and_is_no_probe() {
    let frame = CollectorFrame::ForgetHat {
        hat_id: "hat-0123456789abcdef".into(),
    };
    let wire = json!({"type": "forget_hat", "hat_id": "hat-0123456789abcdef"});
    assert_eq!(serde_json::to_value(&frame).unwrap(), wire);
    assert_eq!(serde_json::from_value::<CollectorFrame>(wire).unwrap(), frame);
    assert!(frame.probe_capability().is_err());
}

// Plan 9d: the agent's own transcript on its host.

#[test]
fn forget_frames_use_the_spec_field_names_and_name_kinds_never_paths() {
    use hennery_proto::frames::{AgentHome, ForgetKind, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat};
    let forget = CollectorFrame::ForgetSession {
        request_id: "r".into(),
        agent: "claude".into(),
        agent_session_id: "a1".into(),
        agent_home: AgentHome {
            root: "/h/.claude".into(),
            sqlite_root: None,
        },
    };
    let expected = json!({
        "type": "forget_session", "request_id": "r", "agent": "claude", "agent_session_id": "a1",
        "agent_home": {"root": "/h/.claude"}
    });
    assert_eq!(serde_json::to_value(&forget).unwrap(), expected);
    assert_eq!(serde_json::from_value::<CollectorFrame>(expected).unwrap(), forget);
    assert_eq!(
        forget.probe_capability(),
        Ok(Some(hennery_proto::frames::Capability::ForgetSession))
    );

    let answer = HostFrame::SessionForgotten {
        request_id: "r".into(),
        outcome: ForgetOutcome::Partial,
        removed: vec![ForgetWhat {
            kind: ForgetKind::Transcript,
            count: 2,
        }],
        remaining: vec![ForgetRemaining {
            what: ForgetWhat {
                kind: ForgetKind::FileHistory,
                count: 1,
            },
            reason: ForgetReason::Symlink,
            retry: false,
        }],
    };
    let expected = json!({
        "type": "session_forgotten", "request_id": "r", "outcome": "partial",
        "removed": [{"kind": "transcript", "count": 2}],
        "remaining": [{"what": {"kind": "file_history", "count": 1}, "reason": "symlink", "retry": false}]
    });
    assert_eq!(serde_json::to_value(&answer).unwrap(), expected);
    assert_eq!(serde_json::from_value::<HostFrame>(expected).unwrap(), answer);
    assert_eq!(answer.probe_request_id(), Some("r"));
    // A reason is a closed set: free text from a host does not parse.
    let free = json!({
        "type": "session_forgotten", "request_id": "r", "outcome": "partial", "removed": [],
        "remaining": [{"what": {"kind": "debug", "count": 1}, "reason": "/home/me/.claude/debug", "retry": true}]
    });
    assert!(serde_json::from_value::<HostFrame>(free).is_err());
}

#[test]
fn session_started_carries_the_agent_home_only_when_there_is_one() {
    use hennery_proto::frames::AgentHome;
    let plain = SessionBody::session_started("r", "a1");
    assert!(serde_json::to_value(&plain).unwrap().get("agent_home").is_none());
    let with = json!({
        "kind": "session_started", "request_id": "r", "agent_session_id": "a1",
        "agent_home": {"root": "/h/.codex", "sqlite_root": "/h/db"}
    });
    let SessionBody::SessionStarted { agent_home, .. } = serde_json::from_value(with).unwrap() else {
        panic!("not a session_started");
    };
    assert_eq!(
        agent_home,
        Some(AgentHome {
            root: "/h/.codex".into(),
            sqlite_root: Some("/h/db".into())
        })
    );
}

#[test]
fn an_agent_home_is_well_formed_only_absolute_bounded_and_without_nul() {
    use hennery_proto::frames::{AGENT_HOME_MAX_BYTES, AgentHome};
    let home = |root: &str, sqlite: Option<&str>| AgentHome {
        root: root.into(),
        sqlite_root: sqlite.map(Into::into),
    };
    assert!(home("/h/.claude", None).is_well_formed());
    assert!(home("/h/.codex", Some("/db")).is_well_formed());
    assert!(!home("h/.claude", None).is_well_formed());
    assert!(!home("", None).is_well_formed());
    assert!(!home("/h/\0x", None).is_well_formed());
    assert!(!home("/h", Some("db")).is_well_formed());
    assert!(!home(&format!("/{}", "a".repeat(AGENT_HOME_MAX_BYTES)), None).is_well_formed());
    assert!(home(&format!("/{}", "a".repeat(AGENT_HOME_MAX_BYTES - 1)), None).is_well_formed());
}
