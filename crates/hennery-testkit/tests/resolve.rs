//! Resolving paths through their host (kernel spec §5.2, §5.4; plan 5b):
//! `POST /api/hats/resolve` and the path rules a connected host resolves.
//! A scripted host plays the wire frame by frame, so its answers can be
//! wrong on purpose; a real host resolves real symlinks.

use futures::{SinkExt, StreamExt};
use hennery_host::HostConfig;
use hennery_host::identity::HostKey;
use hennery_kernel::hats::{HatChange, NewRule};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, CollectorFrame, HostFrame};
use hennery_proto::rest::{HatResolution, PathRuleItem};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::{AppState, store::Store};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const HOST: &str = "host-1";

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
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn hat(&self, name: &str) -> String {
        match self.state.hosts.create_hat(name, None, 1).unwrap() {
            HatChange::Done(hat) => hat.id,
            other => panic!("{other:?}"),
        }
    }

    async fn ready(&self) {
        wait_for("host ready", || async { self.state.hub.is_ready(HOST).then_some(()) }).await;
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The test playing the host.
struct ScriptedHost {
    ws: Ws,
}

impl ScriptedHost {
    /// `hello` and `resend_complete`, then wait until the host is ready.
    async fn connect(collector: &Collector) -> Self {
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
            capabilities: Capabilities::default(),
            attached_sessions: vec![],
        })
        .await;
        assert!(matches!(host.next().await, CollectorFrame::HelloAck { .. }));
        host.send(&HostFrame::ResendComplete).await;
        collector.ready().await;
        host
    }

    async fn send(&mut self, frame: &HostFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

    async fn next(&mut self) -> CollectorFrame {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                    Some(Ok(_)) => {}
                    other => panic!("collector connection ended: {other:?}"),
                }
            }
        })
        .await
        .expect("a collector frame within 10s")
    }

    /// The next `resolve_path`: its request id and path.
    async fn resolve_request(&mut self) -> (String, String) {
        match self.next().await {
            CollectorFrame::ResolvePath { request_id, path } => (request_id, path),
            other => panic!("expected resolve_path, got {other:?}"),
        }
    }

    /// Answer the next `resolve_path` with `canonical`, existing as a
    /// directory or not at all.
    async fn answer(&mut self, canonical: &str, exists: bool) -> String {
        let (request_id, path) = self.resolve_request().await;
        self.send(&HostFrame::ResolvedPath {
            request_id,
            canonical: canonical.into(),
            exists,
            is_dir: exists,
        })
        .await;
        path
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

/// A request from the owner's client, bounded so a hang fails the test.
fn send(collector: &Collector, method: &str, path: &str, body: Value) -> tokio::task::JoinHandle<(u16, Value)> {
    let c = hennery_testkit::operator_client(&collector.state.operator);
    let url = collector.url(path);
    let method: reqwest::Method = method.parse().unwrap();
    tokio::spawn(async move {
        let resp = c
            .request(method, url)
            .json(&body)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .unwrap();
        (resp.status().as_u16(), resp.json().await.unwrap_or(Value::Null))
    })
}

fn resolve(collector: &Collector, path: &str) -> tokio::task::JoinHandle<(u16, Value)> {
    send(
        collector,
        "POST",
        "/api/hats/resolve",
        json!({ "host_id": HOST, "path": path }),
    )
}

/// Kernel spec §5.4, §8: the path as the host resolved it, and the hat it
/// resolves to there, with the deciding rule.
#[tokio::test]
async fn a_path_resolves_through_its_host_to_a_hat() {
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    let stored = collector
        .state
        .hosts
        .replace_path_rules(
            HOST,
            &[NewRule {
                prefix: "/home/me/acme".into(),
                hat_id: acme.clone(),
                verified: true,
            }],
        )
        .unwrap();
    let rule_id = match stored {
        hennery_kernel::hats::RulesChange::Done(rules) => rules[0].id.clone(),
        other => panic!("{other:?}"),
    };
    let mut host = ScriptedHost::connect(&collector).await;

    let call = resolve(&collector, "~/acme/x");
    assert_eq!(host.answer("/home/me/acme/x", true).await, "~/acme/x");
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 200, "{body}");
    let got: HatResolution = serde_json::from_value(body).unwrap();
    assert_eq!(
        got,
        HatResolution {
            canonical: "/home/me/acme/x".into(),
            exists: true,
            is_dir: true,
            hat_id: acme,
            rule_id: Some(rule_id),
        }
    );

    // A sibling sharing the prefix's text is the host's default hat.
    let call = resolve(&collector, "/home/me/acme-infra");
    host.answer("/home/me/acme-infra", false).await;
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 200, "{body}");
    let got: HatResolution = serde_json::from_value(body).unwrap();
    let default = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
    assert_eq!((got.exists, got.hat_id, got.rule_id), (false, default, None));
}

