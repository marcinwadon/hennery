//! Images in prompts over HTTP (ACP core §7, §9; plan 6a): a real collector,
//! with the test playing the host.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
use hennery_proto::rest::{AttachmentUsage, EventDto};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::{AppState, store::Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

const HOST: &str = "host-1";

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
    dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
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
        Self { addr, state, dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// The names in the attachment directory, none if it does not exist.
    fn files(&self) -> Vec<String> {
        match std::fs::read_dir(self.dir.path().join("attachments")) {
            Ok(entries) => entries.map(|e| e.unwrap().file_name().into_string().unwrap()).collect(),
            Err(_) => vec![],
        }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The test playing a host. It reads frames as large as ACP core §11's
/// 32 MiB, as the hennery host does.
struct ScriptedHost {
    ws: Ws,
    seq: u64,
}

impl ScriptedHost {
    async fn connect(collector: &Collector, capabilities: Capabilities) -> Self {
        let config = WebSocketConfig::default()
            .max_message_size(Some(32 << 20))
            .max_frame_size(Some(32 << 20));
        let (ws, response) = tokio_tungstenite::connect_async_with_config(
            format!("ws://{}/api/hosts/ws", collector.addr),
            Some(config),
            false,
        )
        .await
        .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws, seq: 0 };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities,
            attached_sessions: vec![],
        })
        .await;
        assert!(matches!(host.next().await, CollectorFrame::HelloAck { .. }));
        host.send(&HostFrame::ResendComplete).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !collector.state.hub.is_ready(HOST) {
            assert!(tokio::time::Instant::now() < deadline, "host never ready");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        host
    }

    async fn send(&mut self, frame: &HostFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

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
        tokio::time::timeout(Duration::from_secs(20), async {
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
        .expect("a collector frame within 20s")
    }

    /// Nothing but acks arrives for a while.
    async fn nothing_more(&mut self) {
        let more = tokio::time::timeout(Duration::from_millis(300), self.next()).await;
        assert!(more.is_err(), "the host was sent {more:?}");
    }
}

fn client(collector: &Collector) -> reqwest::Client {
    hennery_testkit::operator_client(&collector.state.operator)
}

async fn post(c: &reqwest::Client, url: String, body: &Value) -> (u16, Value) {
    let resp = c
        .post(url)
        .json(body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn started_session(collector: &Collector, host: &mut ScriptedHost) -> String {
    let c = client(collector);
    let url = collector.url("/api/sessions");
    let call =
        tokio::spawn(async move { post(&c, url, &json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" })).await });
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

fn prompt_url(collector: &Collector, session: &str) -> String {
    collector.url(&format!("/api/sessions/{session}/prompt"))
}

/// `len` bytes of a PNG, different for each `seed`.
fn png(seed: u8, len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..len - 8).map(|i| seed.wrapping_mul(31).wrapping_add((i % 251) as u8)));
    bytes
}

fn image(mime: &str, bytes: &[u8]) -> Value {
    json!({ "type": "image", "mimeType": mime, "data": STANDARD.encode(bytes) })
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Answer the prompt the collector sends with `turn_started`, and return
/// its content.
async fn accept_prompt(host: &mut ScriptedHost, session: &str) -> Vec<Value> {
    let CollectorFrame::Prompt {
        request_id,
        turn_id,
        content,
        ..
    } = host.next().await
    else {
        panic!("expected a prompt");
    };
    host.emit(session, SessionBody::TurnStarted { request_id, turn_id })
        .await;
    content
}

/// The most a prompt may carry (ACP core §11): 3 × 5 MiB and 1 MiB of
/// images, 16 MiB together, about 21.4 MiB of base64 and so one frame
/// larger than tungstenite's default 16 MiB, with text around them. The
/// host is sent what was checked and nothing else (the review's A1: no
/// `uri`, `annotations` or `_meta`); the timeline and the turn hold
/// references; each image is served back whole.
#[tokio::test]
async fn a_prompt_at_the_limit_reaches_the_host_whole_and_is_stored_by_reference() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Park, Capability::Images])).await;
    let session = started_session(&collector, &mut host).await;
    let images: Vec<Vec<u8>> = [5 << 20, 5 << 20, 5 << 20, 1 << 20]
        .iter()
        .enumerate()
        .map(|(n, len)| png(n as u8 + 1, *len))
        .collect();
    let mut checked = vec![json!({ "type": "text", "text": "compare these" })];
    checked.extend(images.iter().map(|bytes| image("image/png", bytes)));
    // What the client sends: the same, with fields the agent never sees.
    let mut content = checked.clone();
    content[0]["_meta"] = json!({ "x": 1 });
    content[0]["annotations"] = json!({ "priority": 1 });
    content[1]["uri"] = json!("file:///etc/passwd");
    let body = json!({ "content": content });
    assert!(serde_json::to_vec(&body).unwrap().len() > 21 << 20);

    let c = client(&collector);
    let c2 = c.clone();
    let url = prompt_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c2, url, &body).await });
    let sent = accept_prompt(&mut host, &session).await;
    assert!(sent == checked, "the host is sent only what was checked");
    let (status, answer) = call.await.unwrap();
    assert_eq!(status, 202, "{answer}");

    // The timeline holds references, and no image's bytes.
    let events: Vec<EventDto> = c
        .get(collector.url(&format!("/api/sessions/{session}/events")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_turn = events.iter().find(|e| e.kind == "user_turn").expect("user_turn");
    let stored = &user_turn.body["content"];
    assert_eq!(stored[0], json!({ "type": "text", "text": "compare these" }));
    for (n, bytes) in images.iter().enumerate() {
        assert_eq!(
            stored[n + 1],
            json!({ "type": "image", "mimeType": "image/png", "sha256": sha(bytes), "size": bytes.len() })
        );
    }
    assert!(serde_json::to_vec(&events).unwrap().len() < 64 << 10);

    // Each image is served back, whole, as what it is.
    for bytes in &images {
        let resp = c
            .get(collector.url(&format!("/api/attachments/{}", sha(bytes))))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let headers = resp.headers().clone();
        assert_eq!(headers["content-type"], "image/png");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["content-security-policy"], "default-src 'none'");
        assert_eq!(headers["cache-control"], "private, max-age=31536000, immutable");
        assert_eq!(headers["cross-origin-resource-policy"], "same-origin");
        assert_eq!(resp.bytes().await.unwrap().as_ref(), &bytes[..]);
    }
    let usage: AttachmentUsage = c
        .get(collector.url("/api/settings/attachments"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        usage,
        AttachmentUsage {
            count: 4,
            bytes: 16 << 20
        }
    );
}

/// ACP core §3.3: never an image to a host that did not announce `images`.
/// Refused before a turn opens or a file is written; text still goes.
#[tokio::test]
async fn an_image_prompt_to_a_host_without_images_is_refused_before_anything_is_written() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Park])).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let body = json!({ "content": [{ "type": "text", "text": "see" }, image("image/png", &png(1, 64))] });
    let (status, answer) = post(&c, prompt_url(&collector, &session), &body).await;
    assert_eq!(
        (status, answer["code"].as_str()),
        (409, Some("images_unsupported")),
        "{answer}"
    );
    host.nothing_more().await;
    assert_eq!(
        collector.state.store.session(&session).unwrap().unwrap().open_turn_id,
        None
    );
    assert!(collector.files().is_empty(), "{:?}", collector.files());

    let c2 = c.clone();
    let url = prompt_url(&collector, &session);
    let call =
        tokio::spawn(async move { post(&c2, url, &json!({ "content": [{ "type": "text", "text": "hi" }] })).await });
    accept_prompt(&mut host, &session).await;
    assert_eq!(call.await.unwrap().0, 202);
}

