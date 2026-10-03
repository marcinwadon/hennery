//! Plan 8e end to end: a real collector with the gateway wired in as the
//! binary wires it (sessions mint and revoke through `GatewayMcp`; the
//! proxy watches the same `Revocations`), a real host, the fake ACP adapter
//! as a real child process, and a fake MCP upstream, over real sockets.
//!
//! A Claude session on a host with a mounted connection gets exactly its
//! hat's server with a working token; the token reaches only its hat's
//! connections; a park ends it, and the stream open on it; a resume brings
//! a new token, the old one staying dead. Codex on a mixed host gets
//! nothing outside the default hat, and the default hat's servers inside
//! it. A token the agent prints never reaches the timeline, its stream or
//! the log (decision 11, ACP core §8), nor does the upstream's URL past its
//! origin (lane L11). The whole test runs under the subscriber the process
//! installs (`logging::capped`) at `trace`, the thread's own: the runtime
//! is current-thread, so the collector's and the host's tasks log here.

use axum::body::{Body, Bytes};
use axum::http::{StatusCode, header};
use axum::response::Response;
use futures::StreamExt;
use hennery_gateway::api::GatewayState;
use hennery_gateway::key::KeySource;
use hennery_gateway::model::{Change, CredKind, NewConnection};
use hennery_gateway::proxy::{Limits, ProxyState};
use hennery_gateway::scope::ProxyStore;
use hennery_gateway::session::GatewayMcp;
use hennery_host::identity::HostKey;
use hennery_host::profile::Profile;
use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::egress::{Egress, Timeouts};
use hennery_kernel::hats::{HatChange, NewRule};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_sessions::{AppState, store::Store};
use hennery_testkit::{FakeScript, SCRIPT_ENV};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing_subscriber::util::SubscriberInitExt;

const HOST: &str = "host-1";
/// A secret in the upstream URL's path: never logged (lane L11).
const URL_CANARY: &str = "url-c4n4ry-0123456789";
/// How long anything may take before the test fails.
const BOUND: Duration = Duration::from_secs(30);

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

/// A `tracing` writer into a shared buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// The fake MCP upstream: a JSON answer to every `POST`, and an event
/// stream that sends one event and then holds to every `GET`.
async fn upstream() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let app = {
        let seen = seen.clone();
        axum::Router::new().fallback(move |req: axum::extract::Request| {
            let seen = seen.clone();
            async move {
                let auth = req
                    .headers()
                    .get(header::AUTHORIZATION)
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default();
                seen.lock()
                    .unwrap()
                    .push(format!("{} {} auth={auth}", req.method(), req.uri()));
                if req.method() == axum::http::Method::GET {
                    let stream = futures::stream::once(async {
                        Ok::<_, std::io::Error>(Bytes::from("data: {\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n\n"))
                    })
                    .chain(futures::stream::pending());
                    return Response::builder()
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from_stream(stream))
                        .unwrap();
                }
                Response::builder()
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": []}}).to_string(),
                    ))
                    .unwrap()
            }
        })
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, seen)
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
    gateway: GatewayState,
    db: PathBuf,
    client: reqwest::Client,
    proxy_client: reqwest::Client,
    hat: String,
    work: String,
}

