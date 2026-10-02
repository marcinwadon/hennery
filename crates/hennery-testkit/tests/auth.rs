//! Auth negative paths: an operator route without (or with the wrong)
//! session cookie, or from another origin, must be rejected, and a host
//! `hello` without a valid proof of its key must be rejected without ever
//! registering the host (ACP core §3.5, kernel spec §3.3, §11). `axum`'s
//! `.layer()` only wraps routes added *before* it in the router builder, so
//! a route added after would silently escape auth — the route table below
//! pins that every operator route is actually covered, on every listener
//! (kernel spec §7, §11): the test collector listens on two.

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
    /// The first of `addrs`.
    addr: SocketAddr,
    /// Every address it listens on, the same router on each.
    addrs: Vec<SocketAddr>,
    state: AppState,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut listeners = Vec::new();
        for _ in 0..2 {
            listeners.push(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
        }
        let addrs: Vec<SocketAddr> = listeners.iter().map(|l| l.local_addr().unwrap()).collect();
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
        let task = tokio::spawn(hennery_sessions::serve_on(listeners, state.clone()));
        Self {
            addr: addrs[0],
            addrs,
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

    /// A new session of the owner's, last checked at `at`, opened as a
    /// login would.
    fn session_at(&self, at: i64) -> String {
        let phc = hennery_testkit::owner_phc(&self.state.operator);
        self.state.operator.open_session("test", &phc, at).unwrap().unwrap()
    }

    fn connected(&self) -> Vec<String> {
        self.state.hub.connected_hosts()
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open a host socket on `addr`; the nonce is the one in the upgrade response.
async fn connect(addr: SocketAddr) -> (Ws, Vec<u8>) {
    let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
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
            workspace_roots: vec![],
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

/// Every route outside the operator's session, as a method and a path:
/// the pages, setup, enrollment, the host WebSocket and the health checks.
const EXEMPT_ROUTES: &[(&str, &str)] = &[
    ("GET", "/"),
    ("GET", "/setup"),
    ("GET", "/setup.js"),
    ("POST", "/api/setup"),
    ("POST", "/api/hosts/enroll"),
    ("GET", "/api/hosts/ws"),
    ("GET", "/healthz"),
    ("GET", "/readyz"),
];

/// Every operator route on every listener of `collector`.
fn every_route(collector: &Collector) -> Vec<(SocketAddr, &'static str, &'static str)> {
    let on = |addr: SocketAddr| OPERATOR_ROUTES.iter().map(move |&(method, path)| (addr, method, path));
    collector.addrs.iter().copied().flat_map(on).collect()
}

/// Bounded, so a route that escaped the layer and streams (SSE) fails the
/// test instead of hanging it. To the collector's listener at `addr`.
fn request(client: &reqwest::Client, addr: SocketAddr, method: &str, path: &str) -> reqwest::RequestBuilder {
    client
        .request(method.parse().unwrap(), format!("http://{addr}{path}"))
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
    for (addr, method, path) in every_route(&collector) {
        let none = request(&plain, addr, method, path)
            .header("origin", hennery_testkit::PUBLIC_URL)
            .send()
            .await
            .unwrap();
        assert_eq!(
            code_of(none).await,
            (401, "unauthenticated".into()),
            "{addr} {method} {path}"
        );
        let wrong = request(&plain, addr, method, path)
            .header("origin", hennery_testkit::PUBLIC_URL)
            .header("cookie", format!("hennery_session={}", "0".repeat(64)))
            .send()
            .await
            .unwrap();
        assert_eq!(
            code_of(wrong).await,
            (401, "unauthenticated".into()),
            "{addr} {method} {path}"
        );
        // Sanity: the owner's session gets through, proving the 401s above
        // are about the cookie and not a broken harness.
        let status = request(&signed_in, addr, method, path).send().await.unwrap().status();
        assert!(status != 401 && status != 403, "{addr} {method} {path}: {status}");
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
    let token = collector.session_at(hennery_kernel::secret::unix_now());
    let cookie = format!("hennery_session={token}");
    let plain = reqwest::Client::new();
    for (addr, method, path) in every_route(&collector) {
        let send = |origin: Option<&str>, site: Option<&str>| {
            let mut req = request(&plain, addr, method, path).header("cookie", &cookie);
            if let Some(origin) = origin {
                req = req.header("origin", origin);
            }
            if let Some(site) = site {
                req = req.header("sec-fetch-site", site);
            }
            req.send()
        };
        let evil = code_of(send(Some("https://evil.example"), None).await.unwrap()).await;
        assert_eq!(evil, (403, "origin_mismatch".into()), "{addr} {method} {path}");
        if method == "GET" {
            for site in ["cross-site", "same-site"] {
                let resp = send(None, Some(site)).await.unwrap();
                assert_eq!(
                    code_of(resp).await,
                    (403, "cross_site".into()),
                    "{addr} {method} {path} {site}"
                );
            }
            for site in [Some("same-origin"), Some("none"), None] {
                let status = send(None, site).await.unwrap().status();
                assert!(
                    status != 401 && status != 403,
                    "{addr} {method} {path} {site:?}: {status}"
                );
            }
        } else {
            let missing = code_of(send(None, None).await.unwrap()).await;
            assert_eq!(missing, (403, "origin_mismatch".into()), "{addr} {method} {path}");
            // From the right origin, the body must still be JSON: the rules
            // refuse any other type (the code proves it is them, not an
            // extractor), and accept `application/json` with parameters.
            let with_body = |content_type: &'static str, body: &'static str| {
                request(&plain, addr, method, path)
                    .header("cookie", &cookie)
                    .header("origin", hennery_testkit::PUBLIC_URL)
                    .header("content-type", content_type)
                    .body(body)
                    .send()
            };
            let text = code_of(with_body("text/plain", "hello").await.unwrap()).await;
            assert_eq!(text, (415, "unsupported_media_type".into()), "{addr} {method} {path}");
            let status = with_body("application/json; charset=utf-8", "{}")
                .await
                .unwrap()
                .status();
            assert!(
                status != 401 && status != 403 && status != 415,
                "{addr} {method} {path} application/json; charset=utf-8: {status}"
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
    for (addr, method, path) in every_route(&collector) {
        let evil = request(&plain, addr, method, path)
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(
            code_of(evil).await,
            (403, "origin_mismatch".into()),
            "{addr} {method} {path}"
        );
        if method == "GET" {
            let cross = request(&plain, addr, method, path)
                .header("sec-fetch-site", "cross-site")
                .send()
                .await
                .unwrap();
            assert_eq!(
                code_of(cross).await,
                (403, "cross_site".into()),
                "{addr} {method} {path}"
            );
        }
    }
    collector.stop().await;
}

/// Enrollment and the host WebSocket are authenticated otherwise and are
/// exempt from the browser rules (kernel spec §3.3): no cookie, any origin.
/// On every listener (kernel spec §7, §11): the same router is cloned onto
/// each, and these two routes are the ones the layer-order comment at the
/// top of this file warns about escaping the browser rules altogether.
#[tokio::test]
async fn enrollment_and_the_host_socket_need_neither_a_session_nor_an_origin() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let plain = reqwest::Client::new();
    for &addr in &collector.addrs {
        let enroll = request(&plain, addr, "POST", "/api/hosts/enroll")
            .header("origin", "https://evil.example")
            .json(&serde_json::json!({
                "code": "0000-0000", "public_key": host_key().public_key_hex(),
                "name": "x", "host_version": "x", "platform": "x"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(enroll).await, (401, "invalid_code".into()), "{addr}");
        let (mut ws, nonce) = connect(addr).await;
        let proof = host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION);
        assert!(
            matches!(hello(&mut ws, HOST, proof).await, CollectorFrame::HelloAck { .. }),
            "{addr}"
        );
    }
    collector.stop().await;
}

/// `/healthz` and `/readyz` (kernel spec §3.3, §8) answer on every listener
/// without a session, from any origin, and say nothing but a fixed word:
/// no cookie, no data.
#[tokio::test]
async fn the_health_checks_are_exempt_and_carry_no_data() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let plain = reqwest::Client::new();
    for &addr in &collector.addrs {
        for (path, word) in [("/healthz", "ok"), ("/readyz", "ready")] {
            let resp = request(&plain, addr, "GET", path)
                .header("origin", "https://evil.example")
                .header("sec-fetch-site", "cross-site")
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 200, "{addr} {path}");
            assert!(resp.headers().get("set-cookie").is_none(), "{addr} {path}");
            assert_eq!(resp.text().await.unwrap(), word, "{addr} {path}");
            let post = request(&plain, addr, "POST", path).send().await.unwrap();
            assert_eq!(post.status(), 405, "{addr} POST {path}");
        }
    }
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
    let fresh = collector.session_at(now);
    let resp = reqwest::Client::new()
        .get(&url)
        .header("cookie", format!("hennery_session={fresh}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("set-cookie").is_none());

    let stale = collector.session_at(now - 120);
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
    let real = collector.session_at(now);
    let resp = reqwest::Client::new()
        .get(&url)
        .header("cookie", format!("hennery_session={tossed}; hennery_session={real}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let stale = collector.session_at(now - 120);
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
    let (mut ws, nonce) = connect(collector.addr).await;
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
    let (_first, first_nonce) = connect(collector.addr).await;
    let (mut second, second_nonce) = connect(collector.addr).await;
    assert_ne!(first_nonce, second_nonce);
    // Replaying the first connection's proof on the second is refused.
    let replayed = host_key().sign_hello(&first_nonce, HOST, PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut second, HOST, replayed).await), "bad_proof");

    let (mut third, nonce) = connect(collector.addr).await;
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
    let (mut ws, nonce) = connect(collector.addr).await;
    let proof = host_key().sign_hello(&nonce, "host-9", PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut ws, "host-9", proof).await), "bad_proof");
    collector.stop().await;
}

#[tokio::test]
async fn a_revoked_host_is_told_so_but_only_with_a_valid_proof() {
    let collector = Collector::start().await;
    collector.state.hosts.revoke(HOST, 1).unwrap();

    let (mut ws, nonce) = connect(collector.addr).await;
    let forged = HostKey::from_seed([2; 32]).sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut ws, HOST, forged).await), "bad_proof");

    let (mut ws, nonce) = connect(collector.addr).await;
    let proof = host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut ws, HOST, proof).await), "revoked");
    assert!(collector.connected().is_empty());
    collector.stop().await;
}

