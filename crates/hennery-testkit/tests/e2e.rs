//! End to end: a real collector (in process), a real host (in process) and the
//! fake ACP adapter as a real child process, talking over real sockets.

use hennery_host::identity::HostKey;
use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame};
use hennery_proto::rest::{EventDto, HostItem, PromptResponse, StartSessionResponse};
use hennery_sessions::{AppState, store::Store};
use hennery_testkit::{FakeScript, SCRIPT_ENV};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

/// The key `host-1` is paired with in every collector here.
fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

/// Pair `host-1` (a no-op for a collector restarted over the same database).
fn pair_host(hosts: &Hosts) {
    let enrollment = Enrollment {
        public_key: host_key().public_key_hex(),
        name: "test".into(),
        host_version: "test".into(),
        platform: "test".into(),
    };
    hosts.register("host-1", &enrollment, 0).unwrap();
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Collector {
    async fn start(db: &Path, addr: Option<SocketAddr>) -> Self {
        Self::start_with(db, addr, hennery_sessions::offline::OFFLINE_THRESHOLD).await
    }

    /// A collector that presumes a host's sessions parked once it has been
    /// offline for `offline`.
    async fn start_with(db: &Path, addr: Option<SocketAddr>, offline: Duration) -> Self {
        let listener = tokio::net::TcpListener::bind(addr.unwrap_or_else(|| "127.0.0.1:0".parse().unwrap()))
            .await
            .expect("bind collector");
        let addr = listener.local_addr().unwrap();
        let hosts = Hosts::open(db).unwrap();
        pair_host(&hosts);
        let mut state = AppState::new(Store::open(db).unwrap(), hosts, Operator::open(db).unwrap());
        state.offline_threshold = offline;
        let task = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, task }
    }

    async fn stop(self) {
        self.state.shutdown.cancel();
        self.task.await.unwrap().unwrap();
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

fn start_host(collector: SocketAddr, data_dir: &Path, script: &FakeScript) -> tokio::task::JoinHandle<()> {
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    start_host_with(collector, data_dir, fake)
}

/// A host whose `fake` agent is `fake` (plus a `broken` one that cannot
/// spawn). Aborting the returned task is a host restart: its actors end and
/// kill their adapters.
fn start_host_with(collector: SocketAddr, data_dir: &Path, fake: AgentCommand) -> tokio::task::JoinHandle<()> {
    let cfg = host_config(collector, data_dir, fake);
    tokio::spawn(async move {
        hennery_host::run(cfg).await.unwrap();
    })
}

fn host_config(collector: SocketAddr, data_dir: &Path, fake: AgentCommand) -> HostConfig {
    let mut cfg = HostConfig::new(
        format!("ws://{collector}/api/hosts/ws"),
        "host-1",
        host_key(),
        data_dir.to_path_buf(),
    );
    cfg.reconnect_min = Duration::from_millis(100);
    cfg.reconnect_max = Duration::from_millis(500);
    cfg.agents.insert("fake".into(), fake);
    cfg.agents.insert(
        "broken".into(),
        AgentCommand::parse("/nonexistent/hennery-test-adapter").unwrap(),
    );
    cfg
}

fn client(collector: &Collector) -> reqwest::Client {
    hennery_testkit::operator_client(&collector.state.operator)
}

async fn wait_for<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(v) = probe().await {
            return v;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_host_connected(c: &reqwest::Client, collector: &Collector) {
    let url = collector.url("/api/hosts");
    wait_for("host connection", || async {
        let hosts: Vec<HostItem> = c.get(&url).send().await.ok()?.json().await.ok()?;
        hosts.iter().any(|h| h.host_id == "host-1" && h.connected).then_some(())
    })
    .await;
}

async fn start_session(c: &reqwest::Client, collector: &Collector) -> String {
    let resp = c
        .post(collector.url("/api/sessions"))
        .json(&json!({ "host_id": "host-1", "agent": "fake", "cwd": std::env::temp_dir() }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202, "{}", resp.text().await.unwrap());
    resp.json::<StartSessionResponse>().await.unwrap().session_id
}

fn text(s: &str) -> Value {
    json!([{ "type": "text", "text": s }])
}

async fn events(c: &reqwest::Client, collector: &Collector, session: &str) -> Vec<EventDto> {
    c.get(collector.url(&format!("/api/sessions/{session}/events")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// Concatenated text of every agent_message_chunk in the timeline.
fn agent_text(events: &[EventDto]) -> String {
    events
        .iter()
        .filter(|e| e.kind == "acp_update")
        .filter_map(|e| e.body["payload"]["update"]["content"]["text"].as_str())
        .collect()
}

fn turn_ends(events: &[EventDto]) -> Vec<&EventDto> {
    events.iter().filter(|e| e.kind == "turn_ended").collect()
}

#[tokio::test]
async fn a_prompt_streams_the_agents_reply_and_ends_the_turn_once() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;

    let session = start_session(&c, &collector).await;
    let resp = c
        .post(collector.url(&format!("/api/sessions/{session}/prompt")))
        .json(&json!({ "content": text("hi") }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    let turn = resp.json::<PromptResponse>().await.unwrap().turn_id;

    let evs = wait_for("turn end", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    assert_eq!(agent_text(&evs), "Hello world");
    let ends = turn_ends(&evs);
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].body["turn_id"], turn);
    assert_eq!(ends[0].body["outcome"], "completed");
    // The user's turn precedes the agent's reply.
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    let user = kinds
        .iter()
        .position(|k| *k == "user_turn")
        .expect("user turn recorded");
    let first_update = kinds.iter().position(|k| *k == "acp_update").unwrap();
    assert!(user < first_update, "{kinds:?}");
}

#[tokio::test]
async fn empty_prompts_and_overlapping_prompts_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let slow = FakeScript {
        chunks: vec!["a".into(), "b".into(), "c".into()],
        chunk_delay_ms: 300,
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &slow);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));

    let empty = c
        .post(&prompt_url)
        .json(&json!({ "content": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(empty.status(), 400);

    let first = c
        .post(&prompt_url)
        .json(&json!({ "content": text("one") }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 202);
    let second = c
        .post(&prompt_url)
        .json(&json!({ "content": text("two") }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 409);
    assert_eq!(second.json::<Value>().await.unwrap()["code"], "turn_in_progress");
}

#[tokio::test]
async fn a_collector_restart_mid_turn_loses_nothing_and_duplicates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let collector = Collector::start(&db, None).await;
    let addr = collector.addr;
    let script = FakeScript {
        chunks: (1..=6).map(|n| format!("{n}.")).collect(),
        chunk_delay_ms: 250,
        ..FakeScript::default()
    };
    start_host(addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let resp = c
        .post(collector.url(&format!("/api/sessions/{session}/prompt")))
        .json(&json!({ "content": text("go") }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);

    // Wait for the first chunk, then take the collector down mid-turn.
    wait_for("first chunk", || async {
        (!agent_text(&events(&c, &collector, &session).await).is_empty()).then_some(())
    })
    .await;
    collector.stop().await;
    tokio::time::sleep(Duration::from_millis(700)).await; // chunks keep arriving at the host
    let collector = Collector::start(&db, Some(addr)).await;

    let evs = wait_for("turn end after restart", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    assert_eq!(agent_text(&evs), "1.2.3.4.5.6.");
    assert_eq!(turn_ends(&evs).len(), 1);
    let mut seqs: Vec<u64> = evs.iter().filter_map(|e| e.host_seq).collect();
    let n = seqs.len();
    seqs.dedup();
    assert_eq!(seqs.len(), n, "duplicate host seqs stored");
}

#[tokio::test]
async fn the_session_stream_replays_from_last_event_id() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    c.post(collector.url(&format!("/api/sessions/{session}/prompt")))
        .json(&json!({ "content": text("hi") }))
        .send()
        .await
        .unwrap();
    let evs = wait_for("turn end", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    let resume_after = evs[1].event_id;

    let resp = c
        .get(collector.url(&format!("/api/stream/sessions/{session}")))
        .header("last-event-id", resume_after.to_string())
        .send()
        .await
        .unwrap();
    let mut body = resp.bytes_stream();
    let mut buf = String::new();
    let expected_ids: Vec<String> = evs
        .iter()
        .filter(|e| e.event_id > resume_after)
        .map(|e| format!("id: {}", e.event_id))
        .collect();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !expected_ids.iter().all(|id| buf.contains(id.as_str())) {
        let chunk = tokio::time::timeout_at(deadline, body.next())
            .await
            .expect("stream stalled")
            .unwrap()
            .unwrap();
        buf.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(
        !buf.contains(&format!("id: {}\n", evs[0].event_id)),
        "replayed an event before Last-Event-ID: {buf}"
    );
}

#[tokio::test]
async fn a_start_that_fails_on_the_host_is_reported_as_502() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;

    for (agent, expected) in [("broken", "start_failed"), ("not-configured", "unknown_agent")] {
        let resp = c
            .post(collector.url("/api/sessions"))
            .json(&json!({ "host_id": "host-1", "agent": agent, "cwd": std::env::temp_dir() }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 502, "agent {agent}");
        assert_eq!(resp.json::<Value>().await.unwrap()["code"], expected, "agent {agent}");
    }
}

/// A start whose delivery is unknown (the connection died before a reply)
/// still creates a session that may go on to start; the 503 must carry its
/// id, or the caller has no way to ever look it up again.
#[tokio::test]
async fn a_start_with_unknown_delivery_reports_503_with_the_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let c = client(&collector);

    // Register a fake host connection directly (no real socket): it answers
    // the cwd's `resolve_path` as a host would (plan 5c), and once the
    // collector sends `start_session` on it, drops the connection before
    // any reply arrives, so the request resolves as DeliveryUnknown rather
    // than a rejection.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let conn_id = collector
        .state
        .hub
        .register("host-1", tx, Capabilities(vec![Capability::ResolvePath]))
        .expect("register fake host")
        .conn_id;
    collector.state.hub.mark_ready("host-1", conn_id);
    let hub = collector.state.hub.clone();
    let canonical = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    tokio::spawn(async move {
        if let Some(CollectorFrame::ResolvePath { request_id, .. }) = rx.recv().await {
            let answer = HostFrame::ResolvedPath {
                request_id,
                canonical: canonical.to_str().unwrap().into(),
                exists: true,
                is_dir: true,
            };
            hub.probe_reply("host-1", conn_id, answer);
        }
        rx.recv().await; // the StartSession frame; proves it was delivered
        hub.unregister("host-1", conn_id);
    });

    let resp = c
        .post(collector.url("/api/sessions"))
        .json(&json!({ "host_id": "host-1", "agent": "fake", "cwd": std::env::temp_dir() }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "delivery_unknown");
    let session_id = body["session_id"].as_str().expect("session_id present in the 503 body");

    let row = collector.state.store.find_session(session_id).unwrap().unwrap();
    assert_eq!(
        row.lifecycle, "starting",
        "an unknown-delivery start must not be marked failed"
    );
}

async fn lifecycle_is(collector: &Collector, session: &str, want: &str) {
    wait_for(&format!("lifecycle {want}"), || async {
        let row = collector.state.store.find_session(session).unwrap().unwrap();
        (row.lifecycle == want).then_some(())
    })
    .await;
}

fn of_kind<'a>(events: &'a [EventDto], kind: &str) -> Vec<&'a EventDto> {
    events.iter().filter(|e| e.kind == kind).collect()
}

async fn post_json(c: &reqwest::Client, url: String, body: Value) -> (u16, Value) {
    let resp = c.post(url).json(&body).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

fn slow_script(chunks: usize) -> FakeScript {
    FakeScript {
        chunks: (1..=chunks).map(|n| format!("{n}.")).collect(),
        chunk_delay_ms: 200,
        ..FakeScript::default()
    }
}

fn pid_from(path: &Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

async fn wait_dead(pid: i32) {
    wait_for("adapter subtree killed", || async {
        (!hennery_testkit::pid_alive(pid)).then_some(())
    })
    .await;
}

#[tokio::test]
async fn an_adapter_crash_mid_turn_parks_the_session_with_a_scrubbed_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = FakeScript {
        chunks: vec!["1.".into(), "2.".into(), "3.".into()],
        chunk_delay_ms: 100,
        exit_after_chunks: Some(1),
        stderr_lines: vec!["using key sk-live-abcdefghijk".into()],
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
    assert_eq!(
        post_json(&c, prompt_url.clone(), json!({ "content": text("go") }))
            .await
            .0,
        202
    );

    lifecycle_is(&collector, &session, "parked").await;
    let evs = events(&c, &collector, &session).await;
    let ends = turn_ends(&evs);
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].body["outcome"], "interrupted");
    let exited = of_kind(&evs, "adapter_exited");
    assert_eq!(exited.len(), 1);
    let tail = exited[0].body["stderr_tail"].as_str().unwrap();
    assert!(
        tail.contains("sk-[redacted]") && !tail.contains("abcdefghijk"),
        "{tail}"
    );
    assert_eq!(of_kind(&evs, "session_parked")[0].body["reason"], "adapter_exited");
    let (status, body) = post_json(&c, prompt_url, json!({ "content": text("again") })).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
}

#[tokio::test]
async fn park_then_close_through_the_api_kill_the_adapters_whole_group() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let pid_file = dir.path().join("grandchild.pid");
    let script = FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..slow_script(20)
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let grandchild = wait_for("grandchild pid", || async { pid_from(&pid_file) }).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
    assert_eq!(post_json(&c, prompt_url, json!({ "content": text("go") })).await.0, 202);

    let (status, body) = post_json(&c, collector.url(&format!("/api/sessions/{session}/park")), json!({})).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("parked")), "{body}");
    wait_dead(grandchild).await;
    let evs = events(&c, &collector, &session).await;
    assert_eq!(turn_ends(&evs)[0].body["outcome"], "interrupted");
    assert_eq!(of_kind(&evs, "session_parked")[0].body["reason"], "operator");

    // A parked session closes at once, collector-side.
    let (status, body) = post_json(&c, collector.url(&format!("/api/sessions/{session}/close")), json!({})).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("closed")), "{body}");
    let kinds: Vec<String> = events(&c, &collector, &session)
        .await
        .into_iter()
        .map(|e| e.kind)
        .collect();
    let tail: Vec<&str> = kinds.iter().rev().take(1).map(String::as_str).collect();
    assert_eq!(tail, ["operator_closed"], "{kinds:?}");
    assert!(kinds.contains(&"operator_parked".to_string()), "{kinds:?}");
}

#[tokio::test]
async fn closing_an_attached_session_waits_for_the_host_to_close_it() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let (status, body) = post_json(&c, collector.url(&format!("/api/sessions/{session}/close")), json!({})).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("closed")), "{body}");
    let kinds: Vec<String> = events(&c, &collector, &session)
        .await
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(
        kinds.ends_with(&["operator_closed".to_string(), "session_closed".to_string()]),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn a_host_restart_mid_turn_parks_and_interrupts_without_respawning() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let spawns = dir.path().join("spawns");
    let script = slow_script(20);
    // Counts adapter launches, then becomes the fake adapter.
    let counting = AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "echo spawned >> {}; exec {}",
                spawns.display(),
                env!("CARGO_BIN_EXE_hennery-fake-acp")
            ),
        ],
        env: vec![(SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap())],
    };
    let host = start_host_with(collector.addr, &dir.path().join("host"), counting.clone());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
    assert_eq!(
        post_json(&c, prompt_url.clone(), json!({ "content": text("go") }))
            .await
            .0,
        202
    );
    wait_for("first chunk", || async {
        (!agent_text(&events(&c, &collector, &session).await).is_empty()).then_some(())
    })
    .await;

    host.abort(); // the host process dies with its adapters
    let _ = host.await;
    start_host_with(collector.addr, &dir.path().join("host"), counting);

    lifecycle_is(&collector, &session, "parked").await;
    let evs = events(&c, &collector, &session).await;
    assert_eq!(of_kind(&evs, "host_restarted").len(), 1);
    let synthesized = of_kind(&evs, "turn_ended_synthesized");
    assert_eq!(synthesized.len(), 1);
    assert_eq!(synthesized[0].body["outcome"], "interrupted");
    assert!(turn_ends(&evs).is_empty(), "the dead host cannot have ended the turn");
    let (status, body) = post_json(&c, prompt_url, json!({ "content": text("again") })).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        std::fs::read_to_string(&spawns).unwrap().lines().count(),
        1,
        "no eager re-spawn"
    );
}

