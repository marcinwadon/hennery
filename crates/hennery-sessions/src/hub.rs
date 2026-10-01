//! Connected hosts and in-flight collector→host requests.

use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
use hennery_proto::rest::EventDto;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
    /// The host's connection lacks the capability the probe needs (ACP core
    /// §3.3); nothing was sent.
    Unsupported,
    /// The connection has `MAX_PROBES` probes in flight already; nothing
    /// was sent.
    Busy,
}

/// The most probes one host connection has in flight (decision 1): a probe
/// that times out keeps its connection, so its host may still be working
/// on it.
pub const MAX_PROBES: usize = 8;

/// The capability a probe needs from its host (ACP core §3.3), if any.
fn probe_capability(frame: &CollectorFrame) -> Option<Capability> {
    match frame {
        CollectorFrame::ListProjects { .. } | CollectorFrame::BrowseDirectory { .. } => Some(Capability::Projects),
        _ => None,
    }
}

/// The fact that completes a request, besides a rejection of its
/// `request_id` (ACP core §3.2, §3.3).
enum CompletedBy {
    /// A fact that carries the `request_id` (`resolve`).
    Request,
    /// A fact that names only the session: `session_parked`,
    /// `session_closed` (`resolve_session`).
    Session(String),
    /// The turn's `turn_ended` (`resolve_turn`): `cancel_turn`. Scoped to
    /// its session too, so a host cannot complete another session's waiter
    /// merely by naming that turn_id under a session it owns (final review
    /// M1).
    Turn { session_id: String, turn_id: String },
}

/// What a request changed in the store before it was sent. A rejection
/// means nothing happened on the host, so the socket task undoes it when
/// the rejection arrives: it sees every rejection, even when the HTTP
/// handler that sent the request is gone (its client disconnected).
#[derive(Debug, Clone, PartialEq)]
pub enum Undo {
    /// A start or resume left the session `starting`: it becomes `failed`
    /// with the rejection's code.
    Start { session_id: String },
    /// A prompt holds the session's turn slot as `sent`: the turn is
    /// removed.
    Prompt { session_id: String, turn_id: String },
}

struct Waiter {
    /// The connection the request went out on: only its loss or its
    /// timeout concerns this waiter.
    conn_id: u64,
    completed_by: CompletedBy,
    undo: Option<Undo>,
    tx: oneshot::Sender<Result<SessionBody, RequestError>>,
}

/// A probe in flight (ACP core §3.3): a request answered by a reply frame
/// that is not outboxed and changes no state (`list_projects`,
/// `browse_directory`). Kept apart from the session waiters, and scoped to
/// the connection it went out on: only that connection's reply answers it.
struct Probe {
    conn_id: u64,
    tx: oneshot::Sender<Result<HostFrame, RequestError>>,
}

struct HostConn {
    conn_id: u64,
    tx: mpsc::UnboundedSender<CollectorFrame>,
    /// Requests are sent only after the post-`resend_complete`
    /// reconciliation, so nothing sent on this connection is mistaken for
    /// something lost on the previous one (ACP core §5.1).
    ready: bool,
    kicked: CancellationToken,
    /// Cancelled once this connection is unregistered (or replaced).
    ended: CancellationToken,
    /// From this connection's `hello` (ACP core §3.3).
    capabilities: Capabilities,
}

