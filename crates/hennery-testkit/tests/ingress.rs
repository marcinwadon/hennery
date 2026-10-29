//! Internet admission and negotiated proof over actual HTTP and WebSocket I/O.
use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::{
    hosts::{Enrollment, Hosts},
    operator::Operator,
    server::{Admission, Limits},
};
use hennery_proto::{
    HELLO_AUTH_HEADER, HELLO_NONCE_HEADER, PROTOCOL_VERSION,
    frames::{CollectorFrame, HostFrame},
};
use hennery_sessions::{AppState, store::Store};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error, Message};

struct Server {
    address: SocketAddr,
    state: AppState,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.shutdown.cancel();
    }
}
impl Server {
    async fn start() -> Self {
        Self::with_admission(64, 4).await
    }
    async fn with_admission(total: usize, per_client: usize) -> Self {
        let mut state = AppState::new(
            Store::open_in_memory().unwrap(),
            Hosts::open_in_memory().unwrap(),
            Operator::open_in_memory().unwrap(),
        );
        state.handshakes = Arc::new(Admission::new(total, per_client));
        let key = HostKey::from_seed([91; 32]);
        state
            .hosts
            .register(
                "fixture",
                &Enrollment {
                    public_key: key.public_key_hex(),
                    name: "fixture".into(),
                    host_version: "0".into(),
                    platform: "test".into(),
                },
                0,
            )
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = hennery_sessions::router(state.clone());
        tokio::spawn(hennery_kernel::server::serve_all(
            vec![listener],
            app,
            state.shutdown.clone(),
            Limits {
                connections: 8,
                header_timeout: Duration::from_secs(1),
            },
        ));
        Self { address, state }
    }
    fn url(&self) -> String {
        format!("ws://{}/api/hosts/ws", self.address)
    }
}
fn hello(proof: String, large: bool) -> HostFrame {
    HostFrame::Hello {
        protocol_version: PROTOCOL_VERSION.into(),
        host_version: "fixture".into(),
        host_id: "fixture".into(),
        proof,
        capabilities: Default::default(),
        mcp_isolation: Default::default(),
        runtime_profiles: Default::default(),
        workspace_roots: if large { vec!["x".repeat(128 * 1024)] } else { vec![] },
        attached_sessions: vec![],
    }
}
#[tokio::test]
async fn negotiated_proof_allows_large_hello_and_legacy_small_hello_still_connects() {
    for staged in [false, true] {
        let server = Server::start().await;
        let (mut ws, response) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
        assert_eq!(response.headers()[HELLO_AUTH_HEADER], "1");
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let proof = HostKey::from_seed([91; 32]).sign_hello(&nonce, "fixture", PROTOCOL_VERSION);
        if staged {
            ws.send(Message::text(
                serde_json::to_string(&HostFrame::Authenticate {
                    protocol_version: PROTOCOL_VERSION.into(),
                    host_id: "fixture".into(),
                    proof: proof.clone(),
                })
                .unwrap(),
            ))
            .await
            .unwrap();
        }
        ws.send(Message::text(serde_json::to_string(&hello(proof, staged)).unwrap()))
            .await
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(response) = response else {
            panic!("expected hello ack")
        };
        assert!(matches!(
            serde_json::from_str::<CollectorFrame>(&response).unwrap(),
            CollectorFrame::HelloAck { .. }
        ));
    }
}

