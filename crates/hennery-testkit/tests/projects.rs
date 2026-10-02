//! The project picker (ACP core §3.3, §7, §9; plan 6c) against a scripted
//! host over a real WebSocket: the test plays the host, so it controls
//! every probe reply, its timing and its connection.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame, Project};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::hub::RequestError;
use hennery_sessions::{AppState, store::Store};
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
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        hosts.register(HOST, &enrollment, 0).unwrap();
        let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The test playing a host.
struct ScriptedHost {
    ws: Ws,
}

impl ScriptedHost {
    /// `hello` announcing `capabilities` and `workspace_roots`, then
    /// `resend_complete`, and wait until the collector lists the host as
    /// connected (reconciled).
    async fn connect(collector: &Collector, capabilities: Capabilities, workspace_roots: Vec<String>) -> Self {
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities,
            workspace_roots,
            attached_sessions: vec![],
        })
        .await;
        let ack = host.next().await;
        assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
        host.send(&HostFrame::ResendComplete).await;
        wait_for("host ready", || async {
            collector.state.hub.is_ready(HOST).then_some(())
        })
        .await;
        host
    }

    /// `connect` announcing the `projects` capability and no roots.
    async fn with_projects(collector: &Collector) -> Self {
        Self::connect(collector, Capabilities(vec![Capability::Projects]), vec![]).await
    }

    async fn send(&mut self, frame: &HostFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
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

    /// Nothing but pings reaches this host within `within`.
    async fn hears_nothing(&mut self, within: Duration) {
        let more = tokio::time::timeout(within, self.next()).await;
        assert!(more.is_err(), "the host was sent {more:?}");
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

fn projects(request_id: String, paths: &[&str]) -> HostFrame {
    HostFrame::Projects {
        request_id,
        items: paths.iter().map(|p| Project { path: p.to_string() }).collect(),
        partial: false,
        home: None,
    }
}

/// Send `list_projects` through the hub, as a route does, with `timeout`.
fn probe(
    collector: &Collector,
    request_id: &str,
    timeout: Duration,
) -> tokio::task::JoinHandle<Result<HostFrame, RequestError>> {
    let hub = collector.state.hub.clone();
    let request_id = request_id.to_string();
    tokio::spawn(async move {
        let frame = CollectorFrame::ListProjects {
            request_id: request_id.clone(),
        };
        hub.probe(HOST, &request_id, frame, timeout).await
    })
}

/// The `request_id` of the `list_projects` the host was sent next.
async fn listed(host: &mut ScriptedHost) -> String {
    match host.next().await {
        CollectorFrame::ListProjects { request_id } => request_id,
        other => panic!("expected list_projects, got {other:?}"),
    }
}

// Task 1: the socket routes probe replies and rejections (decision 1).

#[tokio::test]
async fn a_probe_reply_from_the_host_answers_its_probe() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let call = probe(&collector, "p1", Duration::from_secs(5));
    let request_id = listed(&mut host).await;
    assert_eq!(request_id, "p1");
    host.send(&projects(request_id, &["/p/a"])).await;
    assert_eq!(call.await.unwrap(), Ok(projects("p1".into(), &["/p/a"])));
}

#[tokio::test]
async fn a_host_error_answers_its_probe_and_keeps_the_connection() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let call = probe(&collector, "p1", Duration::from_secs(5));
    let request_id = listed(&mut host).await;
    host.send(&HostFrame::Error {
        request_id,
        code: "outside_workspace".into(),
        message: "no".into(),
    })
    .await;
    assert_eq!(
        call.await.unwrap(),
        Err(RequestError::Rejected {
            code: "outside_workspace".into(),
            message: "no".into()
        })
    );
    assert!(collector.state.hub.is_ready(HOST));
}

/// Task 1's review: probe routing leaves the session waiters' rejections
/// alone, and a waiter's rejection leaves a probe alone.
#[tokio::test]
async fn a_host_error_answers_only_the_request_it_names() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let hub = collector.state.hub.clone();
    let waiter = tokio::spawn(async move {
        let frame = CollectorFrame::Prompt {
            request_id: "w1".into(),
            session_id: "s1".into(),
            turn_id: "t1".into(),
            content: vec![serde_json::json!({"type": "text", "text": "hi"})],
        };
        hub.request(HOST, "w1", frame, Duration::from_secs(10)).await
    });
    assert!(matches!(host.next().await, CollectorFrame::Prompt { .. }));
    let call = probe(&collector, "p1", Duration::from_secs(10));
    let request_id = listed(&mut host).await;
    host.send(&HostFrame::Error {
        request_id: "w1".into(),
        code: "not_attached".into(),
        message: "no".into(),
    })
    .await;
    let answered = waiter.await.unwrap();
    assert!(
        matches!(answered, Err(RequestError::Rejected { ref code, .. }) if code == "not_attached"),
        "{answered:?}"
    );
    assert_eq!(
        collector.state.hub.pending_probes(),
        1,
        "the waiter's rejection answered the probe"
    );
    host.send(&HostFrame::Error {
        request_id,
        code: "invalid".into(),
        message: "no".into(),
    })
    .await;
    let answered = call.await.unwrap();
    assert!(
        matches!(answered, Err(RequestError::Rejected { ref code, .. }) if code == "invalid"),
        "{answered:?}"
    );
    assert_eq!(collector.state.hub.pending_requests(), 0);
}

#[tokio::test]
async fn a_late_probe_reply_is_dropped_and_the_connection_kept() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let call = probe(&collector, "p1", Duration::from_millis(200));
    let late = listed(&mut host).await;
    assert_eq!(call.await.unwrap(), Err(RequestError::DeliveryUnknown));
    host.send(&projects(late, &["/late"])).await;
    // Time for the socket to read it: it answers nothing, and waits on
    // nothing.
    host.hears_nothing(Duration::from_millis(200)).await;
    assert_eq!(collector.state.hub.pending_probes(), 0);
    assert!(
        collector.state.hub.is_ready(HOST),
        "a probe timeout dropped the connection"
    );
    let call = probe(&collector, "p2", Duration::from_secs(5));
    let request_id = listed(&mut host).await;
    host.send(&projects(request_id, &["/p/b"])).await;
    assert_eq!(call.await.unwrap(), Ok(projects("p2".into(), &["/p/b"])));
}
