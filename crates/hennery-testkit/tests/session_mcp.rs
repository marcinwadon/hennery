//! Plan 8c: a session's MCP servers and its profile's `_meta` reach the
//! adapter on `session/new` and on every `session/load` (the spike: Claude
//! keeps neither across a load), and the session's secret values never
//! reach what the session reports (ACP core §8).

use hennery_host::outbox::Outbox;
use hennery_host::profile::Profile;
use hennery_host::session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionOptions};
use hennery_host::uplink::Uplink;
use hennery_proto::frames::{ConfigValue, HostFrame, McpServer, NameValue, SessionBody, SessionConfig};
use hennery_testkit::{FakeScript, SCRIPT_ENV};
use serde_json::{Value, json};
use std::time::Duration;

const TOKEN: &str = "hst_live_token_0123456789";
const ENV_KEY: &str = "files-key-0123456789";
/// A key in a stdio server's argument.
const ARG_KEY: &str = "arg-secret-0123456789";
/// A key in an HTTP server's URL path: only its origin may show (the
/// gateway lane's rule L11).
const URL_KEY: &str = "url-secret-0123456789";
/// A value `scrub` cuts in part (`ghp_…` up to the colon): redacted whole
/// only if it is redacted before `scrub`.
const SCRUBBED: &str = "ghp_0123456789:tail-secret-0123";

fn servers() -> Vec<McpServer> {
    vec![
        McpServer::Http {
            name: "hennery-notes".into(),
            url: format!("https://hennery.example/mcp/notes/{URL_KEY}"),
            headers: vec![NameValue::new("Authorization", format!("Bearer {TOKEN}"))],
        },
        McpServer::Stdio {
            name: "hennery-files".into(),
            command: "/usr/local/bin/files-mcp".into(),
            args: vec!["--root".into(), "/srv".into(), format!("--key={ARG_KEY}")],
            env: vec![NameValue::new("FILES_KEY", ENV_KEY), NameValue::new("GH", SCRUBBED)],
        },
    ]
}

/// The ACP form of `servers()`: an `http` entry, and an untagged stdio one.
fn acp_servers() -> Value {
    json!([
        {"type": "http", "name": "hennery-notes", "url": format!("https://hennery.example/mcp/notes/{URL_KEY}"),
         "headers": [{"name": "Authorization", "value": format!("Bearer {TOKEN}")}]},
        {"name": "hennery-files", "command": "/usr/local/bin/files-mcp",
         "args": ["--root", "/srv", format!("--key={ARG_KEY}")],
         "env": [{"name": "FILES_KEY", "value": ENV_KEY}, {"name": "GH", "value": SCRUBBED}]}
    ])
}

fn strict() -> Value {
    json!({"claudeCode": {"options": {"extraArgs": {"strict-mcp-config": ""}}}})
}

fn fake(script: &FakeScript) -> AgentCommand {
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    fake
}

/// Launch one session actor and wait for its first `session_started` or
/// `start_failed`.
async fn attach(script: &FakeScript, attach: Attach, profile: Profile, mcp_servers: Vec<McpServer>) -> Uplink {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let launch = Launch {
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach,
        config: Default::default(),
        agent: fake(script),
        cwd: std::env::temp_dir(),
        profile,
        mcp_servers,
    };
    let handle = session::launch(uplink.clone(), launch, SessionOptions::default());
    wait_for(&uplink, |body| {
        matches!(
            body,
            SessionBody::SessionStarted { .. } | SessionBody::StartFailed { .. }
        )
    })
    .await;
    // Closed so the adapter is gone before the next launch reads the log
    // (a failed start has ended already).
    let _ = handle.send(SessionCmd::Close {
        request_id: "r-close".into(),
    });
    handle.finished().await;
    uplink
}

async fn wait_for(uplink: &Uplink, pred: impl Fn(&SessionBody) -> bool) -> Vec<HostFrame> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frames = uplink.pending().unwrap();
        if frames
            .iter()
            .any(|f| matches!(f, HostFrame::Session { body, .. } if pred(body)))
        {
            return frames;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out: {frames:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The session log's lines, as `(method, params)`.
fn logged(path: &std::path::Path) -> Vec<(String, Value)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let line: Value = serde_json::from_str(line).unwrap();
            (line["method"].as_str().unwrap().to_string(), line["params"].clone())
        })
        .collect()
}

