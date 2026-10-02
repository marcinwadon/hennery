//! Resolving paths through their host (kernel spec §5.2, §5.4; plans 5b
//! and 5c): `POST /api/hats/resolve`, the path rules a connected host
//! resolves, and the hat a session gets at its start and keeps at its
//! resume.
//! A scripted host plays the wire frame by frame, so its answers can be
//! wrong on purpose; a real host resolves real symlinks.

use futures::{SinkExt, StreamExt};
use hennery_host::HostConfig;
use hennery_host::identity::HostKey;
use hennery_kernel::hats::{HatChange, NewRule};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame};
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
    /// The last seq sent, per session.
    seqs: std::collections::HashMap<String, u64>,
}

impl ScriptedHost {
    /// `hello` and `resend_complete`, then wait until the host is ready.
    /// Announces `resolve_path` (every test here resolves one).
    async fn connect(collector: &Collector) -> Self {
        Self::connect_with(collector, Capabilities(vec![Capability::ResolvePath])).await
    }

    /// `connect` announcing `capabilities`.
    async fn connect_with(collector: &Collector, capabilities: Capabilities) -> Self {
        Self::connect_announcing(collector, capabilities, Default::default()).await
    }

    /// `connect_with`, announcing `mcp_isolation` too (plan 8c).
    async fn connect_announcing(
        collector: &Collector,
        capabilities: Capabilities,
        mcp_isolation: hennery_proto::frames::AgentIsolation,
    ) -> Self {
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self {
            ws,
            seqs: Default::default(),
        };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities,
            workspace_roots: vec![],
            attached_sessions: vec![],
            mcp_isolation,
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

    /// The next collector frame.
    async fn next_any(&mut self) -> CollectorFrame {
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

    /// The next collector frame that is not an `ack`: the sessions this
    /// host runs are acked (plan 5c).
    async fn next(&mut self) -> CollectorFrame {
        loop {
            match self.next_any().await {
                CollectorFrame::Ack { .. } => {}
                frame => return frame,
            }
        }
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
    // review's P5): it could make a client prompt for a step-up. Nor is
    // its text: the collector's own message is shown instead (Minor 4).
    let call = resolve(&collector, "/p");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "step_up_required".into(),
        message: "no".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (502, Some("host_refused"), Some("the host refused the request"))
    );

    // An `invalid` refusal whose text cannot be shown (the review's Minor
    // 4) is answered a fixed message instead, not the host's bytes.
    let call = resolve(&collector, "/p");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "invalid".into(),
        message: "x".repeat(257),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (400, Some("invalid"), Some("the host refused the path"))
    );

    let call = resolve(&collector, "/p");
    host.resolve_request().await;
    drop(host);
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (503, Some("no_answer")));
    assert_eq!(collector.state.hub.pending_probes(), 0);
}

/// The review's Minor 1: the host's own `busy` is a probe's rejection,
/// answered as the probes' own is (503 `busy`), not passed on verbatim.
#[tokio::test]
async fn a_hosts_own_busy_answers_503_busy() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector).await;
    let call = resolve(&collector, "/p");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "busy".into(),
        message: "the host is busy resolving another path".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (503, Some("busy")));
    assert_eq!(collector.state.hub.pending_probes(), 0);
}

/// Final review I1 (ACP core §3.3): the collector never sends a frame
/// that needs a capability to a host that lacks it. A host connected
/// without `resolve_path` is refused plainly, for the resolver and for
/// rules alike, and nothing is sent its way.
#[tokio::test]
async fn a_host_without_resolve_path_is_refused_without_a_frame() {
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    let mut host = ScriptedHost::connect_with(&collector, Capabilities::default()).await;

    let call = resolve(&collector, "/p");
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("resolve_unsupported")));

    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": [{ "prefix": "/p", "hat_id": acme }] }),
    );
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("resolve_unsupported")));
    assert_eq!(collector.state.hub.pending_probes(), 0);

    // Neither refusal queued the host anything: a frame sent now is the
    // first thing it sees.
    assert!(collector.state.hub.notify(
        HOST,
        CollectorFrame::Ack {
            session_id: "s1".into(),
            ack_seq: 0,
        },
    ));
    assert!(matches!(host.next_any().await, CollectorFrame::Ack { .. }));
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
    assert_eq!(collector.state.hub.pending_probes(), 0);

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
    assert_eq!(collector.state.hub.pending_probes(), 0);

    // A prefix that exists as a file, not a directory, is refused (final
    // review M3): a session's cwd can never be one.
    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": [{ "prefix": "/p/file", "hat_id": acme }] }),
    );
    let (request_id, path) = host.resolve_request().await;
    assert_eq!(path, "/p/file");
    host.send(&HostFrame::ResolvedPath {
        request_id,
        canonical: "/p/file".into(),
        exists: true,
        is_dir: false,
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (400, Some("invalid"), Some("/p/file is not a directory on that host"))
    );
    assert_eq!(collector.state.hosts.path_rules(HOST).unwrap().unwrap().len(), 2);
    assert_eq!(collector.state.hub.pending_probes(), 0);

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
    assert_eq!(collector.state.hub.pending_probes(), 0);

    let (status, body) = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": [{ "prefix": "/p/new", "hat_id": "hat-nope" }] }),
    )
    .await
    .unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(collector.state.hub.pending_probes(), 0);

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

