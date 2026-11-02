//! Mounted live proof using real collector/service processes and a signed scripted host.
//! Build both binaries before explicitly running this ignored test. All state
//! is disposable and private; no model or real adapter is exercised.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::{
    hats::{HatChange, NewRule, RulesChange},
    hosts::{Enrollment, Hosts},
    operator::{Operator, SetupOutcome},
    secret::unix_now,
};
use hennery_memory::consumer::FeedConfig;
use hennery_proto::{
    HELLO_NONCE_HEADER, PROTOCOL_VERSION,
    frames::{
        AgentIsolation, Capabilities, Capability, CollectorFrame, HostFrame, Indexed, McpDelivery, McpIsolation,
        McpServer, ParkReason, SessionBody, TurnOutcome,
    },
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::Write,
    net::{SocketAddr, TcpListener},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const HOST: &str = "recall-proof-host";
const BOUND: Duration = Duration::from_secs(60);
const MARKER_A: &str = "recallonlinefixture9xz";
const MARKER_B: &str = "recallofflinefixture8xz";

struct Running(Child);
impl Running {
    fn stop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}
struct HostTask(tokio::task::JoinHandle<()>);
impl Drop for HostTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct Namespace {
    id: String,
    tool: String,
    lifecycle: String,
}
struct Grant {
    epoch: String,
    token: String,
}
struct Delivery {
    session: String,
    url: String,
    bearer: String,
}

fn private_dir(path: &Path) {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .unwrap();
}
fn command(binary: &Path, scratch: &Path) -> Command {
    hennery_testkit::first_exec(binary.to_str().unwrap());
    let home = scratch.join("home");
    private_dir(&home);
    let mut command = Command::new(binary);
    command
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("HENNERY_DATA_DIR")
        .env_remove("HENNERY_HOST_DATA_DIR")
        .env_remove("HENNERY_MASTER_KEY")
        .env_remove("HENNERY_PUBLIC_URL")
        .env_remove("HENNERY_LISTEN")
        .env_remove("HENNERY_DEV_TOKEN")
        .env_remove("CREDENTIALS_DIRECTORY")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME");
    command
}
fn output_file(path: &Path) -> File {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .unwrap()
}
fn spawn(mut command: Command, log: &Path) -> Running {
    command
        .stdout(Stdio::from(output_file(log)))
        .stderr(Stdio::from(output_file(log)));
    Running(command.spawn().unwrap())
}
fn collector_binary() -> PathBuf {
    std::env::var_os("HENNERY_LIVE_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("hennery")
        })
}
fn create(memory: &Path, scratch: &Path, data: &Path, owner: &str, hat: &str) -> Namespace {
    let output = command(memory, scratch)
        .arg("--data-dir")
        .arg(data)
        .args(["create", "--owner", owner, "--hat", hat])
        .output()
        .unwrap();
    assert!(output.status.success(), "memory bootstrap failed");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    Namespace {
        id: value["namespace"].as_str().unwrap().into(),
        tool: value["tool_credential"].as_str().unwrap().into(),
        lifecycle: value["lifecycle_credential"].as_str().unwrap().into(),
    }
}
fn memory_server(binary: &Path, scratch: &Path, data: &Path, addr: SocketAddr) -> (Running, SocketAddr) {
    let log = scratch.join("memory.log");
    let start = std::fs::metadata(&log).map(|m| m.len() as usize).unwrap_or(0);
    let mut cmd = command(binary, scratch);
    cmd.arg("--data-dir")
        .arg(data)
        .args(["serve", "--listen", &addr.to_string()]);
    let mut child = spawn(cmd, &log);
    let end = std::time::Instant::now() + BOUND;
    loop {
        assert!(std::time::Instant::now() < end, "memory readiness line deadline");
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "memory process exited before readiness"
        );
        if let Ok(text) = std::fs::read_to_string(&log)
            && let Some(addr) = text[start..].lines().find_map(|line| {
                line.strip_prefix("memory listening on ")
                    .and_then(|s| s.parse::<SocketAddr>().ok())
            })
        {
            return (child, addr);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn attach(binary: &Path, scratch: &Path, data: &Path, namespace: &Namespace, config: FeedConfig) {
    let mut child = command(binary, scratch)
        .arg("--data-dir")
        .arg(data)
        .args(["attach", "--namespace", &namespace.id])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(output_file(&scratch.join("memory.log"))))
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&serde_json::to_vec(&config).unwrap()).unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success(), "explicit offline grant attachment");
}
async fn wait_health(client: &Client, base: &str) {
    tokio::time::timeout(BOUND, async {
        loop {
            if client
                .get(format!("{base}/healthz"))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("real collector became healthy");
}
async fn owner(client: &Client, base: &str, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = client.request(method, format!("{base}{path}"));
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}
async fn raw_rpc(client: &Client, url: &str, token: &str, message: Value) -> (StatusCode, Value) {
    let response = client
        .post(url)
        .bearer_auth(token)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&message)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}
async fn wait_memory(client: &Client, url: &str, namespace: &Namespace) {
    tokio::time::timeout(BOUND, async {
        loop {
            let answer = client
                .post(url)
                .bearer_auth(&namespace.lifecycle)
                .json(&json!({"jsonrpc":"2.0","id":"capabilities",
                    "method":"hennery_plugin_capabilities","params":{}}))
                .send()
                .await;
            if let Ok(response) = answer
                && response.status().is_success()
            {
                let value: Value = response.json().await.unwrap();
                assert_eq!(value["result"]["version"], 1);
                assert_eq!(value["result"]["activity_v1"], true);
                assert_eq!(value["result"]["forget_v1"], true);
                assert_eq!(value["result"]["startup_context_v1"], false);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("real memory lifecycle endpoint became ready");
}
async fn register(
    owner_client: &Client,
    collector: &str,
    memory_url: &str,
    namespace: &Namespace,
    hat: &str,
    slug: &str,
) -> Grant {
    let (status, connection) = owner(
        owner_client,
        collector,
        Method::POST,
        "/api/mcp/connections",
        Some(json!({"slug":slug,"label":slug,"url":memory_url,"hat_id":hat,
            "cred_kind":"static","internal_network":true,
            "tool_allowlist":["memory_recent","memory_search","memory_brief","memory_remember"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = connection["id"].as_str().unwrap();
    let (status, _) = owner(
        owner_client,
        collector,
        Method::PUT,
        &format!("/api/mcp/connections/{id}/credential"),
        Some(json!({"token":namespace.tool})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = owner(
        owner_client,
        collector,
        Method::PUT,
        &format!("/api/mcp/connections/{id}/mounts"),
        Some(json!({"host_ids":[HOST]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, grant) = owner(
        owner_client,
        collector,
        Method::POST,
        "/api/plugins/registrations",
        Some(json!({"connection_id":id,"lifecycle_credential":namespace.lifecycle,
            "activity":true,"startup":false,"namespace_isolated":true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    Grant {
        epoch: grant["namespace_epoch"].as_str().unwrap().into(),
        token: grant["token"].as_str().unwrap().into(),
    }
}
async fn send_session(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    seqs: &mut HashMap<String, u64>,
    session: &str,
    body: SessionBody,
) {
    let seq = seqs.entry(session.into()).or_default();
    *seq += 1;
    ws.send(Message::text(
        serde_json::to_string(&HostFrame::Session {
            session_id: session.into(),
            seq: *seq,
            body,
        })
        .unwrap(),
    ))
    .await
    .unwrap();
}
fn delivery(session: String, mcp: McpDelivery) -> Delivery {
    let servers = mcp
        .mcp_servers
        .into_iter()
        .filter_map(|s| match s {
            McpServer::Http { name, url, headers } if name.starts_with("hennery-recall-") => Some((url, headers)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(servers.len(), 1, "exactly this hat's memory mount");
    let (url, headers) = servers.into_iter().next().unwrap();
    let bearer = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("authorization"))
        .unwrap()
        .value
        .strip_prefix("Bearer ")
        .unwrap()
        .to_string();
    Delivery { session, url, bearer }
}
async fn host(collector: &str, roots: &[PathBuf]) -> (HostTask, mpsc::UnboundedReceiver<Delivery>) {
    let key = HostKey::from_seed([21; 32]);
    let ws_url = format!("{}/api/hosts/ws", collector.replacen("http://", "ws://", 1));
    let (mut ws, response) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
    let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
    ws.send(Message::text(
        serde_json::to_string(&HostFrame::Hello {
            runtime_profiles: Default::default(),
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "recall-fixture".into(),
            host_id: HOST.into(),
            proof: key.sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities: Capabilities(vec![Capability::ResolvePath, Capability::Park, Capability::McpServers]),
            mcp_isolation: AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect()),
            workspace_roots: roots.iter().map(|r| r.to_str().unwrap().into()).collect(),
            attached_sessions: vec![],
        })
        .unwrap(),
    ))
    .await
    .unwrap();
    let first = tokio::time::timeout(BOUND, ws.next()).await.unwrap().unwrap().unwrap();
    let Message::Text(text) = first else {
        panic!("expected hello acknowledgement");
    };
    assert!(matches!(
        serde_json::from_str::<CollectorFrame>(&text).unwrap(),
        CollectorFrame::HelloAck { .. }
    ));
    ws.send(Message::text(
        serde_json::to_string(&HostFrame::ResendComplete).unwrap(),
    ))
    .await
    .unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let mut seqs = HashMap::<String, u64>::new();
        loop {
            let Some(message) = ws.next().await else {
                break;
            };
            let message = message.unwrap();
            match message {
                Message::Ping(payload) => {
                    ws.send(Message::Pong(payload)).await.unwrap();
                    continue;
                }
                Message::Text(text) => {
                    let frame: CollectorFrame = serde_json::from_str(&text).unwrap();
                    match frame {
                        CollectorFrame::ResolvePath { request_id, path } => {
                            ws.send(Message::text(
                                serde_json::to_string(&HostFrame::ResolvedPath {
                                    request_id,
                                    canonical: path,
                                    exists: true,
                                    is_dir: true,
                                })
                                .unwrap(),
                            ))
                            .await
                            .unwrap();
                        }
                        CollectorFrame::StartSession {
                            request_id,
                            session_id,
                            committed_seq,
                            mcp,
                            ..
                        }
                        | CollectorFrame::ResumeSession {
                            request_id,
                            session_id,
                            committed_seq,
                            mcp,
                            ..
                        } => {
                            let current = seqs.entry(session_id.clone()).or_default();
                            *current = (*current).max(committed_seq);
                            send_session(
                                &mut ws,
                                &mut seqs,
                                &session_id,
                                SessionBody::session_started(request_id, format!("agent-{session_id}")),
                            )
                            .await;
                            tx.send(delivery(session_id, mcp)).unwrap();
                        }
                        CollectorFrame::Prompt {
                            request_id,
                            session_id,
                            turn_id,
                            content,
                            plugin_context,
                        } => {
                            assert!(plugin_context.is_none(), "R1 never opts into startup context");
                            send_session(
                                &mut ws,
                                &mut seqs,
                                &session_id,
                                SessionBody::TurnStarted {
                                    request_id,
                                    turn_id: turn_id.clone(),
                                },
                            )
                            .await;
                            let marker = content
                                .iter()
                                .filter_map(|v| v["text"].as_str())
                                .collect::<Vec<_>>()
                                .join(" ");
                            send_session(&mut ws,&mut seqs,&session_id,SessionBody::AcpUpdate {
                                indexed:Indexed::default(),payload:json!({"update":{
                                    "sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("Observed {marker}")}}}),
                            }).await;
                            send_session(
                                &mut ws,
                                &mut seqs,
                                &session_id,
                                SessionBody::TurnEnded {
                                    usage: None,
                                    turn_id,
                                    outcome: TurnOutcome::Completed,
                                    stop_reason: None,
                                    error: None,
                                },
                            )
                            .await;
                        }
                        CollectorFrame::ParkSession { session_id, .. } => {
                            send_session(
                                &mut ws,
                                &mut seqs,
                                &session_id,
                                SessionBody::SessionParked {
                                    reason: ParkReason::Operator,
                                },
                            )
                            .await;
                        }
                        CollectorFrame::CloseSession { session_id, .. } => {
                            send_session(&mut ws, &mut seqs, &session_id, SessionBody::SessionClosed).await;
                        }
                        CollectorFrame::Ack { .. } | CollectorFrame::ForgetHat { .. } => {}
                        other => panic!(
                            "unexpected collector operation: {}",
                            serde_json::to_value(other).unwrap()["type"]
                        ),
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });
    (HostTask(task), rx)
}
async fn start(
    client: &Client,
    collector: &str,
    cwd: &Path,
    deliveries: &mut mpsc::UnboundedReceiver<Delivery>,
) -> Delivery {
    let (status, session) = owner(
        client,
        collector,
        Method::POST,
        "/api/sessions",
        Some(json!({"host_id":HOST,"agent":"claude","cwd":cwd})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let result = tokio::time::timeout(BOUND, deliveries.recv()).await.unwrap().unwrap();
    assert_eq!(session["session_id"], result.session);
    result
}
async fn tool(client: &Client, delivery: &Delivery, name: &str, arguments: Value) -> Option<Value> {
    let (status, answer) = raw_rpc(
        client,
        &delivery.url,
        &delivery.bearer,
        json!({
            "jsonrpc":"2.0","id":"live-tool","method":"tools/call","params":{"name":name,"arguments":arguments}
        }),
    )
    .await;
    if !status.is_success() || answer.get("error").is_some() {
        return None;
    }
    assert_eq!(answer["result"]["isError"], false);
    Some(serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap())
}
async fn initialize(client: &Client, delivery: &Delivery) {
    tokio::time::timeout(BOUND, async {
        loop {
            let (status, answer) = raw_rpc(
                client,
                &delivery.url,
                &delivery.bearer,
                json!({
                "jsonrpc":"2.0","id":"init","method":"initialize","params":{
                    "protocolVersion":"2025-06-18","capabilities":{},
                    "clientInfo":{"name":"recall-live-proof","version":"1"}}}),
            )
            .await;
            if status == StatusCode::OK {
                assert_eq!(answer["result"]["serverInfo"]["name"], "hennery-memory");
                assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
                break;
            }
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "awaiting the initial valid namespace lease"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("MCP initialize through the session gateway");
    let (status, _) = raw_rpc(
        client,
        &delivery.url,
        &delivery.bearer,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, list) = raw_rpc(
        client,
        &delivery.url,
        &delivery.bearer,
        json!({"jsonrpc":"2.0","id":"list","method":"tools/list","params":{}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 4);
}
async fn wait_note(client: &Client, delivery: &Delivery, marker: &str) -> Value {
    tokio::time::timeout(BOUND, async {
        loop {
            if let Some(page) = tool(client, delivery, "memory_recent", json!({})).await
                && page["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e.to_string().contains(marker))
            {
                return page;
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    })
    .await
    .expect("real feed consumer projected a completed operator turn")
}
async fn prompt(client: &Client, collector: &str, delivery: &Delivery, marker: &str) {
    let (status, _) = owner(
        client,
        collector,
        Method::POST,
        &format!("/api/sessions/{}/prompt", delivery.session),
        Some(json!({"content":[{"type":"text","text":marker}]})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}
async fn park(owner_client: &Client, proxy_client: &Client, collector: &str, delivery: &Delivery) {
    let (status, _) = owner(
        owner_client,
        collector,
        Method::POST,
        &format!("/api/sessions/{}/park", delivery.session),
        Some(json!({})),
    )
    .await;
    assert!(status.is_success());
    let (status, _) = raw_rpc(
        proxy_client,
        &delivery.url,
        &delivery.bearer,
        json!({"jsonrpc":"2.0","id":"parked","method":"tools/list","params":{}}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
async fn receipt(client: &Client, collector: &str, hat: &str) -> Value {
    let (status, receipts) = owner(client, collector, Method::GET, "/api/plugins/receipts", None).await;
    assert_eq!(status, StatusCode::OK);
    receipts
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["hat_id"] == hat)
        .unwrap()
        .clone()
}
fn entry_count(data: &Path, namespace: &str) -> i64 {
    let db = rusqlite::Connection::open_with_flags(data.join("memory.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    db.query_row("SELECT count(*) FROM entries WHERE namespace=?1", [namespace], |r| {
        r.get(0)
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires freshly built hennery and hennery-memory binaries; run explicitly for the mounted live proof"]
async fn real_binaries_feed_four_tools_park_resume_and_honest_online_offline_purge() {
    let scratch = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(scratch.path()).unwrap();
    let core_data = base.join("collector");
    let memory_data = base.join("memory");
    let root_a = base.join("project-a");
    let root_b = base.join("project-b");
    for path in [&core_data, &memory_data, &root_a, &root_b] {
        private_dir(path);
    }
    let core_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let core_addr = core_listener.local_addr().unwrap();
    let first_memory_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let collector = format!("http://{core_addr}");
    let (owner_id, cookie, hat_a, hat_b) = {
        // Seed only owner/host/routing configuration while the real collector is stopped.
        let path = core_data.join("hennery.db");
        let _sessions = hennery_sessions::store::Store::open(&path).unwrap();
        let hosts = Hosts::open(&path).unwrap();
        let operator = Operator::open(&path).unwrap();
        let now = unix_now();
        let setup = operator.issue_setup_token(now).unwrap().unwrap();
        let SetupOutcome::Done { phc, .. } = operator
            .set_up(&setup, "correct horse battery", &collector, now)
            .unwrap()
        else {
            panic!("setup");
        };
        let cookie = operator
            .open_session("recall-live-fixture", &phc, now)
            .unwrap()
            .unwrap();
        let key = HostKey::from_seed([21; 32]);
        hosts
            .register(
                HOST,
                &Enrollment {
                    public_key: key.public_key_hex(),
                    name: "Recall proof".into(),
                    host_version: "fixture".into(),
                    platform: "test".into(),
                },
                now,
            )
            .unwrap();
        let HatChange::Done(a) = hosts.create_hat("Recall online", None, now).unwrap() else {
            panic!("hat");
        };
        let HatChange::Done(b) = hosts.create_hat("Recall offline", None, now).unwrap() else {
            panic!("hat");
        };
        assert!(matches!(
            hosts
                .replace_path_rules(
                    HOST,
                    &[
                        NewRule {
                            prefix: root_a.to_str().unwrap().into(),
                            hat_id: a.id.clone(),
                            verified: true
                        },
                        NewRule {
                            prefix: root_b.to_str().unwrap().into(),
                            hat_id: b.id.clone(),
                            verified: true
                        },
                    ]
                )
                .unwrap(),
            RulesChange::Done(_)
        ));
        (operator.owner_id().to_string(), cookie, a.id, b.id)
    };
    let memory_binary = PathBuf::from(env!("CARGO_BIN_EXE_hennery-memory"));
    let core_binary = collector_binary();
    assert!(
        core_binary.is_file(),
        "build freshly: cargo build -p hennery -p hennery-memory --bins --locked"
    );
    let ns_a = create(&memory_binary, &base, &memory_data, &owner_id, &hat_a);
    let ns_b = create(&memory_binary, &base, &memory_data, &owner_id, &hat_b);
    let plain = Client::builder().no_proxy().timeout(BOUND).build().unwrap();
    let owner_client = Client::builder()
        .no_proxy()
        .timeout(BOUND)
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(reqwest::header::ORIGIN, collector.parse().unwrap());
            headers.insert(
                reqwest::header::COOKIE,
                format!("hennery_session={cookie}").parse().unwrap(),
            );
            headers
        })
        .build()
        .unwrap();
    let mut cmd = command(&core_binary, &base);
    cmd.env("HENNERY_MASTER_KEY", "07".repeat(32))
        .args(["collector", "--data-dir"])
        .arg(&core_data)
        .args([
            "--listen-fd",
            &core_listener.as_raw_fd().to_string(),
            "--public-url",
            &collector,
        ]);
    let listener_fd = core_listener.as_raw_fd();
    // SAFETY: the listener remains owned/alive through spawn; this child-only
    // async-signal-safe fcntl merely preserves that explicit listener on exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fcntl(listener_fd, libc::F_SETFD, 0) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut core = spawn(cmd, &base.join("collector.log"));
    drop(core_listener);
    wait_health(&plain, &collector).await;
    let (mut memory, memory_addr) = memory_server(&memory_binary, &base, &memory_data, first_memory_addr);
    let memory_url = format!("http://{memory_addr}/mcp");
    wait_memory(&plain, &memory_url, &ns_a).await;
    let grant_a = register(&owner_client, &collector, &memory_url, &ns_a, &hat_a, "recall-a").await;
    let grant_b = register(&owner_client, &collector, &memory_url, &ns_b, &hat_b, "recall-b").await;
    memory.stop();
    // Registration supplies the authoritative epochs. Attach via the real CLI
    // offline so no second process opens the service's running writer database.
    for (namespace, grant) in [(&ns_a, &grant_a), (&ns_b, &grant_b)] {
        attach(
            &memory_binary,
            &base,
            &memory_data,
            namespace,
            FeedConfig {
                collector_url: format!("{collector}/"),
                namespace_epoch: grant.epoch.clone(),
                activity_token: grant.token.clone(),
                activity: true,
                internal_network: true,
            },
        );
    }
    memory = memory_server(&memory_binary, &base, &memory_data, memory_addr).0;
    wait_memory(&plain, &memory_url, &ns_a).await;
    let (_host, mut deliveries) = host(&collector, &[root_a.clone(), root_b.clone()]).await;
    tokio::time::timeout(BOUND, async {
        loop {
            let (status, hosts) = owner(&owner_client, &collector, Method::GET, "/api/hosts", None).await;
            if status.is_success()
                && hosts
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|h| h["host_id"] == HOST && h["connected"] == true)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("signed host reconciled");
    let a = start(&owner_client, &collector, &root_a, &mut deliveries).await;
    let b = start(&owner_client, &collector, &root_b, &mut deliveries).await;
    assert_ne!(
        a.bearer, ns_a.tool,
        "gateway bearer must differ from upstream credential"
    );
    assert_ne!(a.bearer, b.bearer, "each live session has an independent bearer");
    assert_eq!(a.url, format!("{collector}/mcp/recall-a"));
    assert_eq!(b.url, format!("{collector}/mcp/recall-b"));
    initialize(&plain, &a).await;
    initialize(&plain, &b).await;
    prompt(&owner_client, &collector, &a, MARKER_A).await;
    prompt(&owner_client, &collector, &b, MARKER_B).await;
    let recent = wait_note(&plain, &a, MARKER_A).await;
    assert!(recent["entries"][0]["provenance"].to_string().contains("operator"));
    assert!(recent["entries"][0]["turns"].to_string().contains("assistant"));
    wait_note(&plain, &b, MARKER_B).await;
    let stored = tool(
        &plain,
        &a,
        "memory_remember",
        json!({
        "title":"Live retained observation","body":format!("deliberate {MARKER_A}"),
        "idempotency_key":"real-mcp-retention"}),
    )
    .await
    .unwrap();
    assert_eq!(stored["revision"], 1);
    let searched = tool(&plain, &a, "memory_search", json!({"query":MARKER_A}))
        .await
        .unwrap();
    assert!(searched["entries"].as_array().unwrap().len() >= 2);
    let brief = tool(&plain, &a, "memory_brief", json!({"subject":MARKER_A}))
        .await
        .unwrap();
    assert_eq!(brief["mode_used"], "deterministic");
    assert!(brief["text"].as_str().unwrap().contains(MARKER_A));
    let foreign = tool(&plain, &b, "memory_search", json!({"query":MARKER_A}))
        .await
        .unwrap();
    assert!(foreign["entries"].as_array().unwrap().is_empty());
    park(&owner_client, &plain, &collector, &a).await;
    let (status, _) = owner(
        &owner_client,
        &collector,
        Method::POST,
        &format!("/api/sessions/{}/resume", a.session),
        Some(json!({})),
    )
    .await;
    assert!(status.is_success());
    let resumed = tokio::time::timeout(BOUND, deliveries.recv()).await.unwrap().unwrap();
    assert_eq!(resumed.session, a.session);
    assert_ne!(resumed.bearer, a.bearer);
    initialize(&plain, &resumed).await;
    assert!(tool(&plain, &resumed, "memory_recent", json!({})).await.is_some());
    let (status, _) = raw_rpc(
        &plain,
        &a.url,
        &a.bearer,
        json!({"jsonrpc":"2.0","id":"old","method":"tools/list"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    park(&owner_client, &plain, &collector, &resumed).await;
    let (status, _) = owner(
        &owner_client,
        &collector,
        Method::POST,
        &format!("/api/hats/{hat_a}/purge"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt(&owner_client, &collector, &hat_a).await["status"], "confirmed");
    assert_eq!(entry_count(&memory_data, &ns_a.id), 0);
    assert_marker_erased(&memory_data, MARKER_A);
    assert!(
        !tool(&plain, &b, "memory_search", json!({"query":MARKER_B}))
            .await
            .unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    memory.stop();
    park(&owner_client, &plain, &collector, &b).await;
    let (status, _) = owner(
        &owner_client,
        &collector,
        Method::POST,
        &format!("/api/hats/{hat_b}/purge"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        receipt(&owner_client, &collector, &hat_b).await["status"],
        "unconfirmed"
    );
    assert!(
        entry_count(&memory_data, &ns_b.id) > 0,
        "unreachable external copy remains honestly present"
    );
    memory = memory_server(&memory_binary, &base, &memory_data, memory_addr).0;
    wait_memory(&plain, &memory_url, &ns_b).await;
    let (status, _) = raw_rpc(
        &plain,
        &memory_url,
        &ns_b.tool,
        json!({"jsonrpc":"2.0","id":"revoked","method":"tools/list","params":{}}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cold restart cannot resurrect a revoked grant lease"
    );
    assert!(
        entry_count(&memory_data, &ns_b.id) > 0,
        "access denial does not imply deletion"
    );
    let (status, cleanup) = raw_rpc(
        &plain,
        &memory_url,
        &ns_b.lifecycle,
        json!({
        "jsonrpc":"2.0","id":"cleanup","method":"hennery_plugin_forget","params":{
            "request_id":"fixture-cleanup-after-unconfirmed","namespace_epoch":grant_b.epoch,
            "target":{"kind":"scope"}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleanup["result"]["status"], "confirmed");
    assert_eq!(entry_count(&memory_data, &ns_b.id), 0);
    assert_marker_erased(&memory_data, MARKER_B);
    assert_eq!(
        receipt(&owner_client, &collector, &hat_b).await["status"],
        "unconfirmed",
        "manual cleanup does not rewrite earlier collector evidence"
    );
    println!(
        "{}",
        json!({"proof":"real collector + optional memory process + signed scripted host","model_used":false,"tools":["memory_recent","memory_remember","memory_search","memory_brief"],"namespaces":2,"cross_hat_search":"empty","park_resume_bearer":"rotated; old denied","online_purge":"confirmed; body+FTS/WAL marker absent","offline_purge":"unconfirmed; copy retained and cold access denied","authenticated_cleanup":"confirmed; historical core receipt remains unconfirmed"})
    );
    memory.stop();
    drop(_host);
    core.stop();
}

fn assert_marker_erased(data: &Path, marker: &str) {
    for name in ["memory.db", "memory.db-wal"] {
        if let Ok(bytes) = std::fs::read(data.join(name)) {
            assert!(
                !bytes.windows(marker.len()).any(|s| s == marker.as_bytes()),
                "fixture marker remains in {name}"
            );
        }
    }
}