#[tokio::test]
async fn a_dropped_connection_mid_turn_parks_nothing_and_loses_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &slow_script(6));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
    assert_eq!(post_json(&c, prompt_url, json!({ "content": text("go") })).await.0, 202);
    wait_for("first chunk", || async {
        (!agent_text(&events(&c, &collector, &session).await).is_empty()).then_some(())
    })
    .await;

    collector.state.hub.disconnect("host-1");
    let evs = wait_for("turn end after the drop", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    assert_eq!(agent_text(&evs), "1.2.3.4.5.6.");
    assert_eq!(turn_ends(&evs)[0].body["outcome"], "completed");
    assert!(of_kind(&evs, "host_restarted").is_empty());
    assert!(of_kind(&evs, "turn_ended_synthesized").is_empty());
    assert_eq!(
        collector.state.store.find_session(&session).unwrap().unwrap().lifecycle,
        "active"
    );
}

/// The fake adapter behind a shell (the group leader, its pid written to
/// `pid_file`) that records a SIGTERM in `marker` and exits. The fake runs
/// in the background with the shell's stdin dup'ed in: a non-interactive
/// shell would otherwise give a background job `/dev/null` as stdin.
fn fake_recording_sigterm(marker: &Path, pid_file: &Path) -> AgentCommand {
    AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            r#"trap 'touch "$0"; exit 0' TERM; echo $$ > "$1"; exec 3<&0; "$2" <&3 3<&- & wait"#.into(),
            marker.to_string_lossy().into_owned(),
            pid_file.to_string_lossy().into_owned(),
            env!("CARGO_BIN_EXE_hennery-fake-acp").into(),
        ],
        env: vec![(
            SCRIPT_ENV.into(),
            serde_json::to_string(&FakeScript::default()).unwrap(),
        )],
    }
}

