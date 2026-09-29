//! The collector's reconciliation (ACP core §3.4, §5.1, §5.2) against a
//! scripted host over a real WebSocket: the test plays the host frame by
//! frame, so it controls exactly what was delivered before a drop.

use futures::{SinkExt, StreamExt};
use hennery_kernel::auth::DevToken;
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
use hennery_proto::rest::EventDto;
use hennery_sessions::{AppState, store::Store};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "dev-token";
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
            DevToken::new(TOKEN),
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
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let mut host = Self { ws, seq };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            token: TOKEN.into(),
            capabilities: Capabilities(vec![Capability::Park]),
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
        let mut host = Self::hello(collector, attached, seq).await;
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
    host.emit(
        &session_id,
        SessionBody::SessionStarted {
            request_id,
            agent_session_id: "agent-1".into(),
        },
    )
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
    host.emit(
        &session,
        SessionBody::SessionStarted {
            request_id: resume_request,
            agent_session_id: "agent-1".into(),
        },
    )
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
            .ingest(
                "s1",
                1,
                &SessionBody::SessionStarted {
                    request_id: "r0".into(),
                    agent_session_id: "a1".into(),
                },
            )
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
            .ingest(
                "s1",
                1,
                &SessionBody::SessionStarted {
                    request_id: "r0".into(),
                    agent_session_id: "a1".into(),
                },
            )
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
    host.emit(
        &session,
        SessionBody::SessionStarted {
            request_id,
            agent_session_id: "agent-1".into(),
        },
    )
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
    host.emit(
        &session,
        SessionBody::SessionStarted {
            request_id,
            agent_session_id: "agent-1".into(),
        },
    )
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
            "open_turn": { "turn_id": turn, "state": "started" }
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
