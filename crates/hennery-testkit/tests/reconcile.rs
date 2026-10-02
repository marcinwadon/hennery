//! The collector's reconciliation (ACP core §3.4, §5.1, §5.2) against a
//! scripted host over a real WebSocket: the test plays the host frame by
//! frame, so it controls exactly what was delivered before a drop.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
use hennery_proto::rest::EventDto;
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::{AppState, store::Store};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const HOST: &str = "host-1";

/// The key `HOST` is paired with.
fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
    /// What the push triggers queued (plan 10b), unread by any delivery.
    notices: std::sync::Mutex<hennery_kernel::push::Notices>,
}

impl Collector {
    async fn start() -> Self {
        Self::start_in(
            tempfile::tempdir().unwrap(),
            hennery_sessions::offline::OFFLINE_THRESHOLD,
        )
        .await
    }

    /// A collector over the database in `dir` that presumes a host's
    /// sessions parked once it has been offline for `offline`.
    async fn start_in(dir: tempfile::TempDir, offline: Duration) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hosts = Hosts::open(&dir.path().join("hennery.db")).unwrap();
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        // A no-op when the directory is reused (a collector restart).
        hosts.register(HOST, &enrollment, 0).unwrap();
        let mut state = AppState::new(
            Store::open(&dir.path().join("hennery.db")).unwrap(),
            hosts,
            Operator::open(&dir.path().join("hennery.db")).unwrap(),
        );
        state.offline_threshold = offline;
        let (push, notices) = hennery_kernel::push::Push::new();
        state.push = push;
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            addr,
            state,
            _dir: dir,
            notices: std::sync::Mutex::new(notices),
        }
    }

    /// Every notice queued since the last call.
    fn notices(&self) -> Vec<hennery_kernel::push::Notice> {
        let mut queue = self.notices.lock().unwrap();
        std::iter::from_fn(|| queue.try_recv()).collect()
    }

    /// The notices queued from now until one has `body`, that one last: a
    /// notice is queued after its fact's commit, so the store showing the
    /// fact does not mean the notice is there yet.
    async fn notices_until(&self, body: &str) -> Vec<hennery_kernel::push::Notice> {
        let mut seen = Vec::new();
        wait_for(body, || {
            seen.extend(self.notices());
            let done = seen.iter().any(|n| n.body == body);
            async move { done.then_some(()) }
        })
        .await;
        seen
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn lifecycle(&self, session: &str) -> String {
        self.state.store.find_session(session).unwrap().unwrap().lifecycle
    }

    fn event_kinds(&self, session: &str) -> Vec<String> {
        let events: Vec<EventDto> = self.state.store.events(session, 0, 1000).unwrap();
        events.into_iter().map(|e| e.kind).collect()
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The test playing a host.
struct ScriptedHost {
    ws: Ws,
    seq: u64,
}

impl ScriptedHost {
    /// Connect and complete `hello` / `hello_ack`, without `resend_complete`.
    async fn hello(collector: &Collector, attached: Vec<AttachedSession>, seq: u64) -> Self {
        Self::hello_with(
            collector,
            attached,
            seq,
            Capabilities(vec![Capability::Park, Capability::ResolvePath]),
        )
        .await
    }

    /// `hello` announcing `capabilities`.
    async fn hello_with(
        collector: &Collector,
        attached: Vec<AttachedSession>,
        seq: u64,
        capabilities: Capabilities,
    ) -> Self {
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws, seq };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities,
            workspace_roots: vec![],
            attached_sessions: attached,
        })
        .await;
        let ack = host.next().await;
        assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
        host
    }

    /// Connect and send `hello` (no sessions attached): the collector's
    /// answer, whatever it is.
    async fn hello_reply(collector: &Collector) -> CollectorFrame {
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws, seq: 0 };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities: Capabilities::default(),
            workspace_roots: vec![],
            attached_sessions: vec![],
        })
        .await;
        host.next().await
    }

    /// Wait until the collector has closed this connection.
    async fn closed(mut self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(Ok(msg)) = self.ws.next().await {
                if matches!(msg, Message::Close(_)) {
                    break;
                }
            }
        })
        .await
        .expect("the collector closes the connection");
    }

    /// `hello` then `resend_complete`, and wait until the collector lists
    /// the host as connected (reconciled).
    async fn connect(collector: &Collector, attached: Vec<AttachedSession>, seq: u64) -> Self {
        Self::connect_with(
            collector,
            attached,
            seq,
            Capabilities(vec![Capability::Park, Capability::ResolvePath]),
        )
        .await
    }

    /// `connect` announcing `capabilities`.
    async fn connect_with(
        collector: &Collector,
        attached: Vec<AttachedSession>,
        seq: u64,
        capabilities: Capabilities,
    ) -> Self {
        let mut host = Self::hello_with(collector, attached, seq, capabilities).await;
        host.send(&HostFrame::ResendComplete).await;
        wait_for("host ready", || async {
            collector
                .state
                .hub
                .connected_hosts()
                .contains(&HOST.to_string())
                .then_some(())
        })
        .await;
        host
    }

    async fn send(&mut self, frame: &HostFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

    /// Emit the next sequenced session frame.
    async fn emit(&mut self, session_id: &str, body: SessionBody) {
        self.seq += 1;
        let frame = HostFrame::Session {
            session_id: session_id.into(),
            seq: self.seq,
            body,
        };
        self.send(&frame).await;
    }

    /// The next collector frame that is not an `ack`, or a `resolve_path`:
    /// this host resolves every path to itself (plan 5c), as a host with no
    /// symlinks would.
    async fn next(&mut self) -> CollectorFrame {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str(&text).unwrap() {
                        CollectorFrame::Ack { .. } => {}
                        CollectorFrame::ResolvePath { request_id, path } => {
                            let answer = HostFrame::ResolvedPath {
                                request_id,
                                canonical: path,
                                exists: true,
                                is_dir: true,
                            };
                            self.ws
                                .send(Message::text(serde_json::to_string(&answer).unwrap()))
                                .await
                                .unwrap();
                        }
                        frame => return frame,
                    },
                    Some(Ok(_)) => {}
                    other => panic!("collector connection ended: {other:?}"),
                }
            }
        })
        .await
        .expect("a collector frame within 10s")
    }

    /// Drop the connection and wait until the collector has noticed.
    async fn drop_connection(self, collector: &Collector) {
        drop(self.ws);
        wait_for("host gone", || async {
            (!collector.state.hub.connected_hosts().contains(&HOST.to_string())).then_some(())
        })
        .await;
    }
}

async fn wait_for<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = probe().await {
            return v;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn client(collector: &Collector) -> reqwest::Client {
    hennery_testkit::operator_client(&collector.state.operator)
}

fn attached(session_id: &str, last_seq: u64) -> AttachedSession {
    AttachedSession {
        session_id: session_id.into(),
        last_seq,
        open_turn_id: None,
    }
}

async fn post(c: &reqwest::Client, url: String, body: Value) -> (u16, Value) {
    // Bounded, so a request the collector wrongly sends to a silent host
    // fails the test instead of hanging it for the 90 s start timeout.
    let resp = c
        .post(url)
        .json(&body)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// Start a session through the API, answering the host side by hand.
async fn started_session(collector: &Collector, host: &mut ScriptedHost) -> String {
    let c = client(collector);
    let url = collector.url("/api/sessions");
    let call =
        tokio::spawn(async move { post(&c, url, json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" })).await });
    let CollectorFrame::StartSession {
        request_id, session_id, ..
    } = host.next().await
    else {
        panic!("expected start_session");
    };
    host.emit(&session_id, SessionBody::session_started(request_id, "agent-1"))
        .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 202, "{body}");
    session_id
}

fn prompt_body() -> Value {
    json!({ "content": [{ "type": "text", "text": "hi" }] })
}

#[tokio::test]
async fn nothing_is_sent_to_a_host_before_its_reconciliation() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::hello(&collector, vec![], 0).await;
    // Connected, but its resend is not complete: not offered to clients...
    assert!(collector.state.hub.connected_hosts().is_empty());
    // ...and a start is refused rather than sent, so it cannot be mistaken
    // for a start lost on an earlier connection.
    let (status, body) = post(
        &client(&collector),
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));
    host.send(&HostFrame::ResendComplete).await;
    wait_for("host ready", || async {
        collector
            .state
            .hub
            .connected_hosts()
            .contains(&HOST.to_string())
            .then_some(())
    })
    .await;
    let session = started_session(&collector, &mut host).await;
    assert_eq!(collector.lifecycle(&session), "active");
}

#[tokio::test]
async fn a_start_lost_in_a_drop_fails_as_not_delivered_after_the_next_handshake() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let c = client(&collector);
    let url = collector.url("/api/sessions");
    let call =
        tokio::spawn(async move { post(&c, url, json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" })).await });
    assert!(matches!(host.next().await, CollectorFrame::StartSession { .. }));
    host.drop_connection(&collector).await;
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 503);
    let session = body["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        collector.lifecycle(&session),
        "starting",
        "not failed before reconciliation"
    );

    // The host comes back without the session: the start never happened.
    let _host = ScriptedHost::connect(&collector, vec![], 0).await;
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle.as_str(), row.failure_reason.as_deref()),
        ("failed", Some("start_not_delivered"))
    );
}

#[tokio::test]
async fn a_prompt_lost_in_a_drop_is_released_and_the_next_prompt_runs() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));

    let c = client(&collector);
    let url = prompt_url.clone();
    let call = tokio::spawn(async move { post(&c, url, prompt_body()).await });
    assert!(matches!(host.next().await, CollectorFrame::Prompt { .. }));
    let seq = host.seq;
    host.drop_connection(&collector).await; // the prompt never reached the adapter
    assert_eq!(call.await.unwrap().0, 503);
    // Still wedged until the host is back: one turn at a time.
    assert_eq!(
        post(&client(&collector), prompt_url.clone(), prompt_body()).await.0,
        409
    );

    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    assert!(
        collector
            .event_kinds(&session)
            .contains(&"turn_not_delivered".to_string())
    );
    let c = client(&collector);
    let url = prompt_url.clone();
    let call = tokio::spawn(async move { post(&c, url, prompt_body()).await });
    let CollectorFrame::Prompt {
        request_id, turn_id, ..
    } = host.next().await
    else {
        panic!("expected the next prompt");
    };
    host.emit(&session, SessionBody::TurnStarted { request_id, turn_id })
        .await;
    assert_eq!(call.await.unwrap().0, 202);
}

