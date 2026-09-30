//! Auth negative paths: an operator route without (or with the wrong)
//! session cookie, or from another origin, must be rejected, and a host
//! `hello` without a valid proof of its key must be rejected without ever
//! registering the host (ACP core §3.5, kernel spec §3.3, §11). `axum`'s
//! `.layer()` only wraps routes added *before* it in the router builder, so
//! a route added after would silently escape auth — the route table below
//! pins that every operator route is actually covered.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{CollectorFrame, HostFrame};
use hennery_proto::rest::{ApiError, HostItem};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;
use tokio_tungstenite::tungstenite::Message;

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

/// Every operator route, as a method and a path. A route missing from
/// here is a route nobody checked.
const OPERATOR_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/hosts"),
    ("POST", "/api/hosts/pairing-codes"),
    ("DELETE", "/api/hosts/host-9"),
    ("POST", "/api/sessions"),
    ("GET", "/api/sessions/s-1"),
    ("POST", "/api/sessions/s-1/resume"),
    ("POST", "/api/sessions/s-1/prompt"),
    ("POST", "/api/sessions/s-1/cancel"),
    ("POST", "/api/sessions/s-1/park"),
    ("POST", "/api/sessions/s-1/close"),
    ("GET", "/api/sessions/s-1/catalog"),
    ("POST", "/api/sessions/s-1/config"),
    ("POST", "/api/sessions/s-1/pending/p-1/answer"),
    ("GET", "/api/sessions/s-1/events"),
    ("GET", "/api/stream/sessions/s-1"),
    ("POST", "/api/auth/step-up/password"),
    ("GET", "/api/auth/sessions"),
    ("DELETE", "/api/auth/sessions/0000"),
];

/// Bounded, so a route that escaped the layer and streams (SSE) fails the
/// test instead of hanging it.
fn request(client: &reqwest::Client, collector: &Collector, method: &str, path: &str) -> reqwest::RequestBuilder {
    client
        .request(method.parse().unwrap(), collector.url(path))
        .timeout(std::time::Duration::from_secs(10))
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (
        status,
        resp.json::<ApiError>().await.map(|e| e.code).unwrap_or_default(),
    )
}

#[tokio::test]
async fn every_operator_route_needs_the_session_cookie() {
    let collector = Collector::start().await;
    let signed_in = hennery_testkit::operator_client(&collector.state.operator);
    let plain = reqwest::Client::new();
    for &(method, path) in OPERATOR_ROUTES {
        let none = request(&plain, &collector, method, path)
            .header("origin", hennery_testkit::PUBLIC_URL)
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(none).await, (401, "unauthenticated".into()), "{method} {path}");
        let wrong = request(&plain, &collector, method, path)
            .header("origin", hennery_testkit::PUBLIC_URL)
            .header("cookie", format!("hennery_session={}", "0".repeat(64)))
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(wrong).await, (401, "unauthenticated".into()), "{method} {path}");
        // Sanity: the owner's session gets through, proving the 401s above
        // are about the cookie and not a broken harness.
        let status = request(&signed_in, &collector, method, path)
            .send()
            .await
            .unwrap()
            .status();
        assert!(status != 401 && status != 403, "{method} {path}: {status}");
    }
    let hosts: Vec<HostItem> = signed_in
        .get(collector.url("/api/hosts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!((hosts[0].host_id.as_str(), hosts[0].connected), (HOST, false));
    collector.stop().await;
}

/// Kernel spec §3.3, with a valid session: state-changing requests need the
/// `public_url`'s `Origin`; `GET`s are refused when the browser says they
/// are cross-site, or carry another `Origin`.
#[tokio::test]
async fn every_operator_route_applies_the_browser_rules() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let token = collector
        .state
        .operator
        .open_session("test", hennery_kernel::secret::unix_now())
        .unwrap()
        .unwrap();
    let cookie = format!("hennery_session={token}");
    let plain = reqwest::Client::new();
    for &(method, path) in OPERATOR_ROUTES {
        let send = |origin: Option<&str>, site: Option<&str>| {
            let mut req = request(&plain, &collector, method, path).header("cookie", &cookie);
            if let Some(origin) = origin {
                req = req.header("origin", origin);
            }
            if let Some(site) = site {
                req = req.header("sec-fetch-site", site);
            }
            req.send()
        };
        let evil = code_of(send(Some("https://evil.example"), None).await.unwrap()).await;
        assert_eq!(evil, (403, "origin_mismatch".into()), "{method} {path}");
        if method == "GET" {
            for site in ["cross-site", "same-site"] {
                let resp = send(None, Some(site)).await.unwrap();
                assert_eq!(
                    code_of(resp).await,
                    (403, "cross_site".into()),
                    "{method} {path} {site}"
                );
            }
            for site in [Some("same-origin"), Some("none"), None] {
                let status = send(None, site).await.unwrap().status();
                assert!(status != 401 && status != 403, "{method} {path} {site:?}: {status}");
            }
        } else {
            let missing = code_of(send(None, None).await.unwrap()).await;
            assert_eq!(missing, (403, "origin_mismatch".into()), "{method} {path}");
            // From the right origin, the body must still be JSON: the rules
            // refuse any other type (the code proves it is them, not an
            // extractor), and accept `application/json` with parameters.
            let with_body = |content_type: &'static str, body: &'static str| {
                request(&plain, &collector, method, path)
                    .header("cookie", &cookie)
                    .header("origin", hennery_testkit::PUBLIC_URL)
                    .header("content-type", content_type)
                    .body(body)
                    .send()
            };
            let text = code_of(with_body("text/plain", "hello").await.unwrap()).await;
            assert_eq!(text, (415, "unsupported_media_type".into()), "{method} {path}");
            let status = with_body("application/json; charset=utf-8", "{}")
                .await
                .unwrap()
                .status();
            assert!(
                status != 401 && status != 403 && status != 415,
                "{method} {path} application/json; charset=utf-8: {status}"
            );
        }
    }
    collector.stop().await;
}