/// SIGKILLs the process group of the pid in the file, on every path. Its
/// group, not a group of that number: the adapter joins its guard's.
struct ReapGroup(std::path::PathBuf);

impl Drop for ReapGroup {
    fn drop(&mut self) {
        if let Some(pid) = pid_from(&self.0) {
            // SAFETY: getpgid(2) and killpg(2) on an adapter this test started.
            unsafe {
                let pgid = libc::getpgid(pid);
                if pgid > 0 {
                    libc::killpg(pgid, libc::SIGKILL);
                }
            }
        }
    }
}

#[tokio::test]
async fn host_shutdown_gives_every_adapter_its_sigterm_grace() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let marker = dir.path().join("sigterm");
    let pid_file = dir.path().join("adapter.pid");
    let _reap = ReapGroup(pid_file.clone());
    let cfg = host_config(
        collector.addr,
        &dir.path().join("host"),
        fake_recording_sigterm(&marker, &pid_file),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let host = tokio::spawn(hennery_host::run_until(cfg, async {
        let _ = stopped.await;
    }));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    lifecycle_is(&collector, &session, "active").await;
    let leader = pid_from(&pid_file).expect("the adapter wrapper never wrote its pid");

    let bound = hennery_host::adapter::KILL_GRACE + Duration::from_secs(2);
    let begun = std::time::Instant::now();
    stop.send(()).unwrap();
    tokio::time::timeout(bound, host)
        .await
        .expect("the host did not return within the kill grace")
        .unwrap()
        .unwrap();
    assert!(
        marker.exists(),
        "the host returned without giving its adapter SIGTERM (after {:?})",
        begun.elapsed()
    );
    // The host awaited its actor: the group leader is already reaped.
    assert!(
        !hennery_testkit::pid_alive(leader),
        "the host returned before its adapter exited"
    );
    collector.stop().await;
}