#[tokio::test]
async fn a_restarted_host_parks_its_sessions_and_interrupts_the_open_turn_only_after_resend() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let call = tokio::spawn(async move { post(&c, url, prompt_body()).await });
    let CollectorFrame::Prompt {
        request_id, turn_id, ..
    } = host.next().await
    else {
        panic!("expected a prompt");
    };
    host.emit(&session, SessionBody::TurnStarted { request_id, turn_id })
        .await;
    assert_eq!(call.await.unwrap().0, 202);
    host.drop_connection(&collector).await;

    // The restarted host lists nothing. Before its resend completes the
    // collector must not act: the turn's end may still be in the outbox.
    let mut host = ScriptedHost::hello(&collector, vec![], 0).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(collector.lifecycle(&session), "active");
    host.send(&HostFrame::ResendComplete).await;
    wait_for("parked", || async {
        (collector.lifecycle(&session) == "parked").then_some(())
    })
    .await;
    let kinds = collector.event_kinds(&session);
    assert!(
        kinds.ends_with(&["host_restarted".to_string(), "turn_ended_synthesized".to_string()]),
        "{kinds:?}"
    );
    let (status, body) = post(
        &client(&collector),
        collector.url(&format!("/api/sessions/{session}/prompt")),
        prompt_body(),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
}

#[tokio::test]
async fn a_reconcile_close_rejected_not_attached_still_closes_it_collector_side() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;

    // Close it through the API while the host is connected, then drop the
    // connection before it answers: the close stays `close_requested` and is
    // re-sent by the reconciliation loop itself (`ws.rs`, outside
    // `Hub::request_for_session` — no waiter is registered for it).
    let c = client(&collector);
    let url = collector.url(&format!("/api/sessions/{session}/close"));
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    assert!(matches!(host.next().await, CollectorFrame::CloseSession { .. }));
    let seq = host.seq;
    host.drop_connection(&collector).await;
    assert_eq!(call.await.unwrap().0, 503);

    // The host reconnects still reporting the session attached.
    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    let CollectorFrame::CloseSession { request_id, .. } = host.next().await else {
        panic!("expected the reconcile-driven close_session");
    };
    // The host answers `not_attached`: decision 7 says this still closes the
    // session collector-side, even though nothing is waiting on this
    // request_id.
    host.send(&HostFrame::Error {
        request_id,
        code: "not_attached".into(),
        message: "no such session".into(),
    })
    .await;

    wait_for("closed", || async {
        (collector.lifecycle(&session) == "closed").then_some(())
    })
    .await;
    assert_eq!(
        collector
            .event_kinds(&session)
            .iter()
            .filter(|k| *k == "operator_closed")
            .count(),
        1,
        "the rejection must not record a second operator_closed"
    );
}

/// A reconcile close answered `not_attached` only after the operator has
/// resumed the session (final review F1): the old actor was already tearing
/// down, its park closed the session, the resume moved it to `starting`, and
/// the late rejection must not close that fresh start.
#[tokio::test]
async fn a_reconcile_close_rejected_after_a_resume_leaves_the_resume_alone() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = collector.url(&format!("/api/sessions/{session}/close"));
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    assert!(matches!(host.next().await, CollectorFrame::CloseSession { .. }));
    let seq = host.seq;
    host.drop_connection(&collector).await;
    assert_eq!(call.await.unwrap().0, 503);

    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    let CollectorFrame::CloseSession {
        request_id: close_request,
        ..
    } = host.next().await
    else {
        panic!("expected the reconcile-driven close_session");
    };
    // The actor was already being reaped: its park lands first and, since a
    // close was requested, closes the session.
    host.emit(
        &session,
        SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Idle,
        },
    )
    .await;
    wait_for("closed", || async {
        (collector.lifecycle(&session) == "closed").then_some(())
    })
    .await;

    let c = client(&collector);
    let url = resume_url(&collector, &session);
    let resume = tokio::spawn(async move { post(&c, url, json!({})).await });
    let resume_request = expect_resume(&mut host, &session).await;
    // The drained close is answered only now, then the fresh adapter starts.
    host.send(&HostFrame::Error {
        request_id: close_request,
        code: "not_attached".into(),
        message: "no such session".into(),
    })
    .await;
    host.emit(&session, SessionBody::session_started(resume_request, "agent-1"))
        .await;

    let (status, body) = resume.await.unwrap();
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("active")), "{body}");
    assert_eq!(collector.lifecycle(&session), "active");
    let kinds = collector.event_kinds(&session);
    assert_eq!(kinds.iter().filter(|k| *k == "operator_closed").count(), 1, "{kinds:?}");
    assert!(
        kinds.ends_with(&["operator_resumed".to_string(), "session_started".to_string()]),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn a_request_that_times_out_on_a_live_connection_drops_it() {
    let collector = Collector::start().await;
    let _host = ScriptedHost::connect(&collector, vec![], 0).await;
    let request = CollectorFrame::StartSession {
        request_id: "r-timeout".into(),
        session_id: "s-timeout".into(),
        committed_seq: 0,
        agent: "fake".into(),
        cwd: "/tmp".into(),
        config: Default::default(),
    };
    let outcome = collector
        .state
        .hub
        .request(HOST, "r-timeout", request, Duration::from_millis(100))
        .await;
    assert_eq!(outcome, Err(hennery_sessions::hub::RequestError::DeliveryUnknown));
    wait_for("connection dropped", || async {
        (!collector.state.hub.connected_hosts().contains(&HOST.to_string())).then_some(())
    })
    .await;
}

// Plan B: host offline past the threshold (ACP core §5.3, §12 scenario 8).

fn presumed(collector: &Collector, session: &str) -> bool {
    collector
        .state
        .store
        .find_session(session)
        .unwrap()
        .unwrap()
        .presumed_parked
}

async fn presumed_parked(collector: &Collector, session: &str) {
    wait_for("presumed parked", || async {
        (collector.lifecycle(session) == "parked" && presumed(collector, session)).then_some(())
    })
    .await;
}

/// A prompt the host has started; returns its turn id.
async fn started_turn(collector: &Collector, host: &mut ScriptedHost, session: &str) -> String {
    let c = client(collector);
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let call = tokio::spawn(async move { post(&c, url, prompt_body()).await });
    let CollectorFrame::Prompt {
        request_id, turn_id, ..
    } = host.next().await
    else {
        panic!("expected a prompt");
    };
    host.emit(
        session,
        SessionBody::TurnStarted {
            request_id,
            turn_id: turn_id.clone(),
        },
    )
    .await;
    assert_eq!(call.await.unwrap().0, 202);
    turn_id
}

#[tokio::test]
async fn a_host_offline_past_the_threshold_is_presumed_parked_and_reattached_on_reconnect() {
    let collector = Collector::start_in(tempfile::tempdir().unwrap(), Duration::from_millis(500)).await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    assert_eq!(collector.lifecycle(&session), "active", "presumed before the threshold");

    presumed_parked(&collector, &session).await;
    assert!(
        collector
            .event_kinds(&session)
            .ends_with(&["presumed_parked".to_string()])
    );
    let (status, body) = post(
        &client(&collector),
        collector.url(&format!("/api/sessions/{session}/prompt")),
        prompt_body(),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));

    // The host comes back; its adapter is still running the turn.
    let listed = AttachedSession {
        open_turn_id: Some(turn.clone()),
        ..attached(&session, seq)
    };
    let mut host = ScriptedHost::connect(&collector, vec![listed], seq).await;
    assert_eq!(collector.lifecycle(&session), "active");
    assert!(!presumed(&collector, &session));
    let kinds = collector.event_kinds(&session);
    assert!(
        kinds.ends_with(&["presumed_parked".to_string(), "reattached".to_string()]),
        "{kinds:?}"
    );
    assert_eq!(
        collector.state.store.turn_state(&turn).unwrap().as_deref(),
        Some("started")
    );
    host.emit(
        &session,
        SessionBody::TurnEnded {
            turn_id: turn.clone(),
            outcome: hennery_proto::frames::TurnOutcome::Completed,
            stop_reason: None,
            error: None,
        },
    )
    .await;
    wait_for("turn ended", || async {
        (collector.state.store.turn_state(&turn).unwrap().as_deref() == Some("ended")).then_some(())
    })
    .await;
}

/// The offline clock restarts with every connection: a host that dropped,
/// came back and dropped again is presumed offline only a full threshold
/// after the second drop.
#[tokio::test]
async fn an_older_drop_never_presumes_a_host_that_came_back() {
    let collector = Collector::start_in(tempfile::tempdir().unwrap(), Duration::from_millis(1000)).await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let seq = host.seq;
    let t0 = tokio::time::Instant::now();
    host.drop_connection(&collector).await; // its timer fires at ~t0 + 1000 ms
    let host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    tokio::time::sleep_until(t0 + Duration::from_millis(500)).await;
    host.drop_connection(&collector).await; // its timer fires at ~t0 + 1500 ms
    tokio::time::sleep_until(t0 + Duration::from_millis(1250)).await;
    assert_eq!(
        collector.lifecycle(&session),
        "active",
        "presumed by the first drop's timer"
    );
    presumed_parked(&collector, &session).await;
}

/// ACP core §5.4: after a collector restart, a host that never connects
/// again gets the same threshold, counted from the restart.
#[tokio::test]
async fn a_host_that_never_returns_after_a_collector_restart_is_presumed_offline() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(&dir.path().join("hennery.db")).unwrap();
        store.create_session("s1", HOST, "fake", "/tmp", "hat-1", None).unwrap();
        store
            .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
            .unwrap();
    }
    let collector = Collector::start_in(dir, Duration::from_millis(300)).await;
    assert_eq!(collector.lifecycle("s1"), "active");
    presumed_parked(&collector, "s1").await;
}

#[tokio::test]
async fn a_host_that_returns_after_a_collector_restart_is_not_presumed_offline() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(&dir.path().join("hennery.db")).unwrap();
        store.create_session("s1", HOST, "fake", "/tmp", "hat-1", None).unwrap();
        store
            .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
            .unwrap();
    }
    let collector = Collector::start_in(dir, Duration::from_millis(300)).await;
    let _host = ScriptedHost::connect(&collector, vec![attached("s1", 1)], 1).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(collector.lifecycle("s1"), "active");
    assert!(!presumed(&collector, "s1"));
}

// Plan B: the resume and detail endpoints, against a scripted host.

