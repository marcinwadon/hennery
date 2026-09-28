use hennery_proto::frames::{CollectorFrame, HostFrame, Indexed, ParkReason, SessionBody, TurnOutcome};
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
        },
        CollectorFrame::ResumeSession {
            request_id: "r".into(),
            session_id: "s".into(),
            committed_seq: 41,
            agent: "claude".into(),
            cwd: "/tmp".into(),
            agent_session_id: "a1".into(),
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
    };
    assert_eq!(
        serde_json::to_value(&detail).unwrap(),
        json!({
            "session_id": "s", "host_id": "h", "agent": "claude", "cwd": "/tmp",
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "open_turn": {"turn_id": "t", "state": "started"}
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
