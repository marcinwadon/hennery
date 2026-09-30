//! The host's session actor against the fake adapter, without a collector:
//! the outbox must receive session_started, then per turn turn_started, the
//! adapter's updates verbatim, and exactly one turn_ended, in that order.

use hennery_host::outbox::Outbox;
use hennery_host::session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
use hennery_host::uplink::Uplink;
use hennery_proto::frames::{ConfigValue, HostFrame, Indexed, SessionBody, SessionConfig, TurnOutcome};
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
                SessionBody::ConfigApplied { .. } => "config_applied".to_string(),
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
                        ..
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

// Plan B2b: model, axes and mode (ACP core §4.3).

/// A fake with the sample catalogue whose model switch resets the mode,
/// logging every switch to `log`.
fn config_script(log: &Path) -> FakeScript {
    FakeScript {
        config_options: hennery_testkit::sample_config_options(),
        model_switch_sets_mode: Some("default".into()),
        config_log: Some(log.to_string_lossy().into_owned()),
        ..FakeScript::default()
    }
}

fn wanted(model: Option<&str>, mode: Option<&str>, axes: &[(&str, ConfigValue)]) -> SessionConfig {
    SessionConfig {
        model: model.map(str::to_string),
        mode: mode.map(str::to_string),
        axes: axes.iter().map(|(id, v)| (id.to_string(), v.clone())).collect(),
    }
}

fn launching(
    uplink: &Uplink,
    script: &FakeScript,
    attach: Attach,
    config: SessionConfig,
    options: SessionOptions,
) -> SessionHandle {
    let launch = Launch {
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach,
        config,
        agent: fake_with(script),
        cwd: std::env::temp_dir(),
    };
    session::launch(uplink.clone(), launch, options)
}

/// The catalogue extracts of the first `session_started`.
fn started_extracts(frames: &[HostFrame]) -> Indexed {
    frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::SessionStarted { indexed, .. },
                ..
            } => Some(indexed.clone()),
            _ => None,
        })
        .expect("a session_started")
}

fn note_text(frames: &[HostFrame], code: &str) -> String {
    frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::HostNote { note, text },
                ..
            } if note == code => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no host_note {code}: {:?}", kinds(frames)))
}

fn switches(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

#[tokio::test]
async fn a_start_applies_the_model_then_the_axes_then_the_mode_and_announces_the_result() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let config = wanted(
        Some("large"),
        Some("plan"),
        &[
            ("effort", ConfigValue::Id("high".into())),
            ("fast", ConfigValue::Bool(true)),
        ],
    );
    let _handle = launching(
        &uplink,
        &config_script(&log),
        Attach::New,
        config.clone(),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    // Mode last: the model switch reset it, and it is set again after.
    assert_eq!(switches(&log), "model=large\neffort=high\nfast=true\nmode=plan\n");
    let indexed = started_extracts(&frames);
    assert_eq!(indexed.current_config(), Some(config));
    assert_eq!(indexed.config_options.map(|o| o.len()), Some(4));
    assert_eq!(kinds(&frames), ["session_started"]);
}

#[tokio::test]
async fn values_already_current_are_not_switched_and_the_catalogue_is_still_announced() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let config = wanted(Some("small"), Some("default"), &[("fast", ConfigValue::Bool(false))]);
    let _handle = launching(
        &uplink,
        &config_script(&log),
        Attach::New,
        config.clone(),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    assert_eq!(switches(&log), "");
    let current = started_extracts(&frames).current_config().unwrap();
    assert_eq!((current.model, current.mode), (config.model, config.mode));
}

/// A picker value the adapter does not offer (a stale catalogue) must not
/// cost the operator the session: it starts, and a note says what did not
/// take. The model the adapter refused is never announced as current.
#[tokio::test]
async fn a_start_whose_switches_fail_still_starts_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let config = wanted(Some("huge"), Some("plan"), &[("nope", ConfigValue::Id("x".into()))]);
    let _handle = launching(
        &uplink,
        &config_script(&log),
        Attach::New,
        config,
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    assert_eq!(kinds(&frames), ["session_started", "host_note:config_failed"]);
    // The adapter refused `huge`; `nope` was never sent.
    assert_eq!(switches(&log), "model=huge\nmode=plan\n");
    let current = started_extracts(&frames).current_config().unwrap();
    assert_eq!(
        (current.model.as_deref(), current.mode.as_deref()),
        (Some("small"), Some("plan"))
    );
    let text = note_text(&frames, "config_failed");
    assert!(text.contains("model=huge") && text.contains("nope"), "{text}");
}

#[tokio::test]
async fn a_resume_re_applies_the_stored_config_and_a_failed_re_apply_is_only_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        replay: vec![json!({"sessionUpdate": "available_commands_update", "availableCommands": []})],
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::Load {
            agent_session_id: "agent-7".into(),
        },
        wanted(Some("huge"), Some("bypass"), &[]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("host_note:reapply_failed")).await;
    assert_eq!(
        kinds(&frames),
        ["session_started", "update:?", "host_note:reapply_failed"]
    );
    assert_eq!(
        started_extracts(&frames).current_mode.as_deref(),
        Some("bypass"),
        "the mode survived the resume"
    );
    // Still attached: the failed re-apply did not fail the resume.
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_ended")).await;
}

