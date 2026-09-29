//! The host's session actor against the fake adapter, without a collector:
//! the outbox must receive session_started, then per turn turn_started, the
//! adapter's updates verbatim, and exactly one turn_ended, in that order.

use hennery_host::outbox::Outbox;
use hennery_host::session::{self, AgentCommand, SessionCmd, SessionHandle, SessionOptions};
use hennery_host::uplink::Uplink;
use hennery_proto::frames::{HostFrame, SessionBody, TurnOutcome};
use hennery_testkit::{FakeScript, SCRIPT_ENV, pid_alive};
use serde_json::json;
use std::path::Path;
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
                SessionBody::SessionParked { reason } => {
                    format!(
                        "session_parked:{}",
                        serde_json::to_value(reason).unwrap().as_str().unwrap()
                    )
                }
                SessionBody::SessionClosed => "session_closed".to_string(),
                SessionBody::AdapterExited { .. } => "adapter_exited".to_string(),
                SessionBody::HostNote { note, .. } => format!("host_note:{note}"),
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

    assert!(tx.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type":"text","text":"hi"})],
    }));
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
    assert!(tx.send(prompt()));
    assert!(tx.send(prompt()));
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
    assert!(tx.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![],
    }));
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
    let _tx = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        hanger,
        std::env::temp_dir(),
        SessionOptions {
            start_timeout: Duration::from_millis(200),
            ..SessionOptions::default()
        },
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
    assert!(tx.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![],
    }));
    let reply = tokio::time::timeout(Duration::from_secs(5), replies.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, HostFrame::Error { ref code, .. } if code == "invalid"),
        "{reply:?}"
    );

    // ...then a corrected retry under the SAME turn_id must still run.
    assert!(tx.send(SessionCmd::Prompt {
        request_id: "r2".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type":"text","text":"hi"})],
    }));
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
    assert!(tx.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type":"bogus"})],
    }));
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

/// The fake adapter with a script.
fn fake_with(script: &FakeScript) -> AgentCommand {
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    fake
}

/// The fake adapter, but with SIGTERM ignored (`trap '' TERM` survives the
/// `exec` into the same process, per POSIX): its teardown must ride out the
/// full kill grace before SIGKILL lands, exactly like a real agent CLI that
/// does not react to SIGTERM.
fn fake_ignoring_sigterm(script: &FakeScript) -> AgentCommand {
    AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!("trap '' TERM; exec {}", env!("CARGO_BIN_EXE_hennery-fake-acp")),
        ],
        env: vec![(SCRIPT_ENV.into(), serde_json::to_string(script).unwrap())],
    }
}

fn slow_script() -> FakeScript {
    FakeScript {
        chunks: (1..=20).map(|n| n.to_string()).collect(),
        chunk_delay_ms: 100,
        ..FakeScript::default()
    }
}

fn prompt(request_id: &str, turn_id: &str) -> SessionCmd {
    SessionCmd::Prompt {
        request_id: request_id.into(),
        turn_id: turn_id.into(),
        content: vec![json!({"type":"text","text":"hi"})],
    }
}

fn has(kind: &str) -> impl Fn(&[HostFrame]) -> bool + '_ {
    move |frames| kinds(frames).iter().any(|k| k == kind)
}

