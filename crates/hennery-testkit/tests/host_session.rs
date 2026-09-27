//! The host's session actor against the fake adapter, without a collector:
//! the outbox must receive session_started, then per turn turn_started, the
//! adapter's updates verbatim, and exactly one turn_ended, in that order.

use hennery_host::outbox::Outbox;
use hennery_host::session::{self, AgentCommand, SessionCmd};
use hennery_host::uplink::Uplink;
use hennery_proto::frames::{HostFrame, SessionBody, TurnOutcome};
use serde_json::json;
use std::time::Duration;

fn kinds(frames: &[HostFrame]) -> Vec<String> {
    frames
        .iter()
        .map(|f| match f {
            HostFrame::Session { body, .. } => match body {
                SessionBody::SessionStarted { .. } => "session_started".to_string(),
                SessionBody::StartFailed { .. } => "start_failed".to_string(),
                SessionBody::TurnStarted { .. } => "turn_started".to_string(),
                SessionBody::AcpUpdate { payload, .. } => {
                    format!(
                        "update:{}",
                        payload["update"]["content"]["text"].as_str().unwrap_or("?")
                    )
                }
                SessionBody::TurnEnded { .. } => "turn_ended".to_string(),
            },
            other => format!("{other:?}"),
        })
        .collect()
}

async fn wait_until(uplink: &Uplink, pred: impl Fn(&[HostFrame]) -> bool) -> Vec<HostFrame> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frames = uplink.pending().unwrap();
        if pred(&frames) {
            return frames;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out; outbox: {:?}",
            kinds(&frames)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_turn_produces_ordered_outboxed_facts() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let tx = session::start(uplink.clone(), "r0".into(), "s1".into(), fake, std::env::temp_dir());
    wait_until(&uplink, |f| !f.is_empty()).await;

    tx.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type":"text","text":"hi"})],
    })
    .unwrap();
    let frames = wait_until(&uplink, |f| kinds(f).contains(&"turn_ended".to_string())).await;
    assert_eq!(
        kinds(&frames),
        [
            "session_started",
            "turn_started",
            "update:Hello",
            "update: world",
            "turn_ended"
        ]
    );
    match frames.last().unwrap() {
        HostFrame::Session {
            body: SessionBody::TurnEnded {
                outcome, stop_reason, ..
            },
            ..
        } => {
            assert_eq!(*outcome, TurnOutcome::Completed);
            assert_eq!(stop_reason.as_deref(), Some("end_turn"));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_repeated_turn_id_is_not_run_twice() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let tx = session::start(uplink.clone(), "r0".into(), "s1".into(), fake, std::env::temp_dir());
    let prompt = || SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type":"text","text":"hi"})],
    };
    tx.send(prompt()).unwrap();
    tx.send(prompt()).unwrap();
    wait_until(&uplink, |f| kinds(f).contains(&"turn_ended".to_string())).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let ends = kinds(&uplink.pending().unwrap())
        .iter()
        .filter(|k| *k == "turn_ended")
        .count();
    assert_eq!(ends, 1);
}

#[tokio::test]
async fn an_empty_prompt_is_rejected_without_starting_a_turn() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let tx = session::start(uplink.clone(), "r0".into(), "s1".into(), fake, std::env::temp_dir());
    wait_until(&uplink, |f| !f.is_empty()).await;
    tx.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![],
    })
    .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), replies.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, HostFrame::Error { ref code, .. } if code == "invalid"),
        "{reply:?}"
    );
    assert_eq!(kinds(&uplink.pending().unwrap()), ["session_started"]);
}

#[tokio::test]
async fn an_adapter_that_cannot_start_reports_start_failed_durably() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let broken = AgentCommand::parse("/nonexistent/hennery-test-adapter").unwrap();
    let _tx = session::start(uplink.clone(), "r0".into(), "s1".into(), broken, std::env::temp_dir());
    let frames = wait_until(&uplink, |f| !f.is_empty()).await;
    assert_eq!(kinds(&frames), ["start_failed"]);
}

#[tokio::test]
async fn an_adapter_that_exits_immediately_reports_start_failed_durably_and_quickly() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    // Spawns fine, but never speaks ACP: exits before answering `initialize`.
    let dying = AgentCommand::parse("/usr/bin/false").unwrap();
    let began = tokio::time::Instant::now();
    let _tx = session::start(uplink.clone(), "r0".into(), "s1".into(), dying, std::env::temp_dir());
    // Uses the production `start` (75s START_TIMEOUT), not a shortened test
    // timeout: the point is that a dead process fails fast on its own,
    // without waiting anywhere near that deadline.
    let frames = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frames = uplink.pending().unwrap();
            if !frames.is_empty() {
                return frames;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("start_failed did not arrive quickly");
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "took {:?}, expected well under the 75s START_TIMEOUT",
        began.elapsed()
    );
    assert_eq!(kinds(&frames), ["start_failed"]);
}

#[tokio::test]
async fn an_adapter_that_hangs_on_start_reports_start_failed() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    // Never answers `initialize`: simulates an adapter that hangs on start.
    let hanger = AgentCommand::parse("/bin/sleep 100").unwrap();
    let _tx = session::start_with_timeout(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        hanger,
        std::env::temp_dir(),
        Duration::from_millis(200),
    );
    let frames = wait_until(&uplink, |f| !f.is_empty()).await;
    assert_eq!(kinds(&frames), ["start_failed"]);
    match frames.last().unwrap() {
        HostFrame::Session {
            body: SessionBody::StartFailed { message, .. },
            ..
        } => {
            assert!(message.contains("did not start"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_invalid_prompt_does_not_consume_its_turn_id() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let tx = session::start(uplink.clone(), "r0".into(), "s1".into(), fake, std::env::temp_dir());
    wait_until(&uplink, |f| !f.is_empty()).await;

    // Reject an empty prompt under turn_id "t1"...
    tx.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![],
    })
    .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), replies.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, HostFrame::Error { ref code, .. } if code == "invalid"),
        "{reply:?}"
    );

    // ...then a corrected retry under the SAME turn_id must still run.
    tx.send(SessionCmd::Prompt {
        request_id: "r2".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type":"text","text":"hi"})],
    })
    .unwrap();
    let frames = wait_until(&uplink, |f| kinds(f).contains(&"turn_ended".to_string())).await;
    let kinds = kinds(&frames);
    assert_eq!(kinds.iter().filter(|k| *k == "turn_started").count(), 1, "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| *k == "turn_ended").count(), 1, "{kinds:?}");
}

#[tokio::test]
async fn an_unparseable_prompt_is_rejected_without_starting_a_turn() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let tx = session::start(uplink.clone(), "r0".into(), "s1".into(), fake, std::env::temp_dir());
    wait_until(&uplink, |f| !f.is_empty()).await;
    tx.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type":"bogus"})],
    })
    .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), replies.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, HostFrame::Error { ref code, .. } if code == "invalid"),
        "{reply:?}"
    );
    assert_eq!(kinds(&uplink.pending().unwrap()), ["session_started"]);
}