/// An adapter that answers a switch without a catalogue, or never answers,
/// leaves hennery not knowing the current values: it announces none, so
/// the collector keeps what it stored (a resume's stored mode is not
/// overwritten with a guess).
#[tokio::test]
async fn a_switch_without_a_read_back_announces_no_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        empty_config_read_back: true,
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(Some("large"), None, &[]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    assert_eq!(switches(&log), "model=large\n");
    assert_eq!(started_extracts(&frames), Indexed::default());
    assert_eq!(kinds(&frames), ["session_started"]);
}

#[tokio::test]
async fn a_hung_switch_is_reported_and_the_session_still_starts() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(Some("large"), Some("plan"), &[]),
        SessionOptions {
            config_timeout: Duration::from_millis(200),
            ..SessionOptions::default()
        },
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    assert_eq!(kinds(&frames), ["session_started", "host_note:config_failed"]);
    assert_eq!(started_extracts(&frames), Indexed::default());
    let text = note_text(&frames, "config_failed");
    assert!(text.contains("model=large: no answer within"), "{text}");
    // Nothing is sent after a switch that did not answer.
    assert!(
        text.contains("mode=plan: not sent: an earlier switch did not answer"),
        "{text}"
    );
    assert_eq!(switches(&log), "model=large\n");
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_ended")).await;
}

/// The real adapters handle requests concurrently. A model switch that
/// answers after its timeout can still land, and clamp the mode, after a
/// mode switch sent behind it has answered: model first and mode last would
/// silently break. So nothing more is sent once a switch has not answered.
#[tokio::test]
async fn a_late_model_switch_stops_the_starts_switches_so_its_clamp_cannot_undo_the_mode() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        slow_model_switch_ms: Some(600),
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(Some("large"), Some("plan"), &[]),
        SessionOptions {
            config_timeout: Duration::from_millis(200),
            ..SessionOptions::default()
        },
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    // Give a wrongly sent mode switch time to reach the fake.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        switches(&log),
        "model=large\n",
        "the mode switch went out behind a late model switch"
    );
    assert_eq!(started_extracts(&frames), Indexed::default(), "the values are unknown");
    let text = note_text(&frames, "config_failed");
    assert!(
        text.contains("mode=plan: not sent: an earlier switch did not answer"),
        "{text}"
    );
}

#[tokio::test]
async fn a_switch_cut_off_by_the_start_deadline_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(Some("large"), Some("plan"), &[]),
        SessionOptions {
            start_timeout: Duration::from_secs(1),
            config_timeout: Duration::from_secs(10),
            ..SessionOptions::default()
        },
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    assert_eq!(kinds(&frames), ["session_started", "host_note:config_failed"]);
    let text = note_text(&frames, "config_failed");
    assert!(
        text.contains("model=large: no answer before the start deadline"),
        "{text}"
    );
    assert!(text.contains("mode=plan: not sent"), "{text}");
}

/// An adapter may accept a value and report another. What it reports is
/// what counts, so the note says so.
#[tokio::test]
async fn a_value_the_agent_accepts_but_does_not_report_is_noted() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        sticky_options: vec!["effort".into()],
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(None, None, &[("effort", ConfigValue::Id("high".into()))]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    assert_eq!(switches(&log), "effort=high\n");
    let text = note_text(&frames, "config_failed");
    assert!(text.contains("effort: asked high, agent reports low"), "{text}");
    let axes = started_extracts(&frames).current_axes.unwrap();
    assert_eq!(axes.get("effort"), Some(&ConfigValue::Id("low".into())));
}