async fn read_pid(path: &Path) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|s| s.parse().ok()) {
            return pid;
        }
        assert!(tokio::time::Instant::now() < deadline, "no pid file");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_dead(pid: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while pid_alive(pid) {
        assert!(tokio::time::Instant::now() < deadline, "grandchild {pid} survived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_ended(handle: &SessionHandle) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !handle.is_ended() {
        assert!(tokio::time::Instant::now() < deadline, "actor did not end");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn session_with_grandchild(uplink: &Uplink, dir: &Path) -> (SessionHandle, std::path::PathBuf) {
    let pid_file = dir.join("grandchild.pid");
    let script = FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..slow_script()
    };
    let handle = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&script),
        std::env::temp_dir(),
        SessionOptions {
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    );
    (handle, pid_file)
}

#[tokio::test]
async fn an_adapter_crash_mid_turn_interrupts_the_turn_and_parks_the_session() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: vec!["1".into(), "2".into(), "3".into()],
        chunk_delay_ms: 50,
        exit_after_chunks: Some(1),
        stderr_lines: vec!["auth header: Bearer secret-token-123".into()],
        ..FakeScript::default()
    };
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&script),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("session_parked:adapter_exited")).await;
    assert_eq!(
        kinds(&frames),
        [
            "session_started",
            "turn_started",
            "update:1",
            "turn_ended",
            "adapter_exited",
            "session_parked:adapter_exited"
        ]
    );
    for frame in &frames {
        match frame {
            HostFrame::Session {
                body: SessionBody::TurnEnded { outcome, .. },
                ..
            } => assert_eq!(*outcome, TurnOutcome::Interrupted),
            HostFrame::Session {
                body: SessionBody::AdapterExited { code, stderr_tail, .. },
                ..
            } => {
                assert_eq!(*code, Some(hennery_testkit::CRASH_EXIT_CODE));
                assert!(stderr_tail.contains("Bearer [redacted]"), "{stderr_tail}");
                assert!(!stderr_tail.contains("secret-token-123"), "{stderr_tail}");
                assert!(stderr_tail.contains("crashing mid-turn"), "{stderr_tail}");
            }
            _ => {}
        }
    }
    wait_ended(&handle).await;
    assert!(!handle.send(prompt("r2", "t2")), "an ended actor accepts no prompt");
}

#[tokio::test]
async fn park_mid_turn_interrupts_the_turn_and_kills_the_adapters_group() {
    let dir = tempfile::tempdir().unwrap();
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let (handle, pid_file) = session_with_grandchild(&uplink, dir.path());
    let grandchild = read_pid(&pid_file).await;
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("update:1")).await;
    assert!(handle.send(SessionCmd::Park {
        request_id: "rp".into()
    }));
    let frames = wait_until(&uplink, has("session_parked:operator")).await;
    let kinds = kinds(&frames);
    let tail: Vec<&str> = kinds.iter().rev().take(2).rev().map(String::as_str).collect();
    assert_eq!(tail, ["turn_ended", "session_parked:operator"], "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| *k == "turn_ended").count(), 1);
    assert!(
        !kinds.contains(&"adapter_exited".to_string()),
        "a requested kill is not an exit: {kinds:?}"
    );
    wait_dead(grandchild).await;
    wait_ended(&handle).await;
}

#[tokio::test]
async fn close_emits_session_closed_and_kills_the_adapters_group() {
    let dir = tempfile::tempdir().unwrap();
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let (handle, pid_file) = session_with_grandchild(&uplink, dir.path());
    let grandchild = read_pid(&pid_file).await;
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(SessionCmd::Close {
        request_id: "rc".into()
    }));
    let frames = wait_until(&uplink, has("session_closed")).await;
    assert_eq!(kinds(&frames), ["session_started", "session_closed"]);
    wait_dead(grandchild).await;
    wait_ended(&handle).await;
}

#[tokio::test]
async fn dropping_every_handle_kills_the_adapter_without_emitting_anything() {
    let dir = tempfile::tempdir().unwrap();
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let (handle, pid_file) = session_with_grandchild(&uplink, dir.path());
    let grandchild = read_pid(&pid_file).await;
    wait_until(&uplink, has("session_started")).await;
    drop(handle); // host shutdown: the registry is gone
    wait_dead(grandchild).await;
    assert_eq!(kinds(&uplink.pending().unwrap()), ["session_started"]);
}

