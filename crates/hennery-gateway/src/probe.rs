//! The health probe (gateway spec §7): a real MCP handshake through the
//! proxy's own forwarding path (`proxy::send_through`): `initialize`, then
//! `notifications/initialized` and `tools/list` on that session, then
//! `DELETE` (G-21: a session-less `tools/list` gets 400 from stateful
//! servers).
//!
//! - **What it concludes:** every step 2xx → `ok`; a 401 after one
//!   single-flight refresh → `needs_auth`; a 5xx, a redirect or a transport
//!   failure → `error`, with a note; any other 4xx → no change. Each moves
//!   `checked_at`. The `DELETE`'s answer is not read: many servers take no
//!   `DELETE`.
//! - **Background:** every `EVERY`, for OAuth connections with a grant only
//!   (`ProxyStore::probe_targets`); static and `none` connections are not
//!   probed, nor any without a credential. Each tick is a task of its own,
//!   bounded by `TICK_TIMEOUT`, so a panic or a hang ends that tick, not
//!   the loop.
//! - **Probe now** (`POST …/probe`, api-8e-8f B6, may change):
//!   single-flight per connection; within `REUSE` of a completed probe the
//!   result stands without a new request; each probe is bounded by
//!   `PROBE_TIMEOUT`.

use crate::model::Status;
use crate::proxy::{Sent, send_through};
use crate::runtime::Runtime;
use crate::scope::ScopedConnection;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, Method};
use hennery_kernel::secret::unix_now;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the background probe runs (gateway spec §7).
pub const EVERY: Duration = Duration::from_secs(15 * 60);

/// The most one background tick takes, all its connections included.
pub const TICK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The most one probe takes (api-8e-8f B6).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Within this many seconds of a completed probe, its result stands.
pub const REUSE: i64 = 10;

/// The largest answer the probe reads.
const MAX_ANSWER: usize = 1024 * 1024;

/// What a probe concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// The upstream refused the credential, after one refresh.
    NeedsAuth,
    /// An outage, with what the operator is shown.
    Error(String),
    /// Nothing that changes the status (another 4xx).
    NoChange,
    /// The connection has no credential to send: nothing was sent.
    NoCredential,
}

/// A connection's latest completed probe (when, and what it concluded),
/// behind the lock its next probe takes.
type Slot = Arc<tokio::sync::Mutex<Option<(i64, Verdict)>>>;

/// One slot per connection.
#[derive(Default)]
pub struct Probes {
    slots: Mutex<HashMap<String, Slot>>,
}

impl Probes {
    fn slot(&self, id: &str) -> Slot {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(id.to_string())
            .or_default()
            .clone()
    }
}

/// Probe `connection` now, single-flight: a concurrent caller waits for
/// the probe in flight and gets its result, and so does one within `REUSE`
/// seconds of it.
pub async fn probe_now(runtime: &Arc<Runtime>, connection: &ScopedConnection) -> Verdict {
    let slot = runtime.probes.slot(&connection.id);
    let mut last = slot.lock().await;
    if let Some((at, verdict)) = &*last
        && unix_now() - *at < REUSE
    {
        return verdict.clone();
    }
    let verdict = match tokio::time::timeout(runtime.probe_timeout, handshake(runtime, connection)).await {
        Ok(verdict) => verdict,
        Err(_) => Verdict::Error("the probe timed out".into()),
    };
    record(runtime, connection, &verdict);
    *last = Some((unix_now(), verdict.clone()));
    verdict
}

/// Write what `verdict` says of `connection`'s status (gateway spec §7).
fn record(runtime: &Runtime, connection: &ScopedConnection, verdict: &Verdict) {
    let now = unix_now();
    let url = &connection.url;
    match verdict {
        Verdict::Ok => runtime.announce(
            runtime
                .statuses
                .record_status(&connection.id, url, Status::Ok, None, now),
        ),
        Verdict::Error(note) => {
            runtime.announce(
                runtime
                    .statuses
                    .record_status(&connection.id, url, Status::Error, Some(note), now),
            )
        }
        Verdict::NoChange => {
            if let Err(err) = runtime.statuses.record_checked(&connection.id, url, now) {
                tracing::error!(connection_id = %connection.id, error = %err, "gateway probe: not recorded");
            }
        }
        // Recorded by the forwarding path itself (`send_through`).
        Verdict::NeedsAuth | Verdict::NoCredential => {}
    }
}

/// One JSON-RPC message, as the probe sends it.
fn message(id: Option<i64>, method: &str, params: Option<Value>) -> Bytes {
    let mut out = json!({ "jsonrpc": "2.0", "method": method });
    if let Some(id) = id {
        out["id"] = json!(id);
    }
    if let Some(params) = params {
        out["params"] = params;
    }
    Bytes::from(out.to_string())
}

/// What one step came to: its answer's session and message, or the
/// verdict that ends the handshake.
enum Step {
    Done {
        session_id: Option<HeaderValue>,
        message: Option<Value>,
    },
    End(Verdict),
}