// Plan B: resume end to end (ACP core §12 scenarios 2, 3, 17).

fn history_and_state() -> FakeScript {
    FakeScript {
        replay: vec![
            json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "old question"}}),
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "old answer"}}),
            json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "ls"}),
            json!({"sessionUpdate": "available_commands_update", "availableCommands": []}),
            json!({"sessionUpdate": "from_the_future"}),
        ],
        ..FakeScript::default()
    }
}

async fn prompt_and_wait(c: &reqwest::Client, collector: &Collector, session: &str, ends: usize) -> Vec<EventDto> {
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let (status, body) = post_json(c, url, json!({ "content": text("hi") })).await;
    assert_eq!(status, 202, "{body}");
    wait_for("turn end", || async {
        let evs = events(c, collector, session).await;
        (turn_ends(&evs).len() == ends).then_some(evs)
    })
    .await
}

async fn resume(c: &reqwest::Client, collector: &Collector, session: &str) -> (u16, Value) {
    post_json(c, collector.url(&format!("/api/sessions/{session}/resume")), json!({})).await
}

fn assert_no_duplicate_seqs(evs: &[EventDto]) {
    let mut seqs: Vec<u64> = evs.iter().filter_map(|e| e.host_seq).collect();
    let n = seqs.len();
    seqs.sort_unstable();
    seqs.dedup();
    assert_eq!(seqs.len(), n, "duplicate host seqs stored");
}