#[tokio::test]
async fn a_prompt_during_a_turn_is_refused_and_the_open_turn_is_reported() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&slow_script()),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert_eq!(handle.open_turn_id(), None);
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    assert_eq!(handle.open_turn_id().as_deref(), Some("t1"));
    assert!(handle.send(prompt("r2", "t2")));
    let reply = tokio::time::timeout(Duration::from_secs(5), replies.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, HostFrame::Error { ref request_id, ref code, .. } if request_id == "r2" && code == "turn_in_progress"),
        "{reply:?}"
    );
    wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(handle.open_turn_id(), None);
    // The refused prompt did not consume t2: it can run now.
    assert!(handle.send(prompt("r3", "t2")));
    let frames = wait_until(&uplink, |f| kinds(f).iter().filter(|k| *k == "turn_ended").count() == 2).await;
    assert_eq!(kinds(&frames).iter().filter(|k| *k == "turn_started").count(), 2);
}

#[tokio::test]
async fn a_repeated_start_re_emits_session_started_with_the_new_request_id() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&FakeScript::default()),
        std::env::temp_dir(),
    );
    assert!(handle.send(SessionCmd::Restart {
        request_id: "r9".into()
    }));
    let frames = wait_until(&uplink, |f| {
        kinds(f).iter().filter(|k| *k == "session_started").count() == 2
    })
    .await;
    let ids: Vec<(String, String)> = frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body:
                    SessionBody::SessionStarted {
                        request_id,
                        agent_session_id,
                    },
                ..
            } => Some((request_id.clone(), agent_session_id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        [
            ("r0".to_string(), "fake-session-1".to_string()),
            ("r9".to_string(), "fake-session-1".to_string())
        ]
    );
}

/// The adapter's stdout reaches EOF 300 ms before its process exits, so the
/// prompt fails before the exit watcher sees the exit. The turn must still
/// end `interrupted` (the adapter is gone), not `failed`.
#[tokio::test]
async fn a_prompt_that_fails_because_the_adapter_is_dying_ends_interrupted() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: vec!["1".into(), "2".into()],
        exit_after_chunks: Some(1),
        ..FakeScript::default()
    };
    let wrapper = AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!("{} ; exec >&- ; sleep 0.3", env!("CARGO_BIN_EXE_hennery-fake-acp")),
        ],
        env: vec![(SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap())],
    };
    let handle = session::start(uplink.clone(), "r0".into(), "s1".into(), wrapper, std::env::temp_dir());
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("session_parked:adapter_exited")).await;
    let outcomes: Vec<TurnOutcome> = frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::TurnEnded { outcome, .. },
                ..
            } => Some(*outcome),
            _ => None,
        })
        .collect();
    assert_eq!(outcomes, [TurnOutcome::Interrupted], "{:?}", kinds(&frames));
}

/// Commands that reach an actor while it is ending (its adapter is being
/// killed, or has exited) must be answered, not dropped: the collector would
/// otherwise wait out its timeout and drop the whole host connection.
#[tokio::test]
async fn commands_queued_behind_an_ending_actor_are_answered_not_attached() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&FakeScript::default()),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    // Queued back to back: the actor sees the park first and ends.
    assert!(handle.send(SessionCmd::Park {
        request_id: "r1".into()
    }));
    assert!(handle.send(prompt("r2", "t2")));
    assert!(handle.send(SessionCmd::Close {
        request_id: "r3".into()
    }));
    let mut refused = Vec::new();
    for _ in 0..2 {
        let reply = tokio::time::timeout(Duration::from_secs(10), replies.recv())
            .await
            .expect("a reply, not silence")
            .unwrap();
        match reply {
            HostFrame::Error { request_id, code, .. } => refused.push((request_id, code)),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        refused,
        [
            ("r2".to_string(), "not_attached".to_string()),
            ("r3".to_string(), "not_attached".to_string())
        ]
    );
    wait_until(&uplink, has("session_parked:operator")).await;
}