fn script_logging_to(path: &std::path::Path) -> FakeScript {
    FakeScript {
        session_log: Some(path.to_string_lossy().into_owned()),
        ..Default::default()
    }
}

#[tokio::test]
async fn claudes_servers_and_strict_flag_go_on_new_and_on_every_load() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log);
    attach(&script, Attach::New, Profile::Claude, servers()).await;
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    attach(&script, load.clone(), Profile::Claude, servers()).await;
    attach(&script, load, Profile::Claude, servers()).await;
    let logged = logged(&log);
    let methods: Vec<&str> = logged.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(methods, ["session/new", "session/load", "session/load"]);
    for (method, params) in &logged {
        assert_eq!(params["mcpServers"], acp_servers(), "{method}: {params}");
        assert_eq!(params["_meta"], strict(), "{method}: {params}");
    }
}

/// The strict flag is sent with no servers too (umbrella §8.5): without it
/// the user's own MCP servers and claude.ai connectors would load.
#[tokio::test]
async fn claude_is_strict_without_servers_too_and_its_own_cli_as_well() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log);
    attach(&script, Attach::New, Profile::Claude, vec![]).await;
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    attach(&script, load, Profile::ClaudeOwnCli, vec![]).await;
    for (method, params) in logged(&log) {
        assert_eq!(params["mcpServers"], json!([]), "{method}: {params}");
        assert_eq!(params["_meta"], strict(), "{method}: {params}");
    }
}

#[tokio::test]
async fn a_generic_agent_gets_its_servers_and_no_meta() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log);
    attach(&script, Attach::New, Profile::Generic, servers()).await;
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    attach(&script, load, Profile::Generic, vec![]).await;
    let logged = logged(&log);
    assert_eq!(logged.len(), 2);
    assert_eq!(logged[0].1["mcpServers"], acp_servers());
    assert_eq!(logged[1].1["mcpServers"], json!([]));
    for (method, params) in &logged {
        assert!(params.get("_meta").is_none_or(Value::is_null), "{method}: {params}");
    }
}

/// An adapter that echoes its MCP config (on stderr, then exiting): the
/// bare token and the env value are redacted from `adapter_exited`, as
/// from a `start_failed` or a `host_note`.
#[tokio::test]
async fn the_sessions_secret_values_never_reach_what_it_reports() {
    let script = FakeScript {
        stderr_lines: vec![
            format!("mcp config: token={TOKEN}"),
            format!("files: FILES_KEY={ENV_KEY}"),
            format!("argv: files-mcp --key={ARG_KEY}"),
            format!("upstream: https://hennery.example/mcp/notes/{URL_KEY}"),
            format!("gh: {SCRUBBED}"),
        ],
        exit_after_chunks: Some(0),
        ..Default::default()
    };
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let launch = Launch {
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach: Attach::New,
        config: Default::default(),
        agent: fake(&script),
        cwd: std::env::temp_dir(),
        profile: Profile::Claude,
        mcp_servers: servers(),
    };
    let handle = session::launch(uplink.clone(), launch, SessionOptions::default());
    wait_for(&uplink, |body| matches!(body, SessionBody::SessionStarted { .. })).await;
    assert!(handle.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type": "text", "text": "hi"})],
    }));
    let frames = wait_for(&uplink, |body| matches!(body, SessionBody::AdapterExited { .. })).await;
    let tail = frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::AdapterExited { stderr_tail, .. },
                ..
            } => Some(stderr_tail.clone()),
            _ => None,
        })
        .unwrap();
    assert!(tail.contains("mcp config: token=[redacted]"), "{tail}");
    assert!(tail.contains("FILES_KEY=[redacted]"), "{tail}");
    let everything = serde_json::to_string(&frames).unwrap();
    for secret in [TOKEN, ENV_KEY, ARG_KEY, URL_KEY, "tail-secret"] {
        assert!(!everything.contains(secret), "{everything}");
    }
}

