//! The hook sets over HTTP (skills spec §8, §9): create behind step-up, the
//! refusals, the events table, delivery at session start to a fake host on
//! the websocket, the capability refusal, the start's acknowledgement and
//! the `hook_sets` feature flag.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_proto::frames::{
    Capabilities, Capability, CollectorFrame, HookSetApplied, HostFrame, Indexed, SessionBody,
};
use hennery_proto::rest::{
    ApiError, CapabilitiesResponse, HookEventsResponse, HookSetDetail, HookSetHostStatus, HookSetHostsResponse,
    HookSetItem, HostItem, SessionDetail, SessionPage,
};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::PUBLIC_URL;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

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
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// The owner's client, stepped up for five minutes.
    fn client(&self) -> reqwest::Client {
        hennery_testkit::operator_client(&self.state.operator)
    }

    /// A request from a signed-in session whose step-up is too old.
    fn stale(&self, method: &str, path: &str) -> reqwest::RequestBuilder {
        self.client();
        let phc = hennery_testkit::owner_phc(&self.state.operator);
        let token = self
            .state
            .operator
            .open_session("test", &phc, unix_now() - 301)
            .unwrap()
            .unwrap();
        reqwest::Client::new()
            .request(method.parse().unwrap(), self.url(path))
            .header("origin", PUBLIC_URL)
            .header("cookie", format!("__Host-hennery_session={token}"))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> T {
        self.client()
            .get(self.url(path))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn create(&self, body: serde_json::Value) -> reqwest::Response {
        self.client()
            .post(self.url("/api/hook-sets"))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    /// A hook set named `name` with one `PreToolUse` command, created.
    async fn guard(&self, name: &str) -> HookSetItem {
        let resp = self.create(guard_body(name)).await;
        assert_eq!(resp.status(), 201);
        resp.json().await.unwrap()
    }

    async fn set_hosts(&self, id: &str, hosts: &[&str]) -> reqwest::Response {
        self.client()
            .put(self.url(&format!("/api/hook-sets/{id}/hosts")))
            .json(&json!({ "host_ids": hosts }))
            .send()
            .await
            .unwrap()
    }

    /// Pair `host-1` with the test key.
    fn pair(&self) {
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        self.state.hosts.register("host-1", &enrollment, 0).unwrap();
    }
}

fn guard_body(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "hooks": {
            "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "/usr/local/bin/guard" }] }]
        }
    })
}

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

async fn code_of(resp: reqwest::Response) -> (u16, ApiError) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap())
}

type Sink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type Stream = futures::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// `host-1` on the websocket, announcing `capabilities` and `resolve_path`,
/// reconciled.
struct FakeHost {
    sink: Sink,
    stream: Stream,
}

impl FakeHost {
    async fn connect(c: &Collector, mut capabilities: Vec<Capability>) -> Self {
        capabilities.push(Capability::ResolvePath);
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", c.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let (mut sink, stream) = ws.split();
        let hello = HostFrame::Hello {
            runtime_profiles: Default::default(),
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "0".into(),
            host_id: "host-1".into(),
            proof: host_key().sign_hello(&nonce, "host-1", PROTOCOL_VERSION),
            capabilities: Capabilities(capabilities),
            workspace_roots: vec![],
            attached_sessions: vec![],
            mcp_isolation: Default::default(),
        };
        sink.send(Message::text(serde_json::to_string(&hello).unwrap()))
            .await
            .unwrap();
        let mut host = Self { sink, stream };
        assert!(matches!(host.next().await, CollectorFrame::HelloAck { .. }));
        host.send(&HostFrame::ResendComplete).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let hosts: Vec<HostItem> = c.get("/api/hosts").await;
            if hosts.iter().any(|h| h.host_id == "host-1" && h.connected) {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "host never became ready");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        host
    }

    async fn send(&mut self, frame: &HostFrame) {
        self.sink
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

    /// The next collector frame, skipping pings and the skills manifest.
    async fn next(&mut self) -> CollectorFrame {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(10), self.stream.next())
                .await
                .expect("a frame in time");
            match message {
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<CollectorFrame>(&t).unwrap() {
                    CollectorFrame::SkillSync { .. } => continue,
                    frame => return frame,
                },
                Some(Ok(_)) => continue,
                other => panic!("expected a frame, got {other:?}"),
            }
        }
    }

    /// Answer the start's `resolve_path` with the path itself.
    async fn resolve(&mut self) {
        match self.next().await {
            CollectorFrame::ResolvePath { request_id, path } => {
                self.send(&HostFrame::ResolvedPath {
                    request_id,
                    canonical: path,
                    exists: true,
                    is_dir: true,
                })
                .await
            }
            other => panic!("expected resolve_path, got {other:?}"),
        }
    }
}