#[tokio::test]
async fn a_parked_session_resumes_without_replaying_its_history() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &history_and_state());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt_and_wait(&c, &collector, &session, 1).await;
    let (status, body) = post_json(&c, collector.url(&format!("/api/sessions/{session}/park")), json!({})).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("parked")), "{body}");

    let (status, body) = resume(&c, &collector, &session).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("active")), "{body}");
    // The 202 fires once `session_started` is ingested (decision 3); the
    // trailing `available_commands_update` and `host_note` land afterward,
    // so wait for the last of the three before asserting on the sequence.
    let evs = wait_for("post-resume host_note", || async {
        let evs = events(&c, &collector, &session).await;
        let resumed = evs.iter().position(|e| e.kind == "operator_resumed")?;
        evs[resumed + 1..].iter().any(|e| e.kind == "host_note").then_some(evs)
    })
    .await;
    let resumed = evs.iter().position(|e| e.kind == "operator_resumed").unwrap();
    let after: Vec<String> = evs[resumed + 1..]
        .iter()
        .map(|e| match e.kind.as_str() {
            "acp_update" => format!(
                "acp_update:{}",
                e.body["payload"]["update"]["sessionUpdate"].as_str().unwrap()
            ),
            kind => kind.to_string(),
        })
        .collect();
    assert_eq!(
        after,
        ["session_started", "acp_update:available_commands_update", "host_note"]
    );
    assert!(
        evs[resumed + 3].body["text"]
            .as_str()
            .unwrap()
            .contains("from_the_future"),
        "{:?}",
        evs[resumed + 3]
    );

    // The resumed session takes a prompt; the transcript holds each reply once.
    let evs = prompt_and_wait(&c, &collector, &session, 2).await;
    assert_eq!(agent_text(&evs), "Hello worldHello world");
    assert_eq!(of_kind(&evs, "user_turn").len(), 2);
    assert_no_duplicate_seqs(&evs);
}

#[tokio::test]
async fn resuming_a_session_the_agent_has_no_record_of_fails_agent_has_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = FakeScript {
        load_error: Some(-32002),
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    post_json(&c, collector.url(&format!("/api/sessions/{session}/park")), json!({})).await;
    let (status, body) = resume(&c, &collector, &session).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (502, Some("agent_has_no_record")),
        "{body}"
    );
    let detail: Value = c
        .get(collector.url(&format!("/api/sessions/{session}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (detail["lifecycle"].as_str(), detail["failure_reason"].as_str()),
        (Some("failed"), Some("agent_has_no_record"))
    );
}

/// The whole restart path a user meets: the host restarts, the collector
/// parks its session, the operator resumes it on the new host process and
/// keeps working. With `lose_outbox`, the host also comes back with an empty
/// data dir: its seq counter restarts at 0 and only `resume_session`'s
/// `committed_seq` keeps the new frames from being taken as duplicates of
/// stored ones (§12 scenario 17).
async fn resume_after_a_host_restart(lose_outbox: bool) {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env.push((
        SCRIPT_ENV.into(),
        serde_json::to_string(&FakeScript::default()).unwrap(),
    ));
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt_and_wait(&c, &collector, &session, 1).await;

    host.abort();
    let _ = host.await;
    let data_dir = if lose_outbox {
        dir.path().join("host-fresh")
    } else {
        dir.path().join("host")
    };
    start_host_with(collector.addr, &data_dir, fake);
    lifecycle_is(&collector, &session, "parked").await;

    let (status, body) = resume(&c, &collector, &session).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("active")), "{body}");
    let evs = prompt_and_wait(&c, &collector, &session, 2).await;
    assert_eq!(agent_text(&evs), "Hello worldHello world");
    assert_no_duplicate_seqs(&evs);
    assert!(
        of_kind(&evs, "conflict").is_empty(),
        "new frames collided with stored seqs"
    );
}

#[tokio::test]
async fn a_session_parked_by_a_host_restart_resumes_on_the_restarted_host() {
    resume_after_a_host_restart(false).await;
}

#[tokio::test]
async fn a_session_resumes_on_a_host_that_lost_its_outbox() {
    resume_after_a_host_restart(true).await;
}

// Plan B2a: cancel end to end (ACP core §3.3 `cancel_turn`, §4.4).

#[tokio::test]
async fn a_cancel_mid_turn_ends_it_cancelled_once_and_the_next_prompt_runs() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &slow_script(20));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let (status, body) = post_json(&c, url, json!({ "content": text("long") })).await;
    assert_eq!(status, 202, "{body}");
    let turn = body["turn_id"].as_str().unwrap().to_string();

    let cancel_url = collector.url(&format!("/api/sessions/{session}/cancel"));
    let (status, body) = post_json(&c, cancel_url.clone(), json!({})).await;
    assert_eq!(
        (status, body),
        (202, json!({ "turn_id": turn, "outcome": "cancelled" }))
    );
    let evs = events(&c, &collector, &session).await;
    let ends = turn_ends(&evs);
    assert_eq!(ends.len(), 1, "{evs:?}");
    assert_eq!(
        (&ends[0].body["turn_id"], &ends[0].body["outcome"]),
        (&json!(turn), &json!("cancelled"))
    );
    assert!(
        agent_text(&evs).len() < "1.2.3.4.5.6.7.8.9.10.11.12.13.14.15.16.17.18.19.20.".len(),
        "the turn ran to its end"
    );
    // Nothing left to cancel; the session takes the next prompt.
    let (status, body) = post_json(&c, cancel_url, json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("no_open_turn")));
    lifecycle_is(&collector, &session, "active").await;
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let (status, body) = post_json(&c, url, json!({ "content": text("again") })).await;
    assert_eq!(status, 202, "{body}");
}

// Plan B2b: model, axes and mode end to end (ACP core §12 scenario 1 and
// the fake-adapter half of its live gates).

fn config_script(log: &Path) -> FakeScript {
    FakeScript {
        config_options: hennery_testkit::sample_config_options(),
        model_switch_sets_mode: Some("default".into()),
        config_log: Some(log.to_string_lossy().into_owned()),
        ..FakeScript::default()
    }
}