async fn get(c: &reqwest::Client, url: String) -> (u16, Value) {
    let resp = c.get(url).timeout(Duration::from_secs(15)).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// A started session its host has just parked (idle reap).
async fn parked_session(collector: &Collector, host: &mut ScriptedHost) -> String {
    let session = started_session(collector, host).await;
    host.emit(
        &session,
        SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Idle,
        },
    )
    .await;
    wait_for("parked", || async {
        (collector.lifecycle(&session) == "parked").then_some(())
    })
    .await;
    session
}

fn resume_url(collector: &Collector, session: &str) -> String {
    collector.url(&format!("/api/sessions/{session}/resume"))
}

/// The next frame must be a `resume_session` carrying everything the host
/// needs, including the seq of the last frame it sent (all of them are
/// committed by now); returns its request id.
async fn expect_resume(host: &mut ScriptedHost, session: &str) -> String {
    let committed_seq = host.seq;
    match host.next().await {
        CollectorFrame::ResumeSession {
            request_id,
            session_id,
            committed_seq: seq,
            agent,
            cwd,
            agent_session_id,
            ..
        } => {
            assert_eq!(
                (
                    session_id.as_str(),
                    seq,
                    agent.as_str(),
                    cwd.as_str(),
                    agent_session_id.as_str()
                ),
                (session, committed_seq, "fake", "/tmp", "agent-1")
            );
            request_id
        }
        other => panic!("expected resume_session, got {other:?}"),
    }
}

#[tokio::test]
async fn a_resume_attaches_a_parked_session_again() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = resume_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
    assert_eq!(collector.lifecycle(&session), "starting");
    host.emit(&session, SessionBody::session_started(request_id, "agent-1"))
        .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("active")), "{body}");
    let kinds = collector.event_kinds(&session);
    assert!(
        kinds.ends_with(&["operator_resumed".to_string(), "session_started".to_string()]),
        "{kinds:?}"
    );
}

/// ACP core §12 scenario 11: of two concurrent resumes, one attaches and
/// the other gets 409; the host is asked only once.
#[tokio::test]
async fn two_concurrent_resumes_attach_once() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = resume_url(&collector, &session);
    let first = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
    let (status, body) = post(&client(&collector), resume_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("starting")), "{body}");
    host.emit(&session, SessionBody::session_started(request_id, "agent-1"))
        .await;
    assert_eq!(first.await.unwrap().0, 202);
    let more = tokio::time::timeout(Duration::from_millis(300), host.next()).await;
    assert!(more.is_err(), "a second request reached the host: {more:?}");
}

/// ACP core §12 scenario 3, collector side: the host's `start_failed`
/// reason becomes the session's failure reason and the HTTP error code.
#[tokio::test]
async fn a_resume_the_agent_cannot_load_fails_with_its_reason_and_can_be_retried() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = resume_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
    host.emit(
        &session,
        SessionBody::StartFailed {
            request_id,
            code: "agent_has_no_record".into(),
            message: "Resource not found".into(),
        },
    )
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (502, Some("agent_has_no_record")));
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle.as_str(), row.failure_reason.as_deref()),
        ("failed", Some("agent_has_no_record"))
    );
    // Failed is resumable: the operator may try again.
    let c = client(&collector);
    let url = resume_url(&collector, &session);
    let _retry = tokio::spawn(async move { post(&c, url, json!({})).await });
    expect_resume(&mut host, &session).await;
}

/// Decision 3: a resume the host rejects is answered like one it fails —
/// 502 with the host's code, the session `failed` with it — whatever the
/// code, not the 409/400 the shared request mapping gives other endpoints
/// (final review F2).
#[tokio::test]
async fn a_resume_the_host_rejects_is_a_502_with_its_code() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    for code in ["not_attached", "invalid"] {
        let c = client(&collector);
        let url = resume_url(&collector, &session);
        let call = tokio::spawn(async move { post(&c, url, json!({})).await });
        let request_id = expect_resume(&mut host, &session).await;
        host.send(&HostFrame::Error {
            request_id,
            code: code.into(),
            message: "rejected".into(),
        })
        .await;
        let (status, body) = call.await.unwrap();
        assert_eq!((status, body["code"].as_str()), (502, Some(code)), "{body}");
        let row = collector.state.store.find_session(&session).unwrap().unwrap();
        assert_eq!(
            (row.lifecycle.as_str(), row.failure_reason.as_deref()),
            ("failed", Some(code))
        );
    }
}

#[tokio::test]
async fn a_resume_is_refused_while_active_offline_or_without_agent_history() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let active = started_session(&collector, &mut host).await;
    let (status, body) = post(&client(&collector), resume_url(&collector, &active), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("active")));

    // A start the host failed: the agent never created a session.
    let c = client(&collector);
    let url = collector.url("/api/sessions");
    let call =
        tokio::spawn(async move { post(&c, url, json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" })).await });
    let CollectorFrame::StartSession {
        request_id, session_id, ..
    } = host.next().await
    else {
        panic!("expected start_session");
    };
    host.emit(
        &session_id,
        SessionBody::StartFailed {
            request_id,
            code: "agent_not_logged_in".into(),
            message: "log in".into(),
        },
    )
    .await;
    assert_eq!(call.await.unwrap().0, 502);
    let (status, body) = post(&client(&collector), resume_url(&collector, &session_id), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("agent_has_no_record")));

    let parked = parked_session(&collector, &mut host).await;
    host.drop_connection(&collector).await;
    let (status, body) = post(&client(&collector), resume_url(&collector, &parked), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));
    assert_eq!(
        collector.lifecycle(&parked),
        "parked",
        "an offline resume changed the session"
    );
    let (status, _) = post(
        &client(&collector),
        resume_url(&collector, "no-such-session"),
        json!({}),
    )
    .await;
    assert_eq!(status, 404);
}

/// ACP core §5.1: a resume must not reach a host before its post-drop
/// reconciliation, or it could race the resend of a turn it is still
/// running against the presumed-parked session (Task 8 review).
#[tokio::test]
async fn a_resume_is_refused_while_the_host_is_connected_but_not_yet_reconciled() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let _host = ScriptedHost::hello(&collector, vec![attached(&session, seq)], seq).await;
    // Connected, but its resend is not complete: a resume must be refused,
    // never sent, so it cannot race the resend (ACP core §5.1).
    let (status, body) = post(&client(&collector), resume_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));
    assert_eq!(
        collector.lifecycle(&session),
        "parked",
        "a resume during reconciliation changed the session"
    );
}

#[tokio::test]
async fn a_resume_lost_in_a_drop_is_reconciled_like_a_start() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = resume_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    expect_resume(&mut host, &session).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (503, Some("delivery_unknown")));
    assert_eq!(collector.lifecycle(&session), "starting");
    // The host comes back without it: the resume never happened.
    let _host = ScriptedHost::connect(&collector, vec![], seq).await;
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle.as_str(), row.failure_reason.as_deref()),
        ("failed", Some("start_not_delivered"))
    );
}

#[tokio::test]
async fn the_session_detail_shows_the_open_turn() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let (status, body) = get(&client(&collector), collector.url(&format!("/api/sessions/{session}"))).await;
    assert_eq!(status, 200, "{body}");
    let item = collector.state.store.find_session_item(&session).unwrap().unwrap();
    // No rule: the host's default hat (umbrella §8.2).
    let hat = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
    assert_eq!(
        body,
        json!({
            "session_id": session, "host_id": HOST, "agent": "fake", "cwd": "/tmp", "hat_id": hat,
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "created_at": item.created_at, "last_event_at": item.last_event_at,
            "open_turn": { "turn_id": turn, "state": "started" }, "pending": []
        })
    );
    let (status, _) = get(&client(&collector), collector.url("/api/sessions/no-such-session")).await;
    assert_eq!(status, 404);
}

// Plan B2a: capabilities (ACP core §3.3).

/// A newer host may announce a capability this collector does not know: its
/// `hello` is still accepted, and the capabilities it shares are kept.
#[tokio::test]
async fn a_hello_with_an_unknown_capability_is_accepted_with_the_known_ones() {
    let collector = Collector::start().await;
    let (mut ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
        .await
        .unwrap();
    let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
    let hello = json!({
        "type": "hello", "protocol_version": PROTOCOL_VERSION, "host_version": "future",
        "host_id": HOST, "proof": host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
        "capabilities": ["teleport", "park"], "attached_sessions": []
    });
    ws.send(Message::text(hello.to_string())).await.unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("an answer to hello")
        .unwrap()
        .unwrap();
    let ack: CollectorFrame = serde_json::from_str(ack.to_text().unwrap()).unwrap();
    assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
    assert!(collector.state.hub.has_capability(HOST, Capability::Park));
    assert!(!collector.state.hub.has_capability(HOST, Capability::Images));
}

// Plan B2a: a host's rejection reaches the store even when the HTTP caller
// has given up (plan B1, "Execution status"). The client times out, hyper
// drops the handler, and only then does the host answer.

/// POST with a client that gives up after 300 ms; resolves once it has.
fn post_and_give_up(c: reqwest::Client, url: String, body: Value) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let sent = c.post(url).json(&body).timeout(Duration::from_millis(300)).send().await;
        assert!(sent.is_err(), "the collector answered before the host did: {sent:?}");
    })
}

/// Reject `request_id` once its caller is gone.
async fn reject_after_the_caller_left(
    host: &mut ScriptedHost,
    caller: tokio::task::JoinHandle<()>,
    request_id: String,
) {
    caller.await.unwrap();
    // Let the server notice the closed connection and drop the handler.
    tokio::time::sleep(Duration::from_millis(200)).await;
    host.send(&HostFrame::Error {
        request_id,
        code: "unknown_agent".into(),
        message: "rejected".into(),
    })
    .await;
}

fn failed_with(collector: &Collector, session: &str) -> Option<String> {
    let row = collector.state.store.find_session(session).unwrap().unwrap();
    (row.lifecycle == "failed").then_some(row.failure_reason).flatten()
}

#[tokio::test]
async fn a_start_rejected_after_its_caller_gave_up_still_fails_the_session() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let caller = post_and_give_up(
        client(&collector),
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" }),
    );
    let CollectorFrame::StartSession {
        request_id, session_id, ..
    } = host.next().await
    else {
        panic!("expected start_session");
    };
    reject_after_the_caller_left(&mut host, caller, request_id).await;
    let reason = wait_for("start failed", || async { failed_with(&collector, &session_id) }).await;
    assert_eq!(reason, "unknown_agent");
}

