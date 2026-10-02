//! Plan 8e, ACP core §8: the collector's own log never shows a session
//! token a host's frame quotes. A frame that does not decode is logged by
//! its error's kind and place (serde's message quotes the offending value),
//! and a host's refusal by its message with any token redacted (decision
//! 11). Under the subscriber the process installs (`logging::capped`), at
//! `trace`, the thread's own: the runtime is current-thread.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::HostFrame;
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tracing_subscriber::util::SubscriberInitExt;

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

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

#[test]
fn a_token_a_host_frame_quotes_never_reaches_the_collectors_log() {
    let token = format!("{}{}", hennery_gateway::tokens::SESSION_TOKEN_PREFIX, "5a".repeat(32));
    let logs = Captured::default();
    let subscriber = hennery_host::logging::capped(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer({
                let logs = logs.clone();
                move || logs.clone()
            })
            .finish(),
    );
    let _default = subscriber.set_default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        hosts.register("host-1", &enrollment, 0).unwrap();
        let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = state.shutdown.clone();
        let server = tokio::spawn(hennery_sessions::serve(listener, state));
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let (mut sink, mut stream) = ws.split();
        let hello = HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "0".into(),
            host_id: "host-1".into(),
            proof: host_key().sign_hello(&nonce, "host-1", PROTOCOL_VERSION),
            capabilities: Default::default(),
            workspace_roots: vec![],
            attached_sessions: vec![],
            mcp_isolation: Default::default(),
        };
        sink.send(Message::text(serde_json::to_string(&hello).unwrap()))
            .await
            .unwrap();
        assert!(matches!(stream.next().await, Some(Ok(Message::Text(_)))), "hello_ack");
        // A frame that does not decode, the token where a number goes:
        // serde's error would quote it.
        let undecodable = serde_json::json!({"type": "session", "session_id": "s1", "seq": token, "body": {}});
        sink.send(Message::text(undecodable.to_string())).await.unwrap();
        // A refusal nobody waits for, quoting the token.
        let refusal = HostFrame::Error {
            request_id: "nobody".into(),
            code: "start_failed".into(),
            message: format!("the agent said {token}"),
        };
        sink.send(Message::text(serde_json::to_string(&refusal).unwrap()))
            .await
            .unwrap();
        // Positive signal: both were logged.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !(logs.text().contains("ignoring unknown or invalid frame")
            && logs.text().contains("host refused a request nobody waits for"))
        {
            assert!(tokio::time::Instant::now() < deadline, "not logged: {}", logs.text());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        shutdown.cancel();
        let _ = server.await;
    });
    drop(runtime);
    let logged = logs.text();
    assert!(!logged.contains(&token), "a session token reached the log: {logged}");
    assert!(logged.contains("hnry_session_<redacted>"), "{logged}");
}