/// An adapter that announces its options in a `config_option_update` just
/// before answering `session/new` or `session/load`, not in the answer:
/// those options are the ones switched, and that update, older than the
/// announced catalogue, carries none.
#[tokio::test]
async fn options_announced_in_an_update_before_the_answer_are_the_ones_switched() {
    for attach in [
        Attach::New,
        Attach::Load {
            agent_session_id: "agent-7".into(),
        },
    ] {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("config.log");
        let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
        let script = FakeScript {
            config_in_update_only: true,
            ..config_script(&log)
        };
        let _handle = launching(
            &uplink,
            &script,
            attach.clone(),
            wanted(None, Some("plan"), &[]),
            SessionOptions::default(),
        );
        let frames = wait_until(&uplink, has("update:?")).await;
        assert_eq!(kinds(&frames), ["session_started", "update:?"], "{attach:?}");
        assert_eq!(switches(&log), "mode=plan\n", "{attach:?}");
        assert_eq!(started_extracts(&frames).current_mode.as_deref(), Some("plan"));
        let HostFrame::Session {
            body: SessionBody::AcpUpdate { indexed, .. },
            ..
        } = &frames[1]
        else {
            panic!("{frames:?}");
        };
        assert_eq!(*indexed, Indexed::default(), "{attach:?}");
    }
}

// Fix round 1: a stale or seeded catalogue must never gate a skip decision.

/// The model switch clamps the mode to "plan" (`model_switch_sets_mode`),
/// but its own read-back is empty, so the host does not know that: the
/// pre-switch catalogue (still showing mode=default) is now stale. The
/// mode=default switch must still be sent — skipping it as "already
/// current" would leave the session in the clamped "plan" mode the operator
/// never asked for.
#[tokio::test]
async fn a_switch_without_a_read_back_does_not_skip_later_values_on_stale_data() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        empty_config_read_back: true,
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(Some("large"), Some("default"), &[]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    assert_eq!(switches(&log), "model=large\nmode=default\n");
    assert_eq!(started_extracts(&frames), Indexed::default());
    assert_eq!(kinds(&frames), ["session_started"]);
}

/// A catalogue seeded from a pre-answer `config_option_update` (decision 4)
/// is not authoritative, even when it already happens to show the wanted
/// value as current: the seed may be stale or incomplete, so the switch is
/// sent anyway (a redundant switch is harmless; a skipped one lies).
#[tokio::test]
async fn a_seeded_catalogue_does_not_skip_a_switch_that_already_matches_it() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        config_in_update_only: true,
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::Load {
            agent_session_id: "agent-7".into(),
        },
        // "default" is already the seeded catalogue's current mode.
        wanted(None, Some("default"), &[]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    assert_eq!(switches(&log), "mode=default\n");
    assert_eq!(started_extracts(&frames).current_mode.as_deref(), Some("default"));
}

fn set_config(request_id: &str, config_id: &str, value: ConfigValue) -> SessionCmd {
    SessionCmd::SetConfig {
        request_id: request_id.into(),
        config_id: config_id.into(),
        value,
    }
}

/// The catalogue extracts of every `config_applied`, with its request id.
fn applied(frames: &[HostFrame]) -> Vec<(String, Indexed)> {
    frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::ConfigApplied { request_id, indexed },
                ..
            } => Some((request_id.clone(), indexed.clone())),
            _ => None,
        })
        .collect()
}

async fn refusal(replies: &mut tokio::sync::mpsc::UnboundedReceiver<HostFrame>) -> (String, String) {
    match tokio::time::timeout(Duration::from_secs(10), replies.recv())
        .await
        .expect("a reply, not silence")
        .unwrap()
    {
        HostFrame::Error { request_id, code, .. } => (request_id, code),
        other => panic!("{other:?}"),
    }
}