/// How many sessions the store holds, read from the file.
fn session_count(collector: &Collector) -> i64 {
    let conn = rusqlite::Connection::open(collector._dir.path().join("hennery.db")).unwrap();
    conn.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap()
}

fn start(collector: &Collector, cwd: &str) -> tokio::task::JoinHandle<(u16, Value)> {
    send(
        collector,
        "POST",
        "/api/sessions",
        json!({ "host_id": HOST, "agent": "fake", "cwd": cwd, "hat_id": "hat-chosen-by-the-client" }),
    )
}

fn rule(collector: &Collector, prefix: &str, hat_id: &str, verified: bool) {
    let stored = collector
        .state
        .hosts
        .replace_path_rules(
            HOST,
            &[NewRule {
                prefix: prefix.into(),
                hat_id: hat_id.into(),
                verified,
            }],
        )
        .unwrap();
    assert!(
        matches!(stored, hennery_kernel::hats::RulesChange::Done(_)),
        "{stored:?}"
    );
}

impl ScriptedHost {
    /// Answer the next `start_session` or `resume_session` with
    /// `session_started`: its session id and cwd.
    async fn started(&mut self) -> (String, String) {
        let (request_id, session_id, cwd) = match self.next().await {
            CollectorFrame::StartSession {
                request_id,
                session_id,
                cwd,
                ..
            }
            | CollectorFrame::ResumeSession {
                request_id,
                session_id,
                cwd,
                ..
            } => (request_id, session_id, cwd),
            other => panic!("expected a start or resume, got {other:?}"),
        };
        let body = hennery_proto::frames::SessionBody::session_started(request_id, "agent-1");
        self.emit(&session_id, body).await;
        (session_id, cwd)
    }

    async fn parked(&mut self, session_id: &str) {
        let body = hennery_proto::frames::SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Idle,
        };
        self.emit(session_id, body).await;
    }

    /// Send the session's next sequenced frame.
    async fn emit(&mut self, session_id: &str, body: hennery_proto::frames::SessionBody) {
        let seq = self.seqs.entry(session_id.to_string()).or_insert(0);
        *seq += 1;
        let frame = HostFrame::Session {
            session_id: session_id.into(),
            seq: *seq,
            body,
        };
        self.send(&frame).await;
    }
}

/// Umbrella §8.2: the host resolves the typed path, the collector matches
/// the rules, the hat is stored with the canonical cwd, and only then does
/// the session start, in that cwd. Whatever hat the client names is not
/// asked for and changes nothing.
#[tokio::test]
async fn a_start_stores_the_canonical_cwd_and_the_hat_its_rules_give() {
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    rule(&collector, "/home/me/acme", &acme, true);
    let mut host = ScriptedHost::connect(&collector).await;

    let call = start(&collector, "~/acme/x/");
    assert_eq!(host.answer("/home/me/acme/x", true).await, "~/acme/x/");
    let (session, cwd) = host.started().await;
    assert_eq!(cwd, "/home/me/acme/x");
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 202, "{body}");
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!(
        (row.cwd.as_str(), row.hat_id.as_str()),
        ("/home/me/acme/x", acme.as_str())
    );
    let (status, detail) = send(&collector, "GET", &format!("/api/sessions/{session}"), Value::Null)
        .await
        .unwrap();
    assert_eq!((status, detail["hat_id"].as_str()), (200, Some(acme.as_str())));
    let conn = rusqlite::Connection::open(collector._dir.path().join("hennery.db")).unwrap();
    let rule_id: Option<String> = conn
        .query_row("SELECT hat_rule_id FROM sessions WHERE id = ?1", [&session], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(rule_id.is_some_and(|id| id.starts_with("rule-")));

    // No rule covers a sibling: the host's default hat, and no rule.
    let call = start(&collector, "/home/me/acme-infra");
    host.answer("/home/me/acme-infra", true).await;
    let (session, _) = host.started().await;
    assert_eq!(call.await.unwrap().0, 202);
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    let default = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
    assert_eq!(row.hat_id, default);
}