async fn catalog(c: &reqwest::Client, collector: &Collector, session: &str) -> Value {
    let url = collector.url(&format!("/api/sessions/{session}/catalog"));
    let resp = c.get(url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

fn current(catalog: &Value) -> (Value, Value, Value) {
    (
        catalog["model"].clone(),
        catalog["mode"].clone(),
        catalog["axes"].clone(),
    )
}

/// Scenario 1: one request starts the session with model, mode and axes;
/// the mode goes last, so the model's clamp cannot undo it, and the
/// announced catalogue is the one after the switches.
#[tokio::test]
async fn a_start_with_model_mode_and_axes_announces_the_catalogue_after_the_switches() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &config_script(&log));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let (status, body) = post_json(
        &c,
        collector.url("/api/sessions"),
        json!({
            "host_id": "host-1", "agent": "fake", "cwd": std::env::temp_dir(),
            "model": "large", "mode": "plan", "axes": {"effort": "high"}
        }),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    let session = body["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "model=large\neffort=high\nmode=plan\n"
    );
    let catalog = catalog(&c, &collector, &session).await;
    assert_eq!(
        current(&catalog),
        (json!("large"), json!("plan"), json!({"effort": "high", "fast": false}))
    );
    assert_eq!(catalog["config_options"].as_array().map(Vec::len), Some(4));
}

/// The model-switch gate against the fake: the read-back is what the
/// adapter reports, including the mode it clamped, and a model it refuses
/// is never reported as current.
#[tokio::test]
async fn a_model_switch_answers_with_the_adapters_read_back_and_a_bogus_model_is_never_current() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &config_script(&log));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let url = collector.url(&format!("/api/sessions/{session}/config"));
    let (status, body) = post_json(&c, url.clone(), json!({ "config_id": "mode", "value": "plan" })).await;
    assert_eq!((status, body["mode"].as_str()), (202, Some("plan")), "{body}");
    let (status, body) = post_json(&c, url.clone(), json!({ "config_id": "model", "value": "large" })).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(
        (body["model"].as_str(), body["mode"].as_str()),
        (Some("large"), Some("default")),
        "the adapter clamped the mode: {body}"
    );
    let (status, body) = post_json(&c, url, json!({ "config_id": "model", "value": "bogus" })).await;
    assert_eq!((status, body["code"].as_str()), (502, Some("config_failed")), "{body}");
    assert_eq!(catalog(&c, &collector, &session).await["model"], "large");
    let evs = events(&c, &collector, &session).await;
    assert_eq!(of_kind(&evs, "config_applied").len(), 2);
}

/// The resume gate against the fake: a mode the agent chose by itself mid
/// turn is stored from its live update, and after a host restart the
/// resume applies it to the new adapter, which starts from its default.
#[tokio::test]
async fn a_mode_the_agent_chose_survives_a_host_restart_and_resume() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = FakeScript {
        prompt_sets_mode: Some("bypass".into()),
        ..config_script(&log)
    };
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt_and_wait(&c, &collector, &session, 1).await;
    assert_eq!(catalog(&c, &collector, &session).await["mode"], "bypass");
    assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");

    host.abort();
    let _ = host.await;
    start_host_with(collector.addr, &dir.path().join("host"), fake);
    lifecycle_is(&collector, &session, "parked").await;
    let (status, body) = resume(&c, &collector, &session).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("active")), "{body}");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "mode=bypass\n");
    let catalog = catalog(&c, &collector, &session).await;
    assert_eq!(current(&catalog).1, json!("bypass"));
}

// Plan (2): permission and elicitation end to end (ACP core §4.6, §12
// scenarios 5 and 7 to 10, and the elicitation live gate against the fake).

fn asking(asks: Vec<hennery_testkit::FakeAsk>) -> FakeScript {
    FakeScript {
        asks,
        ..FakeScript::default()
    }
}