#[tokio::test]
async fn a_resume_rejected_after_its_caller_gave_up_still_fails_the_session() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let caller = post_and_give_up(client(&collector), resume_url(&collector, &session), json!({}));
    let request_id = expect_resume(&mut host, &session).await;
    reject_after_the_caller_left(&mut host, caller, request_id).await;
    let reason = wait_for("resume failed", || async { failed_with(&collector, &session) }).await;
    assert_eq!(reason, "unknown_agent");
}

#[tokio::test]
async fn a_prompt_rejected_after_its_caller_gave_up_still_frees_the_turn_slot() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let caller = post_and_give_up(
        client(&collector),
        collector.url(&format!("/api/sessions/{session}/prompt")),
        prompt_body(),
    );
    let CollectorFrame::Prompt { request_id, .. } = host.next().await else {
        panic!("expected a prompt");
    };
    reject_after_the_caller_left(&mut host, caller, request_id).await;
    wait_for("turn slot free", || async {
        let row = collector.state.store.find_session(&session).unwrap().unwrap();
        row.open_turn_id.is_none().then_some(())
    })
    .await;
    // The next prompt is not refused `turn_in_progress`.
    started_turn(&collector, &mut host, &session).await;
}

// Plan B2a: `POST /api/sessions/{id}/cancel` and the park gate.

fn cancel_url(collector: &Collector, session: &str) -> String {
    collector.url(&format!("/api/sessions/{session}/cancel"))
}

/// The next frame must be a `cancel_turn` for `turn`; returns its request id.
async fn expect_cancel(host: &mut ScriptedHost, session: &str, turn: &str) -> String {
    match host.next().await {
        CollectorFrame::CancelTurn {
            request_id,
            session_id,
            turn_id,
        } => {
            assert_eq!((session_id.as_str(), turn_id.as_str()), (session, turn));
            request_id
        }
        other => panic!("expected cancel_turn, got {other:?}"),
    }
}

fn turn_ended(turn: &str, outcome: hennery_proto::frames::TurnOutcome) -> SessionBody {
    SessionBody::TurnEnded {
        turn_id: turn.into(),
        outcome,
        stop_reason: None,
        error: None,
    }
}

#[tokio::test]
async fn a_cancel_ends_the_open_turn_and_answers_with_its_outcome() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client(&collector);
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    expect_cancel(&mut host, &session, &turn).await;
    host.emit(
        &session,
        turn_ended(&turn, hennery_proto::frames::TurnOutcome::Cancelled),
    )
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 202, "{body}");
    assert_eq!(body, json!({ "turn_id": turn, "outcome": "cancelled" }));
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!((row.open_turn_id, row.activity.as_deref()), (None, Some("idle")));
}

/// The turn finished just before the cancel reached the host: the host's
/// `turn_ended{completed}` is on the wire ahead of its `not_running`
/// rejection, and the cancel answers with that outcome.
#[tokio::test]
async fn a_cancel_that_loses_the_race_with_the_turns_end_answers_how_it_ended() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client(&collector);
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_cancel(&mut host, &session, &turn).await;
    host.emit(
        &session,
        turn_ended(&turn, hennery_proto::frames::TurnOutcome::Completed),
    )
    .await;
    host.send(&HostFrame::Error {
        request_id,
        code: "not_running".into(),
        message: "that turn is not running".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body),
        (202, json!({ "turn_id": turn, "outcome": "completed" }))
    );
}

#[tokio::test]
async fn a_cancel_is_refused_without_a_turn_the_host_runs() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let (status, body) = post(&client(&collector), cancel_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("no_open_turn")));
    let (status, _) = post(
        &client(&collector),
        cancel_url(&collector, "no-such-session"),
        json!({}),
    )
    .await;
    assert_eq!(status, 404);

    // The host has no such turn in flight.
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client(&collector);
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_cancel(&mut host, &session, &turn).await;
    host.send(&HostFrame::Error {
        request_id,
        code: "not_running".into(),
        message: "that turn is not running".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("not_running")));

    let parked = parked_session(&collector, &mut host).await;
    let (status, body) = post(&client(&collector), cancel_url(&collector, &parked), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
}

/// ACP core §3.3: `park_session` goes only to hosts with the `park`
/// capability (this drops plan A's decision 12).
#[tokio::test]
async fn park_goes_only_to_a_host_that_announced_it_can_park() {
    let collector = Collector::start().await;
    // It resolves paths, which a start needs (plan 5c), but cannot park.
    let mut host = ScriptedHost::connect_with(&collector, vec![], 0, Capabilities(vec![Capability::ResolvePath])).await;
    let session = started_session(&collector, &mut host).await;
    let park_url = collector.url(&format!("/api/sessions/{session}/park"));
    let (status, body) = post(&client(&collector), park_url.clone(), json!({})).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("park_unsupported")),
        "{body}"
    );
    assert_eq!(collector.lifecycle(&session), "active");
    let more = tokio::time::timeout(Duration::from_millis(300), host.next()).await;
    assert!(more.is_err(), "park reached a host that cannot park: {more:?}");

    // The same host, upgraded: now it is asked.
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    let c = client(&collector);
    let call = tokio::spawn(async move { post(&c, park_url, json!({})).await });
    assert!(matches!(host.next().await, CollectorFrame::ParkSession { .. }));
    host.emit(
        &session,
        SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Operator,
        },
    )
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("parked")), "{body}");
}

/// The turn's end was ingested after the handler read the open turn but
/// before its waiter existed, so nothing resolved the waiter: the host's
/// `not_running` is answered with the stored outcome, not 409. The end is
/// written straight into the store here to open exactly that window.
#[tokio::test]
async fn a_cancel_whose_turn_ended_before_it_was_sent_answers_the_stored_outcome() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client(&collector);
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_cancel(&mut host, &session, &turn).await;
    host.seq += 1;
    collector
        .state
        .store
        .ingest(
            &session,
            host.seq,
            &turn_ended(&turn, hennery_proto::frames::TurnOutcome::Completed),
        )
        .unwrap();
    host.send(&HostFrame::Error {
        request_id,
        code: "not_running".into(),
        message: "that turn is not running".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body),
        (202, json!({ "turn_id": turn, "outcome": "completed" }))
    );
}

// Plan B2b: the collector's side of model, axes and mode.

/// Catalogue extracts whose current mode is `mode`.
fn catalogue(mode: &str) -> hennery_proto::frames::Indexed {
    hennery_proto::frames::Indexed {
        config_options: Some(vec![json!({"id": "mode", "currentValue": mode})]),
        current_mode: Some(mode.into()),
        current_axes: Some(Default::default()),
        ..Default::default()
    }
}

/// The start request's config reaches the host; what the host then reports
/// as current is what a resume re-applies, not what was asked for.
#[tokio::test]
async fn a_resume_re_sends_the_config_its_host_last_reported() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let c = client(&collector);
    let url = collector.url("/api/sessions");
    let call = tokio::spawn(async move {
        post(
            &c,
            url,
            json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp", "mode": "plan", "axes": {"fast": true} }),
        )
        .await
    });
    let CollectorFrame::StartSession {
        request_id,
        session_id,
        config,
        ..
    } = host.next().await
    else {
        panic!("expected start_session");
    };
    assert_eq!(config.mode.as_deref(), Some("plan"));
    assert_eq!(
        config.axes.get("fast"),
        Some(&hennery_proto::frames::ConfigValue::Bool(true))
    );
    // The adapter clamped the mode to `default`.
    host.emit(
        &session_id,
        SessionBody::SessionStarted {
            request_id,
            agent_session_id: "agent-1".into(),
            indexed: catalogue("default"),
        },
    )
    .await;
    assert_eq!(call.await.unwrap().0, 202);
    host.emit(
        &session_id,
        SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Idle,
        },
    )
    .await;
    wait_for("parked", || async {
        (collector.lifecycle(&session_id) == "parked").then_some(())
    })
    .await;
    let c = client(&collector);
    let url = resume_url(&collector, &session_id);
    tokio::spawn(async move { post(&c, url, json!({})).await });
    match host.next().await {
        CollectorFrame::ResumeSession { config, .. } => {
            assert_eq!(config.mode.as_deref(), Some("default"));
            assert!(config.axes.is_empty(), "{config:?}");
        }
        other => panic!("expected resume_session, got {other:?}"),
    }
}

fn config_url(collector: &Collector, session: &str) -> String {
    collector.url(&format!("/api/sessions/{session}/config"))
}

/// The next frame must be a `set_config` for `session`; returns its request
/// id, config id and value.
async fn expect_set_config(host: &mut ScriptedHost, session: &str) -> (String, String, Value) {
    match host.next().await {
        CollectorFrame::SetConfig {
            request_id,
            session_id,
            config_id,
            value,
        } => {
            assert_eq!(session_id, session);
            (request_id, config_id, serde_json::to_value(value).unwrap())
        }
        other => panic!("expected set_config, got {other:?}"),
    }
}

#[tokio::test]
async fn set_config_answers_with_the_catalogue_the_host_read_back() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = config_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({ "config_id": "mode", "value": "plan" })).await });
    let (request_id, config_id, value) = expect_set_config(&mut host, &session).await;
    assert_eq!((config_id.as_str(), value), ("mode", json!("plan")));
    host.emit(
        &session,
        SessionBody::ConfigApplied {
            request_id,
            indexed: catalogue("plan"),
        },
    )
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["mode"].as_str()), (202, Some("plan")), "{body}");
    assert_eq!(body["config_options"], json!([{"id": "mode", "currentValue": "plan"}]));
    let (status, catalog) = get(
        &client(&collector),
        collector.url(&format!("/api/sessions/{session}/catalog")),
    )
    .await;
    assert_eq!((status, catalog), (200, body));
}