/// The host is authenticated, but its answers are its own words (plan 5b
/// decision 2): a path not in canonical form is matched against nothing.
#[tokio::test]
async fn an_answer_not_in_canonical_form_is_refused() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector).await;
    for bad in [
        "/home/me/acme/",
        "/home/me/../acme",
        "home/me",
        "/home//me",
        "/a\u{0}b",
        "",
    ] {
        let call = resolve(&collector, "/home/me/acme");
        host.answer(bad, true).await;
        let (status, body) = call.await.unwrap();
        assert_eq!(
            (status, body["code"].as_str()),
            (502, Some("bad_host_answer")),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn a_refusal_an_offline_host_and_a_lost_connection_each_answer_plainly() {
    let collector = Collector::start().await;
    let (status, body) = resolve(&collector, "/p").await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));

    let mut host = ScriptedHost::connect(&collector).await;
    let call = resolve(&collector, "relative");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "invalid".into(),
        message: "a path must be absolute".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (400, Some("invalid"), Some("a path must be absolute"))
    );
    // A code the host has no business choosing is not passed on (the
    // review's P5): it could make a client prompt for a step-up.
    let call = resolve(&collector, "/p");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "step_up_required".into(),
        message: "no".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (502, Some("host_refused")));

    let call = resolve(&collector, "/p");
    host.resolve_request().await;
    drop(host);
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (503, Some("delivery_unknown")));
    assert_eq!(collector.state.hub.pending_requests(), 0);
}