fn start(c: &Collector) -> tokio::task::JoinHandle<reqwest::Response> {
    let client = c.client();
    let url = c.url("/api/sessions");
    tokio::spawn(async move {
        client
            .post(url)
            .json(&json!({ "host_id": "host-1", "agent": "fake", "cwd": "/tmp" }))
            .send()
            .await
            .unwrap()
    })
}

#[tokio::test]
async fn a_hook_set_is_created_listed_detailed_and_deleted_behind_step_up() {
    let c = Collector::start().await;
    let resp = c
        .stale("POST", "/api/hook-sets")
        .json(&guard_body("guard"))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await.1.code, "step_up_required");

    let resp = c.create(guard_body("guard")).await;
    assert_eq!(resp.status(), 201);
    assert_eq!(resp.headers()["cache-control"], "private, no-store");
    let item: HookSetItem = resp.json().await.unwrap();
    assert_eq!(item.name, "guard");
    assert_eq!(item.events, vec!["PreToolUse".to_string()]);
    assert!(item.host_ids.is_empty());

    let (status, err) = code_of(c.create(guard_body("guard")).await).await;
    assert_eq!((status, err.code.as_str()), (409, "hook_set_exists"));
    let (status, err) = code_of(c.create(guard_body("Not A Name")).await).await;
    assert_eq!((status, err.code.as_str()), (400, "invalid_hook_set"));

    let list: Vec<HookSetItem> = c.get("/api/hook-sets").await;
    assert_eq!(list, std::slice::from_ref(&item));

    let path = format!("/api/hook-sets/{}", item.id);
    let detail: HookSetDetail = c.get(&path).await;
    assert_eq!(detail.item, item);
    assert_eq!(detail.hooks.0["PreToolUse"][0].hooks[0].command, "/usr/local/bin/guard");
    let resp = c.client().get(c.url("/api/hook-sets/nope")).send().await.unwrap();
    assert_eq!(code_of(resp).await.1.code, "hook_set_not_found");

    let replaced = json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "/bin/true" }] }] } });
    let resp = c.stale("PUT", &path).json(&replaced).send().await.unwrap();
    assert_eq!(code_of(resp).await.1.code, "step_up_required");
    let resp = c.client().put(c.url(&path)).json(&replaced).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let after: HookSetItem = resp.json().await.unwrap();
    assert_eq!(after.events, vec!["Stop".to_string()]);
    assert_ne!(after.content_hash, item.content_hash);

    c.pair();
    let resp = c.set_hosts(&item.id, &["host-1"]).await;
    assert_eq!(resp.status(), 200);
    let hosts: HookSetHostsResponse = resp.json().await.unwrap();
    assert_eq!(hosts.hosts.len(), 1);
    // Paired, never said hello: no `hook_sets` on record.
    assert_eq!(hosts.hosts[0].status, HookSetHostStatus::Unsupported);
    let (status, err) = code_of(c.set_hosts(&item.id, &["ghost"]).await).await;
    assert_eq!((status, err.code.as_str()), (400, "unknown_host"));

    let resp = c.stale("DELETE", &path).send().await.unwrap();
    assert_eq!(code_of(resp).await.1.code, "step_up_required");
    assert_eq!(c.client().delete(c.url(&path)).send().await.unwrap().status(), 204);
    let list: Vec<HookSetItem> = c.get("/api/hook-sets").await;
    assert!(list.is_empty());
    let resp = c.client().delete(c.url(&path)).send().await.unwrap();
    assert_eq!(code_of(resp).await.1.code, "hook_set_not_found");
}

#[tokio::test]
async fn a_non_command_handler_is_refused() {
    let c = Collector::start().await;
    let body = json!({
        "name": "prompting",
        "hooks": { "Stop": [{ "hooks": [{ "type": "prompt", "command": "x" }] }] }
    });
    let (status, err) = code_of(c.create(body).await).await;
    assert_eq!((status, err.code.as_str()), (400, "invalid_hook_set"));
    assert!(err.message.contains("command"), "{}", err.message);
    // Never stored.
    let list: Vec<HookSetItem> = c.get("/api/hook-sets").await;
    assert!(list.is_empty(), "{list:?}");
}

#[tokio::test]
async fn an_unknown_event_is_refused() {
    let c = Collector::start().await;
    let body = json!({
        "name": "bogus",
        "hooks": { "BeforeEverything": [{ "hooks": [{ "type": "command", "command": "x" }] }] }
    });
    let (status, err) = code_of(c.create(body).await).await;
    assert_eq!((status, err.code.as_str()), (400, "invalid_hook_set"));
    assert!(err.message.contains("BeforeEverything"), "{}", err.message);
    // Never stored.
    let list: Vec<HookSetItem> = c.get("/api/hook-sets").await;
    assert!(list.is_empty(), "{list:?}");
}

