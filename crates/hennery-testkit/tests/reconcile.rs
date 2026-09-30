//! The collector's reconciliation (ACP core §3.4, §5.1, §5.2) against a
//! scripted host over a real WebSocket: the test plays the host frame by
//! frame, so it controls exactly what was delivered before a drop.

use futures::{SinkExt, StreamExt};
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
use hennery_proto::rest::EventDto;
use hennery_sessions::{AppState, store::Store};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "dev-token-for-tests";
const HOST: &str = "host-1";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
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
        let mut state = AppState::new(
            Store::open(&dir.path().join("hennery.db")).unwrap(),
            Hosts::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
        state.offline_threshold = offline;
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn lifecycle(&self, session: &str) -> String {
        self.state.store.session(session).unwrap().unwrap().lifecycle
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
        Self::hello_with(collector, attached, seq, Capabilities(vec![Capability::Park])).await
    }

    /// `hello` announcing `capabilities`.
    async fn hello_with(
        collector: &Collector,
        attached: Vec<AttachedSession>,
        seq: u64,
        capabilities: Capabilities,
    ) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let mut host = Self { ws, seq };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            token: TOKEN.into(),
            capabilities,
            attached_sessions: attached,
        })
        .await;
        let ack = host.next().await;
        assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
        host
    }

    /// `hello` then `resend_complete`, and wait until the collector lists
    /// the host as connected (reconciled).
    async fn connect(collector: &Collector, attached: Vec<AttachedSession>, seq: u64) -> Self {
        Self::connect_with(collector, attached, seq, Capabilities(vec![Capability::Park])).await
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

    /// The next collector frame that is not an `ack`.
    async fn next(&mut self) -> CollectorFrame {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str(&text).unwrap() {
                        CollectorFrame::Ack { .. } => {}
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

fn client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
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
    let c = client();
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
        &client(),
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
    let c = client();
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
    let row = collector.state.store.session(&session).unwrap().unwrap();
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

    let c = client();
    let url = prompt_url.clone();
    let call = tokio::spawn(async move { post(&c, url, prompt_body()).await });
    assert!(matches!(host.next().await, CollectorFrame::Prompt { .. }));
    let seq = host.seq;
    host.drop_connection(&collector).await; // the prompt never reached the adapter
    assert_eq!(call.await.unwrap().0, 503);
    // Still wedged until the host is back: one turn at a time.
    assert_eq!(post(&client(), prompt_url.clone(), prompt_body()).await.0, 409);

    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    assert!(
        collector
            .event_kinds(&session)
            .contains(&"turn_not_delivered".to_string())
    );
    let c = client();
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
    let c = client();
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
        &client(),
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
    let c = client();
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
    let c = client();
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

    let c = client();
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
    collector.state.store.session(session).unwrap().unwrap().presumed_parked
}

async fn presumed_parked(collector: &Collector, session: &str) {
    wait_for("presumed parked", || async {
        (collector.lifecycle(session) == "parked" && presumed(collector, session)).then_some(())
    })
    .await;
}

/// A prompt the host has started; returns its turn id.
async fn started_turn(collector: &Collector, host: &mut ScriptedHost, session: &str) -> String {
    let c = client();
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
        &client(),
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
        store.create_session("s1", HOST, "fake", "/tmp").unwrap();
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
        store.create_session("s1", HOST, "fake", "/tmp").unwrap();
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
    let c = client();
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
    let c = client();
    let url = resume_url(&collector, &session);
    let first = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
    let (status, body) = post(&client(), resume_url(&collector, &session), json!({})).await;
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
    let c = client();
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
    let row = collector.state.store.session(&session).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle.as_str(), row.failure_reason.as_deref()),
        ("failed", Some("agent_has_no_record"))
    );
    // Failed is resumable: the operator may try again.
    let c = client();
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
        let c = client();
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
        let row = collector.state.store.session(&session).unwrap().unwrap();
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
    let (status, body) = post(&client(), resume_url(&collector, &active), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("active")));

    // A start the host failed: the agent never created a session.
    let c = client();
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
    let (status, body) = post(&client(), resume_url(&collector, &session_id), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("agent_has_no_record")));

    let parked = parked_session(&collector, &mut host).await;
    host.drop_connection(&collector).await;
    let (status, body) = post(&client(), resume_url(&collector, &parked), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));
    assert_eq!(
        collector.lifecycle(&parked),
        "parked",
        "an offline resume changed the session"
    );
    let (status, _) = post(&client(), resume_url(&collector, "no-such-session"), json!({})).await;
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
    let (status, body) = post(&client(), resume_url(&collector, &session), json!({})).await;
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
    let c = client();
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
    let row = collector.state.store.session(&session).unwrap().unwrap();
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
    let (status, body) = get(&client(), collector.url(&format!("/api/sessions/{session}"))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({
            "session_id": session, "host_id": HOST, "agent": "fake", "cwd": "/tmp",
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "open_turn": { "turn_id": turn, "state": "started" }, "pending": []
        })
    );
    let (status, _) = get(&client(), collector.url("/api/sessions/no-such-session")).await;
    assert_eq!(status, 404);
}

// Plan B2a: capabilities (ACP core §3.3).

