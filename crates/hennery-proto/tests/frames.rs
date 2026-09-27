use hennery_proto::frames::{CollectorFrame, HostFrame, Indexed, SessionBody, TurnOutcome};
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
            agent: "claude".into(),
            cwd: "/tmp".into(),
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
    ];
    for f in frames {
        let back: CollectorFrame = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }
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