#[tokio::test]
async fn set_config_refusals_answer_with_their_codes() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    for (code, status) in [("unknown_option", 409), ("config_failed", 502), ("invalid", 400)] {
        let c = client(&collector);
        let url = config_url(&collector, &session);
        let call = tokio::spawn(async move { post(&c, url, json!({ "config_id": "model", "value": "huge" })).await });
        let (request_id, _, _) = expect_set_config(&mut host, &session).await;
        host.send(&HostFrame::Error {
            request_id,
            code: code.into(),
            message: "no".into(),
        })
        .await;
        let (got, body) = call.await.unwrap();
        assert_eq!((got, body["code"].as_str()), (status, Some(code)), "{body}");
    }
    // Not a string or a boolean: refused before anything is sent.
    let (status, body) = post(
        &client(&collector),
        config_url(&collector, &session),
        json!({ "config_id": "x", "value": 3 }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (422, Some("invalid_body")), "{body}");
    let (status, _) = post(
        &client(&collector),
        config_url(&collector, "nope"),
        json!({ "config_id": "x", "value": "y" }),
    )
    .await;
    assert_eq!(status, 404);
    host.emit(
        &session,
        SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Idle,
        },
    )
    .await;
    wait_for("parked", || async {
        (collector.lifecycle(&session) == "parked").then_some(())
    })
    .await;
    let (status, body) = post(
        &client(&collector),
        config_url(&collector, &session),
        json!({ "config_id": "x", "value": "y" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
    let more = tokio::time::timeout(Duration::from_millis(300), host.next()).await;
    assert!(more.is_err(), "a request reached the host: {more:?}");
}

/// Final review I2: a switch the host gave up on (502) may still land at
/// the adapter. The host then announces its late read-back as a
/// `config_applied` under the same request id: the store applies it, and
/// the request, already answered, is not answered again.
#[tokio::test]
async fn a_late_config_applied_for_a_failed_switch_still_updates_the_stored_catalogue() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = config_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({ "config_id": "mode", "value": "plan" })).await });
    let (request_id, _, _) = expect_set_config(&mut host, &session).await;
    host.send(&HostFrame::Error {
        request_id: request_id.clone(),
        code: "config_failed".into(),
        message: "no answer within 15s of the request".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (502, Some("config_failed")), "{body}");
    let url = collector.url(&format!("/api/sessions/{session}/catalog"));
    let (_, before) = get(&client(&collector), url.clone()).await;
    assert_ne!(before["mode"], "plan", "{before}");
    host.emit(
        &session,
        SessionBody::ConfigApplied {
            request_id,
            indexed: catalogue("plan"),
        },
    )
    .await;
    wait_for("the late read-back stored", || async {
        let (status, catalog) = get(&client(&collector), url.clone()).await;
        (status == 200 && catalog["mode"] == "plan").then_some(())
    })
    .await;
}

/// Read a session's SSE stream from its start until `pred` holds for the
/// text received so far.
async fn read_stream(collector: &Collector, session: &str, pred: impl Fn(&str) -> bool) -> String {
    use futures::StreamExt;
    let resp = client(collector)
        .get(collector.url(&format!("/api/stream/sessions/{session}")))
        .send()
        .await
        .unwrap();
    let mut body = resp.bytes_stream();
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !pred(&buf) {
        let chunk = tokio::time::timeout_at(deadline, body.next())
            .await
            .unwrap_or_else(|_| panic!("stream stalled: {buf}"))
            .unwrap()
            .unwrap();
        buf.push_str(&String::from_utf8_lossy(&chunk));
    }
    buf
}

#[tokio::test]
async fn every_catalogue_change_is_also_a_catalog_changed_message_on_the_session_stream() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    // The agent changes its own mode: a live update with the catalogue.
    host.emit(
        &session,
        SessionBody::AcpUpdate {
            indexed: catalogue("bypass"),
            payload: json!({"update": {"sessionUpdate": "config_option_update"}}),
        },
    )
    .await;
    // An update without one is only an event.
    host.emit(
        &session,
        SessionBody::AcpUpdate {
            indexed: Default::default(),
            payload: json!({"update": {"sessionUpdate": "agent_message_chunk"}}),
        },
    )
    .await;
    let stream = read_stream(&collector, &session, |s| s.matches("event: event").count() >= 3).await;
    let changed: Vec<&str> = stream
        .split("\n\n")
        .filter(|m| m.contains("event: catalog_changed"))
        .collect();
    assert_eq!(changed.len(), 1, "{stream}");
    assert!(changed[0].contains(r#""mode":"bypass""#), "{}", changed[0]);
    let id = |m: &str| m.lines().find(|l| l.starts_with("id: ")).map(str::to_string);
    let update = stream
        .split("\n\n")
        .find(|m| m.contains("config_option_update"))
        .unwrap();
    assert_eq!(id(changed[0]), id(update), "catalog_changed carries its event's id");
}

// Plan (2): answers through the API, the queue and the host (ACP core §4.6,
// §5, §9).

fn opened(pending_id: &str, turn_id: &str) -> SessionBody {
    use hennery_proto::frames::{Indexed, PendingExtract, PendingKind};
    SessionBody::PendingOpened {
        pending_id: pending_id.into(),
        indexed: Indexed {
            turn_id: Some(turn_id.into()),
            pending: Some(Box::new(PendingExtract {
                id: pending_id.into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into(), "reject".into()]),
                title: None,
            })),
            ..Indexed::default()
        },
        payload: json!({"toolCall": {"toolCallId": "call-1"}}),
    }
}

fn answer_url(collector: &Collector, session: &str, pending_id: &str) -> String {
    collector.url(&format!("/api/sessions/{session}/pending/{pending_id}/answer"))
}

/// A session with turn `t` running and question `p1` open in it.
async fn asking_session(collector: &Collector, host: &mut ScriptedHost) -> (String, String) {
    let session = started_session(collector, host).await;
    let turn = started_turn(collector, host, &session).await;
    host.emit(&session, opened("p1", &turn)).await;
    wait_for("the question", || async {
        (!collector.state.store.open_pending(&session).unwrap().is_empty()).then_some(())
    })
    .await;
    (session, turn)
}

/// The next frame must be `p1`'s answer; returns its request id.
async fn expect_answer(host: &mut ScriptedHost, session: &str, option: &str) -> String {
    match host.next().await {
        CollectorFrame::AnswerPermission {
            request_id,
            session_id,
            pending_id,
            option_id,
        } => {
            assert_eq!(
                (session_id.as_str(), pending_id.as_str(), option_id.as_str()),
                (session, "p1", option)
            );
            request_id
        }
        other => panic!("expected an answer, got {other:?}"),
    }
}

fn verdict_of(collector: &Collector) -> Option<bool> {
    collector.state.store.pending_item("p1").unwrap().unwrap().delivered
}

#[tokio::test]
async fn an_answer_is_queued_sent_to_the_host_and_its_verdict_recorded() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    let detail_url = collector.url(&format!("/api/sessions/{session}"));
    let (_, detail) = get(&client(&collector), detail_url.clone()).await;
    assert_eq!(detail["activity"], "blocked");
    assert_eq!(detail["pending"][0]["pending_id"], "p1");
    assert_eq!(detail["pending"][0]["option_ids"], json!(["allow", "reject"]));

    let url = answer_url(&collector, &session, "p1");
    let (status, body) = post(&client(&collector), url.clone(), json!({"option_id": "allow"})).await;
    assert_eq!(status, 202, "{body}");
    let request_id = expect_answer(&mut host, &session, "allow").await;
    assert_eq!(body, json!({"pending_id": "p1", "request_id": request_id}));
    // One answer per question, even before its verdict.
    let (status, body) = post(&client(&collector), url.clone(), json!({"option_id": "reject"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("already_answered")));

    host.emit(
        &session,
        SessionBody::AnswerResult {
            pending_id: "p1".into(),
            request_id,
            delivered: true,
        },
    )
    .await;
    host.emit(
        &session,
        SessionBody::PendingResolved {
            pending_id: "p1".into(),
            resolution: hennery_proto::frames::PendingResolution::Delivered,
            reason: None,
        },
    )
    .await;
    wait_for("delivered", || async {
        (verdict_of(&collector) == Some(true)).then_some(())
    })
    .await;
    let (_, detail) = get(&client(&collector), detail_url).await;
    assert_eq!(
        (detail["activity"].as_str(), &detail["pending"]),
        (Some("running"), &json!([]))
    );
    let (status, body) = post(&client(&collector), url, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
}

#[tokio::test]
async fn answers_are_checked_against_the_stored_request() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    let url = answer_url(&collector, &session, "p1");
    let (status, body) = post(&client(&collector), url.clone(), json!({"option_id": "maybe"})).await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    let (status, _) = post(&client(&collector), url.clone(), json!({"action": "accept"})).await;
    assert_eq!(status, 400, "an elicitation's answer to a permission request");
    // A fixed error, not serde's message quoting the body back.
    let (status, body) = post(&client(&collector), url, json!({"action": "whatever"})).await;
    assert_eq!((status, body["code"].as_str()), (422, Some("invalid_body")), "{body}");
    let (status, body) = post(
        &client(&collector),
        answer_url(&collector, &session, "no-such-question"),
        json!({"option_id": "allow"}),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (404, Some("not_found")));
    assert!(verdict_of(&collector).is_none(), "nothing was queued");
}

/// Scenario 9: an answer given while the host is away is delivered after
/// its next handshake, and again after the one after that for as long as
/// no verdict came (the host dedupes by pending id).
#[tokio::test]
async fn an_answer_given_while_the_host_is_offline_is_sent_after_its_next_handshake() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, turn) = asking_session(&collector, &mut host).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let (status, body) = post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    assert_eq!(status, 202, "{body}");

    let listed = || {
        vec![AttachedSession {
            session_id: session.clone(),
            last_seq: seq,
            open_turn_id: Some(turn.clone()),
        }]
    };
    let mut host = ScriptedHost::hello(&collector, listed(), seq).await;
    // Nothing before the reconciliation.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), host.next())
            .await
            .is_err(),
        "sent before resend_complete"
    );
    host.send(&HostFrame::ResendComplete).await;
    let first = expect_answer(&mut host, &session, "allow").await;
    host.drop_connection(&collector).await;
    let mut host = ScriptedHost::connect(&collector, listed(), seq).await;
    assert_eq!(expect_answer(&mut host, &session, "allow").await, first, "resent as is");
    assert!(verdict_of(&collector).is_none());
}