/// A newer host may announce a capability this collector does not know: its
/// `hello` is still accepted, and the capabilities it shares are kept.
#[tokio::test]
async fn a_hello_with_an_unknown_capability_is_accepted_with_the_known_ones() {
    let collector = Collector::start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
        .await
        .unwrap();
    let hello = json!({
        "type": "hello", "protocol_version": PROTOCOL_VERSION, "host_version": "future",
        "host_id": HOST, "token": TOKEN, "capabilities": ["teleport", "park"], "attached_sessions": []
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
fn post_and_give_up(url: String, body: Value) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let sent = client()
            .post(url)
            .json(&body)
            .timeout(Duration::from_millis(300))
            .send()
            .await;
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
    let row = collector.state.store.session(session).unwrap().unwrap();
    (row.lifecycle == "failed").then_some(row.failure_reason).flatten()
}

#[tokio::test]
async fn a_start_rejected_after_its_caller_gave_up_still_fails_the_session() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let caller = post_and_give_up(
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
    let caller = post_and_give_up(resume_url(&collector, &session), json!({}));
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
    let caller = post_and_give_up(collector.url(&format!("/api/sessions/{session}/prompt")), prompt_body());
    let CollectorFrame::Prompt { request_id, .. } = host.next().await else {
        panic!("expected a prompt");
    };
    reject_after_the_caller_left(&mut host, caller, request_id).await;
    wait_for("turn slot free", || async {
        let row = collector.state.store.session(&session).unwrap().unwrap();
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
    let c = client();
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
    let row = collector.state.store.session(&session).unwrap().unwrap();
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
    let c = client();
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
    let (status, body) = post(&client(), cancel_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("no_open_turn")));
    let (status, _) = post(&client(), cancel_url(&collector, "no-such-session"), json!({})).await;
    assert_eq!(status, 404);

    // The host has no such turn in flight.
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client();
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
    let (status, body) = post(&client(), cancel_url(&collector, &parked), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
}

/// ACP core §3.3: `park_session` goes only to hosts with the `park`
/// capability (this drops plan A's decision 12).
#[tokio::test]
async fn park_goes_only_to_a_host_that_announced_it_can_park() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect_with(&collector, vec![], 0, Capabilities::default()).await;
    let session = started_session(&collector, &mut host).await;
    let park_url = collector.url(&format!("/api/sessions/{session}/park"));
    let (status, body) = post(&client(), park_url.clone(), json!({})).await;
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
    let c = client();
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
    let c = client();
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
    let c = client();
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
    let c = client();
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
    let c = client();
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
    let (status, catalog) = get(&client(), collector.url(&format!("/api/sessions/{session}/catalog"))).await;
    assert_eq!((status, catalog), (200, body));
}

#[tokio::test]
async fn set_config_refusals_answer_with_their_codes() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    for (code, status) in [("unknown_option", 409), ("config_failed", 502), ("invalid", 400)] {
        let c = client();
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
    let (status, _) = post(
        &client(),
        config_url(&collector, &session),
        json!({ "config_id": "x", "value": 3 }),
    )
    .await;
    assert_eq!(status, 422);
    let (status, _) = post(
        &client(),
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
        &client(),
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
    let c = client();
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
    let (_, before) = get(&client(), url.clone()).await;
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
        let (status, catalog) = get(&client(), url.clone()).await;
        (status == 200 && catalog["mode"] == "plan").then_some(())
    })
    .await;
}

/// Read a session's SSE stream from its start until `pred` holds for the
/// text received so far.
async fn read_stream(collector: &Collector, session: &str, pred: impl Fn(&str) -> bool) -> String {
    use futures::StreamExt;
    let resp = client()
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
            pending: Some(PendingExtract {
                id: pending_id.into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into(), "reject".into()]),
            }),
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
    let (_, detail) = get(&client(), detail_url.clone()).await;
    assert_eq!(detail["activity"], "blocked");
    assert_eq!(detail["pending"][0]["pending_id"], "p1");
    assert_eq!(detail["pending"][0]["option_ids"], json!(["allow", "reject"]));

    let url = answer_url(&collector, &session, "p1");
    let (status, body) = post(&client(), url.clone(), json!({"option_id": "allow"})).await;
    assert_eq!(status, 202, "{body}");
    let request_id = expect_answer(&mut host, &session, "allow").await;
    assert_eq!(body, json!({"pending_id": "p1", "request_id": request_id}));
    // One answer per question, even before its verdict.
    let (status, body) = post(&client(), url.clone(), json!({"option_id": "reject"})).await;
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
    let (_, detail) = get(&client(), detail_url).await;
    assert_eq!(
        (detail["activity"].as_str(), &detail["pending"]),
        (Some("running"), &json!([]))
    );
    let (status, body) = post(&client(), url, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
}

#[tokio::test]
async fn answers_are_checked_against_the_stored_request() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    let url = answer_url(&collector, &session, "p1");
    let (status, body) = post(&client(), url.clone(), json!({"option_id": "maybe"})).await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    let (status, _) = post(&client(), url.clone(), json!({"action": "accept"})).await;
    assert_eq!(status, 400, "an elicitation's answer to a permission request");
    let (status, _) = post(&client(), url, json!({"action": "whatever"})).await;
    assert_eq!(status, 422);
    let (status, body) = post(
        &client(),
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
        &client(),
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
        &client(),
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
        &client(),
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
        &client(),
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