/// Plan 5c decision 2: a start is refused before any session exists when
/// its host is away, its cwd is not a directory there, or its hat is in
/// doubt (decision 3).
#[tokio::test]
async fn a_start_that_cannot_resolve_its_hat_creates_no_session() {
    let collector = Collector::start().await;
    let (status, body) = start(&collector, "/p").await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));

    let mut host = ScriptedHost::connect(&collector).await;
    let call = start(&collector, "/p/file");
    host.answer("/p/file", false).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid_cwd")), "{body}");

    // A rule whose case the directory no longer has: saved before the
    // directory was made (unverified), or verified and renamed since (the
    // review's B2). The hat tester agrees with the start (P1).
    let acme = collector.hat("Acme");
    for verified in [false, true] {
        rule(&collector, "/Users/me/acme", &acme, verified);
        let call = start(&collector, "/Users/me/Acme");
        host.answer("/Users/me/Acme", true).await;
        let (status, body) = call.await.unwrap();
        assert_eq!((status, body["code"].as_str()), (409, Some("hat_ambiguous")), "{body}");
        assert!(body["message"].as_str().unwrap().contains("/Users/me/acme"), "{body}");
        let call = resolve(&collector, "/Users/me/Acme");
        host.answer("/Users/me/Acme", true).await;
        let (status, body) = call.await.unwrap();
        assert_eq!((status, body["code"].as_str()), (409, Some("hat_ambiguous")), "{body}");
    }
    assert_eq!(session_count(&collector), 0);
}

/// ACP core §4.3: a resume re-resolves its cwd on the host and its hat.
/// Another hat refuses it (`hat_mismatch`, naming both); a cwd that now
/// resolves elsewhere refuses it (`cwd_moved`, plan 5c decision 4).
/// Nothing changes either way, and the same hat resumes.
#[tokio::test]
async fn a_resume_re_resolves_its_cwd_and_hat_and_refuses_a_change() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector).await;
    let call = start(&collector, "/home/me/acme");
    host.answer("/home/me/acme", true).await;
    let (session, _) = host.started().await;
    assert_eq!(call.await.unwrap().0, 202);
    host.parked(&session).await;
    wait_for("parked", || async {
        let row = collector.state.store.find_session(&session).unwrap().unwrap();
        (row.lifecycle == "parked").then_some(())
    })
    .await;
    let resume = || {
        send(
            &collector,
            "POST",
            &format!("/api/sessions/{session}/resume"),
            json!({}),
        )
    };

    let acme = collector.hat("Acme");
    rule(&collector, "/home/me", &acme, true);
    let call = resume();
    assert_eq!(host.answer("/home/me/acme", true).await, "/home/me/acme");
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("hat_mismatch")), "{body}");
    let message = body["message"].as_str().unwrap();
    assert!(
        message.contains("\"Personal\"") && message.contains("\"Acme\""),
        "{message}"
    );

    collector.state.hosts.replace_path_rules(HOST, &[]).unwrap();
    let call = resume();
    host.answer("/srv/elsewhere", true).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("cwd_moved")), "{body}");
    let call = resume();
    host.answer("/home/me/acme", false).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid_cwd")), "{body}");
    assert_eq!(
        collector.state.store.find_session(&session).unwrap().unwrap().lifecycle,
        "parked"
    );

    let call = resume();
    host.answer("/home/me/acme", true).await;
    let (_, cwd) = host.started().await;
    assert_eq!(cwd, "/home/me/acme");
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 202, "{body}");
}

