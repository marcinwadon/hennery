//! The project picker (ACP core §3.3, §7, §9; plan 6c) against a scripted
//! host over a real WebSocket: the test plays the host, so it controls
//! every probe reply, its timing and its connection.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, DirEntry, HostFrame, Project};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::hub::RequestError;
use hennery_sessions::projects::ProjectsCache;
use hennery_sessions::{AppState, store::Store};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
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
        Self::start_with(|_| {}).await
    }

    /// A collector whose state `tune` adjusts before it serves (the
    /// cache's lifetime, the probe timeout).
    async fn start_with(tune: impl FnOnce(&mut AppState)) -> Self {
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
        let mut state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        tune(&mut state);
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

    /// `hello` with `workspace_roots` and nothing attached, and its
    /// `hello_ack`, but no `resend_complete`: what `hennery host join`'s
    /// probe sends. Then the connection is closed.
    async fn hello_only(collector: &Collector, workspace_roots: Vec<String>) {
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
            capabilities: Capabilities(vec![Capability::Projects]),
            workspace_roots,
            attached_sessions: vec![],
        })
        .await;
        assert!(matches!(host.next().await, CollectorFrame::HelloAck { .. }));
        host.ws.close(None).await.unwrap();
    }

    /// Drop the connection and wait until the collector has noticed.
    async fn drop_connection(self, collector: &Collector) {
        drop(self.ws);
        wait_for("host gone", || async {
            (!collector.state.hub.is_ready(HOST)).then_some(())
        })
        .await;
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

fn client(collector: &Collector) -> reqwest::Client {
    hennery_testkit::operator_client(&collector.state.operator)
}

/// `GET path` on the collector: the status and the JSON body.
async fn get(collector: &Collector, path: &str) -> (u16, serde_json::Value) {
    let resp = client(collector)
        .get(format!("http://{}{path}", collector.addr))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(serde_json::Value::Null))
}

async fn listed_roots(collector: &Collector) -> serde_json::Value {
    let (status, hosts) = get(collector, "/api/hosts").await;
    assert_eq!(status, 200, "{hosts}");
    hosts[0]["workspace_roots"].clone()
}

// Task 4: the roots a reconciled connection reported (decision 7).

#[tokio::test]
async fn a_reconciled_connections_roots_are_stored_and_listed() {
    let collector = Collector::start().await;
    assert_eq!(listed_roots(&collector).await, serde_json::json!([]));
    let roots = vec!["/srv/projects".to_string(), "relative".into(), "/lap\u{202E}top".into()];
    let host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Projects]), roots).await;
    assert_eq!(listed_roots(&collector).await, serde_json::json!(["/srv/projects"]));
    host.drop_connection(&collector).await;
    // `host join`'s probe: a hello that is never reconciled changes nothing.
    ScriptedHost::hello_only(&collector, vec![]).await;
    assert_eq!(listed_roots(&collector).await, serde_json::json!(["/srv/projects"]));
    // The next reconciled connection's report replaces them.
    let _host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Projects]), vec!["/p".into()]).await;
    assert_eq!(listed_roots(&collector).await, serde_json::json!(["/p"]));
}

// Task 5: the routes and the cache (decisions 2, 3, 10).

/// `GET` in a task of its own, so the test can play the host meanwhile.
fn get_later(collector: &Collector, path: &str) -> tokio::task::JoinHandle<(u16, serde_json::Value)> {
    let (c, url) = (client(collector), format!("http://{}{path}", collector.addr));
    tokio::spawn(async move {
        let resp = c.get(url).timeout(Duration::from_secs(20)).send().await.unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(serde_json::Value::Null))
    })
}

/// `GET /api/hosts/{HOST}/projects`, answered by the host with `paths`.
async fn listed_projects(collector: &Collector, host: &mut ScriptedHost, paths: &[&str]) -> (u16, serde_json::Value) {
    let call = get_later(collector, &format!("/api/hosts/{HOST}/projects"));
    let request_id = listed(host).await;
    host.send(&projects(request_id, paths)).await;
    call.await.unwrap()
}