/// Like `refusal`, but keeps the message too (fix round 1 tests need to
/// tell "no answer within ..." apart from "an earlier switch is still
/// out").
async fn full_refusal(replies: &mut tokio::sync::mpsc::UnboundedReceiver<HostFrame>) -> (String, String, String) {
    match tokio::time::timeout(Duration::from_secs(10), replies.recv())
        .await
        .expect("a reply, not silence")
        .unwrap()
    {
        HostFrame::Error {
            request_id,
            code,
            message,
        } => (request_id, code, message),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn set_config_switches_during_a_turn_and_answers_with_the_adapters_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: (1..=5).map(|n| n.to_string()).collect(),
        chunk_delay_ms: 100,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(None, Some("plan"), &[]),
        SessionOptions::default(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    // The model switch clamps the mode: the read-back says so.
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    let frames = wait_until(&uplink, has("config_applied")).await;
    let kinds = kinds(&frames);
    let at = kinds.iter().position(|k| k == "config_applied").unwrap();
    assert!(
        at < kinds.iter().position(|k| k == "turn_ended").unwrap_or(usize::MAX),
        "the switch waited for the turn: {kinds:?}"
    );
    let (request, indexed) = applied(&frames).remove(0);
    let current = indexed.current_config().unwrap();
    assert_eq!(
        (request.as_str(), current.model.as_deref(), current.mode.as_deref()),
        ("rc1", Some("large"), Some("default"))
    );
    // A value the adapter refuses, an option it does not have, a value of
    // the wrong kind: each is answered, and only the first reaches it.
    assert!(handle.send(set_config("rc2", "model", ConfigValue::Id("huge".into()))));
    assert_eq!(
        refusal(&mut replies).await,
        ("rc2".to_string(), "config_failed".to_string())
    );
    assert!(handle.send(set_config("rc3", "nope", ConfigValue::Id("x".into()))));
    assert_eq!(
        refusal(&mut replies).await,
        ("rc3".to_string(), "unknown_option".to_string())
    );
    assert!(handle.send(set_config("rc4", "fast", ConfigValue::Id("x".into()))));
    assert_eq!(refusal(&mut replies).await, ("rc4".to_string(), "invalid".to_string()));
    assert_eq!(switches(&log), "mode=plan\nmodel=large\nmodel=huge\n");
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(applied(&frames).len(), 1);
}

/// P-13 at the source: a `config_option_update` replayed by `session/load`
/// predates the switches, so it must not carry a catalogue that could
/// overwrite the announced one. One the agent sends live (it left plan mode
/// on its own) must: that is how its new mode gets stored.
#[tokio::test]
async fn only_a_live_config_option_update_carries_the_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let mut stale = hennery_testkit::sample_config_options();
    stale[0]["currentValue"] = json!("large");
    let script = FakeScript {
        replay: vec![json!({"sessionUpdate": "config_option_update", "configOptions": stale})],
        prompt_sets_mode: Some("bypass".into()),
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::Load {
            agent_session_id: "agent-7".into(),
        },
        wanted(None, Some("plan"), &[]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    assert_eq!(kinds(&frames), ["session_started", "update:?"]);
    assert_eq!(started_extracts(&frames).current_mode.as_deref(), Some("plan"));
    let replayed = match &frames[1] {
        HostFrame::Session {
            body: SessionBody::AcpUpdate { indexed, .. },
            ..
        } => indexed.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(replayed, Indexed::default(), "a replayed update carried a catalogue");

    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    let live: Vec<Indexed> = frames
        .iter()
        .skip(2)
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::AcpUpdate { indexed, .. },
                ..
            } if indexed.config_options.is_some() => Some(indexed.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(live.len(), 1, "{:?}", kinds(&frames));
    assert_eq!(live[0].current_mode.as_deref(), Some("bypass"));
    assert_eq!(live[0].turn_id.as_deref(), Some("t1"));
    // A repeated start announces what the agent last reported.
    assert!(handle.send(SessionCmd::Restart {
        request_id: "r9".into()
    }));
    let frames = wait_until(&uplink, |f| {
        kinds(f).iter().filter(|k| *k == "session_started").count() == 2
    })
    .await;
    let last = frames
        .iter()
        .rev()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::SessionStarted { indexed, .. },
                ..
            } => Some(indexed.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(last.current_mode.as_deref(), Some("bypass"));
}

#[tokio::test]
async fn a_switch_still_out_when_the_session_ends_is_not_attached() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions {
            // Long enough that rc1 is still genuinely out (not yet an
            // orphan, fix round 1, F1) when the park below tears the
            // session down.
            config_timeout: Duration::from_secs(30),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    // Parked while the switch is out: it is answered, after the park.
    assert!(handle.send(SessionCmd::Park {
        request_id: "rp".into()
    }));
    assert_eq!(
        refusal(&mut replies).await,
        ("rc1".to_string(), "not_attached".to_string())
    );
    wait_until(&uplink, has("session_parked:operator")).await;
    assert!(applied(&uplink.pending().unwrap()).is_empty());
}

/// The real adapters handle requests concurrently. If both of these went
/// out at once, the slow model switch would answer last and then clamp the
/// mode the second switch had just set, while hennery showed `plan`. One
/// switch at a time keeps them in the order the operator made them.
#[tokio::test]
async fn set_config_sends_one_switch_at_a_time_so_a_late_clamp_cannot_undo_a_later_switch() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        slow_model_switch_ms: Some(300),
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions::default(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    assert!(handle.send(set_config("rc2", "mode", ConfigValue::Id("plan".into()))));
    let frames = wait_until(&uplink, |f| applied(f).len() == 2).await;
    let answers: Vec<String> = applied(&frames).into_iter().map(|(r, _)| r).collect();
    assert_eq!(answers, ["rc1", "rc2"]);
    // A third switch reads back the agent's real state.
    assert!(handle.send(set_config("rc3", "effort", ConfigValue::Id("high".into()))));
    let frames = wait_until(&uplink, |f| applied(f).len() == 3).await;
    let (_, last) = applied(&frames).remove(2);
    let current = last.current_config().unwrap();
    assert_eq!(
        (current.model.as_deref(), current.mode.as_deref()),
        (Some("large"), Some("plan")),
        "the late clamp undid the later switch"
    );
}

/// A switch's deadline runs from when the host received it, not from when
/// it could be sent: one waiting behind a hung switch does not get a fresh
/// `config_timeout` of its own.
#[tokio::test]
async fn a_switch_waiting_behind_a_hung_one_is_answered_by_its_own_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions {
            config_timeout: Duration::from_millis(300),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    let began = std::time::Instant::now();
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    assert!(handle.send(set_config("rc2", "mode", ConfigValue::Id("plan".into()))));
    assert_eq!(
        refusal(&mut replies).await,
        ("rc1".to_string(), "config_failed".to_string())
    );
    assert_eq!(
        refusal(&mut replies).await,
        ("rc2".to_string(), "config_failed".to_string())
    );
    assert!(
        began.elapsed() < Duration::from_millis(550),
        "the second switch got a timeout of its own: {:?}",
        began.elapsed()
    );
}

// Fix round 1.

/// F1: a switch queued behind one that just timed out (became an orphan)
/// is refused for that reason, in one synchronous flush — not sent and
/// then left to time out on its own, which would put it out at the same
/// time as the (still possibly live) orphan.
#[tokio::test]
async fn a_switch_queued_behind_an_orphaned_one_is_refused_and_never_sent() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions {
            config_timeout: Duration::from_millis(200),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    assert!(handle.send(set_config("rc2", "mode", ConfigValue::Id("plan".into()))));
    let (id, code, message) = full_refusal(&mut replies).await;
    assert_eq!((id.as_str(), code.as_str()), ("rc1", "config_failed"));
    assert!(message.contains("no answer within"), "{message}");
    let (id, code, message) = full_refusal(&mut replies).await;
    assert_eq!((id.as_str(), code.as_str()), ("rc2", "config_failed"));
    assert!(message.contains("an earlier switch is still out"), "{message}");
    // rc2 was flushed with rc1's orphan: it never reached the adapter.
    assert_eq!(switches(&log), "model=large\n");
}

/// F1: an orphaned switch's late answer still updates the catalogue
/// (silently — its requester already has `config_failed`), and once it
/// clears, a later switch is sent normally.
#[tokio::test]
async fn an_orphaned_switchs_late_answer_updates_the_catalogue_and_a_later_switch_is_sent_normally() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        // Well past `config_timeout` (so rc1 orphans), well short of the
        // orphan's own grace (`config_timeout * ORPHAN_GRACE` = 400ms, so
        // rc1's late answer still lands before it is given up on).
        slow_model_switch_ms: Some(250),
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions {
            config_timeout: Duration::from_millis(100),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    let (id, code, message) = full_refusal(&mut replies).await;
    assert_eq!((id.as_str(), code.as_str()), ("rc1", "config_failed"));
    assert!(message.contains("no answer within"), "{message}");
    // Give rc1's late (250ms) answer time to land and update the catalogue.
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(handle.send(SessionCmd::Restart {
        request_id: "r9".into()
    }));
    let frames = wait_until(&uplink, |f| {
        kinds(f).iter().filter(|k| *k == "session_started").count() == 2
    })
    .await;
    let last = frames
        .iter()
        .rev()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::SessionStarted { indexed, .. },
                ..
            } => Some(indexed.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        last.current_config().unwrap().model.as_deref(),
        Some("large"),
        "the orphan's late read-back was not applied"
    );
    // A later switch is not still blocked by the (now resolved) orphan.
    assert!(handle.send(set_config("rc2", "effort", ConfigValue::Id("high".into()))));
    let frames = wait_until(&uplink, has("config_applied")).await;
    let (_, indexed) = applied(&frames).remove(0);
    assert_eq!(
        indexed.current_config().unwrap().axes.get("effort").cloned(),
        Some(ConfigValue::Id("high".into()))
    );
    assert_eq!(switches(&log), "model=large\neffort=high\n");
}

/// F2: the adapter answers a switch (R) and then, right after (no `.await`
/// on its side in between), sends a live `config_option_update` (N) of its
/// own. `agent-client-protocol` 2.2.0's dispatch loop processes incoming
/// messages strictly in order (`concepts::ordering`: "the dispatch loop
/// waits for each handler to complete before processing the next
/// message"), so R's answer is always routed before N is ever dispatched to
/// us. But this actor's own task can still be scheduled late enough to see
/// both ready in the very same poll, and the select above checks the
/// updates arm first. A single-thread runtime can never interleave the ACP
/// connection's task and this actor's task tightly enough to hit that, so
/// this needs `flavor = "multi_thread"`, repeated, for a real chance of
/// exercising the gap — the same pattern as
/// `updates_never_land_outside_their_turn_under_a_multi_thread_runtime`.
/// Whenever it lands, the stored catalogue must end up as N, never
/// overwritten back to R's now-stale one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_update_sent_right_after_a_switchs_answer_wins_over_that_answers_stale_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        announce_after_switch: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions::default(),
    );
    wait_until(&uplink, has("session_started")).await;
    for i in 0..60 {
        let rc = format!("rc{i}");
        assert!(handle.send(set_config(&rc, "model", ConfigValue::Id("large".into()))));
        let expected = i + 1;
        let live_carries_catalogue = |f: &[HostFrame]| {
            f.iter()
                .filter(|frame| {
                    matches!(frame, HostFrame::Session { body: SessionBody::AcpUpdate { indexed, .. }, .. } if indexed.config_options.is_some())
                })
                .count()
        };
        let frames = wait_until(&uplink, move |f| {
            applied(f).len() == expected && live_carries_catalogue(f) == expected
        })
        .await;
        // Whichever of R (`config_applied`) and N (the live update) this
        // actor happened to process last for this round is what is now
        // stored: it must be N, not R.
        assert_eq!(
            kinds(&frames).pop().as_deref(),
            Some("update:?"),
            "R was processed after N on iteration {i}: {:?}",
            kinds(&frames)
        );
    }
    assert!(handle.send(SessionCmd::Restart {
        request_id: "r9".into()
    }));
    let frames = wait_until(&uplink, |f| {
        kinds(f).iter().filter(|k| *k == "session_started").count() == 2
    })
    .await;
    let last = frames
        .iter()
        .rev()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::SessionStarted { indexed, .. },
                ..
            } => Some(indexed.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        last.current_mode.as_deref(),
        Some("bypass"),
        "N, not R, must be the stored catalogue"
    );
}

