//! Plan 8b-ii's obligation to the proxy: a connection not marked
//! `internal_network` is refused plain `http` to a LAN address, by the
//! egress policy, before anything is sent. The API never stores that (plan
//! 8a saves `http` only on a marked connection), so the row is written as a
//! damaged or older store might hold it. The refusal and a failed connect
//! answer the same 502, so the test reads the proxy's log line: a binary
//! of its own, the subscriber being the process's.

mod support;

use axum::http::StatusCode;
use hennery_gateway::model::CredKind;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::upstream::Harness;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unmarked_connection_is_refused_plain_http_to_a_lan_address() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();

    let h = Harness::new().await;
    h.host("host-a", 1);
    // An RFC 1918 address: nothing here answers it.
    let id = h.connection("lan", "http://10.255.255.1:9/mcp", CredKind::None);
    h.mount(&id, &["host-a"]);
    let changed = h
        .raw()
        .execute("UPDATE gw_connections SET internal_network = 0 WHERE id = ?1", [&id])
        .unwrap();
    assert_eq!(changed, 1);
    let token = h.mint("s1", "host-a", &h.hat());

    let resp = h
        .post("lan", &token, &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_unreachable");

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|line| line.contains("gateway proxy: the upstream was not reached"))
        .unwrap_or_else(|| panic!("no warning:\n{logs}"));
    assert!(line.contains("http://10.255.255.1:9"), "{line}");
    assert!(
        line.contains("refused by the egress policy"),
        "sent, not refused: {line}"
    );
}
