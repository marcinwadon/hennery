//! Plan 8c, ACP core §8: a session's gateway token never reaches the log,
//! even at `RUST_LOG=trace`, nor its server's URL past the origin (the
//! gateway lane's rule L11). A real host gets a `start_session` with the
//! token in an MCP header from a fake collector, and passes it to the fake
//! adapter's `session/new`: tungstenite traces the frame and the ACP crate
//! the JSON-RPC line, both with the token. The same for a `session/load`,
//! an adapter error that quotes the servers, and a frame that does not
//! decode, which the host logs by its error's kind and place only. The fake
//! adapter also prints a stray (non-JSON-RPC) stdout line quoting the token
//! and the URL key, which the ACP crate cannot parse and so quotes whole in
//! a `warn` event of its own (`agent-client-protocol` 2.2.0) — the reason
//! that target is held at `error`, not `info`.
//! Run twice: once under a plain
//! subscriber, which proves they do, and once under the one the process
//! installs (`logging::capped`), which must not show it. Each run's
//! subscriber is the thread's own (the runtime is current-thread, so the
//! host's tasks run here); the `log` bridge tungstenite needs is installed
//! once, globally, so this test has a binary of its own.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_host::{HostConfig, run};
use hennery_proto::frames::{CollectorFrame, HostFrame, McpDelivery, McpServer, NameValue};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tracing_subscriber::util::SubscriberInitExt;

const TOKEN: &str = "hst_logged_token_0123456789";
/// A key in the server's URL path: never logged (the gateway lane's L11).
const URL_KEY: &str = "url-logged-0123456789";

/// A `tracing` writer into a shared buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

#[allow(clippy::result_large_err)]
fn with_nonce(
    _: &tokio_tungstenite::tungstenite::handshake::server::Request,
    mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    response
        .headers_mut()
        .insert(HELLO_NONCE_HEADER, hex::encode([5u8; 32]).parse().unwrap());
    Ok(response)
}

fn servers() -> McpDelivery {
    McpDelivery {
        mcp_servers: vec![McpServer::Http {
            name: "hennery-notes".into(),
            url: format!("https://hennery.example/mcp/notes/{URL_KEY}"),
            headers: vec![NameValue::new("Authorization", format!("Bearer {TOKEN}"))],
        }],
        isolation_waived: false,
    }
}