/// An adapter whose errors quote the session's secrets: in the note about
/// a start's switch that failed (with a value `scrub` would cut in part,
/// so it must be redacted first), in a refused `set_config`'s answer, and
/// in a failed turn's error. None reaches what the host reports.
#[tokio::test]
async fn an_adapters_errors_quoting_its_secrets_are_redacted_in_every_answer_and_fact() {
    let script = FakeScript {
        config_options: hennery_testkit::sample_config_options(),
        config_error: Some(format!("refused {SCRUBBED} for {TOKEN}")),
        prompt_error: Some(format!("prompt failed: {TOKEN} {ENV_KEY} {URL_KEY}")),
        ..Default::default()
    };
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let launch = Launch {
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach: Attach::New,
        config: SessionConfig {
            model: Some("large".into()),
            ..Default::default()
        },
        agent: fake(&script),
        cwd: std::env::temp_dir(),
        profile: Profile::Claude,
        mcp_servers: servers(),
    };
    let handle = session::launch(uplink.clone(), launch, SessionOptions::default());
    wait_for(&uplink, |body| matches!(body, SessionBody::HostNote { .. })).await;
    assert!(handle.send(SessionCmd::SetConfig {
        request_id: "r1".into(),
        config_id: "effort".into(),
        value: ConfigValue::Id("high".into()),
    }));
    let answer = tokio::time::timeout(Duration::from_secs(10), replies.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(handle.send(SessionCmd::Prompt {
        request_id: "r2".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type": "text", "text": "hi"})],
    }));
    let frames = wait_for(&uplink, |body| matches!(body, SessionBody::TurnEnded { .. })).await;
    let _ = handle.send(SessionCmd::Close {
        request_id: "r-close".into(),
    });
    handle.finished().await;
    let (answer, facts) = (
        serde_json::to_string(&answer).unwrap(),
        serde_json::to_string(&frames).unwrap(),
    );
    // The adapter did quote them, and they were redacted.
    assert!(
        answer.contains("config_failed") && answer.contains("[redacted]"),
        "{answer}"
    );
    for kind in ["host_note", "turn_ended"] {
        assert!(facts.contains(kind), "{facts}");
    }
    assert!(facts.contains("prompt failed: [redacted]"), "{facts}");
    for secret in [TOKEN, ENV_KEY, URL_KEY, "tail-secret"] {
        assert!(!answer.contains(secret), "{answer}");
        assert!(!facts.contains(secret), "{facts}");
    }
}

/// A resume whose replay names an unknown update kind that quotes a
/// secret `scrub` would cut in part: the note about it is redacted before
/// it is scrubbed.
#[tokio::test]
async fn a_replay_note_quoting_a_secret_is_redacted_before_scrub() {
    let script = FakeScript {
        replay: vec![json!({"sessionUpdate": format!("unknown {SCRUBBED}")})],
        ..Default::default()
    };
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    let uplink = attach(&script, load, Profile::Claude, servers()).await;
    let frames = uplink.pending().unwrap();
    let note = frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::HostNote { note, text },
                ..
            } if note == "replay_unknown_dropped" => Some(text.clone()),
            _ => None,
        })
        .expect("a replay note");
    assert!(note.contains("[redacted]") && !note.contains("tail-secret"), "{note}");
}

/// A `tracing` writer into a shared buffer.
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An adapter whose `session/new` error quotes its MCP config: the
/// `start_failed` it becomes, and the host's log line about it, show
/// neither the token nor the env value. The actor runs on the test's own
/// thread (a current-thread runtime), so the thread's subscriber sees it.
/// Only the host's own lines: the ACP crate logs the adapter's answer at
/// `debug`, echo included, which the process's output caps
/// (`hennery_host::logging`, `session_mcp_log.rs`).
#[tokio::test]
async fn a_start_failure_quoting_the_servers_is_redacted_in_the_fact_and_the_log() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("hennery_host=debug")
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let script = FakeScript {
        new_session_error: Some(-32603),
        new_session_error_echoes: true,
        ..Default::default()
    };
    let uplink = attach(&script, Attach::New, Profile::Claude, servers()).await;
    let frames = uplink.pending().unwrap();
    let message = frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::StartFailed { message, .. },
                ..
            } => Some(message.clone()),
            _ => None,
        })
        .unwrap();
    // The adapter did quote them, and they were redacted.
    assert!(
        message.contains("hennery-notes") && message.contains("[redacted]"),
        "{message}"
    );
    let log = String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
    assert!(log.contains("session start failed"), "{log}");
    for secret in [TOKEN, ENV_KEY, ARG_KEY, URL_KEY, "tail-secret"] {
        assert!(!message.contains(secret), "{message}");
        assert!(!log.contains(secret), "{log}");
    }
}