/// A host's refusal (no live actor for the session) is logged, not a
/// verdict: the answer waits for its question's resolution.
#[tokio::test]
async fn a_refused_answer_gets_its_verdict_from_its_questions_resolution() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    let request_id = expect_answer(&mut host, &session, "allow").await;
    host.send(&HostFrame::Error {
        request_id,
        code: "not_attached".into(),
        message: "session is not attached on this host".into(),
    })
    .await;
    // Frames are read in order: once this note is in, so is the refusal.
    host.emit(
        &session,
        SessionBody::HostNote {
            note: "marker".into(),
            text: String::new(),
        },
    )
    .await;
    wait_for("the marker", || async {
        collector
            .event_kinds(&session)
            .contains(&"host_note".to_string())
            .then_some(())
    })
    .await;
    assert_eq!(verdict_of(&collector), None, "a refusal is logged, not a verdict");
    host.emit(
        &session,
        SessionBody::PendingResolved {
            pending_id: "p1".into(),
            resolution: hennery_proto::frames::PendingResolution::Cancelled,
            reason: Some(hennery_proto::frames::PendingReason::AdapterLost),
        },
    )
    .await;
    wait_for("the verdict", || async {
        (verdict_of(&collector) == Some(false)).then_some(())
    })
    .await;
}

/// Scenario 7's questions: a restarted host holds none, so they are
/// cancelled after its resend, and an answer queued for one is never sent.
#[tokio::test]
async fn a_host_restart_cancels_the_open_questions_and_drops_their_queued_answers() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    host.drop_connection(&collector).await;
    post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    assert_eq!(collector.lifecycle(&session), "parked");
    let item = collector.state.store.pending_item("p1").unwrap().unwrap();
    assert_eq!(
        serde_json::to_value((item.state, item.reason, item.delivered)).unwrap(),
        json!(["cancelled", "host_restarted", false])
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), host.next())
            .await
            .is_err(),
        "an answer to a cancelled question was sent"
    );
}

#[tokio::test]
async fn every_step_of_a_question_is_a_pending_changed_message_on_the_session_stream() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    let request_id = expect_answer(&mut host, &session, "allow").await;
    host.emit(
        &session,
        SessionBody::AnswerResult {
            pending_id: "p1".into(),
            request_id,
            delivered: true,
        },
    )
    .await;
    let stream = read_stream(&collector, &session, |s| {
        s.matches("event: pending_changed").count() >= 3
    })
    .await;
    let messages: Vec<&str> = stream.split("\n\n").collect();
    let id = |m: &str| m.lines().find(|l| l.starts_with("id: ")).map(str::to_string);
    for kind in ["pending_opened", "answer_submitted", "answer_result"] {
        let at = messages
            .iter()
            .position(|m| m.contains("event: event") && m.contains(&format!(r#""kind":"{kind}""#)))
            .unwrap_or_else(|| panic!("no {kind} in {stream}"));
        let changed = messages[at + 1];
        assert!(changed.contains("event: pending_changed"), "{changed}");
        assert_eq!(id(changed), id(messages[at]), "pending_changed carries its event's id");
        assert!(changed.contains(r#""pending_id":"p1""#), "{changed}");
    }
}

// Plan 3a: host revoke (kernel spec §4.3).

async fn revoke(collector: &Collector, host_id: &str) -> (u16, Value) {
    let resp = client(collector)
        .delete(collector.url(&format!("/api/hosts/{host_id}")))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_revoke_closes_the_hosts_connection_and_parks_its_sessions_for_good() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    // An answer the host has, with no verdict yet.
    let (status, _) = post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    assert_eq!(status, 202);
    expect_answer(&mut host, &session, "allow").await;

    let (status, body) = revoke(&collector, HOST).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        (body["host_id"].as_str(), body["connected"].as_bool()),
        (Some(HOST), Some(false))
    );
    assert!(body["revoked_at"].is_string(), "{body}");
    host.closed().await;

    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle.as_str(), row.presumed_parked, row.open_turn_id),
        ("parked", true, None)
    );
    let kinds = collector.event_kinds(&session);
    assert!(
        kinds.ends_with(&[
            "presumed_parked".to_string(),
            "turn_ended_synthesized".to_string(),
            "pending_cancelled".to_string()
        ]),
        "{kinds:?}"
    );
    let item = collector.state.store.pending_item("p1").unwrap().unwrap();
    assert_eq!(
        serde_json::to_value((item.state, item.reason, item.delivered)).unwrap(),
        json!(["cancelled", "host_revoked", false])
    );
    // Refused from now on, and told why.
    let reply = ScriptedHost::hello_reply(&collector).await;
    assert!(
        matches!(&reply, CollectorFrame::HelloError { code, .. } if code == "revoked"),
        "{reply:?}"
    );
    // A repeated revoke changes nothing more; an unknown host is 404.
    let before = collector.event_kinds(&session).len();
    assert_eq!(revoke(&collector, HOST).await.0, 200);
    assert_eq!(collector.event_kinds(&session).len(), before);
    assert_eq!(revoke(&collector, "host-9").await.0, 404);
}

/// A revoke that interrupts a handshake: the connection is closed before
/// the session is parked, and a kicked connection reads nothing more, so
/// the `resend_complete` the host sends afterwards never reconciles the
/// session back to `active` (`reattached`).
#[tokio::test]
async fn a_revoke_during_a_handshake_is_not_undone_by_its_reconciliation() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    // Back, still running the session, its resend not complete yet.
    let mut host = ScriptedHost::hello(&collector, vec![attached(&session, seq)], seq).await;
    assert_eq!(revoke(&collector, HOST).await.0, 200);
    let _ = host
        .ws
        .send(Message::text(
            serde_json::to_string(&HostFrame::ResendComplete).unwrap(),
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!((row.lifecycle.as_str(), row.presumed_parked), ("parked", true));
    assert!(!collector.event_kinds(&session).contains(&"reattached".to_string()));
    assert!(collector.state.hub.connected_hosts().is_empty());
}

/// A revoke whose wait for the connection ran out has already parked the
/// sessions while the connection could still apply a frame. The socket
/// task parks them again once it is gone: here the registry is marked
/// revoked and the connection kicked, with no hook run by anyone else.
#[tokio::test]
async fn a_connection_that_ends_after_its_host_was_revoked_parks_its_sessions() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    collector.state.hosts.revoke(HOST, 1).unwrap();
    collector.state.hub.disconnect(HOST);
    host.closed().await;
    wait_for("the session parked", || async {
        let row = collector.state.store.find_session(&session).unwrap().unwrap();
        (row.lifecycle == "parked" && row.presumed_parked).then_some(())
    })
    .await;
    assert!(collector.event_kinds(&session).contains(&"presumed_parked".to_string()));
}

/// 3a's M2: the registry check behind that second park fails while the
/// connection ends (here: its table is briefly away). The socket task does
/// not give up on the first error: it retries, bounded, and the session is
/// parked once the registry answers again.
#[tokio::test]
async fn a_registry_error_when_a_revoked_hosts_connection_ends_is_retried() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    collector.state.hosts.revoke(HOST, 1).unwrap();
    let db = rusqlite::Connection::open(collector._dir.path().join("hennery.db")).unwrap();
    db.execute_batch("ALTER TABLE hosts RENAME TO hosts_away").unwrap();
    collector.state.hub.disconnect(HOST);
    host.closed().await;
    // Long past the socket task's first check, which runs right after the
    // close; without the retry the session would stay active for good.
    tokio::time::sleep(Duration::from_millis(300)).await;
    db.execute_batch("ALTER TABLE hosts_away RENAME TO hosts").unwrap();
    wait_for("the session parked", || async {
        let row = collector.state.store.find_session(&session).unwrap().unwrap();
        (row.lifecycle == "parked" && row.presumed_parked).then_some(())
    })
    .await;
    // Parked for the revoke, not presumed offline.
    let reason: String = db
        .query_row(
            "SELECT json_extract(body, '$.reason') FROM events
             WHERE session_id = ?1 AND kind = 'presumed_parked' ORDER BY rowid DESC LIMIT 1",
            [&session],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reason, "host_revoked");
}

// Fix round 1: F1 (defence in depth) and F2.

/// F1, defence in depth: a revoke whose wait for the connection timed out
/// already parked the session, but that connection is still live and about
/// to reconcile. Its `resend_complete` must never reattach what the revoke
/// parked, or mark the connection ready — `Store::revoke_host` converges the
/// session either way (pinned above), but the socket task must not hand a
/// revoked host's zombie connection a window to look reconciled in the
/// meantime.
#[tokio::test]
async fn a_revoked_hosts_resend_complete_does_not_reattach_what_the_revoke_already_parked() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    // Back, still holding the session; resend not sent yet.
    let mut host = ScriptedHost::hello(&collector, vec![attached(&session, seq)], seq).await;
    // A revoke whose wait for this very connection timed out: the registry
    // refuses it and the session is already parked for it, but this
    // connection was never actually kicked.
    collector.state.hosts.revoke(HOST, 1).unwrap();
    collector.state.store.revoke_host(HOST).unwrap();
    host.send(&HostFrame::ResendComplete).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!((row.lifecycle.as_str(), row.presumed_parked), ("parked", true));
    assert!(!collector.event_kinds(&session).contains(&"reattached".to_string()));
    assert!(collector.state.hub.connected_hosts().is_empty(), "never marked ready");
    host.closed().await;
}

/// F2: the common case is a host that is already offline (no live
/// connection at all) when the operator revokes it. `disconnect_and_wait`
/// finds nothing to kick and returns at once; nothing but the handler's own
/// `on_host_revoked` call parks the session — the socket task's exit-path
/// hook never runs, because there is no socket task left to run it.
#[tokio::test]
async fn a_revoke_of_an_already_offline_host_still_parks_its_sessions_and_cancels_its_question() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    let (status, _) = post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    assert_eq!(status, 202);
    expect_answer(&mut host, &session, "allow").await;
    host.drop_connection(&collector).await;
    assert!(collector.state.hub.connected_hosts().is_empty());

    let (status, body) = revoke(&collector, HOST).await;
    assert_eq!(status, 200, "{body}");

    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle.as_str(), row.presumed_parked, row.open_turn_id),
        ("parked", true, None)
    );
    let item = collector.state.store.pending_item("p1").unwrap().unwrap();
    assert_eq!(
        serde_json::to_value((item.state, item.reason, item.delivered)).unwrap(),
        json!(["cancelled", "host_revoked", false])
    );
}

// Plan 6b: commands join the catalogue (ACP core §7, §9).

fn commands_update(names: &[&str]) -> SessionBody {
    let list = names.iter().map(|n| json!({ "name": n, "description": n })).collect();
    SessionBody::AcpUpdate {
        indexed: hennery_proto::frames::Indexed {
            commands: Some(list),
            ..Default::default()
        },
        payload: json!({"update": {"sessionUpdate": "available_commands_update"}}),
    }
}

fn mode_update(mode: &str) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: catalogue(mode),
        payload: json!({"update": {"sessionUpdate": "config_option_update"}}),
    }
}