/// A host with the fake adapter as `claude` (and as `echo`, whose
/// `session/new` error quotes its servers), driven by a fake collector: a
/// frame that does not decode, with the token in it; a start and a resume
/// with the token in an MCP header; a start that fails quoting it. Returns
/// once each has been answered.
async fn run_sessions_with_the_token() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        HostKey::from_seed([1; 32]),
        data.path().to_path_buf(),
    );
    let mut claude = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    // A stray (non-JSON-RPC) line on its stdout, quoting the token and the
    // URL key, right before it answers `session/new`: the ACP crate cannot
    // parse it and quotes it whole in a `warn` event of its own.
    let claude_script = hennery_testkit::FakeScript {
        stdout_lines: vec![format!("debug: servers [{TOKEN}] {URL_KEY}")],
        ..Default::default()
    };
    claude.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&claude_script).unwrap(),
    ));
    cfg.agents.insert("claude".into(), claude);
    let mut echo = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let script = hennery_testkit::FakeScript {
        new_session_error: Some(-32603),
        new_session_error_echoes: true,
        ..Default::default()
    };
    echo.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    cfg.agents.insert("echo".into(), echo);
    for agent in ["claude", "echo"] {
        cfg.profiles
            .insert(agent.into(), hennery_host::profile::Profile::Claude);
    }
    let host = tokio::spawn(run(cfg));

    let (tcp, _) = listener.accept().await.unwrap();
    let ws = tokio_tungstenite::accept_hdr_async(tcp, with_nonce).await.unwrap();
    let (mut sink, mut stream) = ws.split();
    let next = async |stream: &mut futures::stream::SplitStream<_>| -> HostFrame {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => return serde_json::from_str(&text).unwrap(),
                Ok(Some(Ok(_))) => {}
                other => panic!("expected a host frame: {other:?}"),
            }
        }
    };
    assert!(matches!(next(&mut stream).await, HostFrame::Hello { .. }));
    let send = |frame: &CollectorFrame| Message::text(serde_json::to_string(frame).unwrap());
    sink.send(send(&CollectorFrame::HelloAck {
        protocol_version: PROTOCOL_VERSION.into(),
        collector_version: "test".into(),
        committed: Default::default(),
    }))
    .await
    .unwrap();
    // Headers as a string: serde's error quotes it.
    let malformed = serde_json::json!({
        "type": "start_session", "request_id": "r0", "session_id": "s0", "committed_seq": 0,
        "agent": "claude", "cwd": "/",
        "mcp_servers": [{"type": "http", "name": "n", "url": "https://h.example", "headers": format!("Bearer {TOKEN}")}]
    });
    sink.send(Message::text(malformed.to_string())).await.unwrap();
    let cwd = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let start = |request_id: &str, session_id: &str, agent: &str| CollectorFrame::StartSession {
        request_id: request_id.into(),
        session_id: session_id.into(),
        committed_seq: 0,
        agent: agent.into(),
        cwd: cwd.clone(),
        config: Default::default(),
        hat_id: "hat-1".into(),
        mcp: servers(),
    };
    let resume = CollectorFrame::ResumeSession {
        request_id: "r3".into(),
        session_id: "s2".into(),
        committed_seq: 0,
        agent: "claude".into(),
        cwd: cwd.clone(),
        agent_session_id: "fake-session-1".into(),
        config: Default::default(),
        hat_id: "hat-1".into(),
        mcp: servers(),
    };
    for (frame, session, kind) in [
        (start("r1", "s1", "claude"), "s1", "session_started"),
        (resume, "s2", "session_started"),
        (start("r4", "s3", "echo"), "s3", "start_failed"),
    ] {
        sink.send(send(&frame)).await.unwrap();
        loop {
            if let HostFrame::Session { session_id, body, .. } = next(&mut stream).await
                && session_id == session
                && serde_json::to_value(&body).unwrap()["kind"] == kind
            {
                break;
            }
        }
    }
    host.abort();
}

fn plain(writer: Captured, filter: &str) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish()
}

#[tokio::test]
async fn a_sessions_token_is_never_logged_even_at_trace() {
    // The `log` bridge, for tungstenite; each run below sets its own
    // subscriber over this empty global one.
    tracing_subscriber::registry().try_init().unwrap();
    let everything = "trace,agent_client_protocol=trace,tungstenite=trace";

    // Both libraries really trace the token...
    let control = Captured::default();
    {
        let _guard = tracing::subscriber::set_default(plain(control.clone(), everything));
        run_sessions_with_the_token().await;
    }
    let control = control.text();
    for target in ["tungstenite", "agent_client_protocol"] {
        for secret in [TOKEN, URL_KEY] {
            assert!(
                control
                    .lines()
                    .any(|line| line.contains(target) && line.contains(secret)),
                "no {target} line with {secret} in the plain log"
            );
        }
    }
    // The ACP crate's own `warn`, quoting the adapter's stray stdout whole:
    // the probe for capping that target at `error`.
    assert!(
        control.lines().any(|line| {
            line.contains("agent_client_protocol") && line.contains("Invalid transport input") && line.contains(TOKEN)
        }),
        "no agent_client_protocol warn line quoting the stray stdout's token in the plain log: {control}"
    );

    // ...and the process's own subscriber, which logs, never shows it.
    let capped = Captured::default();
    {
        let subscriber = hennery_host::logging::capped(plain(capped.clone(), everything));
        let _guard = tracing::subscriber::set_default(subscriber);
        run_sessions_with_the_token().await;
    }
    let capped = capped.text();
    assert!(capped.contains("connected to collector"), "{capped}");
    assert!(capped.contains("ignoring unknown or invalid frame"), "{capped}");
    assert!(capped.contains("session start failed"), "{capped}");
    assert!(!capped.contains(TOKEN), "{capped}");
    assert!(!capped.contains(URL_KEY), "{capped}");
}