/// A start that reaches this actor while it is still tearing down (mid
/// `kill_grace`, because its adapter ignores SIGTERM) must be answered, not
/// dropped: the connection routes a repeated `start_session` for an attached
/// session to `Restart` as long as `is_ended()` is false, but by the time the
/// post-teardown drain reads it the session really has ended. Without an
/// answer, the collector's start waiter sits out its own timeout and then
/// (maintainer decision 1) drops the whole host connection.
#[tokio::test]
async fn a_start_that_reaches_an_ending_actor_is_answered_not_attached() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_ignoring_sigterm(&FakeScript::default()),
        std::env::temp_dir(),
        SessionOptions {
            kill_grace: Duration::from_millis(300),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(SessionCmd::Park {
        request_id: "rp".into()
    }));
    // The actor is now inside `teardown()`, riding out the full kill grace
    // because the adapter ignores SIGTERM: it has not read this yet, and its
    // handle is not `is_ended()` yet either — exactly what routes a real
    // `start_session` here to `Restart` instead of a fresh spawn.
    assert!(!handle.is_ended());
    assert!(handle.send(SessionCmd::Restart {
        request_id: "r5".into()
    }));
    let reply = tokio::time::timeout(Duration::from_secs(10), replies.recv())
        .await
        .expect("a reply, not silence")
        .unwrap();
    assert!(
        matches!(&reply, HostFrame::Error { request_id, code, .. } if request_id == "r5" && code == "not_attached"),
        "{reply:?}"
    );
    wait_until(&uplink, has("session_parked:operator")).await;
    wait_ended(&handle).await;
}

/// Every `update:*` must sit between the `turn_started`/`turn_ended` pair of
/// the turn that produced it: an update outside that window means it landed
/// after its turn had already ended (or before it started).
fn assert_updates_stay_inside_their_turns(kinds: &[String]) {
    let mut in_turn = false;
    for kind in kinds {
        if kind == "turn_started" {
            in_turn = true;
        } else if kind == "turn_ended" {
            in_turn = false;
        } else if kind.starts_with("update:") {
            assert!(in_turn, "an update landed outside any open turn: {kinds:?}");
        }
    }
}

/// Regression test for a multi-thread-only race: the ACP connection runs in
/// its own task, on its own worker thread. It can push a `session/update`
/// notification and then resolve the matching prompt reply in quick
/// succession; the actor's `select!` can observe the reply as ready before
/// it happens to observe the notification in the very same poll, emitting
/// `turn_ended` first and the update after, as a stray. A single-thread
/// runtime can never interleave the two tasks like this, so this needs
/// `flavor = "multi_thread"` to have a chance of ever exercising the gap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn updates_never_land_outside_their_turn_under_a_multi_thread_runtime() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: (1..=30).map(|n| n.to_string()).collect(),
        chunk_delay_ms: 0,
        ..FakeScript::default()
    };
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&script),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    for i in 0..40 {
        assert!(handle.send(prompt(&format!("r{i}"), &format!("t{i}"))));
        let ended = i + 1;
        wait_until(&uplink, move |f| {
            kinds(f).iter().filter(|k| *k == "turn_ended").count() == ended
        })
        .await;
    }
    let frames = uplink.pending().unwrap();
    assert_updates_stay_inside_their_turns(&kinds(&frames));
}

fn reaping(uplink: &Uplink, script: &FakeScript, idle: Option<Duration>) -> SessionHandle {
    session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(script),
        std::env::temp_dir(),
        SessionOptions {
            idle_timeout: idle,
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    )
}

#[tokio::test]
async fn an_idle_session_is_reaped_its_group_killed_and_parked() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let script = FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..FakeScript::default()
    };
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = reaping(&uplink, &script, Some(Duration::from_millis(300)));
    let grandchild = read_pid(&pid_file).await;
    let frames = wait_until(&uplink, has("session_parked:idle")).await;
    assert_eq!(kinds(&frames), ["session_started", "session_parked:idle"]);
    wait_dead(grandchild).await;
    wait_ended(&handle).await;
}

