//! The collector's reconciliation (ACP core §3.4, §5.1, §5.2) against a
//! scripted host over a real WebSocket: the test plays the host frame by
//! frame, so it controls exactly what was delivered before a drop.

use futures::{SinkExt, StreamExt};
use hennery_kernel::auth::DevToken;
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{AttachedSession, CollectorFrame, HostFrame, SessionBody};
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
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN),
        );
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
async fn a_request_that_times_out_on_a_live_connection_drops_it() {
    let collector = Collector::start().await;
    let _host = ScriptedHost::connect(&collector, vec![], 0).await;
    let request = CollectorFrame::StartSession {
        request_id: "r-timeout".into(),
        session_id: "s-timeout".into(),
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
