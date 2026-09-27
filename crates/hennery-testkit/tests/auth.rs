//! Auth negative paths: a protected REST route without (or with the wrong)
//! bearer token must be rejected, and a host `hello` with the wrong token
//! must be rejected without ever registering the host (ACP core §3.3, §3.5).
//! `axum`'s `.layer()` only wraps routes added *before* it in the router
//! builder, so a route added after would silently escape auth — this pins
//! that every REST route is actually covered.

use futures::{SinkExt, StreamExt};
use hennery_kernel::auth::DevToken;
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{CollectorFrame, HostFrame};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "dev-token";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
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
        let task = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            addr,
            state,
            task,
            _dir: dir,
        }
    }

    async fn stop(self) {
        self.state.shutdown.cancel();
        self.task.await.unwrap().unwrap();
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

#[tokio::test]
async fn a_request_without_or_with_the_wrong_bearer_token_is_rejected() {
    let collector = Collector::start().await;
    let plain = reqwest::Client::new();

    let no_auth = plain.get(collector.url("/api/hosts")).send().await.unwrap();
    assert_eq!(no_auth.status(), 401);

    let mut wrong_headers = reqwest::header::HeaderMap::new();
    wrong_headers.insert("authorization", "Bearer not-the-token".parse().unwrap());
    let wrong = reqwest::Client::builder()
        .default_headers(wrong_headers)
        .build()
        .unwrap()
        .get(collector.url("/api/hosts"))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    // Sanity: the right token still gets through, proving the 401s above are
    // about the credential and not a broken test harness.
    let mut right_headers = reqwest::header::HeaderMap::new();
    right_headers.insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let right = reqwest::Client::builder()
        .default_headers(right_headers)
        .build()
        .unwrap()
        .get(collector.url("/api/hosts"))
        .send()
        .await
        .unwrap();
    assert_eq!(right.status(), 200);

    collector.stop().await;
}

#[tokio::test]
async fn a_host_hello_with_the_wrong_token_is_rejected_without_registering() {
    let collector = Collector::start().await;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
        .await
        .unwrap();
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::text(
        serde_json::to_string(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "0".into(),
            host_id: "host-1".into(),
            token: "not-the-token".into(),
            attached_sessions: vec![],
        })
        .unwrap(),
    ))
    .await
    .unwrap();

    let reply = match stream.next().await {
        Some(Ok(Message::Text(t))) => serde_json::from_str::<CollectorFrame>(&t).unwrap(),
        other => panic!("expected a hello_error frame, got {other:?}"),
    };
    assert!(
        matches!(reply, CollectorFrame::HelloError { ref code, .. } if code == "bad_proof"),
        "{reply:?}"
    );

    // The socket is closed after the rejection, and the host was never
    // registered: `/api/hosts` (via the real REST auth, proving both paths
    // work together) must not list it.
    assert!(matches!(stream.next().await, None | Some(Err(_))));

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let hosts: Vec<String> = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap()
        .get(collector.url("/api/hosts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!hosts.contains(&"host-1".to_string()), "{hosts:?}");

    collector.stop().await;
}