async fn step(
    runtime: &Arc<Runtime>,
    connection: &ScopedConnection,
    method: Method,
    headers: &HeaderMap,
    body: Option<Bytes>,
    id: Option<i64>,
) -> Step {
    // Each request's head as the whole probe's bound; the whole is
    // `runtime.probe_timeout`.
    let sent = send_through(runtime, connection, &method, headers, body, PROBE_TIMEOUT).await;
    let response = match sent {
        Sent::Answer { response, .. } => response,
        Sent::NoCredential => return Step::End(Verdict::NoCredential),
        Sent::AuthRefused => return Step::End(Verdict::NeedsAuth),
        Sent::Unreachable { .. } => return Step::End(Verdict::Error("the upstream could not be reached".into())),
        Sent::RefreshUnavailable => {
            return Step::End(Verdict::Error(
                "the authorization server could not be reached to refresh the grant".into(),
            ));
        }
        Sent::RefreshUnsaved | Sent::Changed | Sent::Internal(_) => {
            tracing::error!(connection_id = %connection.id, "gateway probe: the gateway failed, not the upstream");
            return Step::End(Verdict::NoChange);
        }
    };
    let status = response.status();
    if status.is_server_error() {
        return Step::End(Verdict::Error(format!(
            "the upstream answered HTTP {}",
            status.as_u16()
        )));
    }
    if status.is_redirection() {
        return Step::End(Verdict::Error("the upstream answered with a redirect".into()));
    }
    if !status.is_success() {
        return Step::End(Verdict::NoChange);
    }
    let session_id = response.headers().get("mcp-session-id").cloned();
    let message = match id {
        Some(id) => answer(response, id).await,
        None => None,
    };
    Step::Done { session_id, message }
}

/// The JSON-RPC answer with `id` in `response`, JSON or an event stream,
/// read up to `MAX_ANSWER` bytes; the stream is not read past it.
async fn answer(mut response: reqwest::Response, id: i64) -> Option<Value> {
    let stream = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/event-stream"));
    let mut read = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if read.len() + chunk.len() > MAX_ANSWER {
            return None;
        }
        read.extend_from_slice(&chunk);
        if stream {
            let text = String::from_utf8_lossy(&read);
            for event in text.split("\n\n") {
                let data: String = event
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Ok(value) = serde_json::from_str::<Value>(&data)
                    && value.get("id").and_then(Value::as_i64) == Some(id)
                {
                    return Some(value);
                }
            }
        }
    }
    if stream {
        None
    } else {
        serde_json::from_slice(&read).ok()
    }
}

/// The handshake (gateway spec §7).
async fn handshake(runtime: &Arc<Runtime>, connection: &ScopedConnection) -> Verdict {
    let mut headers = HeaderMap::new();
    let initialize = message(
        Some(1),
        "initialize",
        Some(json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "hennery-probe", "version": env!("CARGO_PKG_VERSION") },
        })),
    );
    let (session_id, initialized) =
        match step(runtime, connection, Method::POST, &headers, Some(initialize), Some(1)).await {
            Step::Done { session_id, message } => (session_id, message),
            Step::End(verdict) => return verdict,
        };
    if let Some(session_id) = &session_id {
        headers.insert("mcp-session-id", session_id.clone());
    }
    if let Some(version) = initialized
        .as_ref()
        .and_then(|m| m.pointer("/result/protocolVersion"))
        .and_then(Value::as_str)
        .and_then(|v| HeaderValue::from_str(v).ok())
    {
        headers.insert("mcp-protocol-version", version);
    }
    let notified = message(None, "notifications/initialized", None);
    if let Step::End(verdict) = step(runtime, connection, Method::POST, &headers, Some(notified), None).await {
        return verdict;
    }
    let verdict = match step(
        runtime,
        connection,
        Method::POST,
        &headers,
        Some(message(Some(2), "tools/list", None)),
        Some(2),
    )
    .await
    {
        Step::Done { .. } => Verdict::Ok,
        Step::End(verdict) => verdict,
    };
    if session_id.is_some() {
        // Ends the upstream session; its answer is not read.
        let _ = step(runtime, connection, Method::DELETE, &headers, None, None).await;
    }
    verdict
}

/// One background tick: every OAuth connection with a grant, one after
/// another.
pub async fn tick(runtime: &Arc<Runtime>) {
    let targets = match runtime.statuses.probe_targets() {
        Ok(targets) => targets,
        Err(err) => {
            tracing::error!(error = %err, "gateway probe: connections not read");
            return;
        }
    };
    for connection in targets {
        let verdict = probe_now(runtime, &connection).await;
        tracing::debug!(connection_id = %connection.id, verdict = ?verdict, "gateway probe: checked");
    }
}

/// The background probe (gateway spec §7): a tick every `every`, until
/// `shutdown` resolves. A tick that panics or passes `tick_timeout` is
/// logged and the loop goes on.
pub async fn run(runtime: Arc<Runtime>, every: Duration, tick_timeout: Duration, shutdown: impl Future<Output = ()>) {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => return,
            () = tokio::time::sleep(every) => {}
        }
        let rt = runtime.clone();
        let mut task = tokio::spawn(async move { tick(&rt).await });
        match tokio::time::timeout(tick_timeout, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::error!(error = %err, "gateway probe: a tick failed"),
            Err(_) => {
                task.abort();
                tracing::warn!("gateway probe: a tick timed out");
            }
        }
    }
}