#[tokio::test]
async fn the_reaper_never_parks_a_session_mid_turn() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    // A 2 s turn against a 300 ms idle window.
    let handle = reaping(&uplink, &slow_script(), Some(Duration::from_millis(300)));
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        !has("session_parked:idle")(&uplink.pending().unwrap()),
        "reaped mid-turn"
    );
    // Once the turn is over, the window starts again and the reaper parks it.
    let frames = wait_until(&uplink, has("session_parked:idle")).await;
    let kinds = kinds(&frames);
    let ended = kinds.iter().position(|k| k == "turn_ended").expect("turn ended");
    let parked = kinds.iter().position(|k| k == "session_parked:idle").unwrap();
    assert!(ended < parked, "{kinds:?}");
}

#[tokio::test]
async fn a_disabled_reaper_leaves_an_idle_session_attached() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = reaping(&uplink, &FakeScript::default(), None);
    wait_until(&uplink, has("session_started")).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(kinds(&uplink.pending().unwrap()), ["session_started"]);
    assert!(!handle.is_ended());
}

/// Every replayed kind the fake can send: six history kinds, two state
/// kinds and one this build does not know.
fn replay_script() -> FakeScript {
    FakeScript {
        replay: vec![
            json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "old question"}}),
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "old answer"}}),
            json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "old thought"}}),
            json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "ls"}),
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed"}),
            json!({"sessionUpdate": "plan", "entries": []}),
            json!({"sessionUpdate": "available_commands_update", "availableCommands": []}),
            json!({"sessionUpdate": "current_mode_update", "currentModeId": "plan"}),
            json!({"sessionUpdate": "from_the_future"}),
        ],
        ..FakeScript::default()
    }
}

fn resuming(uplink: &Uplink, script: &FakeScript) -> SessionHandle {
    session::resume(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        "agent-7".into(),
        fake_with(script),
        std::env::temp_dir(),
        SessionOptions::default(),
    )
}

/// `sessionUpdate` of every `acp_update` frame, with its `turn_id` extract.
fn updates_of(frames: &[HostFrame]) -> Vec<(String, Option<String>)> {
    frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::AcpUpdate { indexed, payload },
                ..
            } => Some((
                payload["update"]["sessionUpdate"].as_str().unwrap_or("?").to_string(),
                indexed.turn_id.clone(),
            )),
            _ => None,
        })
        .collect()
}

fn start_failed_code(frames: &[HostFrame]) -> Option<String> {
    frames.iter().find_map(|f| match f {
        HostFrame::Session {
            body: SessionBody::StartFailed { code, .. },
            ..
        } => Some(code.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn a_resume_loads_the_agents_session_drops_replayed_history_and_keeps_state() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = resuming(&uplink, &replay_script());
    let frames = wait_until(&uplink, has("host_note:replay_unknown_dropped")).await;
    assert_eq!(
        kinds(&frames),
        [
            "session_started",
            "update:?",
            "update:?",
            "host_note:replay_unknown_dropped"
        ]
    );
    let HostFrame::Session {
        body: SessionBody::SessionStarted { agent_session_id, .. },
        ..
    } = &frames[0]
    else {
        panic!("{frames:?}");
    };
    assert_eq!(agent_session_id, "agent-7");
    assert_eq!(
        updates_of(&frames),
        [
            ("available_commands_update".to_string(), None),
            ("current_mode_update".to_string(), None)
        ]
    );
    // The adapter loaded the id it was given: it streams under that id.
    for frame in &frames[1..3] {
        let HostFrame::Session {
            body: SessionBody::AcpUpdate { payload, .. },
            ..
        } = frame
        else {
            panic!("{frame:?}");
        };
        assert_eq!(payload["sessionId"], "agent-7");
    }
    let HostFrame::Session {
        body: SessionBody::HostNote { text, .. },
        ..
    } = &frames[3]
    else {
        panic!("{frames:?}");
    };
    assert!(text.contains("from_the_future ×1"), "{text}");

    // The resumed session takes prompts; live updates carry their turn.
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(
        kinds(&frames)[4..],
        ["turn_started", "update:Hello", "update: world", "turn_ended"]
    );
    let live: Vec<(String, Option<String>)> = updates_of(&frames).into_iter().skip(2).collect();
    assert_eq!(
        live,
        [
            ("agent_message_chunk".to_string(), Some("t1".to_string())),
            ("agent_message_chunk".to_string(), Some("t1".to_string()))
        ]
    );
}

#[tokio::test]
async fn a_resume_the_agent_has_no_record_of_fails_agent_has_no_record() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        load_error: Some(-32002),
        ..replay_script()
    };
    let handle = resuming(&uplink, &script);
    let frames = wait_until(&uplink, has("start_failed")).await;
    // Nothing the failed load replayed leaks out.
    assert_eq!(kinds(&frames), ["start_failed"]);
    assert_eq!(start_failed_code(&frames).as_deref(), Some("agent_has_no_record"));
    wait_ended(&handle).await;
}

#[tokio::test]
async fn an_agent_that_is_not_logged_in_fails_start_and_resume_as_agent_not_logged_in() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        new_session_error: Some(-32000),
        ..FakeScript::default()
    };
    let _new = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&script),
        std::env::temp_dir(),
    );
    let frames = wait_until(&uplink, has("start_failed")).await;
    assert_eq!(start_failed_code(&frames).as_deref(), Some("agent_not_logged_in"));

    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        load_error: Some(-32000),
        ..FakeScript::default()
    };
    let _resumed = resuming(&uplink, &script);
    let frames = wait_until(&uplink, has("start_failed")).await;
    assert_eq!(start_failed_code(&frames).as_deref(), Some("agent_not_logged_in"));
}