/// An offline host answers `host_offline`, not `images_unsupported`: it has
/// no capabilities while it is gone.
#[tokio::test]
async fn an_image_prompt_to_an_offline_host_is_host_offline() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Images])).await;
    let session = started_session(&collector, &mut host).await;
    drop(host);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while collector.state.hub.is_ready(HOST) {
        assert!(tokio::time::Instant::now() < deadline, "host never gone");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let body = json!({ "content": [image("image/png", &png(1, 64))] });
    let (status, answer) = post(&client(&collector), prompt_url(&collector, &session), &body).await;
    assert_eq!(
        (status, answer["code"].as_str()),
        (409, Some("host_offline")),
        "{answer}"
    );
    assert!(collector.files().is_empty());
}

/// Content the collector refuses opens no turn, writes no file, and never
/// reaches the host (plan 6a: checked before `open_turn`).
#[tokio::test]
async fn refused_content_opens_no_turn_and_writes_no_file() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Images])).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let jpeg_as_png = image("image/png", &[0xff, 0xd8, 0xff, 0xe0, 1, 2, 3]);
    let small = image("image/png", &png(1, 64));
    for (content, status, code) in [
        (json!([]), 400, "empty_prompt"),
        (json!([{ "type": "text", "text": "  " }]), 400, "empty_prompt"),
        (json!([jpeg_as_png]), 400, "invalid_content"),
        (json!([image("image/svg+xml", b"<svg/>")]), 400, "invalid_content"),
        (
            json!([{ "type": "audio", "mimeType": "audio/wav", "data": "AAAA" }]),
            400,
            "invalid_content",
        ),
        (
            json!([{ "type": "resource_link", "uri": "file:///etc/passwd", "name": "p" }]),
            400,
            "invalid_content",
        ),
        (Value::Array(vec![small; 21]), 413, "content_too_large"),
        (
            json!([image("image/png", &png(2, (5 << 20) + 1))]),
            413,
            "content_too_large",
        ),
    ] {
        let (got, answer) = post(&c, prompt_url(&collector, &session), &json!({ "content": content })).await;
        assert_eq!((got, answer["code"].as_str()), (status, Some(code)), "{answer}");
    }
    host.nothing_more().await;
    assert_eq!(
        collector.state.store.session(&session).unwrap().unwrap().open_turn_id,
        None
    );
    assert!(collector.files().is_empty(), "{:?}", collector.files());
}

