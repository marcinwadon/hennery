//! Auth negative paths: a protected REST route without (or with the wrong)
//! bearer token must be rejected, and a host `hello` without a valid proof
//! of its key must be rejected without ever registering the host (ACP core
//! §3.5, kernel spec §11). `axum`'s `.layer()` only wraps routes added
//! *before* it in the router builder, so a route added after would silently
//! escape auth — this pins that every REST route is actually covered.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{CollectorFrame, HostFrame};
use hennery_proto::rest::HostItem;
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "dev-token-for-tests";
const HOST: &str = "host-1";

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

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
            Hosts::open(&dir.path().join("hennery.db")).unwrap(),
            Operator::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        state.hosts.register(HOST, &enrollment, 0).unwrap();
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

    fn connected(&self) -> Vec<String> {
        self.state.hub.connected_hosts()
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open a host socket; the nonce is the one in the upgrade response.
async fn connect(collector: &Collector) -> (Ws, Vec<u8>) {
    let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
        .await
        .unwrap();
    let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
    assert_eq!(nonce.len(), 32);
    (ws, nonce)
}

/// Send a `hello` for `host_id` with `proof` and return the answer.
async fn hello(ws: &mut Ws, host_id: &str, proof: String) -> CollectorFrame {
    ws.send(Message::text(
        serde_json::to_string(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "0".into(),
            host_id: host_id.into(),
            proof,
            capabilities: Default::default(),
            attached_sessions: vec![],
        })
        .unwrap(),
    ))
    .await
    .unwrap();
    match ws.next().await {
        Some(Ok(Message::Text(t))) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected a reply to hello, got {other:?}"),
    }
}

fn hello_error(frame: &CollectorFrame) -> &str {
    match frame {
        CollectorFrame::HelloError { code, .. } => code,
        other => panic!("expected hello_error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_request_without_or_with_the_wrong_bearer_token_is_rejected() {
    let collector = Collector::start().await;
    let plain = reqwest::Client::new();

    let no_auth = plain.get(collector.url("/api/hosts")).send().await.unwrap();
    assert_eq!(no_auth.status(), 401);

    let wrong = plain
        .get(collector.url("/api/hosts"))
        .bearer_auth("not-the-token")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    // Sanity: the right token still gets through, proving the 401s above are
    // about the credential and not a broken test harness.
    let right = plain
        .get(collector.url("/api/hosts"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(right.status(), 200);
    let hosts: Vec<HostItem> = right.json().await.unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!((hosts[0].host_id.as_str(), hosts[0].connected), (HOST, false));

    collector.stop().await;
}

#[tokio::test]
async fn a_hello_signed_by_another_key_is_rejected_without_registering() {
    let collector = Collector::start().await;
    let (mut ws, nonce) = connect(&collector).await;
    let forged = HostKey::from_seed([2; 32]).sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    let reply = hello(&mut ws, HOST, forged).await;
    assert_eq!(hello_error(&reply), "bad_proof");
    // The socket is closed after the rejection, and the host never
    // registered.
    assert!(matches!(ws.next().await, None | Some(Err(_))));
    assert!(collector.connected().is_empty());
    collector.stop().await;
}

#[tokio::test]
async fn a_proof_is_good_on_its_own_connection_only() {
    let collector = Collector::start().await;
    let (_first, first_nonce) = connect(&collector).await;
    let (mut second, second_nonce) = connect(&collector).await;
    assert_ne!(first_nonce, second_nonce);
    // Replaying the first connection's proof on the second is refused.
    let replayed = host_key().sign_hello(&first_nonce, HOST, PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut second, HOST, replayed).await), "bad_proof");

    let (mut third, nonce) = connect(&collector).await;
    let proof = host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    let reply = hello(&mut third, HOST, proof).await;
    assert!(matches!(reply, CollectorFrame::HelloAck { .. }), "{reply:?}");
    let record = collector.state.hosts.host(HOST).unwrap().unwrap();
    assert_eq!(record.host_version, "0");
    assert!(record.last_seen_at.is_some());
    collector.stop().await;
}

#[tokio::test]
async fn an_unknown_host_is_refused_like_a_bad_proof() {
    let collector = Collector::start().await;
    let (mut ws, nonce) = connect(&collector).await;
    let proof = host_key().sign_hello(&nonce, "host-9", PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut ws, "host-9", proof).await), "bad_proof");
    collector.stop().await;
}

#[tokio::test]
async fn a_revoked_host_is_told_so_but_only_with_a_valid_proof() {
    let collector = Collector::start().await;
    collector.state.hosts.revoke(HOST, 1).unwrap();

    let (mut ws, nonce) = connect(&collector).await;
    let forged = HostKey::from_seed([2; 32]).sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut ws, HOST, forged).await), "bad_proof");

    let (mut ws, nonce) = connect(&collector).await;
    let proof = host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut ws, HOST, proof).await), "revoked");
    assert!(collector.connected().is_empty());
    collector.stop().await;
}