/// The `(id, data)` of every `catalog_changed` message in `stream`.
fn catalog_messages(stream: &str) -> Vec<(String, Value)> {
    stream
        .split("\n\n")
        .filter(|m| m.contains("event: catalog_changed"))
        .map(|m| {
            let field = |name: &str| m.lines().find_map(|l| l.strip_prefix(name)).unwrap().to_string();
            (field("id: "), serde_json::from_str(&field("data: ")).unwrap())
        })
        .collect()
}

/// The `id:` of the `event` message whose data contains `marker`.
fn event_id(stream: &str, marker: &str) -> String {
    let message = stream
        .split("\n\n")
        .find(|m| m.contains("event: event") && m.contains(marker))
        .unwrap();
    message
        .lines()
        .find_map(|l| l.strip_prefix("id: "))
        .unwrap()
        .to_string()
}

/// Decision 4: live, a commands update is a `catalog_changed` too, and each
/// one carries the whole catalogue as it stands: a later config change
/// does not wipe the commands, nor commands the config.
#[tokio::test]
async fn live_catalog_changed_carries_the_whole_catalogue_commands_included() {
    use futures::StreamExt;
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    host.emit(&session, mode_update("plan")).await;
    let resp = client(&collector)
        .get(collector.url(&format!("/api/stream/sessions/{session}")))
        .send()
        .await
        .unwrap();
    let mut body = resp.bytes_stream();
    let mut buf = String::new();
    let mut read_until = async |count: usize| {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while catalog_messages(&buf).len() < count {
            let chunk = tokio::time::timeout_at(deadline, body.next())
                .await
                .unwrap_or_else(|_| panic!("stream stalled: {buf}"))
                .unwrap()
                .unwrap();
            buf.push_str(&String::from_utf8_lossy(&chunk));
        }
        buf.clone()
    };
    // The replayed one, then one per live change.
    read_until(1).await;
    host.emit(&session, commands_update(&["review"])).await;
    let stream = read_until(2).await;
    let (id, data) = catalog_messages(&stream)[1].clone();
    assert_eq!(id, event_id(&stream, "available_commands_update"));
    assert_eq!(data["mode"], "plan", "{data}");
    assert_eq!(
        data["commands"],
        json!([{"name": "review", "description": "review"}]),
        "{data}"
    );
    host.emit(&session, mode_update("bypass")).await;
    let stream = read_until(3).await;
    let (_, data) = catalog_messages(&stream)[2].clone();
    assert_eq!(data["mode"], "bypass", "{data}");
    assert_eq!(
        data["commands"],
        json!([{"name": "review", "description": "review"}]),
        "{data}"
    );
    let catalog: Value = client(&collector)
        .get(collector.url(&format!("/api/sessions/{session}/catalog")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog, data);
}

/// The review's A6: a replay from `Last-Event-ID` sends the catalogue once,
/// after the last event in it that changed the catalogue and with that
/// event's id, however many did: never one whole catalogue per event
/// (P-23).
#[tokio::test]
async fn a_replay_sends_the_catalogue_once_after_the_last_event_that_changed_it() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    host.emit(&session, mode_update("plan")).await;
    host.emit(&session, commands_update(&["review"])).await;
    host.emit(&session, mode_update("bypass")).await;
    host.emit(
        &session,
        SessionBody::AcpUpdate {
            indexed: Default::default(),
            payload: json!({"update": {"sessionUpdate": "agent_message_chunk", "marker": "last"}}),
        },
    )
    .await;
    // `emit` does not wait for the collector: every event must be stored
    // before the stream opens, or some would arrive live, not replayed.
    let stored = wait_for("the last event stored", || async {
        let events = collector.state.store.events(&session, 0, 100).unwrap();
        events
            .iter()
            .any(|e| e.body["payload"]["update"]["marker"] == "last")
            .then_some(events)
    })
    .await;
    let stream = read_stream(&collector, &session, |s| s.contains(r#""marker":"last""#)).await;
    let changed = catalog_messages(&stream);
    assert_eq!(changed.len(), 1, "{stream}");
    let (id, data) = &changed[0];
    let bypass = stream
        .split("\n\n")
        .filter(|m| m.contains("event: event") && m.contains("config_option_update"))
        .last()
        .unwrap();
    assert_eq!(Some(id.as_str()), bypass.lines().find_map(|l| l.strip_prefix("id: ")));
    // Right after that event, before the next one.
    let at = |needle: &str| stream.find(needle).unwrap();
    assert!(at("event: catalog_changed") > at(bypass) && at("event: catalog_changed") < at(r#""marker":"last""#));
    assert_eq!(data["mode"], "bypass", "{data}");
    assert_eq!(
        data["commands"],
        json!([{"name": "review", "description": "review"}]),
        "{data}"
    );
    // From `Last-Event-ID`: after the commands, once, after `bypass`; after
    // `bypass`, not at all.
    let id_of = |update: &str, mode: Option<&str>| {
        stored
            .iter()
            .rfind(|e| {
                e.body["payload"]["update"]["sessionUpdate"] == update
                    && mode.is_none_or(|m| e.body["indexed"]["current_mode"] == m)
            })
            .unwrap()
            .event_id
    };
    let after = async |last: i64| {
        use futures::StreamExt;
        let resp = client(&collector)
            .get(collector.url(&format!("/api/stream/sessions/{session}")))
            .header("last-event-id", last.to_string())
            .send()
            .await
            .unwrap();
        let mut body = resp.bytes_stream();
        let mut buf = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !buf.contains(r#""marker":"last""#) {
            let chunk = tokio::time::timeout_at(deadline, body.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            buf.push_str(&String::from_utf8_lossy(&chunk));
        }
        buf
    };
    let bypass_id = id_of("config_option_update", Some("bypass"));
    let from_commands = after(id_of("available_commands_update", None)).await;
    let changed = catalog_messages(&from_commands);
    assert_eq!(changed.len(), 1, "{from_commands}");
    assert_eq!(changed[0].0, bypass_id.to_string());
    let from_bypass = after(bypass_id).await;
    assert!(catalog_messages(&from_bypass).is_empty(), "{from_bypass}");
}

// Plan 6b: the list item in the detail; what a start may name (the review's
// A1).

/// The detail is the list item (with the title and the current model and
/// mode), the open turn and the open questions.
#[tokio::test]
async fn the_session_detail_carries_the_title_and_the_current_model_and_mode() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    host.emit(&session, mode_update("plan")).await;
    host.emit(
        &session,
        SessionBody::AcpUpdate {
            indexed: hennery_proto::frames::Indexed {
                title: Some("Fix the login bug".into()),
                ..Default::default()
            },
            payload: json!({"update": {"sessionUpdate": "session_info_update"}}),
        },
    )
    .await;
    wait_for("the title", || async {
        collector
            .state
            .store
            .find_session(&session)
            .unwrap()
            .unwrap()
            .title
            .map(|_| ())
    })
    .await;
    let (status, detail) = get(&client(&collector), collector.url(&format!("/api/sessions/{session}"))).await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["title"], "Fix the login bug");
    assert_eq!(detail["mode"], "plan");
    assert_eq!(detail["lifecycle"], "active");
    assert_eq!(detail["pending"], json!([]));
    assert_eq!(detail["last_event_at"].as_str().unwrap().len(), 24);
}

/// A start names a paired host (an unknown one is refused before any
/// session exists), and an agent of at most 32 bytes as JSON writes them,
/// so every field of the list item is bounded.
#[tokio::test]
async fn a_start_names_a_paired_host_and_an_agent_of_at_most_32_bytes() {
    let collector = Collector::start().await;
    let c = client(&collector);
    let (status, body) = post(
        &c,
        collector.url("/api/sessions"),
        json!({ "host_id": "host-unknown", "agent": "fake", "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (400, Some("unknown_host")), "{body}");
    let (status, body) = post(
        &c,
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "a".repeat(33), "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    let (status, body) = post(
        &c,
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "\"".repeat(17), "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    let sessions: i64 = rusqlite::Connection::open(collector._dir.path().join("hennery.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sessions, 0);
    // A paired host that is offline: 409, and no session either, since its
    // cwd cannot be resolved there (plan 5c decision 2).
    let (status, body) = post(
        &c,
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "a".repeat(32), "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")), "{body}");
    let sessions: i64 = rusqlite::Connection::open(collector._dir.path().join("hennery.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sessions, 0);
}

// Plan 6b: `GET /api/sessions` (ACP core §9).

/// Pages, searches and filters, by hat too (plan 5c); every parameter it
/// cannot honour is refused, never ignored.
#[tokio::test]
async fn the_session_list_pages_searches_filters_and_refuses_what_it_cannot_honour() {
    let collector = Collector::start().await;
    let store = &collector.state.store;
    for (id, cwd, hat) in [
        ("a", "/src/alpha", "hat-1"),
        ("b", "/src/beta", "hat-2"),
        ("c", "/src/gamma", "hat-1"),
    ] {
        store.create_session(id, HOST, "fake", cwd, hat, None).unwrap();
        // One millisecond apart at least, so the order is known.
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    store.close_now("a").unwrap();
    let c = client(&collector);
    let list = async |query: &str| get(&c, collector.url(&format!("/api/sessions{query}"))).await;
    let ids = |page: &Value| -> Vec<String> {
        page["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["session_id"].as_str().unwrap().to_string())
            .collect()
    };

    let (status, page) = list("").await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(ids(&page), ["a", "c", "b"]);
    assert_eq!(page.get("next_cursor"), None);
    assert_eq!(page["sessions"][0]["lifecycle"], "closed");
    assert_eq!(page["sessions"][0]["hat_id"], "hat-1");

    let (_, first) = list("?limit=2").await;
    assert_eq!(ids(&first), ["a", "c"]);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    let (_, rest) = list(&format!("?limit=2&cursor={cursor}")).await;
    assert_eq!(ids(&rest), ["b"]);
    assert_eq!(rest.get("next_cursor"), None);
    // Clamped to at least one.
    assert_eq!(ids(&list("?limit=0").await.1), ["a"]);

    let (_, open) = list("?lifecycle=starting,active,parked,failed").await;
    assert_eq!(ids(&open), ["c", "b"]);
    // A search bypasses the lifecycle filter.
    let (_, found) = list("?lifecycle=starting&q=ALPHA").await;
    assert_eq!(ids(&found), ["a"]);
    let (_, blank) = list("?q=%20%20").await;
    assert_eq!(ids(&blank), ["a", "c", "b"]);

    // The hat filters, with the lifecycle's, with a search (which bypasses
    // every filter but the hat's: frontend §5), and across pages.
    assert_eq!(ids(&list("?hat=hat-1").await.1), ["a", "c"]);
    assert_eq!(ids(&list("?hat=hat-2").await.1), ["b"]);
    assert_eq!(ids(&list("?hat=hat-1&lifecycle=starting").await.1), ["c"]);
    assert_eq!(ids(&list("?hat=hat-1&lifecycle=starting&q=ALPHA").await.1), ["a"]);
    assert!(ids(&list("?hat=hat-2&q=alpha").await.1).is_empty());
    let (_, first) = list("?hat=hat-1&limit=1").await;
    assert_eq!(ids(&first), ["a"]);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    assert_eq!(
        ids(&list(&format!("?hat=hat-1&limit=1&cursor={cursor}")).await.1),
        ["c"]
    );
    // An unknown hat has no sessions; an empty one names none.
    assert!(ids(&list("?hat=work").await.1).is_empty());

    for (query, code) in [
        ("?cursor=zz", "invalid_cursor"),
        ("?limit=x", "invalid"),
        ("?limit=-1", "invalid"),
        ("?lifecycle=active,bogus", "invalid"),
        ("?hat=", "invalid"),
    ] {
        let (status, body) = list(query).await;
        assert_eq!((status, body["code"].as_str()), (400, Some(code)), "{query}: {body}");
    }
    let (status, body) = list(&format!("?q={}", "q".repeat(201))).await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    // A control character would cut the pattern short (a NUL ends it).
    let (status, body) = list("?q=a%00b").await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(list(&format!("?q={}", "q".repeat(200))).await.0, 200);
}

/// ACP core §5.1: an attached session the collector has closed, and then
/// re-assigned to another hat while its host was away, is closed on that
/// host when it comes back, so no adapter of the old hat runs on.
#[tokio::test]
async fn a_session_closed_and_reassigned_while_its_host_was_away_is_closed_on_its_return() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let (status, body) = post(
        &client(&collector),
        collector.url(&format!("/api/sessions/{session}/close")),
        json!({}),
    )
    .await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("closed")), "{body}");
    let hennery_kernel::hats::HatChange::Done(acme) = collector.state.hosts.create_hat("Acme", None, 1).unwrap() else {
        panic!("no hat");
    };
    let resp = client(&collector)
        .patch(collector.url(&format!("/api/sessions/{session}")))
        .json(&json!({ "hat_id": acme.id }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    let CollectorFrame::CloseSession { session_id, .. } = host.next().await else {
        panic!("expected the reconcile-driven close_session");
    };
    assert_eq!(session_id, session);
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!((row.lifecycle.as_str(), row.hat_id), ("closed", acme.id));
}

// Plan 10b: push triggers, from the host's facts only (ACP core §10).

/// A question blocks the turn: one urgent notice; a second question, none;
/// the turn's end: "finished".
#[tokio::test]
async fn a_blocked_turn_and_its_end_each_queue_one_notice() {
    use hennery_kernel::push::Urgency;
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, turn) = asking_session(&collector, &mut host).await;
    let notices = collector.notices_until("needs your answer").await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(
        (notices[0].urgency, notices[0].body.as_str(), notices[0].tag.as_str()),
        (Urgency::High, "needs your answer", session.as_str())
    );
    assert_eq!(notices[0].url, format!("/sessions/{session}"));
    host.emit(&session, opened("p2", &turn)).await;
    host.emit(
        &session,
        turn_ended(&turn, hennery_proto::frames::TurnOutcome::Completed),
    )
    .await;
    // One socket's frames are handled in order: by the end's notice, `p2`
    // was handled, and queued none.
    let notices = collector.notices_until("finished").await;
    assert_eq!(
        notices.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
        ["finished"]
    );
}

/// A host that restarted mid-turn: reconciliation ends the turn, and
/// nothing is pushed for it (P-25). Then a question in another session's
/// turn, on the same socket: its notice, queued after everything before
/// it, is the only one.
#[tokio::test]
async fn a_turn_ended_by_reconciliation_queues_no_notice() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    started_turn(&collector, &mut host, &session).await;
    host.drop_connection(&collector).await;
    let mut host = ScriptedHost::hello(&collector, vec![], 0).await;
    host.send(&HostFrame::ResendComplete).await;
    wait_for("parked", || async {
        (collector.lifecycle(&session) == "parked").then_some(())
    })
    .await;
    assert!(
        collector
            .event_kinds(&session)
            .contains(&"turn_ended_synthesized".to_string())
    );
    let (other, _) = asking_session(&collector, &mut host).await;
    let notices = collector.notices_until("needs your answer").await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].tag, other);
}

/// 10b-i's review, A1: facts the host resends after a reconnect notify
/// only once reconciliation is done, and only what still holds. A
/// question asked, withdrawn and then the turn finished, all in the
/// backlog: one "finished", no "needs your answer".
#[tokio::test]
async fn a_resent_backlog_notifies_only_what_still_holds() {
    use hennery_proto::frames::{PendingReason, PendingResolution, TurnOutcome};
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let mut host = ScriptedHost::hello(&collector, vec![attached(&session, seq)], seq).await;
    host.emit(&session, opened("p1", &turn)).await;
    host.emit(
        &session,
        SessionBody::PendingResolved {
            pending_id: "p1".into(),
            resolution: PendingResolution::Cancelled,
            reason: Some(PendingReason::AgentWithdrew),
        },
    )
    .await;
    host.emit(&session, turn_ended(&turn, TurnOutcome::Completed)).await;
    // A question outside a turn, asked and withdrawn in the backlog: it no
    // longer holds at the flush, and must not hide the "finished" (A6).
    host.emit(&session, outside("p2")).await;
    host.emit(
        &session,
        SessionBody::PendingResolved {
            pending_id: "p2".into(),
            resolution: PendingResolution::Cancelled,
            reason: Some(PendingReason::AgentWithdrew),
        },
    )
    .await;
    wait_for("the backlog", || async {
        let resolved = collector
            .event_kinds(&session)
            .iter()
            .filter(|k| *k == "pending_resolved")
            .count();
        (resolved == 2).then_some(())
    })
    .await;
    // Ingested, and nothing queued while the resend runs.
    assert!(collector.notices().is_empty());
    host.send(&HostFrame::ResendComplete).await;
    let notices = collector.notices_until("finished").await;
    assert_eq!(
        notices.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
        ["finished"]
    );
}

/// A1, the other way: a question in the backlog still open once the host
/// is reconciled does ask the owner, then.
#[tokio::test]
async fn a_resent_question_still_open_notifies_after_reconciliation() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let mut host = ScriptedHost::hello(
        &collector,
        vec![AttachedSession {
            session_id: session.clone(),
            last_seq: seq,
            open_turn_id: Some(turn.clone()),
        }],
        seq,
    )
    .await;
    host.emit(&session, opened("p1", &turn)).await;
    wait_for("the question", || async {
        (!collector.state.store.open_pending(&session).unwrap().is_empty()).then_some(())
    })
    .await;
    // Ingested, but not notified while the resend runs.
    assert!(collector.notices().is_empty());
    host.send(&HostFrame::ResendComplete).await;
    let notices = collector.notices_until("needs your answer").await;
    assert_eq!(notices.len(), 1, "{notices:?}");
}

/// 10b-i's review, A2: a question asked again after the agent withdrew one
/// in the same turn does not notify; it is the agent's pace, not the
/// owner's.
#[tokio::test]
async fn a_question_asked_again_after_a_withdrawal_does_not_notify() {
    use hennery_proto::frames::{PendingReason, PendingResolution};
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, turn) = asking_session(&collector, &mut host).await;
    collector.notices_until("needs your answer").await;
    host.emit(
        &session,
        SessionBody::PendingResolved {
            pending_id: "p1".into(),
            resolution: PendingResolution::Cancelled,
            reason: Some(PendingReason::AgentWithdrew),
        },
    )
    .await;
    host.emit(&session, opened("p2", &turn)).await;
    host.emit(
        &session,
        turn_ended(&turn, hennery_proto::frames::TurnOutcome::Completed),
    )
    .await;
    let notices = collector.notices_until("finished").await;
    assert_eq!(
        notices.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
        ["finished"]
    );
}

/// A question asked outside any turn.
fn outside(pending_id: &str) -> SessionBody {
    let SessionBody::PendingOpened {
        pending_id,
        mut indexed,
        payload,
    } = opened(pending_id, "unused")
    else {
        unreachable!("`opened` makes a question");
    };
    indexed.turn_id = None;
    SessionBody::PendingOpened {
        pending_id,
        indexed,
        payload,
    }
}

/// Operator decision 2026-10-02: a question asked while no turn runs asks
/// the owner like a blocked turn; a second one while it is open does not.
/// The queue keeps one notice per tag, so the second is checked by ending
/// on another session's notice: none of this session's may come after the
/// first (plan 10b-iii's review, A2).
#[tokio::test]
async fn a_question_outside_a_turn_queues_one_urgent_notice() {
    use hennery_kernel::push::Urgency;
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    host.emit(&session, outside("p1")).await;
    let notices = collector.notices_until("needs your answer").await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(
        (notices[0].urgency, notices[0].tag.as_str()),
        (Urgency::High, session.as_str())
    );
    host.emit(&session, outside("p2")).await;
    // A sentinel on the same socket: another session's turn, blocked.
    let other = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &other).await;
    host.emit(&other, opened("q1", &turn)).await;
    let notices = collector.notices_until("needs your answer").await;
    assert!(notices.iter().all(|n| n.tag == other), "{notices:?}");
}

/// The same reconnect rule as a blocked turn: a resent question outside a
/// turn notifies once reconciled, if it is still open.
#[tokio::test]
async fn a_resent_question_outside_a_turn_still_open_notifies_after_reconciliation() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let mut host = ScriptedHost::hello(&collector, vec![attached(&session, seq)], seq).await;
    host.emit(&session, outside("p1")).await;
    wait_for("the question", || async {
        (!collector.state.store.open_pending(&session).unwrap().is_empty()).then_some(())
    })
    .await;
    assert!(collector.notices().is_empty());
    host.send(&HostFrame::ResendComplete).await;
    let notices = collector.notices_until("needs your answer").await;
    assert_eq!(notices.len(), 1, "{notices:?}");
}