/// The prompt route alone reads bodies past axum's default 2 MB, and its
/// own limit holds too.
#[tokio::test]
async fn only_the_prompt_route_takes_a_large_body_and_its_limit_holds() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Images])).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let text = "x".repeat(25 << 20);
    let (status, _) = post(
        &c,
        prompt_url(&collector, &session),
        &json!({ "content": [{ "type": "text", "text": text }] }),
    )
    .await;
    assert_eq!(status, 413);
    let (status, _) = post(
        &c,
        collector.url(&format!("/api/sessions/{session}/config")),
        &json!({ "config_id": "x".repeat(3 << 20), "value": true }),
    )
    .await;
    assert_eq!(status, 413);
    host.nothing_more().await;
}

/// A prompt while a turn is open writes no file for its images.
#[tokio::test]
async fn an_image_prompt_during_a_turn_writes_no_file() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Images])).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let c2 = c.clone();
    let url = prompt_url(&collector, &session);
    let call =
        tokio::spawn(async move { post(&c2, url, &json!({ "content": [{ "type": "text", "text": "hi" }] })).await });
    accept_prompt(&mut host, &session).await;
    assert_eq!(call.await.unwrap().0, 202);
    let body = json!({ "content": [image("image/png", &png(1, 64))] });
    let (status, answer) = post(&c, prompt_url(&collector, &session), &body).await;
    assert_eq!(
        (status, answer["code"].as_str()),
        (409, Some("turn_in_progress")),
        "{answer}"
    );
    assert!(collector.files().is_empty());
}

/// The host refuses images its agent cannot take (decision 2): 409
/// `images_unsupported`, and the turn is freed.
#[tokio::test]
async fn a_host_refusing_an_image_prompt_frees_the_turn() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Images])).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let c2 = c.clone();
    let url = prompt_url(&collector, &session);
    let call =
        tokio::spawn(async move { post(&c2, url, &json!({ "content": [image("image/gif", b"GIF89a....")] })).await });
    let CollectorFrame::Prompt { request_id, .. } = host.next().await else {
        panic!("expected a prompt");
    };
    host.send(&HostFrame::Error {
        request_id,
        code: "images_unsupported".into(),
        message: "this agent does not take images".into(),
    })
    .await;
    let (status, answer) = call.await.unwrap();
    assert_eq!(
        (status, answer["code"].as_str()),
        (409, Some("images_unsupported")),
        "{answer}"
    );
    assert_eq!(
        collector.state.store.session(&session).unwrap().unwrap().open_turn_id,
        None
    );
}

/// Attachments are the operator's, and named only by their hash.
#[tokio::test]
async fn attachments_are_served_only_to_the_operator() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Images])).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let bytes = png(1, 64);
    let c2 = c.clone();
    let url = prompt_url(&collector, &session);
    let body = json!({ "content": [image("image/png", &bytes)] });
    let call = tokio::spawn(async move { post(&c2, url, &body).await });
    accept_prompt(&mut host, &session).await;
    assert_eq!(call.await.unwrap().0, 202);
    let url = collector.url(&format!("/api/attachments/{}", sha(&bytes)));
    assert_eq!(c.get(&url).send().await.unwrap().status(), 200);
    // No cookie: 401. Another site's request: 403.
    assert_eq!(reqwest::get(&url).await.unwrap().status(), 401);
    let cross = c.get(&url).header("sec-fetch-site", "cross-site").send().await.unwrap();
    assert_eq!(cross.status(), 403);
    for name in ["0".repeat(64), sha(&bytes).to_uppercase(), "..%2Fhennery.db".into()] {
        let resp = c
            .get(collector.url(&format!("/api/attachments/{name}")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{name}");
    }
    let resp = reqwest::get(collector.url("/api/settings/attachments")).await.unwrap();
    assert_eq!(resp.status(), 401);
}
