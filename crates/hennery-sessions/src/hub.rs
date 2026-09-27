//! Connected hosts and in-flight collector→host requests.

use hennery_proto::frames::{CollectorFrame, SessionBody};
use hennery_proto::rest::EventDto;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Why a request produced no fact.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestError {
    /// The host is not connected.
    NotConnected,
    /// The host rejected the request; nothing happened.
    Rejected { code: String, message: String },
    /// The connection dropped or the timeout passed: the request may or may
    /// not have been delivered. Reconciled later from the host's outbox.
    DeliveryUnknown,
}

struct Waiter {
    host_id: String,
    tx: oneshot::Sender<Result<SessionBody, RequestError>>,
}

struct HostConn {
    conn_id: u64,
    tx: mpsc::UnboundedSender<CollectorFrame>,
}

pub struct Hub {
    next_conn: AtomicU64,
    hosts: Mutex<HashMap<String, HostConn>>,
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
            waiters: Mutex::new(HashMap::new()),
            events: broadcast::channel(1024).0,
        }
    }

    /// Register a host connection. A second live connection for the same
    /// host id is refused, never allowed to supersede the first silently.
    pub fn register(&self, host_id: &str, tx: mpsc::UnboundedSender<CollectorFrame>) -> Option<u64> {
        let mut hosts = self.hosts.lock().expect("hosts lock");
        if hosts.get(host_id).is_some_and(|h| !h.tx.is_closed()) {
            return None;
        }
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        hosts.insert(host_id.to_string(), HostConn { conn_id, tx });
        Some(conn_id)
    }

    /// Drop a connection and fail its in-flight requests as delivery-unknown.
    pub fn unregister(&self, host_id: &str, conn_id: u64) {
        let mut hosts = self.hosts.lock().expect("hosts lock");
        if hosts.get(host_id).is_some_and(|h| h.conn_id == conn_id) {
            hosts.remove(host_id);
        }
        drop(hosts);
        let mut waiters = self.waiters.lock().expect("waiters lock");
        let ids: Vec<String> = waiters
            .iter()
            .filter(|(_, w)| w.host_id == host_id)
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            if let Some(w) = waiters.remove(&id) {
                let _ = w.tx.send(Err(RequestError::DeliveryUnknown));
            }
        }
    }

    pub fn connected_hosts(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.hosts.lock().expect("hosts lock").keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Send a frame to a host without waiting for anything.
    pub fn send(&self, host_id: &str, frame: CollectorFrame) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .is_some_and(|h| h.tx.send(frame).is_ok())
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
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().expect("waiters lock").insert(
            request_id.to_string(),
            Waiter {
                host_id: host_id.to_string(),
                tx,
            },
        );
        if !self.send(host_id, frame) {
            self.waiters.lock().expect("waiters lock").remove(request_id);
            return Err(RequestError::NotConnected);
        }
        let result = tokio::time::timeout(timeout, rx).await;
        self.waiters.lock().expect("waiters lock").remove(request_id);
        match result {
            Ok(Ok(outcome)) => outcome,
            _ => Err(RequestError::DeliveryUnknown),
        }
    }

    pub fn resolve(&self, request_id: &str, fact: SessionBody) {
        if let Some(w) = self.waiters.lock().expect("waiters lock").remove(request_id) {
            let _ = w.tx.send(Ok(fact));
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