/// F3: a switch is checked against the catalogue only when it is actually
/// popped for sending, not when it arrived — the switch ahead of it in the
/// queue may have already changed what the catalogue offers by then.
#[tokio::test]
async fn a_queued_switch_is_checked_against_the_catalogue_after_the_switch_ahead_of_it_not_before() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        model_switch_drops_option: Some("effort".into()),
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions::default(),
    );
    wait_until(&uplink, has("session_started")).await;
    // "effort" exists when rc2 arrives; the model switch ahead of it drops
    // it before rc2 is ever popped for sending.
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    assert!(handle.send(set_config("rc2", "effort", ConfigValue::Id("high".into()))));
    let frames = wait_until(&uplink, has("config_applied")).await;
    assert_eq!(applied(&frames).len(), 1);
    assert_eq!(
        refusal(&mut replies).await,
        ("rc2".to_string(), "unknown_option".to_string())
    );
    // Never reached the adapter: rejected at pop time, not sent and then
    // refused by it.
    assert_eq!(switches(&log), "model=large\n");
}

/// Cheap fix, same area: the idle reaper must not park a session out from
/// under a switch that is out or orphaned — that would drop the adapter
/// the collector is still waiting on an answer from.
#[tokio::test]
async fn the_idle_reaper_does_not_park_while_a_switch_is_out_or_orphaned() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions {
            config_timeout: Duration::from_millis(200),
            idle_timeout: Some(Duration::from_millis(100)),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    let not_parked = |frames: &[HostFrame]| {
        !frames.iter().any(|f| {
            matches!(
                f,
                HostFrame::Session {
                    body: SessionBody::SessionParked { .. },
                    ..
                }
            )
        })
    };
    // The idle window (100ms) is shorter than the config timeout (200ms):
    // if the reaper ignored the switch that is out, it would park here.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(not_parked(&uplink.pending().unwrap()));
    // Still true once the switch has become an orphan.
    let (id, code, _) = full_refusal(&mut replies).await;
    assert_eq!((id.as_str(), code.as_str()), ("rc1", "config_failed"));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(not_parked(&uplink.pending().unwrap()));
}