async fn detail(c: &reqwest::Client, collector: &Collector, session: &str) -> Value {
    let resp = c
        .get(collector.url(&format!("/api/sessions/{session}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

/// The id of the question the session is blocked on, once it is.
async fn open_question(c: &reqwest::Client, collector: &Collector, session: &str) -> String {
    wait_for("an open question", || async {
        let detail = detail(c, collector, session).await;
        let pending = detail["pending"].as_array()?.first()?.clone();
        (detail["activity"] == "blocked").then(|| pending["pending_id"].as_str().unwrap().to_string())
    })
    .await
}

async fn answer(c: &reqwest::Client, collector: &Collector, session: &str, pending: &str, body: Value) -> (u16, Value) {
    let url = collector.url(&format!("/api/sessions/{session}/pending/{pending}/answer"));
    post_json(c, url, body).await
}

async fn prompt(c: &reqwest::Client, collector: &Collector, session: &str) {
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let (status, body) = post_json(c, url, json!({ "content": text("go") })).await;
    assert_eq!(status, 202, "{body}");
}

fn delivered(collector: &Collector, pending: &str) -> Option<bool> {
    collector.state.store.pending_item(pending).unwrap().unwrap().delivered
}

/// Both kinds of question through the API, and the elicitation gate: the
/// form's content reaches the agent and the answer is reported delivered.
#[tokio::test]
async fn questions_answered_through_the_api_reach_the_agent_and_are_reported_delivered() {
    use hennery_testkit::FakeAsk;
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = asking(vec![FakeAsk::Permission, FakeAsk::Elicitation]);
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;

    let first = open_question(&c, &collector, &session).await;
    let (status, body) = answer(&c, &collector, &session, &first, json!({"option_id": "allow"})).await;
    assert_eq!(status, 202, "{body}");
    let second = wait_for("the second question", || async {
        let detail = detail(&c, &collector, &session).await;
        let id = detail["pending"].as_array()?.first()?["pending_id"]
            .as_str()?
            .to_string();
        (id != first).then_some(id)
    })
    .await;
    let form = json!({"action": "accept", "content": {"name": "notes.txt"}});
    assert_eq!(answer(&c, &collector, &session, &second, form).await.0, 202);

    let evs = wait_for("turn end", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    assert_eq!(
        agent_text(&evs),
        r#"permission:selected:allowelicitation:accept:{"name":"notes.txt"}Hello world"#
    );
    let results = of_kind(&evs, "answer_result");
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|e| e.body["delivered"] == true), "{results:?}");
    assert_eq!(
        (delivered(&collector, &first), delivered(&collector, &second)),
        (Some(true), Some(true))
    );
    let detail = detail(&c, &collector, &session).await;
    assert_eq!(
        (detail["activity"].as_str(), &detail["pending"]),
        (Some("idle"), &json!([]))
    );
}

#[tokio::test]
async fn stop_with_a_question_open_ends_the_turn_cancelled_and_closes_the_question() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = asking(vec![hennery_testkit::FakeAsk::Permission]);
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;
    let pending = open_question(&c, &collector, &session).await;

    let url = collector.url(&format!("/api/sessions/{session}/cancel"));
    let (status, body) = post_json(&c, url, json!({})).await;
    assert_eq!((status, body["outcome"].as_str()), (202, Some("cancelled")), "{body}");
    let item = collector.state.store.pending_item(&pending).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value((item.state, item.reason)).unwrap(),
        json!(["cancelled", "turn_cancelled"])
    );
    let (status, body) = answer(&c, &collector, &session, &pending, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
    assert!(agent_text(&events(&c, &collector, &session).await).contains("permission:cancelled"));
}

/// Scenario 5, with a question open.
#[tokio::test]
async fn an_adapter_crash_with_a_question_open_cancels_it_adapter_lost() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = FakeScript {
        crash_while_asking: true,
        ..asking(vec![hennery_testkit::FakeAsk::Permission])
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;
    lifecycle_is(&collector, &session, "parked").await;

    let evs = events(&c, &collector, &session).await;
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    let at = kinds
        .iter()
        .position(|k| *k == "pending_opened")
        .expect("the question was recorded");
    assert_eq!(
        kinds[at..],
        [
            "pending_opened",
            "turn_ended",
            "pending_resolved",
            "adapter_exited",
            "session_parked"
        ]
    );
    assert_eq!(evs[at + 2].body["reason"], "adapter_lost");
    let pending = evs[at].body["pending_id"].as_str().unwrap();
    let (status, body) = answer(&c, &collector, &session, pending, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
}

/// Scenario 7, with a question open: the restarted host holds none, so it
/// is cancelled after the resend, and nothing is re-spawned.
#[tokio::test]
async fn a_host_restart_with_a_question_open_cancels_it_host_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = asking(vec![hennery_testkit::FakeAsk::Permission]);
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;
    let pending = open_question(&c, &collector, &session).await;

    host.abort();
    let _ = host.await;
    start_host_with(collector.addr, &dir.path().join("host"), fake);
    lifecycle_is(&collector, &session, "parked").await;
    let evs = events(&c, &collector, &session).await;
    let cancelled = of_kind(&evs, "pending_cancelled");
    assert_eq!(cancelled.len(), 1, "{evs:?}");
    assert_eq!(
        cancelled[0].body,
        json!({"pending_id": pending, "reason": "host_restarted"})
    );
    let (status, body) = answer(&c, &collector, &session, &pending, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
}

/// Scenarios 8 and 9: while its host is away past the offline threshold
/// the session is presumed parked with its question still open, and an
/// answer given then is delivered once the host is back, the adapter still
/// waiting for it.
#[tokio::test]
async fn a_question_outlasts_its_host_being_away_and_an_answer_given_meanwhile_is_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let collector = Collector::start(&db, None).await;
    let addr = collector.addr;
    start_host(
        addr,
        &dir.path().join("host"),
        &asking(vec![hennery_testkit::FakeAsk::Permission]),
    );
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;
    let pending = open_question(&c, &collector, &session).await;
    collector.stop().await;

    // A collector the host cannot reach (another address) presumes its
    // sessions parked, and takes the answer.
    let away = Collector::start_with(&db, None, Duration::from_millis(200)).await;
    wait_for("presumed parked", || async {
        let row = away.state.store.find_session(&session).unwrap().unwrap();
        (row.lifecycle == "parked" && row.presumed_parked).then_some(())
    })
    .await;
    let item = away.state.store.pending_item(&pending).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(item.state).unwrap(),
        "open",
        "the host may still hold it"
    );
    let (status, body) = answer(&c, &away, &session, &pending, json!({"option_id": "allow"})).await;
    assert_eq!(status, 202, "{body}");
    away.stop().await;

    // Back where the host looks for it: reattached, then the queue drains.
    let collector = Collector::start(&db, Some(addr)).await;
    lifecycle_is(&collector, &session, "active").await;
    let evs = wait_for("turn end", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    assert!(!of_kind(&evs, "reattached").is_empty());
    assert!(
        agent_text(&evs).starts_with("permission:selected:allow"),
        "{}",
        agent_text(&evs)
    );
    assert_eq!(delivered(&collector, &pending), Some(true));
}

// Plan 3a: a revoked host stops its adapters (ACP core §3.5, kernel spec §4.3).

#[tokio::test]
async fn a_revoked_host_stops_its_adapters_and_exits() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let pid_file = dir.path().join("grandchild.pid");
    let script = FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..slow_script(20)
    };
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let host = tokio::spawn(hennery_host::run(host_config(
        collector.addr,
        &dir.path().join("host"),
        fake,
    )));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let grandchild = wait_for("grandchild pid", || async { pid_from(&pid_file) }).await;

    let revoked = c.delete(collector.url("/api/hosts/host-1")).send().await.unwrap();
    assert_eq!(revoked.status(), 200);
    // Kicked, the host reconnects, is told it is revoked, and stops.
    let outcome = tokio::time::timeout(Duration::from_secs(30), host)
        .await
        .expect("the revoked host stops")
        .unwrap();
    let err = outcome.expect_err("a revoked host ends with an error");
    assert!(format!("{err:#}").contains("revoked"), "{err:#}");
    wait_dead(grandchild).await;
    let row = collector.state.store.find_session(&session).unwrap().unwrap();
    assert_eq!((row.lifecycle.as_str(), row.presumed_parked), ("parked", true));
}

