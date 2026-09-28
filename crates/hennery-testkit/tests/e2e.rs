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
        TOKEN,
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
        ..FakeScript::default()
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
        ..FakeScript::default()
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

/// A start whose delivery is unknown (the connection died before a reply)
/// still creates a session that may go on to start; the 503 must carry its
/// id, or the caller has no way to ever look it up again.
#[tokio::test]
async fn a_start_with_unknown_delivery_reports_503_with_the_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let c = client();

    // Register a fake host connection directly (no real socket): once the
    // collector sends `start_session` on it, drop the connection before any
    // reply arrives, so the request resolves as DeliveryUnknown rather than
    // a rejection.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let conn_id = collector
        .state
        .hub
        .register("host-1", tx)
        .expect("register fake host")
        .conn_id;
    collector.state.hub.mark_ready("host-1", conn_id);
    let hub = collector.state.hub.clone();
    tokio::spawn(async move {
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

    let row = collector.state.store.session(session_id).unwrap().unwrap();
    assert_eq!(
        row.lifecycle, "starting",
        "an unknown-delivery start must not be marked failed"
    );
}

async fn lifecycle_is(collector: &Collector, session: &str, want: &str) {
    wait_for(&format!("lifecycle {want}"), || async {
        let row = collector.state.store.session(session).unwrap().unwrap();
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
    let c = client();
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
    let c = client();
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
    let c = client();
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
    let c = client();
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
    let c = client();
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
        collector.state.store.session(&session).unwrap().unwrap().lifecycle,
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

/// SIGKILLs the process group led by the pid in the file, on every path.
struct ReapGroup(std::path::PathBuf);

impl Drop for ReapGroup {
    fn drop(&mut self) {
        if let Some(pid) = pid_from(&self.0) {
            // SAFETY: killpg(2) on the group of an adapter this test started.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
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
    let c = client();
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
    let c = client();
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
    let c = client();
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
    let c = client();
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
