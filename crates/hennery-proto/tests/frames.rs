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

#[test]
fn session_detail_leaves_out_absent_optionals() {
    use hennery_proto::rest::{OpenTurn, SessionDetail};
    let detail = SessionDetail {
        session_id: "s".into(),
        host_id: "h".into(),
        agent: "claude".into(),
        cwd: "/tmp".into(),
        lifecycle: "active".into(),
        activity: Some("running".into()),
        failure_reason: None,
        presumed_parked: false,
        open_turn: Some(OpenTurn {
            turn_id: "t".into(),
            state: "started".into(),
        }),
        pending: vec![],
    };
    assert_eq!(
        serde_json::to_value(&detail).unwrap(),
        json!({
            "session_id": "s", "host_id": "h", "agent": "claude", "cwd": "/tmp",
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "open_turn": {"turn_id": "t", "state": "started"}, "pending": []
        })
    );
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
            pending: Some(PendingExtract {
                id: "p1".into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into(), "reject".into()]),
            }),
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
