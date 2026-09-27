//! End to end: a real collector (in process), a real host (in process) and the
//! fake ACP adapter as a real child process, talking over real sockets.

use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::auth::DevToken;
use hennery_proto::rest::{EventDto, PromptResponse, StartSessionResponse};
use hennery_sessions::{AppState, store::Store};
use hennery_testkit::{FakeScript, SCRIPT_ENV};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

const TOKEN: &str = "dev-token";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Collector {
    async fn start(db: &Path, addr: Option<SocketAddr>) -> Self {
        let listener = tokio::net::TcpListener::bind(addr.unwrap_or_else(|| "127.0.0.1:0".parse().unwrap()))
            .await
            .expect("bind collector");
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(Store::open(db).unwrap(), DevToken::new(TOKEN));
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

fn start_host(collector: SocketAddr, data_dir: &Path, script: &FakeScript) {
    let mut cfg = HostConfig::new(
        format!("ws://{collector}/api/hosts/ws"),
        "host-1",
        TOKEN,
        data_dir.to_path_buf(),
    );
    cfg.reconnect_min = Duration::from_millis(100);
    cfg.reconnect_max = Duration::from_millis(500);
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    cfg.agents.insert("fake".into(), fake);
    cfg.agents.insert(
        "broken".into(),
        AgentCommand::parse("/nonexistent/hennery-test-adapter").unwrap(),
    );
    tokio::spawn(async move {
        hennery_host::run(cfg).await.unwrap();
    });
}

fn client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
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
        let hosts: Vec<String> = c.get(&url).send().await.ok()?.json().await.ok()?;
        hosts.contains(&"host-1".to_string()).then_some(())
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
    let c = client();
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
    };
    start_host(collector.addr, &dir.path().join("host"), &slow);
    let c = client();
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
    };
    start_host(addr, &dir.path().join("host"), &script);
    let c = client();
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
    let c = client();
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
    let c = client();
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