#[tokio::test]
async fn a_resume_on_an_adapter_without_session_load_fails_load_unsupported() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        no_load_session: true,
        ..FakeScript::default()
    };
    let _handle = resuming(&uplink, &script);
    let frames = wait_until(&uplink, has("start_failed")).await;
    assert_eq!(start_failed_code(&frames).as_deref(), Some("load_unsupported"));
}

/// The replay-suppression twin of the multi-thread ordering test above: a
/// replayed notification can still be in the channel when the load's answer
/// resolves. It arrived before the answer, so it is history and must be
/// dropped, never emitted as if it were live.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replayed_history_never_leaks_under_a_multi_thread_runtime() {
    // History last: the notification right before the answer is the one
    // that can race it, and a leaked history chunk is visible.
    let mut replay = vec![json!({"sessionUpdate": "available_commands_update", "availableCommands": []})];
    replay.extend(
        (0..200).map(
            |n| json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": n.to_string()}}),
        ),
    );
    let script = FakeScript {
        replay,
        ..FakeScript::default()
    };
    for _ in 0..30 {
        let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
        let handle = resuming(&uplink, &script);
        wait_until(&uplink, has("session_started")).await;
        // Give a leaked replay frame time to land behind the start.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let frames = uplink.pending().unwrap();
        assert_eq!(kinds(&frames), ["session_started", "update:?"]);
        drop(handle);
    }
}

// Plan B2a: cancel (ACP core §3.3 `cancel_turn`, §4.4).

fn cancel(request_id: &str, turn_id: &str) -> SessionCmd {
    SessionCmd::Cancel {
        request_id: request_id.into(),
        turn_id: turn_id.into(),
    }
}