impl HostConn {
    /// Reconciled and not kicked. A kicked connection may linger until its
    /// socket task notices (a stuck socket, a revoke whose wait timed out),
    /// but nothing is routed to it any more, and it is not listed as
    /// connected (final review M1). Read from the token, not cleared in
    /// `ready`, so every kick counts, the watchdog's included (`expire`
    /// holds no hosts lock), and a `mark_ready` landing after it too.
    fn routable(&self) -> bool {
        self.ready && !self.kicked.is_cancelled()
    }
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
    /// Shared with each waiter's watchdog (`expire`).
    waiters: Arc<Mutex<HashMap<String, Waiter>>>,
    /// Probes in flight, by request id.
    probes: Mutex<HashMap<String, Probe>>,
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
            waiters: Arc::new(Mutex::new(HashMap::new())),
            probes: Mutex::new(HashMap::new()),
            events: broadcast::channel(1024).0,
        }
    }

    /// Register a host connection (not yet ready) with the capabilities its
    /// `hello` announced. A second live connection for the same host id is
    /// refused, never allowed to supersede the first silently.
    pub fn register(
        &self,
        host_id: &str,
        tx: mpsc::UnboundedSender<CollectorFrame>,
        capabilities: Capabilities,
    ) -> Option<Registration> {
        let mut hosts = self.hosts.lock().expect("hosts lock");
        if hosts.get(host_id).is_some_and(|h| !h.tx.is_closed()) {
            return None;
        }
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let kicked = CancellationToken::new();
        let replaced = hosts.insert(
            host_id.to_string(),
            HostConn {
                conn_id,
                tx,
                ready: false,
                kicked: kicked.clone(),
                ended: CancellationToken::new(),
                capabilities,
            },
        );
        if let Some(old) = replaced {
            old.ended.cancel();
        }
        self.last_conn
            .lock()
            .expect("last_conn lock")
            .insert(host_id.to_string(), conn_id);
        Some(Registration { conn_id, kicked })
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
        if hosts.get(host_id).is_some_and(|h| h.conn_id == conn_id)
            && let Some(gone) = hosts.remove(host_id)
        {
            gone.ended.cancel();
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
        drop(waiters);
        // A probe's dropped sender answers its caller "delivery unknown"
        // (`probe`).
        self.probes
            .lock()
            .expect("probes lock")
            .retain(|_, probe| probe.conn_id != conn_id);
    }

    /// Close a host's connection from the collector side. The host
    /// reconnects, and the handshake reconciles whatever was in doubt.
    pub fn disconnect(&self, host_id: &str) {
        if let Some(h) = self.hosts.lock().expect("hosts lock").get(host_id) {
            h.kicked.cancel();
        }
    }

    /// Close the host's connection and wait, at most `bound`, until its
    /// socket task has unregistered it: then nothing that connection reads
    /// can change the store any more. `true` once no connection is left.
    pub async fn disconnect_and_wait(&self, host_id: &str, bound: Duration) -> bool {
        let ended = {
            let hosts = self.hosts.lock().expect("hosts lock");
            let Some(h) = hosts.get(host_id) else {
                return true;
            };
            h.kicked.cancel();
            h.ended.clone()
        };
        tokio::time::timeout(bound, ended.cancelled()).await.is_ok()
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

    /// Hosts that are connected, reconciled and not kicked, sorted.
    pub fn connected_hosts(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .hosts
            .lock()
            .expect("hosts lock")
            .iter()
            .filter(|(_, h)| h.routable())
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// The host is connected, reconciled and not kicked.
    pub fn is_ready(&self, host_id: &str) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .is_some_and(HostConn::routable)
    }

    /// The host's current connection announced `capability`. A host that is
    /// not connected has none.
    pub fn has_capability(&self, host_id: &str, capability: Capability) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .is_some_and(|h| h.capabilities.has(capability))
    }

    /// Send a frame nobody waits for (an answer: its verdict arrives as a
    /// fact, ACP core §4.6) to a host that is connected and reconciled.
    /// `false` if it is not: the frame then goes after its next handshake.
    pub fn notify(&self, host_id: &str, frame: CollectorFrame) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .filter(|h| h.routable())
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
        self.wait(host_id, request_id, CompletedBy::Request, None, frame, timeout)
            .await
    }

    /// Like `request`, for a request whose store change `undo` reverts if
    /// the host rejects it (`take_rejected`).
    pub async fn request_with_undo(
        &self,
        host_id: &str,
        request_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
        undo: Undo,
    ) -> Result<SessionBody, RequestError> {
        self.wait(host_id, request_id, CompletedBy::Request, Some(undo), frame, timeout)
            .await
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
        let completed_by = CompletedBy::Session(session_id.to_string());
        self.wait(host_id, request_id, completed_by, None, frame, timeout).await
    }

    /// Like `request`, for a request completed by the end of `turn_id`
    /// within `session_id` (`resolve_turn`), whatever its outcome.
    /// Rejections still match `request_id`.
    pub async fn request_for_turn(
        &self,
        host_id: &str,
        request_id: &str,
        session_id: &str,
        turn_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        let completed_by = CompletedBy::Turn {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
        };
        self.wait(host_id, request_id, completed_by, None, frame, timeout).await
    }

    async fn wait(
        &self,
        host_id: &str,
        request_id: &str,
        completed_by: CompletedBy,
        undo: Option<Undo>,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        let (tx, mut rx) = oneshot::channel();
        // Registered before the frame leaves, so a fast answer finds it; the
        // hosts lock is held throughout so the connection cannot change in
        // between (lock order: hosts, then waiters, as in `unregister`).
        let (conn_id, kicked) = {
            let hosts = self.hosts.lock().expect("hosts lock");
            let Some(host) = hosts.get(host_id).filter(|h| h.routable()) else {
                return Err(RequestError::NotConnected);
            };
            self.waiters.lock().expect("waiters lock").insert(
                request_id.to_string(),
                Waiter {
                    conn_id: host.conn_id,
                    completed_by,
                    undo,
                    tx,
                },
            );
            if host.tx.send(frame).is_err() {
                self.waiters.lock().expect("waiters lock").remove(request_id);
                return Err(RequestError::NotConnected);
            }
            (host.conn_id, host.kicked.clone())
        };
        // The deadline belongs to the hub, not to this future: the handler
        // awaiting it is dropped when its client disconnects, and a host
        // that never answers must still lose its connection and its waiter.
        let watchdog = tokio::spawn(expire(
            self.waiters.clone(),
            host_id.to_string(),
            request_id.to_string(),
            conn_id,
            kicked,
            timeout,
        ));
        let result = tokio::time::timeout(timeout, &mut rx).await;
        watchdog.abort();
        match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(RequestError::DeliveryUnknown),
            Err(_elapsed) => {
                // Whoever removes the entry answers it. If a fact, a
                // rejection or the watchdog took it just now, its answer is
                // on the way: a rejection's undo is already in the store.
                if self.waiters.lock().expect("waiters lock").remove(request_id).is_none() {
                    return rx.await.unwrap_or(Err(RequestError::DeliveryUnknown));
                }
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

    /// Send a probe (ACP core §3.3) and wait for its reply: the frame
    /// carrying `request_id` that the same connection sends back
    /// (`probe_reply`), its rejection (`probe_rejected`), the connection's
    /// end, or `timeout`. A timeout answers `DeliveryUnknown` but keeps the
    /// connection: a probe changes no state, so there is nothing for a
    /// reconnect to reconcile (decision 1). Dropping this future removes
    /// the probe, so a reply that comes after it is dropped.
    ///
    /// The connection is chosen once, under the hosts lock: it must be
    /// reconciled and have the capability the probe needs, and the frame
    /// goes out on that same connection (the review's A5).
    pub async fn probe(
        &self,
        host_id: &str,
        request_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<HostFrame, RequestError> {
        let (tx, rx) = oneshot::channel();
        // Lock order: hosts, then probes, as for the waiters (`wait`).
        let conn_id = {
            let hosts = self.hosts.lock().expect("hosts lock");
            let Some(host) = hosts.get(host_id).filter(|h| h.routable()) else {
                return Err(RequestError::NotConnected);
            };
            if probe_capability(&frame).is_some_and(|needed| !host.capabilities.has(needed)) {
                return Err(RequestError::Unsupported);
            }
            let mut probes = self.probes.lock().expect("probes lock");
            if probes.values().filter(|p| p.conn_id == host.conn_id).count() >= MAX_PROBES {
                return Err(RequestError::Busy);
            }
            probes.insert(
                request_id.to_string(),
                Probe {
                    conn_id: host.conn_id,
                    tx,
                },
            );
            drop(probes);
            if host.tx.send(frame).is_err() {
                self.probes.lock().expect("probes lock").remove(request_id);
                return Err(RequestError::NotConnected);
            }
            host.conn_id
        };
        let _gone = ProbeGuard {
            probes: &self.probes,
            request_id,
            conn_id,
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(answer)) => answer,
            // The connection ended (`unregister`).
            Ok(Err(_)) => Err(RequestError::DeliveryUnknown),
            Err(_elapsed) => {
                tracing::info!(%host_id, %request_id, "probe unanswered; the connection is kept");
                Err(RequestError::DeliveryUnknown)
            }
        }
    }

    /// The host connection `conn_id` sent a probe reply (`frame` is one,
    /// `HostFrame::probe_request_id`): answer the probe it names, if that
    /// probe went out on this connection and is still waiting. Otherwise
    /// (late, from another connection, or never asked) it is logged and
    /// dropped.
    pub fn probe_reply(&self, host_id: &str, conn_id: u64, frame: HostFrame) {
        let Some(request_id) = frame.probe_request_id() else {
            tracing::warn!(%host_id, "not a probe reply; dropped");
            return;
        };
        match self.take_probe(conn_id, request_id) {
            Some(probe) => {
                let _ = probe.tx.send(Ok(frame));
            }
            None => tracing::info!(%host_id, ?request_id, "dropping a probe reply nobody waits for"),
        }
    }

    /// The host connection `conn_id` rejected `request_id`: if that is a
    /// probe of this connection, answer it `Rejected` and say so. `false`
    /// leaves the rejection to the session waiters.
    pub fn probe_rejected(&self, conn_id: u64, request_id: &str, code: &str, message: &str) -> bool {
        let Some(probe) = self.take_probe(conn_id, request_id) else {
            return false;
        };
        let _ = probe.tx.send(Err(RequestError::Rejected {
            code: code.to_string(),
            message: message.to_string(),
        }));
        true
    }

    fn take_probe(&self, conn_id: u64, request_id: &str) -> Option<Probe> {
        let mut probes = self.probes.lock().expect("probes lock");
        if probes.get(request_id).is_some_and(|p| p.conn_id == conn_id) {
            probes.remove(request_id)
        } else {
            None
        }
    }

    /// Probes still waiting for their reply.
    pub fn pending_probes(&self) -> usize {
        self.probes.lock().expect("probes lock").len()
    }

    pub fn resolve(&self, request_id: &str, fact: SessionBody) {
        if let Some(w) = self.waiters.lock().expect("waiters lock").remove(request_id) {
            let _ = w.tx.send(Ok(fact));
        }
    }

    /// Resolve every waiter registered with `request_for_session` for
    /// `session_id`.
    pub fn resolve_session(&self, session_id: &str, fact: SessionBody) {
        self.resolve_where(|by| matches!(by, CompletedBy::Session(s) if s == session_id), fact);
    }

    /// Resolve every waiter registered with `request_for_turn` for `turn_id`
    /// within `session_id`. A turn_id that matches under a different
    /// session (however that came to be) resolves nothing (final review
    /// M1).
    pub fn resolve_turn(&self, session_id: &str, turn_id: &str, fact: SessionBody) {
        self.resolve_where(
            |by| matches!(by, CompletedBy::Turn { session_id: s, turn_id: t } if s == session_id && t == turn_id),
            fact,
        );
    }

    fn resolve_where(&self, completes: impl Fn(&CompletedBy) -> bool, fact: SessionBody) {
        let mut waiters = self.waiters.lock().expect("waiters lock");
        let ids: Vec<String> = waiters
            .iter()
            .filter(|(_, w)| completes(&w.completed_by))
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            if let Some(w) = waiters.remove(&id) {
                let _ = w.tx.send(Ok(fact.clone()));
            }
        }
    }

    /// Take the waiter of a request the host rejected out of the hub. From
    /// here on nothing else can answer it (not its timeout, not the
    /// watchdog), so the caller applies the undo, then `answer`s, and the
    /// store and the HTTP answer always agree (decision 6). The waiter
    /// outlives an HTTP handler that was dropped mid-request.
    pub fn take_rejected(&self, request_id: &str) -> Option<Rejection> {
        let w = self.waiters.lock().expect("waiters lock").remove(request_id)?;
        Some(Rejection { undo: w.undo, tx: w.tx })
    }

    pub fn reject(&self, request_id: &str, code: String, message: String) {
        if let Some(rejection) = self.take_rejected(request_id) {
            rejection.answer(code, message);
        }
    }

    /// Requests still waiting for their answer.
    pub fn pending_requests(&self) -> usize {
        self.waiters.lock().expect("waiters lock").len()
    }

    pub fn publish(&self, event: EventDto) {
        let _ = self.events.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventDto> {
        self.events.subscribe()
    }
}

/// Removes a probe whose caller is gone: answered, timed out, or dropped
/// with its HTTP handler.
struct ProbeGuard<'a> {
    probes: &'a Mutex<HashMap<String, Probe>>,
    request_id: &'a str,
    conn_id: u64,
}