#[tokio::test]
async fn negotiated_auth_preserves_unknown_host_wrong_key_and_version_diagnostics() {
    for (host, seed, version, expected) in [
        ("fixture", 92, PROTOCOL_VERSION, "bad_proof"),
        ("unknown", 91, PROTOCOL_VERSION, "bad_proof"),
        ("fixture", 91, "2.0", "incompatible"),
    ] {
        let server = Server::start().await;
        let (mut ws, response) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let proof = HostKey::from_seed([seed; 32]).sign_hello(&nonce, host, version);
        ws.send(Message::text(
            serde_json::to_string(&HostFrame::Authenticate {
                protocol_version: version.into(),
                host_id: host.into(),
                proof,
            })
            .unwrap(),
        ))
        .await
        .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .expect("structured refusal")
            .expect("valid refusal frame");
        let Message::Text(response) = response else {
            panic!("expected error text")
        };
        let CollectorFrame::HelloError { code, .. } = serde_json::from_str(&response).unwrap() else {
            panic!("expected hello_error")
        };
        assert_eq!(code, expected);
        assert!(server.state.hub.connected_hosts().is_empty());
    }
}
#[tokio::test]
async fn pending_handshakes_are_refused_before_upgrade_and_release_on_disconnect() {
    let server = Server::with_admission(2, 1).await;
    let (first, _) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
    let mut spoof = server.url().into_client_request().unwrap();
    spoof
        .headers_mut()
        .insert("hennery-client-ip", "192.0.2.99".parse().unwrap());
    match tokio_tungstenite::connect_async(spoof).await.unwrap_err() {
        Error::Http(response) => {
            assert_eq!(response.status(), 503);
            assert_eq!(response.headers()["retry-after"], "1");
        }
        other => panic!("expected capacity refusal: {other}"),
    }
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if tokio_tungstenite::connect_async(server.url()).await.is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn stalled_http_headers_are_closed() {
    let server = Server::start().await;
    let mut socket = tokio::net::TcpStream::connect(server.address).await.unwrap();
    socket
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: fixture\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(!response.starts_with(b"HTTP/1.1 200"));
}

#[tokio::test]
async fn http_connections_are_bounded_and_capacity_returns_after_disconnect() {
    let server = Server::start().await;
    let mut held = Vec::new();
    for _ in 0..8 {
        let mut socket = tokio::net::TcpStream::connect(server.address).await.unwrap();
        socket
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: fixture\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !response.ends_with(b"\r\n\r\nok") {
                let mut bytes = [0; 512];
                let read = socket.read(&mut bytes).await.unwrap();
                assert!(read > 0, "admitted connection closed");
                response.extend_from_slice(&bytes[..read]);
            }
        })
        .await
        .unwrap();
        held.push(socket);
    }
    let mut excess = tokio::net::TcpStream::connect(server.address).await.unwrap();
    let mut byte = [0; 1];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), excess.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    held.clear();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut socket = tokio::net::TcpStream::connect(server.address).await.unwrap();
            if socket
                .write_all(b"GET /healthz HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\n\r\n")
                .await
                .is_ok()
            {
                let mut response = Vec::new();
                if socket.read_to_end(&mut response).await.is_ok() && response.starts_with(b"HTTP/1.1 200") {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_large_declared_frame_is_closed_without_waiting_for_its_payload() {
    let server = Server::start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
    let mut header = vec![0x82, 0xff];
    header.extend_from_slice(&(32u64 * 1024 * 1024).to_be_bytes());
    header.extend_from_slice(&[0; 4]);
    ws.get_mut().write_all(&header).await.unwrap();
    // Only fourteen bytes were sent. Waiting for the payload or the ordinary
    // ten-second hello deadline would fail this check.
    let answer = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap();
    assert!(answer.is_none() || answer.is_some_and(|answer| answer.is_err()));
}

#[tokio::test]
async fn an_oversized_frame_prefetched_with_upgrade_is_also_refused() {
    let server = Server::start().await;
    let mut socket = tokio::net::TcpStream::connect(server.address).await.unwrap();
    let mut request=b"GET /api/hosts/ws HTTP/1.1\r\nHost: fixture\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n".to_vec();
    request.extend_from_slice(&[0x82, 0xff]);
    request.extend_from_slice(&(32u64 * 1024 * 1024).to_be_bytes());
    request.extend_from_slice(&[0; 4]);
    socket.write_all(&request).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 101"));
}

#[derive(Clone, Default)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn rejected_host_identity_is_not_reflected_in_logs() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();
    let server = Server::start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
    let mut frame = serde_json::to_value(hello("00".repeat(64), false)).unwrap();
    frame["host_id"] = serde_json::json!(format!("UNTRUSTED_ID_CANARY_{}", "x".repeat(4096)));
    ws.send(Message::text(frame.to_string())).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap();
    let log = captured.0.lock().unwrap();
    assert!(
        !log.windows("UNTRUSTED_ID_CANARY_".len())
            .any(|w| w == b"UNTRUSTED_ID_CANARY_")
    );
    assert!(log.len() < 2048, "one rejection must have bounded output");
}