/// Every `turn_ended` in the outbox: (turn id, outcome, error).
fn turn_ends(frames: &[HostFrame]) -> Vec<(String, TurnOutcome, Option<String>)> {
    frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body:
                    SessionBody::TurnEnded {
                        turn_id,
                        outcome,
                        error,
                        ..
                    },
                ..
            } => Some((turn_id.clone(), *outcome, error.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_cancel_ends_the_turn_cancelled_and_the_session_stays_attached() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&slow_script()),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    // Twice, as a retried request would: one `session/cancel`, one end.
    assert!(handle.send(cancel("rc1", "t1")));
    assert!(handle.send(cancel("rc2", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(turn_ends(&frames), [("t1".to_string(), TurnOutcome::Cancelled, None)]);
    let stop_reasons: Vec<Option<String>> = frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::TurnEnded { stop_reason, .. },
                ..
            } => Some(stop_reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(stop_reasons, [Some("cancelled".to_string())]);
    assert!(
        kinds(&frames).iter().filter(|k| k.starts_with("update:")).count() < 20,
        "the turn ran to its end: {:?}",
        kinds(&frames)
    );
    assert_eq!(handle.open_turn_id(), None);
    // Still attached: the next prompt runs.
    assert!(handle.send(SessionCmd::Prompt {
        request_id: "r2".into(),
        turn_id: "t2".into(),
        content: vec![json!({"type":"text","text":"again"})],
    }));
    wait_until(&uplink, |f| turn_ends(f).len() == 2).await;
    assert!(replies.try_recv().is_err(), "a cancel was answered with an error");
}

#[tokio::test]
async fn a_cancel_for_a_turn_that_is_not_running_is_refused_not_running() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&FakeScript::default()),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    // No turn at all, then a turn that has already ended.
    assert!(handle.send(cancel("rc1", "t0")));
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_ended")).await;
    assert!(handle.send(cancel("rc2", "t1")));
    let mut refused = Vec::new();
    for _ in 0..2 {
        match tokio::time::timeout(Duration::from_secs(5), replies.recv())
            .await
            .unwrap()
            .unwrap()
        {
            HostFrame::Error { request_id, code, .. } => refused.push((request_id, code)),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        refused,
        [
            ("rc1".to_string(), "not_running".to_string()),
            ("rc2".to_string(), "not_running".to_string())
        ]
    );
    let frames = uplink.pending().unwrap();
    assert_eq!(turn_ends(&frames), [("t1".to_string(), TurnOutcome::Completed, None)]);
}

#[tokio::test]
async fn an_adapter_that_ignores_a_cancel_is_stopped_after_the_grace() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let script = FakeScript {
        ignore_cancel: true,
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        stderr_lines: vec!["auth header: Bearer secret-token-123".into()],
        ..slow_script()
    };
    let handle = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&script),
        std::env::temp_dir(),
        SessionOptions {
            cancel_grace: Duration::from_millis(300),
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    );
    let grandchild = read_pid(&pid_file).await;
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    assert!(handle.send(cancel("rc", "t1")));
    let frames = wait_until(&uplink, has("session_parked:operator")).await;
    let ends = turn_ends(&frames);
    assert_eq!(ends.len(), 1, "{:?}", kinds(&frames));
    assert_eq!((ends[0].0.as_str(), ends[0].1), ("t1", TurnOutcome::Cancelled));
    let tail: Vec<String> = kinds(&frames)
        .into_iter()
        .filter(|k| !k.starts_with("update:"))
        .collect();
    assert_eq!(
        tail,
        [
            "session_started",
            "turn_started",
            "turn_ended",
            "host_note:cancel_unanswered",
            "session_parked:operator"
        ]
    );
    // No `adapter_exited` on this path: the note keeps the stderr tail,
    // scrubbed.
    let note = frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::HostNote { text, .. },
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert!(note.contains("Last stderr:") && note.contains("auth header"), "{note}");
    assert!(!note.contains("secret-token-123"), "{note}");
    wait_dead(grandchild).await;
    wait_ended(&handle).await;
}

/// An agent may answer an aborted prompt with an error rather than the
/// `cancelled` stop reason: after a cancel that is still `cancelled`, with
/// the error kept; without one it would be `failed`.
#[tokio::test]
async fn a_cancelled_prompt_answered_with_an_error_still_ends_cancelled() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        cancel_error: Some(-32603),
        ..slow_script()
    };
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&script),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    assert!(handle.send(cancel("rc", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    let ends = turn_ends(&frames);
    assert_eq!(ends.len(), 1);
    assert_eq!((ends[0].0.as_str(), ends[0].1), ("t1", TurnOutcome::Cancelled));
    assert!(ends[0].2.as_deref().is_some_and(|e| e.contains("aborted")), "{ends:?}");
}