// Plan 6a: images in prompts, through a real host to the agent.

/// `len` bytes of a PNG, different for each `seed`.
fn png(seed: u8, len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..len - 8).map(|i| seed.wrapping_mul(31).wrapping_add((i % 251) as u8)));
    bytes
}

fn image(bytes: &[u8]) -> Value {
    use base64::Engine;
    json!({ "type": "image", "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(bytes) })
}

fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// The most a prompt may carry (ACP core §11): 16 MiB of images, one frame
/// of about 21.4 MiB to the host, past tungstenite's default 16 MiB, which
/// the host must read whole. The fake agent echoes each image's type and
/// hash, so the bytes it decoded are the bytes sent.
#[tokio::test]
async fn a_prompt_at_the_limit_reaches_the_agent_whole() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let quiet = FakeScript {
        chunks: vec![],
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &quiet);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;

    let images: Vec<Vec<u8>> = [5 << 20, 5 << 20, 5 << 20, 1 << 20]
        .iter()
        .enumerate()
        .map(|(n, len)| png(n as u8 + 1, *len))
        .collect();
    let mut content = vec![json!({ "type": "text", "text": "compare these" })];
    content.extend(images.iter().map(|bytes| image(bytes)));
    let (status, body) = post_json(
        &c,
        collector.url(&format!("/api/sessions/{session}/prompt")),
        json!({ "content": content }),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    let evs = wait_for("turn end", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    let echoed: String = images
        .iter()
        .map(|bytes| format!("image:image/png:{}\n", sha(bytes)))
        .collect();
    assert_eq!(agent_text(&evs), echoed);
    assert_eq!(turn_ends(&evs)[0].body["outcome"], "completed");
}

/// An agent whose `initialize` offers no images is never sent one (plan
/// 6a, decision 2): its host refuses the prompt, 409
/// `images_unsupported`, the turn is freed, and text still goes.
#[tokio::test]
async fn an_agent_that_takes_no_images_is_never_sent_one() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = FakeScript {
        no_images: true,
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let (status, body) = post_json(&c, url.clone(), json!({ "content": [image(&png(1, 64))] })).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("images_unsupported")),
        "{body}"
    );

    let (status, body) = post_json(&c, url, json!({ "content": text("hi") })).await;
    assert_eq!(status, 202, "{body}");
    let evs = wait_for("turn end", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    assert_eq!(agent_text(&evs), "Hello world");
    assert_eq!(of_kind(&evs, "user_turn").len(), 1);
}

/// Plan 6c: the project picker against a real host, from its workspace
/// roots (ACP core §7, §9).
#[tokio::test]
async fn the_project_picker_lists_and_browses_a_real_hosts_projects() {
    use hennery_proto::frames::{DirEntry, Project};
    use hennery_proto::rest::{DirectoryListing, HostProjects};
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let projects = tempfile::tempdir().unwrap();
    let root = hennery_host::projects::canonical(projects.path()).unwrap();
    std::fs::create_dir_all(format!("{root}/app/.git")).unwrap();
    std::fs::create_dir_all(format!("{root}/notes")).unwrap();
    let fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let mut cfg = host_config(collector.addr, &dir.path().join("host"), fake);
    cfg.workspace_roots = vec![root.clone().into()];
    cfg.home = None;
    tokio::spawn(async move { hennery_host::run(cfg).await.unwrap() });
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;

    let hosts: Vec<HostItem> = c
        .get(collector.url("/api/hosts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hosts[0].workspace_roots, [root.as_str()]);
    let listed: HostProjects = c
        .get(collector.url("/api/hosts/host-1/projects"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        listed,
        HostProjects {
            recents_hat_id: hosts[0].default_hat_id.clone(),
            recents: vec![],
            items: vec![Project {
                path: format!("{root}/app")
            }],
            partial: false,
            home: None,
        }
    );
    let listing: DirectoryListing = c
        .get(collector.url("/api/hosts/host-1/browse"))
        .query(&[("path", &root)])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listing.path, root);
    assert_eq!(
        listing.entries,
        [
            DirEntry {
                name: "app".into(),
                git: true
            },
            DirEntry {
                name: "notes".into(),
                git: false
            }
        ]
    );
    let resp = c
        .get(collector.url("/api/hosts/host-1/browse"))
        .query(&[("path", "/")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    collector.stop().await;
}