impl Collector {
    async fn start(dir: &Path, upstream: SocketAddr) -> Self {
        let db = dir.join("hennery.db");
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
        let hat = hosts.host(HOST).unwrap().unwrap().default_hat_id;
        let HatChange::Done(work) = hosts.create_hat("Work", None, 0).unwrap() else {
            panic!("no hat");
        };
        let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        // Set up, so the gateway has a `public_url`.
        let client = hennery_testkit::operator_client(&state.operator);
        // As the binary wires it (`hennery`'s `gateway`).
        let keys = KeySource::from_vars(dir, Some("07".repeat(32).into()), None).unwrap();
        let gateway = hennery_gateway::open(&db, &keys, state.operator.clone()).unwrap();
        state.store.set_session_mcp(Arc::new(GatewayMcp::new(&gateway)));
        let egress = Egress::new(Timeouts {
            connect: Duration::from_secs(5),
            request: Duration::from_secs(60),
        })
        .unwrap();
        let proxy = ProxyState::full(
            Arc::new(ProxyStore::open(&db).unwrap()),
            &gateway,
            egress,
            Limits::default(),
        );
        let router = hennery_sessions::router(state.clone())
            .merge(hennery_gateway::api::router(gateway.clone()))
            .merge(hennery_gateway::proxy::router(proxy));
        tokio::spawn(hennery_sessions::serve_all(
            vec![listener],
            router,
            state.shutdown.clone(),
        ));
        let collector = Self {
            addr,
            state,
            gateway,
            db,
            client,
            proxy_client: reqwest::Client::builder().no_proxy().build().unwrap(),
            hat,
            work: work.id,
        };
        let url = format!("http://{upstream}/mcp/{URL_CANARY}");
        collector.connection("linear", &url, &collector.hat.clone());
        collector.connection("acme", &url, &collector.work.clone());
        collector
    }

