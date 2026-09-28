//! Connected hosts and in-flight collector→host requests.

use hennery_proto::frames::{CollectorFrame, SessionBody};
use hennery_proto::rest::EventDto;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Why a request produced no fact.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestError {
    /// The host is not connected, or is connected but not yet reconciled.
    NotConnected,
    /// The host rejected the request; nothing happened.
    Rejected { code: String, message: String },
    /// The connection dropped or the timeout passed: the request may or may
    /// not have been delivered. Reconciled after the host's next handshake.
    DeliveryUnknown,
}

struct Waiter {
    /// The connection the request went out on: only its loss or its
    /// timeout concerns this waiter.
    conn_id: u64,
    /// Set for requests completed by a fact that names only the session
    /// (`session_parked`, `session_closed`), not the request (§3.2).
    session_id: Option<String>,
    tx: oneshot::Sender<Result<SessionBody, RequestError>>,
}

struct HostConn {
    conn_id: u64,
    tx: mpsc::UnboundedSender<CollectorFrame>,
    /// Requests are sent only after the post-`resend_complete`
    /// reconciliation, so nothing sent on this connection is mistaken for
    /// something lost on the previous one (ACP core §5.1).
    ready: bool,
    kicked: CancellationToken,
}

/// A registered host connection.
pub struct Registration {
    pub conn_id: u64,
    /// Cancelled when the hub drops this connection (a request timed out on
    /// it); the socket task must then close the socket.
    pub kicked: CancellationToken,
}