/// The browser rules wrap the session check (3b decision 8): a cross-origin
/// or cross-site request is refused 403 before its missing cookie would get
/// a 401.
#[tokio::test]
async fn the_browser_rules_run_before_the_session_check() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let plain = reqwest::Client::new();
    for &(method, path) in OPERATOR_ROUTES {
        let evil = request(&plain, &collector, method, path)
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(evil).await, (403, "origin_mismatch".into()), "{method} {path}");
        if method == "GET" {
            let cross = request(&plain, &collector, method, path)
                .header("sec-fetch-site", "cross-site")
                .send()
                .await
                .unwrap();
            assert_eq!(code_of(cross).await, (403, "cross_site".into()), "{method} {path}");
        }
    }
    collector.stop().await;
}

/// Enrollment and the host WebSocket are authenticated otherwise and are
/// exempt from the browser rules (kernel spec §3.3): no cookie, any origin.
#[tokio::test]
async fn enrollment_and_the_host_socket_need_neither_a_session_nor_an_origin() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let enroll = reqwest::Client::new()
        .post(collector.url("/api/hosts/enroll"))
        .header("origin", "https://evil.example")
        .json(&serde_json::json!({
            "code": "0000-0000", "public_key": host_key().public_key_hex(),
            "name": "x", "host_version": "x", "platform": "x"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(enroll).await, (401, "invalid_code".into()));
    let (mut ws, nonce) = connect(&collector).await;
    let proof = host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    assert!(matches!(
        hello(&mut ws, HOST, proof).await,
        CollectorFrame::HelloAck { .. }
    ));
    collector.stop().await;
}

/// A request that slides the session's expiry sends the cookie again, so
/// the browser's copy is extended with it; one that does not, sends none.
#[tokio::test]
async fn a_request_that_slides_the_session_sends_its_cookie_again() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let now = hennery_kernel::secret::unix_now();
    let url = collector.url("/api/hosts");
    let fresh = collector.state.operator.open_session("test", now).unwrap().unwrap();
    let resp = reqwest::Client::new()
        .get(&url)
        .header("cookie", format!("hennery_session={fresh}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("set-cookie").is_none());

    let stale = collector
        .state
        .operator
        .open_session("test", now - 120)
        .unwrap()
        .unwrap();
    let resp = reqwest::Client::new()
        .get(&url)
        .header("cookie", format!("hennery_session={stale}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cookie = resp.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with(&format!("hennery_session={stale};")), "{cookie}");
    collector.stop().await;
}

/// A cookie tossed in from a sibling subdomain (`Domain=` set there, same
/// name) can land before the real one in `Cookie`. Every candidate is
/// tried, so it does not sign the owner out; and when the real session
/// slides, the cookie sent again is the real one, not the tossed value.
#[tokio::test]
async fn a_tossed_session_cookie_before_the_real_one_does_not_sign_the_owner_out() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let now = hennery_kernel::secret::unix_now();
    let url = collector.url("/api/hosts");
    // Shaped like a real token, so it costs a lookup and fails it.
    let tossed = "0".repeat(64);
    let real = collector.state.operator.open_session("test", now).unwrap().unwrap();
    let resp = reqwest::Client::new()
        .get(&url)
        .header("cookie", format!("hennery_session={tossed}; hennery_session={real}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let stale = collector
        .state
        .operator
        .open_session("test", now - 120)
        .unwrap()
        .unwrap();
    let resp = reqwest::Client::new()
        .get(&url)
        .header("cookie", format!("hennery_session={tossed}"))
        .header("cookie", format!("hennery_session={stale}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cookie = resp.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with(&format!("hennery_session={stale};")), "{cookie}");
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