// Fix round 2.

/// F2, case A (fix round 2): the adapter sends a `config_option_update`
/// (N0) and only then answers the switch (R) — wire order N0, R, with N0
/// strictly older. The stored catalogue must always end up as R's read-back
/// (newer), never left at N0's (older) one: `send_next_switch` now delivers
/// a switch's answer through the same ordered channel as live updates
/// (`on_receiving_result`, not `block_task`), so both are handled by the
/// actor strictly in the order the adapter's dispatch loop delivered them —
/// no peeking or reordering needed, unlike case B below. Repeated under a
/// multi-thread runtime for the same reason as
/// `updates_never_land_outside_their_turn_under_a_multi_thread_runtime`: a
/// single-thread runtime can't interleave the actor and the ACP connection
/// task tightly enough to have a real chance of ever exercising the gap
/// (this actor being briefly "behind" — busy elsewhere while both N0 and R
/// pile up — is exactly the scheduling window fix round 1's `now_or_never`
/// peek got wrong for this case, applying R as if it were always the newer
/// one).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_update_sent_right_before_a_switch_answers_never_outlives_that_answer() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        announce_before_switch: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions::default(),
    );
    wait_until(&uplink, has("session_started")).await;
    for i in 0..60 {
        // Alternates so N0 (the pre-switch snapshot) never accidentally
        // matches R (this switch's own value), which would hide a wrong
        // final value behind a coincidence.
        let value = if i % 2 == 0 { "large" } else { "small" };
        let rc = format!("rc{i}");
        assert!(handle.send(set_config(&rc, "model", ConfigValue::Id(value.into()))));
        let expected = i + 1;
        let live_carries_catalogue = |f: &[HostFrame]| {
            f.iter()
                .filter(|frame| {
                    matches!(frame, HostFrame::Session { body: SessionBody::AcpUpdate { indexed, .. }, .. } if indexed.config_options.is_some())
                })
                .count()
        };
        let frames = wait_until(&uplink, move |f| {
            applied(f).len() == expected && live_carries_catalogue(f) == expected
        })
        .await;
        // N0 is strictly older: whichever of it and R landed last for this
        // round must be R.
        assert_eq!(
            kinds(&frames).pop().as_deref(),
            Some("config_applied"),
            "N0 (older) ended up after R (newer) on iteration {i}: {:?}",
            kinds(&frames)
        );
        let (_, r) = applied(&frames).into_iter().next_back().unwrap();
        assert_eq!(
            r.current_config().unwrap().model.as_deref(),
            Some(value),
            "the stored catalogue must be R's read-back on iteration {i}"
        );
    }
}