/// Every JSON body is read through `ApiJson`, whose rejections are fixed
/// `ApiError`s, never through axum's `Json`, whose rejections are plain
/// text with serde's message. A new handler that takes `Json` fails here.
#[test]
fn every_json_body_is_read_through_api_json() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut checked = 0;
    let mut found = Vec::new();
    for member in ["hennery-kernel", "hennery-sessions", "hennery"] {
        let mut dirs = vec![crates.join(member).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let name = path.strip_prefix(crates).unwrap().to_string_lossy().into_owned();
                checked += 1;
                let source = std::fs::read_to_string(&path).unwrap();
                let takes_json: Vec<String> = source
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| {
                        [": Json<", ": axum::Json<", "<Json<", "<axum::Json<"]
                            .iter()
                            .any(|p| line.contains(p))
                    })
                    .map(|(n, line)| format!("{name}:{}: {}", n + 1, line.trim()))
                    .collect();
                found.extend(takes_json);
            }
        }
    }
    assert!(checked > 20, "checked only {checked} files");
    assert!(found.is_empty(), "take ApiJson, not Json:\n{}", found.join("\n"));
}

/// Kernel spec §7.2: every HTML response carries the
/// `Content-Security-Policy`, whichever route sends it. Every `GET` in the
/// route tables is fetched signed in, and each HTML answer is checked; the
/// policy comes from one layer over the whole router, not from each page.
#[tokio::test]
async fn every_html_response_carries_the_content_security_policy() {
    let collector = Collector::start().await;
    let signed_in = hennery_testkit::operator_client(&collector.state.operator);
    let mut html = Vec::new();
    for &(method, path) in OPERATOR_ROUTES.iter().chain(EXEMPT_ROUTES) {
        if method != "GET" || path.starts_with("/api/stream/") {
            continue;
        }
        let resp = request(&signed_in, collector.addr, method, path).send().await.unwrap();
        let is_html = resp
            .headers()
            .get("content-type")
            .is_some_and(|t| t.to_str().unwrap().starts_with("text/html"));
        if is_html {
            let csp = resp
                .headers()
                .get("content-security-policy")
                .map(|v| v.to_str().unwrap().to_string());
            assert_eq!(csp.as_deref(), Some(hennery_kernel::csp::POLICY), "{path}");
            html.push(path);
        }
    }
    // Not vacuous: the pages the tables name are HTML.
    assert!(html.contains(&"/") && html.contains(&"/setup"), "{html:?}");
    collector.stop().await;
}