/// ACP core §4.9: re-assigning a parked session moves it to another hat,
/// with a `hat_reassigned` event; its next resume must agree with the new
/// hat. A session in another lifecycle is refused, and so is a stale
/// step-up (`step_up.rs`).
#[tokio::test]
async fn a_reassigned_session_resumes_in_its_new_hat_once_the_rules_agree() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector).await;
    let call = start(&collector, "/home/me/acme");
    host.answer("/home/me/acme", true).await;
    let (session, _) = host.started().await;
    assert_eq!(call.await.unwrap().0, 202);
    let acme = collector.hat("Acme");
    // The session list, by hat: (session id, hat id).
    let listed = |hat: String| {
        let call = send(&collector, "GET", &format!("/api/sessions?hat={hat}"), json!({}));
        async move {
            let (status, body) = call.await.unwrap();
            assert_eq!(status, 200, "{body}");
            body["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| {
                    (
                        s["session_id"].as_str().unwrap().to_string(),
                        s["hat_id"].as_str().unwrap().to_string(),
                    )
                })
                .collect::<Vec<_>>()
        }
    };
    let default = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
    assert_eq!(listed(default.clone()).await, [(session.clone(), default.clone())]);
    assert!(listed(acme.clone()).await.is_empty());
    let patch = |hat: &str| {
        send(
            &collector,
            "PATCH",
            &format!("/api/sessions/{session}"),
            json!({ "hat_id": hat }),
        )
    };
    let (status, body) = patch(&acme).await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("active")), "{body}");

    host.parked(&session).await;
    wait_for("parked", || async {
        let row = collector.state.store.find_session(&session).unwrap().unwrap();
        (row.lifecycle == "parked").then_some(())
    })
    .await;
    let mut stream = collector.state.hub.subscribe();
    let (status, body) = patch(&acme).await.unwrap();
    assert_eq!((status, body["hat_id"].as_str()), (200, Some(acme.as_str())), "{body}");
    // Published on the session's stream, not only stored.
    let published = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = stream.recv().await.unwrap();
            if event.session_id == session && event.kind == "hat_reassigned" {
                return event.body;
            }
        }
    })
    .await
    .expect("hat_reassigned published within 10s");
    assert_eq!(published, json!({ "from": default, "to": acme }));
    let kinds: Vec<String> = collector
        .state
        .store
        .events(&session, 0, 100)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds.last().map(String::as_str), Some("hat_reassigned"));
    // The session list shows it under its new hat, and no longer the old.
    assert_eq!(listed(acme.clone()).await, [(session.clone(), acme.clone())]);
    assert!(listed(default).await.is_empty());

    // The path still resolves to the host's default hat: refused.
    let resume = || {
        send(
            &collector,
            "POST",
            &format!("/api/sessions/{session}/resume"),
            json!({}),
        )
    };
    let call = resume();
    host.answer("/home/me/acme", true).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("hat_mismatch")), "{body}");
    // Once a rule agrees, it resumes.
    rule(&collector, "/home/me/acme", &acme, true);
    let call = resume();
    host.answer("/home/me/acme", true).await;
    host.started().await;
    assert_eq!(call.await.unwrap().0, 202);
}

/// Plan 8c: a start and a resume carry the session's hat, and no servers
/// yet (minting is plan 8e's); the per-agent isolation a `hello` announces
/// reaches the hub, from the live connection.
#[tokio::test]
async fn starts_and_resumes_carry_the_sessions_hat_and_no_servers() {
    use hennery_proto::frames::{AgentIsolation, McpDelivery, McpIsolation, SessionBody};
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    rule(&collector, "/home/me/acme", &acme, true);
    let isolation = AgentIsolation([("fake".to_string(), McpIsolation::ClaudeStrict)].into_iter().collect());
    let capabilities = Capabilities(vec![Capability::ResolvePath, Capability::McpServers]);
    let mut host = ScriptedHost::connect_announcing(&collector, capabilities, isolation).await;
    assert_eq!(
        collector.state.hub.mcp_isolation(HOST, "fake"),
        Some((true, McpIsolation::ClaudeStrict))
    );

    let call = start(&collector, "/home/me/acme");
    host.answer("/home/me/acme", true).await;
    let CollectorFrame::StartSession {
        request_id,
        session_id,
        hat_id,
        mcp,
        ..
    } = host.next().await
    else {
        panic!("expected a start");
    };
    assert_eq!((hat_id.as_str(), &mcp), (acme.as_str(), &McpDelivery::default()));
    host.emit(&session_id, SessionBody::session_started(request_id, "agent-1"))
        .await;
    assert_eq!(call.await.unwrap().0, 202);
    host.parked(&session_id).await;
    wait_for("parked", || async {
        let row = collector.state.store.find_session(&session_id).unwrap().unwrap();
        (row.lifecycle == "parked").then_some(())
    })
    .await;

    let call = send(
        &collector,
        "POST",
        &format!("/api/sessions/{session_id}/resume"),
        json!({}),
    );
    host.answer("/home/me/acme", true).await;
    let CollectorFrame::ResumeSession {
        request_id,
        hat_id,
        mcp,
        ..
    } = host.next().await
    else {
        panic!("expected a resume");
    };
    assert_eq!((hat_id.as_str(), &mcp), (acme.as_str(), &McpDelivery::default()));
    host.emit(&session_id, SessionBody::session_started(request_id, "agent-1"))
        .await;
    assert_eq!(call.await.unwrap().0, 202);
}