#[tokio::test]
async fn projects_come_from_the_host_and_are_cached_for_its_connection() {
    let collector = Collector::start_with(|state| {
        state.projects = Arc::new(ProjectsCache::new(Duration::from_millis(500)));
    })
    .await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let (status, body) = listed_projects(&collector, &mut host, &["/p/a"]).await;
    assert_eq!(
        (status, body),
        (200, json!({"items": [{"path": "/p/a"}], "partial": false}))
    );
    // From the cache: nothing is sent.
    let (status, body) = get(&collector, &format!("/api/hosts/{HOST}/projects")).await;
    assert_eq!((status, &body["items"]), (200, &json!([{"path": "/p/a"}])));
    host.hears_nothing(Duration::from_millis(100)).await;
    // Past its lifetime it is asked again.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, body) = listed_projects(&collector, &mut host, &["/p/b"]).await;
    assert_eq!(body["items"], json!([{"path": "/p/b"}]));
    // A new connection is asked again, at once.
    host.drop_connection(&collector).await;
    let (status, body) = get(&collector, &format!("/api/hosts/{HOST}/projects")).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")), "{body}");
    let mut host = ScriptedHost::with_projects(&collector).await;
    let (_, body) = listed_projects(&collector, &mut host, &["/p/c"]).await;
    assert_eq!(body["items"], json!([{"path": "/p/c"}]));
}

#[tokio::test]
async fn the_answers_are_never_stored_by_the_browser() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let (c, url) = (
        client(&collector),
        format!("http://{}/api/hosts/{HOST}/projects", collector.addr),
    );
    let call = tokio::spawn(async move { c.get(url).send().await.unwrap() });
    let request_id = listed(&mut host).await;
    host.send(&projects(request_id, &[])).await;
    let resp = call.await.unwrap();
    assert_eq!(resp.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn nothing_is_sent_to_an_unknown_offline_or_unable_host() {
    let collector = Collector::start().await;
    for path in ["/api/hosts/host-x/projects", "/api/hosts/host-x/browse?path=/p"] {
        let (status, body) = get(&collector, path).await;
        assert_eq!((status, body["code"].as_str()), (404, Some("not_found")), "{path}");
    }
    for path in [
        format!("/api/hosts/{HOST}/projects"),
        format!("/api/hosts/{HOST}/browse?path=/p"),
    ] {
        let (status, body) = get(&collector, &path).await;
        assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")), "{path}");
    }
    let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Park]), vec![]).await;
    for path in [
        format!("/api/hosts/{HOST}/projects"),
        format!("/api/hosts/{HOST}/browse?path=/p"),
    ] {
        let (status, body) = get(&collector, &path).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (409, Some("projects_unsupported")),
            "{path}"
        );
    }
    host.hears_nothing(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn browse_asks_the_host_for_the_path_and_answers_its_listing() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let call = get_later(&collector, &format!("/api/hosts/{HOST}/browse?path=/p%20q"));
    let CollectorFrame::BrowseDirectory { request_id, path } = host.next().await else {
        panic!("expected browse_directory");
    };
    assert_eq!(path, "/p q");
    host.send(&HostFrame::Directory {
        request_id,
        path: "/p q".into(),
        parent: Some("/".into()),
        entries: vec![DirEntry {
            name: "repo".into(),
            git: true,
        }],
        truncated: false,
    })
    .await;
    assert_eq!(
        call.await.unwrap(),
        (
            200,
            json!({"path": "/p q", "parent": "/", "entries": [{"name": "repo", "git": true}], "truncated": false})
        )
    );
}

#[tokio::test]
async fn browse_checks_the_path_before_asking() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let long = format!("/{}", "x".repeat(4096));
    for query in ["", "?path=", "?path=relative", "?path=/a%0Ab", &format!("?path={long}")] {
        let (status, body) = get(&collector, &format!("/api/hosts/{HOST}/browse{query}")).await;
        assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{query:?}");
    }
    host.hears_nothing(Duration::from_millis(200)).await;
}