    fn connection(&self, slug: &str, url: &str, hat: &str) {
        let new = NewConnection {
            slug: slug.into(),
            label: slug.into(),
            url: url.into(),
            hat_id: hat.into(),
            cred_kind: CredKind::None,
            static_header: None,
            static_prefix: None,
            tool_allowlist: None,
            // Loopback (lane L7): no test-only bypass.
            internal_network: true,
        };
        let Change::Done(record) = self.gateway.store.create(&new, 0).unwrap() else {
            panic!("no connection");
        };
        assert!(matches!(
            self.gateway
                .store
                .replace_mounts(&record.id, &[HOST.to_string()])
                .unwrap(),
            Change::Done(_)
        ));
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn json(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = self.client.request(method, self.url(path));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    async fn start_session(&self, agent: &str, cwd: &Path) -> String {
        let (status, body) = self
            .json(
                reqwest::Method::POST,
                "/api/sessions",
                Some(json!({"host_id": HOST, "agent": agent, "cwd": cwd})),
            )
            .await;
        assert_eq!(status, 202, "{body}");
        let id = body["session_id"].as_str().unwrap().to_string();
        self.wait_lifecycle(&id, "active").await;
        id
    }

    async fn detail(&self, id: &str) -> Value {
        self.json(reqwest::Method::GET, &format!("/api/sessions/{id}"), None)
            .await
            .1
    }

    async fn wait_lifecycle(&self, id: &str, lifecycle: &str) {
        wait_for(&format!("{id} {lifecycle}"), || async {
            (self.detail(id).await["lifecycle"] == lifecycle).then_some(())
        })
        .await;
    }

    /// `POST /mcp/<slug>` with `token`: the status.
    async fn call(&self, slug: &str, token: &str) -> u16 {
        self.proxy_client
            .post(self.url(&format!("/mcp/{slug}")))
            .bearer_auth(token)
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    fn token_rows(&self, session: &str) -> i64 {
        rusqlite::Connection::open(&self.db)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM gw_session_tokens WHERE session_id = ?1",
                [session],
                |r| r.get(0),
            )
            .unwrap()
    }
}

async fn wait_for<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + BOUND;
    loop {
        if let Some(v) = probe().await {
            return v;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The host, its `claude` (the pinned adapter's profile: strict, isolated)
/// and `codex` (no isolation) both the fake adapter logging to `log`.
fn start_host(collector: SocketAddr, data_dir: &Path, log: &Path) -> tokio::task::JoinHandle<()> {
    let script = FakeScript {
        session_log: Some(log.to_string_lossy().into_owned()),
        echo_servers: true,
        ..Default::default()
    };
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let mut cfg = HostConfig::new(
        format!("ws://{collector}/api/hosts/ws"),
        HOST,
        host_key(),
        data_dir.to_path_buf(),
    );
    cfg.reconnect_min = Duration::from_millis(100);
    cfg.reconnect_max = Duration::from_millis(500);
    cfg.agents.insert("claude".into(), fake.clone());
    cfg.agents.insert("codex".into(), fake);
    cfg.profiles.insert("claude".into(), Profile::Claude);
    tokio::spawn(async move {
        hennery_host::run(cfg).await.unwrap();
    })
}

/// The `mcpServers` of the adapter's latest `session/new` or
/// `session/load`.
fn last_servers(log: &Path) -> Value {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let line = text.lines().last().expect("no session/new or session/load logged");
    serde_json::from_str::<Value>(line).unwrap()["params"]["mcpServers"].clone()
}

fn bearer(servers: &Value) -> String {
    servers[0]["headers"][0]["value"]
        .as_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap()
        .to_string()
}

async fn host_connected(c: &Collector) {
    wait_for("host connection", || async {
        let (_, hosts) = c.json(reqwest::Method::GET, "/api/hosts", None).await;
        hosts
            .as_array()?
            .iter()
            .any(|h| h["host_id"] == HOST && h["connected"] == true)
            .then_some(())
    })
    .await;
}

#[test]
fn a_claude_session_gets_its_hats_servers_and_a_token_that_ends_with_its_park() {
    let logs = Captured::default();
    let subscriber = hennery_host::logging::capped(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer({
                let logs = logs.clone();
                move || logs.clone()
            })
            .finish(),
    );
    let _default = subscriber.set_default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut seen_tokens = Vec::new();
    runtime.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (upstream, upstream_seen) = upstream().await;
        let c = Collector::start(dir.path(), upstream).await;
        let log = dir.path().join("sessions.jsonl");
        let _host = start_host(c.addr, &dir.path().join("host"), &log);
        host_connected(&c).await;

        // The host list shows the isolation its `hello` announced.
        let (_, hosts) = c.json(reqwest::Method::GET, "/api/hosts", None).await;
        assert_eq!(
            hosts[0]["mcp_delivery"],
            json!({"claude": "isolated", "codex": "default_hat_only"}),
            "{hosts}"
        );

        let cwd = std::env::temp_dir();
        let id = c.start_session("claude", &cwd).await;
        // The stdio routes are answered as JSON through the merged router,
        // not by the web UI's catch-all (plan 4b).
        let (status, set) = c
            .json(
                reqwest::Method::GET,
                &format!("/api/mcp/stdio-servers?host_id={HOST}&hat_id={}", c.hat),
                None,
            )
            .await;
        assert_eq!((status, &set["servers"]), (200, &json!([])), "{set}");
        // Exactly the hat's server, with a working token.
        let servers = last_servers(&log);
        let first = bearer(&servers);
        seen_tokens.push(first.clone());
        assert_eq!(
            servers,
            json!([{"type": "http", "name": "hennery-linear", "url": "https://hennery.example/mcp/linear",
                    "headers": [{"name": "Authorization", "value": format!("Bearer {first}")}]}])
        );
        assert_eq!(c.call("linear", &first).await, 200);
        // The upstream got the request, and not the session's token.
        let got = upstream_seen.lock().unwrap().clone();
        assert!(got.iter().any(|r| r.starts_with("POST")), "{got:?}");
        assert!(got.iter().all(|r| !r.contains(&first)), "{got:?}");
        // Another hat's connection is not this token's.
        assert_eq!(c.call("acme", &first).await, 404);
        assert_eq!(
            c.detail(&id).await["mcp_delivery"]["mode"],
            "isolated",
            "{}",
            c.detail(&id).await
        );

        // A stream open on the token.
        let stream = c
            .proxy_client
            .get(c.url("/mcp/linear"))
            .bearer_auth(&first)
            .header(header::ACCEPT, "text/event-stream")
            .send()
            .await
            .unwrap();
        assert_eq!(stream.status(), StatusCode::OK);
        let mut stream = stream.bytes_stream();
        let event = tokio::time::timeout(BOUND, stream.next()).await.unwrap();
        assert!(matches!(event, Some(Ok(_))), "{event:?}");

        // Park: the token is dead, and its stream cut.
        let (status, body) = c
            .json(
                reqwest::Method::POST,
                &format!("/api/sessions/{id}/park"),
                Some(json!({})),
            )
            .await;
        assert!(status < 300, "{status} {body}");
        c.wait_lifecycle(&id, "parked").await;
        let next = tokio::time::timeout(BOUND, stream.next())
            .await
            .expect("the stream was not cut");
        assert!(!matches!(next, Some(Ok(_))), "{next:?}");
        assert_eq!(c.call("linear", &first).await, 404);

        // Resume: a new token works, the old one stays dead.
        let (status, body) = c
            .json(
                reqwest::Method::POST,
                &format!("/api/sessions/{id}/resume"),
                Some(json!({})),
            )
            .await;
        assert!(status < 300, "{status} {body}");
        c.wait_lifecycle(&id, "active").await;
        let second = bearer(&last_servers(&log));
        seen_tokens.push(second.clone());
        assert_ne!(first, second);
        assert_eq!(c.call("linear", &second).await, 200);
        assert_eq!(c.call("linear", &first).await, 404);

        // The agent prints its servers, the token included: the timeline and
        // its stream show it redacted.
        let sse = c
            .client
            .get(c.url(&format!("/api/stream/sessions/{id}")))
            .send()
            .await
            .unwrap();
        let mut sse = sse.bytes_stream();
        let (status, body) = c
            .json(
                reqwest::Method::POST,
                &format!("/api/sessions/{id}/prompt"),
                Some(json!({"content": [{"type": "text", "text": "hi"}]})),
            )
            .await;
        assert!(status < 300, "{status} {body}");
        let mut streamed = String::new();
        while !streamed.contains("my servers") {
            let chunk = tokio::time::timeout(BOUND, sse.next()).await.unwrap().unwrap().unwrap();
            streamed.push_str(&String::from_utf8_lossy(&chunk));
        }
        assert!(!streamed.contains(&second), "the token reached the stream: {streamed}");
        assert!(streamed.contains("hnry_session_<redacted>"), "{streamed}");
        let events = wait_for("the printed servers", || async {
            let (_, events) = c
                .json(reqwest::Method::GET, &format!("/api/sessions/{id}/events"), None)
                .await;
            let text = events.to_string();
            text.contains("my servers").then_some(text)
        })
        .await;
        assert!(!events.contains(&second), "the token reached the timeline: {events}");

        // Codex on a mixed host (the work hat's sessions make it so): another
        // hat than the default gets nothing; the default hat its servers.
        let work_dir = tempfile::tempdir().unwrap();
        let work_dir = std::fs::canonicalize(work_dir.path()).unwrap();
        let rules = [NewRule {
            prefix: work_dir.to_string_lossy().into_owned(),
            hat_id: c.work.clone(),
            verified: true,
        }];
        c.state.hosts.replace_path_rules(HOST, &rules).unwrap();
        let fallback = c.start_session("codex", &work_dir).await;
        assert_eq!(last_servers(&log), json!([]));
        assert_eq!(c.detail(&fallback).await["mcp_delivery"]["mode"], "fallback");
        assert_eq!(c.token_rows(&fallback), 0);
        let defaulted = c.start_session("codex", &cwd).await;
        let servers = last_servers(&log);
        assert_eq!(servers[0]["name"], "hennery-linear", "{servers}");
        seen_tokens.push(bearer(&servers));
        assert_eq!(c.detail(&defaulted).await["mcp_delivery"]["mode"], "unisolated");
        // Claude in the work hat: that hat's server, and only it (lane L5:
        // the hat is the rules', never the client's).
        let isolated = c.start_session("claude", &work_dir).await;
        let servers = last_servers(&log);
        assert_eq!(servers[0]["name"], "hennery-acme", "{servers}");
        assert_eq!(servers.as_array().unwrap().len(), 1);
        let token = bearer(&servers);
        seen_tokens.push(token.clone());
        assert_eq!(c.call("acme", &token).await, 200);
        assert_eq!(c.call("linear", &token).await, 404);
        assert_eq!(c.detail(&isolated).await["hat_id"], c.work.as_str());
        // Lane L5: a client naming a hat is not heard; the rules decide, so
        // no request mints a token for a hat the host's rules, its default
        // or a re-assignment did not give the session.
        let (status, body) = c
            .json(
                reqwest::Method::POST,
                "/api/sessions",
                Some(json!({"host_id": HOST, "agent": "claude", "cwd": cwd, "hat_id": c.work})),
            )
            .await;
        assert_eq!(status, 202, "{body}");
        let named = body["session_id"].as_str().unwrap().to_string();
        c.wait_lifecycle(&named, "active").await;
        assert_eq!(c.detail(&named).await["hat_id"], c.hat.as_str());
        let servers = last_servers(&log);
        assert_eq!(servers[0]["name"], "hennery-linear", "{servers}");
        seen_tokens.push(bearer(&servers));
        c.state.shutdown.cancel();
    });
    drop(runtime);
    let logged = logs.text();
    assert!(logged.contains("host connected"), "nothing was logged");
    for token in &seen_tokens {
        assert!(!logged.contains(token.as_str()), "a session token reached the log");
    }
    assert!(
        !logged.contains(URL_CANARY),
        "the upstream's URL past its origin reached the log"
    );
}

/// The security review's finding 2: a host revoke cuts the streams open on
/// the host's tokens at once, not only once its connection has closed. The
/// host here holds a connection that never closes, so the route waits its
/// whole bound (10 s) for it; the stream ends well before that.
#[tokio::test]
async fn a_host_revoke_cuts_its_streams_without_waiting_for_its_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (upstream, _) = upstream().await;
    let c = Collector::start(dir.path(), upstream).await;
    // A connection that never ends: the revoke's wait for it runs out.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let _held = c
        .state
        .hub
        .register(HOST, tx, Default::default(), Default::default())
        .expect("registered");
    let token = {
        let mut conn = rusqlite::Connection::open(&c.db).unwrap();
        let tx = conn.transaction().unwrap();
        let token = hennery_gateway::tokens::mint_in(
            &tx,
            c.gateway.store.owner_id(),
            "s1",
            HOST,
            &c.hat,
            hennery_kernel::secret::unix_now(),
        )
        .unwrap();
        tx.commit().unwrap();
        token.expose().to_string()
    };
    let response = c
        .proxy_client
        .get(c.url("/mcp/linear"))
        .bearer_auth(&token)
        .header(header::ACCEPT, "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.bytes_stream();
    let first = tokio::time::timeout(BOUND, stream.next()).await.unwrap();
    assert!(matches!(first, Some(Ok(_))), "the first event");
    let revoke = {
        let client = c.client.clone();
        let url = c.url(&format!("/api/hosts/{HOST}"));
        tokio::spawn(async move { client.delete(url).send().await.unwrap().status() })
    };
    let started = std::time::Instant::now();
    // The stream ends (broken, or closed) without another event.
    let next = tokio::time::timeout(BOUND, stream.next()).await.unwrap();
    assert!(!matches!(next, Some(Ok(_))), "another event came: {next:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the stream was cut only after {:?}, with the wait for the connection",
        started.elapsed()
    );
    // Revoked too, not only cut (the re-confirmation's note 1): while the
    // route still waits, the token's row is revoked, and the token opens
    // nothing new (that, the host's own revoke already refuses).
    assert!(
        !revoke.is_finished(),
        "the route no longer waits: the test proves nothing"
    );
    let live: i64 = rusqlite::Connection::open(&c.db)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM gw_session_tokens WHERE session_id = 's1' AND revoked_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(live, 0, "the token's row is still live while the route waits");
    assert_eq!(c.call("linear", &token).await, 404);
    assert_eq!(
        tokio::time::timeout(BOUND, revoke).await.unwrap().unwrap(),
        StatusCode::OK
    );
}