#[tokio::test]
async fn events_report_per_agent_support() {
    let c = Collector::start().await;
    let events: HookEventsResponse = c.get("/api/hook-sets/events").await;
    assert_eq!(events.events.len(), hennery_proto::hooks::CLAUDE_EVENTS.len());
    let of = |name: &str| events.events.iter().find(|e| e.event == name).unwrap().clone();
    assert!(of("PreToolUse").claude && of("PreToolUse").codex);
    assert!(of("UserPromptSubmit").claude && !of("UserPromptSubmit").codex);
}

#[tokio::test]
async fn a_start_carries_the_assigned_hook_sets() {
    let c = Collector::start().await;
    c.pair();
    let mut host = FakeHost::connect(&c, vec![Capability::HookSets]).await;
    let set = c.guard("guard").await;
    let hosts: HookSetHostsResponse = c.set_hosts(&set.id, &["host-1"]).await.json().await.unwrap();
    assert_eq!(hosts.hosts[0].status, HookSetHostStatus::Assigned);

    let post = start(&c);
    host.resolve().await;
    match host.next().await {
        CollectorFrame::StartSession { hooks, .. } => {
            let hooks = hooks.expect("the assigned hook sets");
            assert_eq!(hooks.sets.len(), 1);
            assert_eq!(hooks.sets[0].name, "guard");
            assert_eq!(hooks.sets[0].id, set.id);
            assert_eq!(hooks.sets[0].content_hash, set.content_hash);
        }
        other => panic!("expected start_session, got {other:?}"),
    }
    drop(host);
    let _ = post.await;
}

#[tokio::test]
async fn a_host_without_the_capability_is_refused_the_start() {
    let c = Collector::start().await;
    c.pair();
    let mut host = FakeHost::connect(&c, vec![]).await;
    let set = c.guard("guard").await;
    let hosts: HookSetHostsResponse = c.set_hosts(&set.id, &["host-1"]).await.json().await.unwrap();
    assert_eq!(hosts.hosts[0].status, HookSetHostStatus::Unsupported);

    let post = start(&c);
    host.resolve().await;
    let (status, err) = code_of(post.await.unwrap()).await;
    assert_eq!((status, err.code.as_str()), (409, "hook_sets_unsupported"));
    // As for an MCP refusal, the answer carries no session id: the list has it.
    let page: SessionPage = c.get("/api/sessions").await;
    assert_eq!(page.sessions.len(), 1);
    let session = &page.sessions[0];
    assert_eq!(session.lifecycle, "failed");
    assert_eq!(session.failure_reason.as_deref(), Some("hook_sets_unsupported"));
    // No start went out.
    let quiet = tokio::time::timeout(Duration::from_millis(300), host.stream.next()).await;
    match quiet {
        Err(_) => {}
        Ok(Some(Ok(Message::Text(t)))) => {
            let frame: CollectorFrame = serde_json::from_str(&t).unwrap();
            assert!(
                !matches!(frame, CollectorFrame::StartSession { .. }),
                "a start went out: {frame:?}"
            );
        }
        Ok(_) => {}
    }
}

#[tokio::test]
async fn a_session_started_ack_records_the_applied_sets() {
    let c = Collector::start().await;
    c.pair();
    let mut host = FakeHost::connect(&c, vec![Capability::HookSets]).await;
    let set = c.guard("guard").await;
    assert_eq!(c.set_hosts(&set.id, &["host-1"]).await.status(), 200);

    let post = start(&c);
    host.resolve().await;
    let (request_id, session_id) = match host.next().await {
        CollectorFrame::StartSession {
            request_id, session_id, ..
        } => (request_id, session_id),
        other => panic!("expected start_session, got {other:?}"),
    };
    let applied = vec![HookSetApplied {
        id: set.id.clone(),
        name: set.name.clone(),
        content_hash: set.content_hash.clone(),
    }];
    host.send(&HostFrame::Session {
        session_id: session_id.clone(),
        seq: 1,
        body: SessionBody::SessionStarted {
            request_id,
            agent_session_id: "a1".into(),
            indexed: Indexed {
                hook_sets_applied: applied.clone(),
                ..Default::default()
            },
            agent_home: None,
        },
    })
    .await;
    assert_eq!(post.await.unwrap().status(), 202);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let detail: SessionDetail = c.get(&format!("/api/sessions/{session_id}")).await;
        if detail.session.lifecycle == "active" {
            assert_eq!(detail.hook_sets, applied);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the start was never acknowledged"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn capabilities_advertise_hook_sets() {
    let c = Collector::start().await;
    let caps: CapabilitiesResponse = c.get("/api/capabilities").await;
    assert!(caps.features.iter().any(|f| f == "hook_sets"), "{:?}", caps.features);
}