pub struct Hub {
    next_conn: AtomicU64,
    hosts: Mutex<HashMap<String, HostConn>>,
    /// The latest connection each host registered (kept after it ends), so
    /// an offline timer can tell whether the host came back since.
    last_conn: Mutex<HashMap<String, u64>>,
    waiters: Mutex<HashMap<String, Waiter>>,
    events: broadcast::Sender<EventDto>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            next_conn: AtomicU64::new(1),
            hosts: Mutex::new(HashMap::new()),
            last_conn: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            events: broadcast::channel(1024).0,
        }
    }

    /// Register a host connection (not yet ready). A second live connection
    /// for the same host id is refused, never allowed to supersede the
    /// first silently.
    pub fn register(&self, host_id: &str, tx: mpsc::UnboundedSender<CollectorFrame>) -> Option<Registration> {
        let mut hosts = self.hosts.lock().expect("hosts lock");
        if hosts.get(host_id).is_some_and(|h| !h.tx.is_closed()) {
            return None;
        }
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let kicked = CancellationToken::new();
        hosts.insert(
            host_id.to_string(),
            HostConn {
                conn_id,
                tx,
                ready: false,
                kicked: kicked.clone(),
            },
        );
        self.last_conn
            .lock()
            .expect("last_conn lock")
            .insert(host_id.to_string(), conn_id);
        Some(Registration { conn_id, kicked })
    }

    /// The latest connection `host_id` registered, if any since start.
    pub fn last_conn(&self, host_id: &str) -> Option<u64> {
        self.last_conn.lock().expect("last_conn lock").get(host_id).copied()
    }

    /// Run `f` only if `host_id` has registered no connection since `since`
    /// (`None`: none since this collector started). The lock is held while
    /// `f` runs, so a reconnect waits for it and its reconciliation sees
    /// whatever `f` wrote (ACP core §5.3).
    pub fn if_offline_since<R>(&self, host_id: &str, since: Option<u64>, f: impl FnOnce() -> R) -> Option<R> {
        let last = self.last_conn.lock().expect("last_conn lock");
        (last.get(host_id).copied() == since).then(f)
    }

    /// Reconciliation for this connection is done: requests may flow.
    pub fn mark_ready(&self, host_id: &str, conn_id: u64) {
        if let Some(h) = self.hosts.lock().expect("hosts lock").get_mut(host_id)
            && h.conn_id == conn_id
        {
            h.ready = true;
        }
    }

    /// Drop a connection and fail its in-flight requests as delivery-unknown.
    /// Requests a newer connection of the same host carries are untouched.
    pub fn unregister(&self, host_id: &str, conn_id: u64) {
        let mut hosts = self.hosts.lock().expect("hosts lock");
        if hosts.get(host_id).is_some_and(|h| h.conn_id == conn_id) {
            hosts.remove(host_id);
        }
        drop(hosts);
        let mut waiters = self.waiters.lock().expect("waiters lock");
        let ids: Vec<String> = waiters
            .iter()
            .filter(|(_, w)| w.conn_id == conn_id)
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            if let Some(w) = waiters.remove(&id) {
                let _ = w.tx.send(Err(RequestError::DeliveryUnknown));
            }
        }
    }

    /// Close a host's connection from the collector side. The host
    /// reconnects, and the handshake reconciles whatever was in doubt.
    pub fn disconnect(&self, host_id: &str) {
        if let Some(h) = self.hosts.lock().expect("hosts lock").get(host_id) {
            h.kicked.cancel();
        }
    }

    /// Like `disconnect`, but only if `conn_id` is still the host's current
    /// connection.
    pub fn disconnect_conn(&self, host_id: &str, conn_id: u64) {
        if let Some(h) = self.hosts.lock().expect("hosts lock").get(host_id)
            && h.conn_id == conn_id
        {
            h.kicked.cancel();
        }
    }

    /// Hosts that are connected and reconciled, sorted.
    pub fn connected_hosts(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .hosts
            .lock()
            .expect("hosts lock")
            .iter()
            .filter(|(_, h)| h.ready)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// The host is connected and reconciled.
    pub fn is_ready(&self, host_id: &str) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .is_some_and(|h| h.ready)
    }

    /// Send a frame to a ready host without waiting for anything.
    pub fn send(&self, host_id: &str, frame: CollectorFrame) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .is_some_and(|h| h.ready && h.tx.send(frame).is_ok())
    }

    /// Send a request and wait until the outboxed fact carrying `request_id`
    /// is ingested (`resolve`), the host rejects it (`reject`), the
    /// connection drops, or `timeout` passes.
    pub async fn request(
        &self,
        host_id: &str,
        request_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        self.wait(host_id, request_id, None, frame, timeout).await
    }

    /// Like `request`, for requests completed by a fact that names only the
    /// session (`resolve_session`). Rejections still match `request_id`.
    pub async fn request_for_session(
        &self,
        host_id: &str,
        request_id: &str,
        session_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        self.wait(host_id, request_id, Some(session_id.to_string()), frame, timeout)
            .await
    }

    async fn wait(
        &self,
        host_id: &str,
        request_id: &str,
        session_id: Option<String>,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        let (tx, rx) = oneshot::channel();
        // Registered before the frame leaves, so a fast answer finds it; the
        // hosts lock is held throughout so the connection cannot change in
        // between (lock order: hosts, then waiters, as in `unregister`).
        let conn_id = {
            let hosts = self.hosts.lock().expect("hosts lock");
            let Some(host) = hosts.get(host_id).filter(|h| h.ready) else {
                return Err(RequestError::NotConnected);
            };
            self.waiters.lock().expect("waiters lock").insert(
                request_id.to_string(),
                Waiter {
                    conn_id: host.conn_id,
                    session_id,
                    tx,
                },
            );
            if host.tx.send(frame).is_err() {
                self.waiters.lock().expect("waiters lock").remove(request_id);
                return Err(RequestError::NotConnected);
            }
            host.conn_id
        };
        let result = tokio::time::timeout(timeout, rx).await;
        self.waiters.lock().expect("waiters lock").remove(request_id);
        match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(RequestError::DeliveryUnknown),
            Err(_elapsed) => {
                // Every timeout is at least the read deadline, so a live
                // connection that produced neither the fact nor a rejection
                // is not to be trusted: drop it, and the next handshake
                // reconciles this request (ACP core §3.4).
                tracing::warn!(%host_id, %request_id, "request timed out; dropping the host connection");
                self.disconnect_conn(host_id, conn_id);
                Err(RequestError::DeliveryUnknown)
            }
        }
    }

    pub fn resolve(&self, request_id: &str, fact: SessionBody) {
        if let Some(w) = self.waiters.lock().expect("waiters lock").remove(request_id) {
            let _ = w.tx.send(Ok(fact));
        }
    }

    /// Resolve every waiter registered with `request_for_session` for
    /// `session_id`.
    pub fn resolve_session(&self, session_id: &str, fact: SessionBody) {
        let mut waiters = self.waiters.lock().expect("waiters lock");
        let ids: Vec<String> = waiters
            .iter()
            .filter(|(_, w)| w.session_id.as_deref() == Some(session_id))
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            if let Some(w) = waiters.remove(&id) {
                let _ = w.tx.send(Ok(fact.clone()));
            }
        }
    }

    pub fn reject(&self, request_id: &str, code: String, message: String) {
        if let Some(w) = self.waiters.lock().expect("waiters lock").remove(request_id) {
            let _ = w.tx.send(Err(RequestError::Rejected { code, message }));
        }
    }

    pub fn publish(&self, event: EventDto) {
        let _ = self.events.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventDto> {
        self.events.subscribe()
    }
}