impl Drop for ProbeGuard<'_> {
    fn drop(&mut self) {
        let mut probes = self.probes.lock().expect("probes lock");
        if probes.get(self.request_id).is_some_and(|p| p.conn_id == self.conn_id) {
            probes.remove(self.request_id);
        }
    }
}

/// A waiter taken out of the hub by a host rejection (`Hub::take_rejected`).
pub struct Rejection {
    undo: Option<Undo>,
    tx: oneshot::Sender<Result<SessionBody, RequestError>>,
}

impl Rejection {
    /// What the request changed in the store, to revert first.
    pub fn undo(&self) -> Option<&Undo> {
        self.undo.as_ref()
    }

    /// Answer the request's caller, if it is still there.
    pub fn answer(self, code: String, message: String) {
        let _ = self.tx.send(Err(RequestError::Rejected { code, message }));
    }

    /// Answer the request's caller as delivery-unknown, for when applying
    /// this rejection's undo failed: the store no longer agrees with the
    /// rejection it was supposed to reflect, so the caller must not be told
    /// `Rejected` — that would say the store and the answer agree when they
    /// do not. The socket task drops the connection right after this, same
    /// as any other request whose outcome is not known.
    pub fn delivery_unknown(self) {
        let _ = self.tx.send(Err(RequestError::DeliveryUnknown));
    }
}

/// One waiter's deadline, owned by the hub. If the waiter is still there
/// for the same connection when `timeout` has passed, nobody answered it:
/// it is removed, its caller (if any) hears "delivery unknown", and the
/// connection is dropped so the next handshake reconciles the request
/// (ACP core §3.4). Idempotent with the handler's own timeout: whichever
/// removes the entry acts.
async fn expire(
    waiters: Arc<Mutex<HashMap<String, Waiter>>>,
    host_id: String,
    request_id: String,
    conn_id: u64,
    kicked: CancellationToken,
    timeout: Duration,
) {
    tokio::time::sleep(timeout).await;
    let expired = {
        let mut waiters = waiters.lock().expect("waiters lock");
        match waiters.get(&request_id) {
            Some(w) if w.conn_id == conn_id => waiters.remove(&request_id),
            _ => None,
        }
    };
    if let Some(w) = expired {
        tracing::warn!(%host_id, %request_id, "request timed out with no handler left; dropping the host connection");
        kicked.cancel();
        let _ = w.tx.send(Err(RequestError::DeliveryUnknown));
    }
}