/// Kernel spec §5.2: a rule's prefix is resolved through its host when it
/// is saved, verified if it exists there (plan 5b decision 3). Two typed
/// prefixes that resolve to one are refused as a set.
#[tokio::test]
async fn a_connected_host_resolves_and_verifies_rule_prefixes() {
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    let mut host = ScriptedHost::connect(&collector).await;
    let rules =
        |a: &str, b: &str| json!({ "rules": [{ "prefix": a, "hat_id": acme }, { "prefix": b, "hat_id": acme }] });

    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        rules("~/acme", "/tmp/x/../acme/new"),
    );
    // Resolved in the order given; several may be out at once.
    let mut answered = Vec::new();
    for _ in 0..2 {
        let (request_id, path) = host.resolve_request().await;
        let (canonical, exists) = if path == "~/acme" {
            ("/home/me/acme", true)
        } else {
            ("/private/tmp/acme/new", false)
        };
        answered.push(path);
        host.send(&HostFrame::ResolvedPath {
            request_id,
            canonical: canonical.into(),
            exists,
            is_dir: exists,
        })
        .await;
    }
    answered.sort();
    assert_eq!(answered, ["/tmp/x/../acme/new", "~/acme"]);
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 200, "{body}");
    let stored: Vec<PathRuleItem> = serde_json::from_value(body).unwrap();
    let shown: Vec<(&str, bool)> = stored.iter().map(|r| (r.prefix.as_str(), r.verified)).collect();
    assert_eq!(shown, [("/private/tmp/acme/new", false), ("/home/me/acme", true)]);

    // `/p/Acme` and `/p/acme` on a case-insensitive volume: one directory.
    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        rules("/p/Acme", "/p/acme"),
    );
    host.answer("/p/Acme", true).await;
    host.answer("/p/Acme", true).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(collector.state.hosts.path_rules(HOST).unwrap().unwrap().len(), 2);
    assert_eq!(collector.state.hub.pending_requests(), 0);

    // A refused prefix stores nothing.
    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        rules("/p", "~other/p"),
    );
    for _ in 0..2 {
        let (request_id, path) = host.resolve_request().await;
        let frame = if path == "/p" {
            HostFrame::ResolvedPath {
                request_id,
                canonical: "/p".into(),
                exists: true,
                is_dir: true,
            }
        } else {
            HostFrame::Error {
                request_id,
                code: "invalid".into(),
                message: "only ~ and ~/… name a home directory here".into(),
            }
        };
        host.send(&frame).await;
    }
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(collector.state.hosts.path_rules(HOST).unwrap().unwrap().len(), 2);
    assert_eq!(collector.state.hub.pending_requests(), 0);

    // Over the limit, or naming a hat that is not the owner's: refused
    // before any prefix reaches the host (the review's Important 1).
    let over_limit: Vec<Value> = (0..257)
        .map(|i| json!({ "prefix": format!("/p/r{i}"), "hat_id": acme }))
        .collect();
    let (status, body) = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": over_limit }),
    )
    .await
    .unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(collector.state.hub.pending_requests(), 0);

    let (status, body) = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": [{ "prefix": "/p/new", "hat_id": "hat-nope" }] }),
    )
    .await
    .unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(collector.state.hub.pending_requests(), 0);

    // Nothing was queued for the host by either refusal: the next frame it
    // gets is a later resolve's.
    let call = resolve(&collector, "/p");
    let (request_id, path) = host.resolve_request().await;
    assert_eq!(path, "/p");
    host.send(&HostFrame::ResolvedPath {
        request_id,
        canonical: "/p".into(),
        exists: true,
        is_dir: true,
    })
    .await;
    let (status, _) = call.await.unwrap();
    assert_eq!(status, 200);
}

/// A real host resolves symlinks and `~` (kernel spec §5.2: canonical on
/// the host), for the resolver and for rules alike.
#[tokio::test]
async fn a_real_host_resolves_symlinks_for_the_resolver_and_for_rules() {
    let collector = Collector::start().await;
    let data = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tree.path().join("real/acme/x")).unwrap();
    std::os::unix::fs::symlink(tree.path().join("real"), tree.path().join("link")).unwrap();
    let real = std::fs::canonicalize(tree.path().join("real")).unwrap();
    let real = real.to_str().unwrap();
    let cfg = HostConfig::new(
        format!("ws://{}/api/hosts/ws", collector.addr),
        HOST,
        host_key(),
        data.path().to_path_buf(),
    );
    let host = tokio::spawn(async move { hennery_host::run(cfg).await });
    collector.ready().await;
    let acme = collector.hat("Acme");
    let link = tree.path().join("link");
    let link = link.to_str().unwrap();

    let (status, body) = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": [
            { "prefix": format!("{link}/acme/"), "hat_id": acme },
            { "prefix": format!("{link}/later"), "hat_id": acme },
        ] }),
    )
    .await
    .unwrap();
    assert_eq!(status, 200, "{body}");
    let stored: Vec<PathRuleItem> = serde_json::from_value(body).unwrap();
    let shown: Vec<(String, bool)> = stored.into_iter().map(|r| (r.prefix, r.verified)).collect();
    assert_eq!(
        shown,
        [(format!("{real}/later"), false), (format!("{real}/acme"), true)]
    );

    let (status, body) = resolve(&collector, &format!("{link}/acme/./x")).await.unwrap();
    assert_eq!(status, 200, "{body}");
    let got: HatResolution = serde_json::from_value(body).unwrap();
    assert_eq!(
        (got.canonical, got.exists, got.is_dir, got.hat_id),
        (format!("{real}/acme/x"), true, true, acme)
    );
    let (status, body) = resolve(&collector, "not/absolute").await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    host.abort();
}