/// Decision 3, the review's A6: a known refusal keeps its code, with the
/// collector's own message; any other is a bad reply.
#[tokio::test]
async fn a_hosts_refusal_is_answered_with_its_status() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    for (code, status, answered) in [
        ("invalid", 400, "invalid"),
        ("not_a_directory", 400, "not_a_directory"),
        ("outside_workspace", 403, "outside_workspace"),
        ("permission_denied", 403, "permission_denied"),
        ("path_not_found", 404, "path_not_found"),
        ("unreadable", 502, "unreadable"),
        ("busy", 503, "busy"),
        ("teleported", 502, "bad_reply"),
        ("<script>", 502, "bad_reply"),
    ] {
        let call = get_later(&collector, &format!("/api/hosts/{HOST}/browse?path=/p"));
        let CollectorFrame::BrowseDirectory { request_id, .. } = host.next().await else {
            panic!("expected browse_directory");
        };
        host.send(&HostFrame::Error {
            request_id,
            code: code.into(),
            message: "\u{202E}from the host".into(),
        })
        .await;
        let (got, body) = call.await.unwrap();
        assert_eq!((got, body["code"].as_str()), (status, Some(answered)), "{code}");
        assert!(!body["message"].as_str().unwrap().contains("from the host"), "{body}");
    }
}

/// The review's A6: what a host sends back is checked before it is shown.
#[tokio::test]
async fn a_hosts_reply_is_checked_before_it_is_shown() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let call = get_later(&collector, &format!("/api/hosts/{HOST}/projects"));
    let request_id = listed(&mut host).await;
    host.send(&HostFrame::Projects {
        request_id,
        items: ["/ok", "relative", "/hid\u{200B}den", "/new\nline"]
            .iter()
            .map(|p| Project { path: p.to_string() })
            .collect(),
        partial: false,
        home: Some("not absolute".into()),
    })
    .await;
    assert_eq!(
        call.await.unwrap(),
        (200, json!({"items": [{"path": "/ok"}], "partial": false}))
    );

    let browse = |path: &str| get_later(&collector, &format!("/api/hosts/{HOST}/browse?path={path}"));
    let call = browse("/p");
    let CollectorFrame::BrowseDirectory { request_id, .. } = host.next().await else {
        panic!("expected browse_directory");
    };
    let entry = |name: &str| DirEntry {
        name: name.into(),
        git: false,
    };
    host.send(&HostFrame::Directory {
        request_id,
        path: "/p".into(),
        parent: Some("..".into()),
        entries: vec![
            entry("ok"),
            entry("a/b"),
            entry(".."),
            entry(""),
            entry("bell\u{7}"),
            entry(&"n".repeat(256)),
        ],
        truncated: false,
    })
    .await;
    assert_eq!(
        call.await.unwrap(),
        (
            200,
            json!({"path": "/p", "entries": [{"name": "ok", "git": false}], "truncated": false})
        )
    );

    // A listing of a path that cannot be shown, or the wrong kind of reply.
    let call = browse("/p");
    let CollectorFrame::BrowseDirectory { request_id, .. } = host.next().await else {
        panic!("expected browse_directory");
    };
    host.send(&HostFrame::Directory {
        request_id,
        path: "relative".into(),
        parent: None,
        entries: vec![],
        truncated: false,
    })
    .await;
    assert_eq!(call.await.unwrap().1["code"], "bad_reply");
    let call = browse("/p");
    let CollectorFrame::BrowseDirectory { request_id, .. } = host.next().await else {
        panic!("expected browse_directory");
    };
    host.send(&projects(request_id, &["/p"])).await;
    assert_eq!(call.await.unwrap().1["code"], "bad_reply");
}

#[tokio::test]
async fn a_host_that_does_not_answer_is_no_answer_and_keeps_its_connection() {
    let collector = Collector::start_with(|state| state.probe_timeout = Duration::from_millis(300)).await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let call = get_later(&collector, &format!("/api/hosts/{HOST}/projects"));
    let late = listed(&mut host).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (503, Some("no_answer")), "{body}");
    assert!(collector.state.hub.is_ready(HOST));
    // Its late answer is not cached either.
    host.send(&projects(late, &["/late"])).await;
    let (_, body) = listed_projects(&collector, &mut host, &["/p/a"]).await;
    assert_eq!(body["items"], json!([{"path": "/p/a"}]));
}