// Plan B2b: an actor that ends by itself says so at once (B2a's hand-off).

async fn wait_ending(handle: &SessionHandle) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !handle.is_ending() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the handle never said it was ending"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// An idle reap or an adapter stopped for ignoring a cancel ends the actor
/// by itself. Its handle must say so while the adapter is still being
/// killed (here the whole 1 s grace: it ignores SIGTERM), or a resume
/// arriving then is routed to the ending actor and answered `not_attached`.
#[tokio::test]
async fn an_actor_ending_by_itself_marks_its_handle_ending_while_it_kills_the_adapter() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let reaped = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_ignoring_sigterm(&FakeScript::default()),
        std::env::temp_dir(),
        SessionOptions {
            idle_timeout: Some(Duration::from_millis(300)),
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    );
    wait_ending(&reaped).await;
    assert!(!reaped.is_ended(), "still killing its adapter");
    wait_until(&uplink, has("session_parked:idle")).await;
    wait_ended(&reaped).await;

    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        ignore_cancel: true,
        ..slow_script()
    };
    let stopped = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s2".into(),
        fake_ignoring_sigterm(&script),
        std::env::temp_dir(),
        SessionOptions {
            cancel_grace: Duration::from_millis(300),
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(stopped.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    assert!(stopped.send(cancel("rc", "t1")));
    assert!(!stopped.is_ending(), "a cancel alone does not end the actor");
    wait_ending(&stopped).await;
    assert!(!stopped.is_ended(), "still killing its adapter");
    wait_until(&uplink, has("session_parked:operator")).await;
    wait_ended(&stopped).await;
}

/// An adapter streaming faster than the outbox can write keeps the actor's
/// update arm always ready. Without a cap on updates in a row, the Cancel
/// behind them is never read, and the turn only ends when the agent does.
/// The outbox is on disk, as on a real host, so the actor is the slow side.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flooding_adapter_cannot_hold_off_a_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let (uplink, _replies) = Uplink::new(Outbox::open(&dir.path().join("outbox.db")).unwrap());
    let script = FakeScript {
        chunks: vec!["flood".into()],
        flood: true,
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
    let start_deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while handle.open_turn_id().is_none() {
        assert!(tokio::time::Instant::now() < start_deadline, "the turn never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(handle.send(cancel("rc", "t1")));
    let cancel_deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    // Polled on the handle: reading a flooded outbox every few ms would
    // itself slow the actor down.
    while handle.open_turn_id().is_some() {
        assert!(
            tokio::time::Instant::now() < cancel_deadline,
            "the cancel was never read: the flood held it off"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ends = turn_ends(&uplink.pending().unwrap());
    assert_eq!(ends, [("t1".to_string(), TurnOutcome::Cancelled, None)]);
}

/// F1 (review round 1): `out_deadline` could fire while the switch's
/// genuine answer was already queued, undrained, behind the adapter's own
/// output (the burst cap can leave the updates arm disabled long enough for
/// the deadline to win the race) — reporting `config_failed` to the
/// requester and then silently applying the same answer as an orphan.
/// `out_deadline` must drain first and only orphan the switch if it is
/// still out afterward.
///
/// No prompt runs here: `model_switch_chunks_first` gives the switch's own
/// answer a known, deterministic backlog ahead of it (sent inline, no sleep,
/// so it queues at the host within milliseconds), independent of any other
/// task's timing. Calibrated at ~850ms to drain 5000 items on a disk-backed
/// outbox; `config_timeout` (300ms) is well under a third of that, so the
/// deadline reliably fires while the answer is still undrained.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_answered_just_in_time_is_applied_not_orphaned_under_a_backlog() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open(&dir.path().join("outbox.db")).unwrap());
    let script = FakeScript {
        model_switch_chunks_first: Some(5000),
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions {
            config_timeout: Duration::from_millis(300),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    // No prompt is running, so `next_reply` (which never preempts
    // `out_deadline` in the biased select otherwise) cannot resolve either:
    // whatever answers this switch, answers it via `out_deadline`'s own
    // drain or the ordinary update arm, never a turn ending underneath it.
    let frames = wait_until(&uplink, has("config_applied")).await;
    let (request, indexed) = applied(&frames).remove(0);
    assert_eq!(request.as_str(), "rc1");
    assert_eq!(
        indexed.current_config().unwrap().model.as_deref(),
        Some("large"),
        "the answer that came in time was not applied"
    );
    // Nothing else answered rc1 (in particular, not a `config_failed` from
    // the deadline firing before the drain saw the real answer).
    match replies.try_recv() {
        Err(_) => {}
        Ok(frame) => panic!("rc1 was answered a second time: {frame:?}"),
    }
}

/// F2 (review round 1): the pre-turn drain (run right before a new turn
/// starts, so updates from before it are never tagged with the new turn)
/// can answer a switch that was out without a follow-up `send_next_switch`
/// — leaving whatever waits behind it in `configs.queued` unsent until
/// something else happens to call it, in the worst case the new turn's own
/// end.
///
/// `model_switch_chunks_first` gives `rc1` (the model switch) a known
/// backlog ahead of its own answer (calibrated at ~850ms to drain 5000 items
/// on a disk-backed outbox): a brief sleep (30ms) lets the answer reach the
/// host (fast: the connection task's own read is not disk-bound) while the
/// actor's own consumption is still far from reaching it. `rc2` (a plain,
/// fast switch) is sent right behind `rc1` and queues, since only one switch
/// is ever out at a time. `config_timeout` is generous so `out_deadline`
/// never fires here — this is about the *pre-turn* drain specifically, not
/// F1's `out_deadline` drain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_queued_behind_one_the_pre_turn_drain_answers_is_sent_without_waiting_for_the_new_turn() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open(&dir.path().join("outbox.db")).unwrap());
    let script = FakeScript {
        model_switch_chunks_first: Some(5000),
        chunks: (1..=5).map(|n| n.to_string()).collect(),
        chunk_delay_ms: 200,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions {
            config_timeout: Duration::from_secs(10),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    assert!(handle.send(set_config("rc2", "effort", ConfigValue::Id("high".into()))));
    // rc1's answer (behind its 5000-notification backlog) has reached the
    // host by now, but the disk-bound actor's own consumption is nowhere
    // near it yet (calibrated at ~850ms to get there on its own).
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    let kinds = kinds(&frames);
    // `kinds` collapses every `config_applied` to the same string, so
    // finding rc1's or rc2's own position (not just "the first one") needs
    // the frames themselves, matched by request id.
    let index_of = |request_id: &str| {
        frames.iter().position(|f| {
            matches!(
                f,
                HostFrame::Session {
                    body: SessionBody::ConfigApplied { request_id: id, .. },
                    ..
                } if id == request_id
            )
        })
    };
    let turn_ended_at = kinds.iter().position(|k| k == "turn_ended").unwrap();
    assert!(
        index_of("rc2").is_some_and(|at| at < turn_ended_at),
        "rc2 waited for the turn to end instead of being sent by the pre-turn drain: {kinds:?}"
    );
    let applied = applied(&frames);
    assert_eq!(applied.len(), 2, "expected both rc1 and rc2 answered: {kinds:?}");
    assert_eq!(
        applied.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["rc1", "rc2"]
    );
    let (_, indexed) = &applied[1];
    assert_eq!(
        indexed.current_config().unwrap().axes.get("effort").cloned(),
        Some(ConfigValue::Id("high".into())),
        "rc2 (effort=high) was not applied"
    );
}
