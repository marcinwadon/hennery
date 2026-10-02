//! The agent's own transcript on its host (plan 9d): a real collector, a
//! real host and the fake adapter standing in for the agents, under roots
//! of the test's own (never the operator's `~/.claude`).

use hennery_host::agent_home::Registry;
use hennery_host::identity::HostKey;
use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{AgentHome, ForgetKind, ForgetReason};
use hennery_proto::rest::{
    DeleteResult, HostItem, HostRemovalItem, HostRemovalState, RemovalPending, RemovalState, StartSessionResponse,
};
use hennery_sessions::{AppState, store::Store};
use hennery_testkit::{FakeScript, SCRIPT_ENV};
use serde_json::json;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// An agent session id as Claude's SDK makes them.
const AGENT_SESSION: &str = "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
}

impl Collector {
    async fn start(db: &Path) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hosts = Hosts::open(db).unwrap();
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        hosts.register("host-1", &enrollment, 0).unwrap();
        let state = AppState::new(Store::open(db).unwrap(), hosts, Operator::open(db).unwrap());
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn client(&self) -> reqwest::Client {
        hennery_testkit::operator_client(&self.state.operator)
    }
}

/// The umask these tests assume: the root and kind directories must not be
/// writable by group or others (B3), and a 002 umask would make every one
/// so. Set for the whole test binary; every test here wants the same.
fn usual_umask() {
    // SAFETY: umask(2) cannot fail.
    unsafe { libc::umask(0o022) };
}

/// The test's own data roots, canonical (`/var` is a link on macOS).
struct Roots {
    _dir: tempfile::TempDir,
    base: PathBuf,
}

impl Roots {
    fn new() -> Self {
        usual_umask();
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        for sub in ["claude", "codex", "host", "work"] {
            std::fs::create_dir(base.join(sub)).unwrap();
        }
        Self { _dir: dir, base }
    }

    fn claude(&self) -> PathBuf {
        self.base.join("claude")
    }

    fn codex(&self) -> PathBuf {
        self.base.join("codex")
    }

    fn data_dir(&self) -> PathBuf {
        self.base.join("host")
    }

    fn work(&self) -> PathBuf {
        self.base.join("work")
    }
}

fn fake(script: &FakeScript, env: &[(&str, &Path)]) -> AgentCommand {
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    for (name, value) in env {
        fake.env.push((name.to_string(), value.to_string_lossy().into_owned()));
    }
    fake
}

/// The script for a fake answering with `id`.
fn answering(id: &str) -> FakeScript {
    FakeScript {
        session_id: Some(id.into()),
        ..FakeScript::default()
    }
}

/// A host whose `claude` and `codex` agents are the fake, each under its
/// own root, answering `session/new` with `script`'s id.
fn host_config(collector: SocketAddr, roots: &Roots, script: &FakeScript) -> HostConfig {
    let mut cfg = HostConfig::new(
        format!("ws://{collector}/api/hosts/ws"),
        "host-1",
        host_key(),
        roots.data_dir(),
    );
    cfg.reconnect_min = Duration::from_millis(100);
    cfg.reconnect_max = Duration::from_millis(300);
    cfg.agents
        .insert("claude".into(), fake(script, &[("CLAUDE_CONFIG_DIR", &roots.claude())]));
    cfg.agents
        .insert("codex".into(), fake(script, &[("CODEX_HOME", &roots.codex())]));
    cfg
}

fn start_host(cfg: HostConfig) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        hennery_host::run(cfg).await.unwrap();
    })
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

async fn connected(collector: &Collector, yes: bool) {
    let c = collector.client();
    let url = collector.url("/api/hosts");
    wait_for("the host's connection", || async {
        let hosts: Vec<HostItem> = c.get(&url).send().await.ok()?.json().await.ok()?;
        (hosts.iter().any(|h| h.host_id == "host-1" && h.connected) == yes).then_some(())
    })
    .await;
}

async fn start_session(collector: &Collector, agent: &str, roots: &Roots) -> String {
    let resp = collector
        .client()
        .post(collector.url("/api/sessions"))
        .json(&json!({ "host_id": "host-1", "agent": agent, "cwd": roots.work() }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202, "{}", resp.text().await.unwrap());
    resp.json::<StartSessionResponse>().await.unwrap().session_id
}

/// `DELETE /api/sessions/{id}`: its status and its body as text.
async fn delete(collector: &Collector, session: &str) -> (u16, String) {
    let resp = collector
        .client()
        .delete(collector.url(&format!("/api/sessions/{session}")))
        .timeout(Duration::from_secs(40))
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

async fn deleted(collector: &Collector, session: &str) -> (DeleteResult, String) {
    let (status, body) = delete(collector, session).await;
    assert_eq!(status, 200, "{body}");
    (serde_json::from_str(&body).unwrap(), body)
}

async fn removals(collector: &Collector) -> Vec<HostRemovalItem> {
    collector
        .client()
        .get(collector.url("/api/settings/host-removals"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// The recorded homes of a session, as the store keeps them.
fn recorded(db: &Path, session: &str) -> Option<serde_json::Value> {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row("SELECT agent_home FROM sessions WHERE id = ?1", [session], |r| {
        r.get::<_, Option<String>>(0)
    })
    .unwrap()
    .map(|raw| serde_json::from_str(&raw).unwrap())
}

fn home(root: &Path) -> AgentHome {
    AgentHome {
        root: root.to_str().unwrap().into(),
        sqlite_root: None,
    }
}

/// Both timeouts in one place: a live host answers before the collector
/// gives up on it.
#[test]
fn the_hosts_deadline_is_inside_the_collectors_wait() {
    assert!(hennery_host::forget::FORGET_DEADLINE < hennery_sessions::forget::FORGET_WAIT);
}

/// Decision 1, B1: a started session's host registers where its agent keeps
/// its data, then reports it in `session_started`, and the collector
/// records it on the session.
#[tokio::test]
async fn a_started_session_registers_its_agent_home_and_the_collector_records_it() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    start_host(host_config(collector.addr, &roots, &answering(AGENT_SESSION)));
    connected(&collector, true).await;
    let session = start_session(&collector, "claude", &roots).await;
    let root = roots.claude();
    assert_eq!(
        recorded(&db, &session),
        Some(json!([{ "agent_session_id": AGENT_SESSION, "root": root.to_str().unwrap() }]))
    );
    let registry = Registry::open(&roots.data_dir().join(hennery_host::agent_home::FILE)).unwrap();
    assert!(registry.contains("claude", AGENT_SESSION, &home(&root)).unwrap());
    assert!(
        !registry
            .contains("claude", AGENT_SESSION, &home(&roots.codex()))
            .unwrap()
    );
}

/// A Codex rollout of `AGENT_SESSION` under `root`, as Codex names it, and
/// another thread's beside it.
fn codex_rollouts(root: &Path) -> (PathBuf, PathBuf) {
    let day = root.join("sessions/2026/10/02");
    std::fs::create_dir_all(&day).unwrap();
    let own = day.join(format!("rollout-2026-10-02T10-00-00-{AGENT_SESSION}.jsonl"));
    let other = day.join("rollout-2026-10-02T10-00-00-1b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3.jsonl");
    std::fs::write(&own, "rollout").unwrap();
    std::fs::write(&other, "keep").unwrap();
    (own, other)
}

/// The fake app-server (Codex 0.155.1), logging to `log`.
fn fake_codex(log: &Path) -> AgentCommand {
    let script = hennery_testkit::FakeCodex {
        version: "0.155.1".into(),
        log: log.to_str().unwrap().into(),
        ..hennery_testkit::FakeCodex::default()
    };
    let mut command = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-codex")).unwrap();
    command.env.push((
        hennery_testkit::CODEX_SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    command
}

/// Decision 9 end to end: a Codex session deleted over HTTP has its thread
/// deleted by the bundled app-server under its recorded home: `removed`,
/// the record gone, Codex's other residue named in the notes (O12), and no
/// root in the answer (B2).
#[tokio::test]
async fn a_delete_over_http_deletes_the_codex_thread_through_the_app_server() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    let log = roots.base.join("codex.log");
    let mut cfg = host_config(collector.addr, &roots, &answering(AGENT_SESSION));
    cfg.codex_app_server = Some(fake_codex(&log));
    start_host(cfg);
    connected(&collector, true).await;
    let session = start_session(&collector, "codex", &roots).await;
    let (own, other) = codex_rollouts(&roots.codex());
    let (result, body) = deleted(&collector, &session).await;
    let removal = result.host_transcript;
    assert_eq!(removal.state, RemovalState::Removed, "{body}");
    assert_eq!(removal.notes, hennery_sessions::forget::CODEX_NOTES, "{body}");
    assert!(!own.exists() && other.exists());
    assert!(std::fs::read_to_string(&log).unwrap().contains("thread/delete"));
    assert!(!body.contains(roots.base.to_str().unwrap()), "{body}");
    assert!(removals(&collector).await.is_empty());
}

/// Decision 10 end to end: with no app-server to run, the fallback
/// archives through the adapter and removes the rollouts itself: `partial`,
/// with `codex_database_copies` left for good, its notes with it, and the
/// record final.
#[tokio::test]
async fn a_codex_delete_without_the_app_server_falls_back_and_says_what_remains() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    let archive_log = roots.base.join("archive.log");
    let script = FakeScript {
        codex_archive_log: Some(archive_log.to_str().unwrap().into()),
        ..answering(AGENT_SESSION)
    };
    start_host(host_config(collector.addr, &roots, &script));
    connected(&collector, true).await;
    let session = start_session(&collector, "codex", &roots).await;
    let (own, other) = codex_rollouts(&roots.codex());
    let (result, body) = deleted(&collector, &session).await;
    let removal = result.host_transcript;
    assert_eq!(removal.state, RemovalState::Partial, "{body}");
    assert_eq!(
        removal.remaining.iter().map(|r| (r.kind, r.reason)).collect::<Vec<_>>(),
        [(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly)],
        "{body}"
    );
    for note in hennery_sessions::forget::CODEX_FALLBACK_NOTES {
        assert!(removal.notes.iter().any(|n| n == note), "{note}: {body}");
    }
    assert!(!own.exists() && other.exists());
    assert!(std::fs::read_to_string(&archive_log).unwrap().contains("CODEX_HOME="));
    let listed = removals(&collector).await;
    let [item] = listed.as_slice() else {
        panic!("{listed:?}");
    };
    assert_eq!((item.agent.as_str(), item.state), ("codex", HostRemovalState::Final));
    // The list says what the fallback leaves too.
    let listed_notes = &item.last_result.as_ref().expect("a result").notes;
    for note in hennery_sessions::forget::CODEX_FALLBACK_NOTES {
        assert!(listed_notes.iter().any(|n| n == note), "{note}: {listed_notes:?}");
    }
}

/// The parent's rule, B1: a Codex forget for a home the host never
/// registered spawns no app-server and no adapter; and a Codex session
/// with no recorded home (its `CODEX_HOME` did not resolve at the start)
/// is final, `no_recorded_home`, with nothing sent to the host at all.
#[tokio::test]
async fn a_codex_home_mismatched_or_absent_spawns_nothing() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    let log = roots.base.join("codex.log");
    let archive_log = roots.base.join("archive.log");
    let script = FakeScript {
        codex_archive_log: Some(archive_log.to_str().unwrap().into()),
        ..answering(AGENT_SESSION)
    };
    let mut cfg = host_config(collector.addr, &roots, &script);
    cfg.codex_app_server = Some(fake_codex(&log));
    start_host(cfg);
    connected(&collector, true).await;
    // Mismatched: the stored home rewritten to another root.
    let session = start_session(&collector, "codex", &roots).await;
    let elsewhere = roots.base.join("elsewhere");
    let (theirs, _) = codex_rollouts(&elsewhere);
    let lie = json!([{ "agent_session_id": AGENT_SESSION, "root": elsewhere.to_str().unwrap() }]);
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE sessions SET agent_home = ?1 WHERE id = ?2",
            [lie.to_string(), session.clone()],
        )
        .unwrap();
    let (result, body) = deleted(&collector, &session).await;
    assert_eq!(
        result
            .host_transcript
            .remaining
            .iter()
            .map(|r| r.reason)
            .collect::<Vec<_>>(),
        [ForgetReason::UnknownToHost],
        "{body}"
    );
    assert!(theirs.exists());
    // Absent: no recorded home at all.
    let session = start_session(&collector, "codex", &roots).await;
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute("UPDATE sessions SET agent_home = NULL WHERE id = ?1", [session.clone()])
        .unwrap();
    let (result, body) = deleted(&collector, &session).await;
    assert_eq!(
        result
            .host_transcript
            .remaining
            .iter()
            .map(|r| r.reason)
            .collect::<Vec<_>>(),
        [ForgetReason::NoRecordedHome],
        "{body}"
    );
    assert!(!log.exists(), "an app-server was spawned");
    assert!(!archive_log.exists(), "an adapter's delete ran");
}

/// B5 as ruled (the hybrid) end to end: a record whose app-server timed
/// out `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` times in a row is sent
/// flagged `fallback` at its host's return, and the host spawns no Codex:
/// the archive and the walk only, the session's own rollout gone, the
/// other thread's kept, and the record final with `codex_database_copies`.
#[tokio::test]
async fn a_record_past_the_app_server_timeouts_is_forgotten_by_the_fallback_alone() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    let log = roots.base.join("codex.log");
    let archive_log = roots.base.join("archive.log");
    let script = FakeScript {
        codex_archive_log: Some(archive_log.to_str().unwrap().into()),
        ..answering(AGENT_SESSION)
    };
    let mut cfg = host_config(collector.addr, &roots, &script);
    cfg.codex_app_server = Some(fake_codex(&log));
    let host = start_host(cfg.clone());
    connected(&collector, true).await;
    let session = start_session(&collector, "codex", &roots).await;
    let (own, other) = codex_rollouts(&roots.codex());
    host.abort();
    connected(&collector, false).await;
    let (result, body) = deleted(&collector, &session).await;
    assert_eq!(result.host_transcript.state, RemovalState::Pending, "{body}");
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE host_forgets SET app_server_timeouts = ?1",
            [hennery_sessions::forget::APP_SERVER_TIMEOUTS_BEFORE_FALLBACK],
        )
        .unwrap();
    start_host(cfg);
    let item = wait_for("the flagged retry at the host's return", || async {
        let listed = removals(&collector).await;
        (listed[0].state == HostRemovalState::Final).then(|| listed[0].clone())
    })
    .await;
    let result = item.last_result.expect("a result");
    assert_eq!(
        result.remaining.iter().map(|r| (r.kind, r.reason)).collect::<Vec<_>>(),
        [(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly)],
        "{result:?}"
    );
    assert!(!own.exists() && other.exists());
    assert!(!log.exists(), "a Codex was spawned");
    assert!(std::fs::read_to_string(&archive_log).unwrap().contains("CODEX_HOME="));
}

/// Decision 5: a delete while the host is away answers `pending,
/// host_offline`; the record goes when the host is back.
#[tokio::test]
async fn a_delete_while_the_host_is_away_is_pending_and_retried_at_its_return() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    let cfg = host_config(collector.addr, &roots, &answering(AGENT_SESSION));
    let host = start_host(cfg.clone());
    connected(&collector, true).await;
    let session = start_session(&collector, "codex", &roots).await;
    host.abort();
    connected(&collector, false).await;
    let (result, body) = deleted(&collector, &session).await;
    assert_eq!(
        (result.host_transcript.state, result.host_transcript.pending),
        (RemovalState::Pending, Some(RemovalPending::HostOffline)),
        "{body}"
    );
    let listed = removals(&collector).await;
    assert_eq!((listed[0].state, listed[0].attempts), (HostRemovalState::Pending, 0));
    start_host(cfg);
    wait_for("the retry at the host's return", || async {
        let listed = removals(&collector).await;
        (listed[0].attempts == 1).then_some(())
    })
    .await;
}

/// B1: a forget acts only on a home the host registered. A collector
/// that names another root (here, its stored home rewritten) is answered
/// `unknown_to_host`, final, and nothing there is touched.
#[tokio::test]
async fn a_root_the_host_never_registered_is_refused() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    start_host(host_config(collector.addr, &roots, &answering(AGENT_SESSION)));
    connected(&collector, true).await;
    let session = start_session(&collector, "claude", &roots).await;
    let elsewhere = roots.base.join("elsewhere");
    std::fs::create_dir_all(elsewhere.join("projects/p")).unwrap();
    let victim = elsewhere.join(format!("projects/p/{AGENT_SESSION}.jsonl"));
    std::fs::write(&victim, "keep").unwrap();
    let lie = json!([{ "agent_session_id": AGENT_SESSION, "root": elsewhere.to_str().unwrap() }]);
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE sessions SET agent_home = ?1 WHERE id = ?2",
            [lie.to_string(), session.clone()],
        )
        .unwrap();
    let (result, body) = deleted(&collector, &session).await;
    assert_eq!(
        result
            .host_transcript
            .remaining
            .iter()
            .map(|r| r.reason)
            .collect::<Vec<_>>(),
        [ForgetReason::UnknownToHost],
        "{body}"
    );
    assert_eq!(result.host_transcript.state, RemovalState::Partial);
    assert_eq!(removals(&collector).await[0].state, HostRemovalState::Final);
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
}

/// Decision 8, O10: an agent session id that is not one the agent writes
/// is refused `invalid` before anything is looked at: final.
#[tokio::test]
async fn an_invalid_agent_session_id_is_refused_for_good() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    // The fake's default id, `fake-session-1`, is no UUID.
    start_host(host_config(collector.addr, &roots, &FakeScript::default()));
    connected(&collector, true).await;
    let session = start_session(&collector, "claude", &roots).await;
    let (result, body) = deleted(&collector, &session).await;
    assert_eq!(
        result
            .host_transcript
            .remaining
            .iter()
            .map(|r| r.reason)
            .collect::<Vec<_>>(),
        [ForgetReason::InvalidId],
        "{body}"
    );
    assert_eq!(removals(&collector).await[0].state, HostRemovalState::Final);
}

/// B8: an agent session another kept session still refers to is not
/// forgotten: `shared`, and no record.
#[tokio::test]
async fn an_agent_session_another_session_still_uses_is_left_shared() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    start_host(host_config(collector.addr, &roots, &answering(AGENT_SESSION)));
    connected(&collector, true).await;
    let first = start_session(&collector, "claude", &roots).await;
    let _second = start_session(&collector, "claude", &roots).await;
    let (result, body) = deleted(&collector, &first).await;
    assert_eq!(result.host_transcript.state, RemovalState::Partial, "{body}");
    assert_eq!(
        result
            .host_transcript
            .remaining
            .iter()
            .map(|r| r.reason)
            .collect::<Vec<_>>(),
        [ForgetReason::Shared]
    );
    assert!(removals(&collector).await.is_empty());
}

/// `GET /api/settings/host-removals` lists what is left; a step-up
/// `DELETE` dismisses one.
#[tokio::test]
async fn a_host_removal_is_listed_until_it_is_dismissed() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    start_host(host_config(collector.addr, &roots, &FakeScript::default()));
    connected(&collector, true).await;
    let session = start_session(&collector, "claude", &roots).await;
    deleted(&collector, &session).await;
    let listed = removals(&collector).await;
    assert_eq!(listed.len(), 1);
    // The Claude notes come with every listed Claude result.
    let notes = &listed[0].last_result.as_ref().unwrap().notes;
    assert!(notes.iter().any(|n| n.contains("context clear")), "{notes:?}");
    let url = collector.url(&format!("/api/settings/host-removals/{}", listed[0].id));
    let resp = collector.client().delete(&url).send().await.unwrap();
    assert_eq!(resp.status(), 204);
    assert!(removals(&collector).await.is_empty());
    assert_eq!(collector.client().delete(&url).send().await.unwrap().status(), 404);
}

/// The transcript and the other entries a Claude session leaves under its
/// root, and one name beside them that is not the session's.
fn claude_files(roots: &Roots) -> Vec<PathBuf> {
    let root = roots.claude();
    std::fs::create_dir_all(root.join("projects/-work")).unwrap();
    std::fs::create_dir_all(root.join(format!("file-history/{AGENT_SESSION}"))).unwrap();
    std::fs::create_dir_all(root.join("debug")).unwrap();
    let files = vec![
        root.join(format!("projects/-work/{AGENT_SESSION}.jsonl")),
        root.join(format!("projects/-work/{AGENT_SESSION}.ccr-tip.json")),
        root.join(format!("file-history/{AGENT_SESSION}/edit-1")),
        root.join(format!("debug/{AGENT_SESSION}.txt")),
    ];
    for file in &files {
        std::fs::write(file, "content").unwrap();
    }
    std::fs::write(root.join("projects/-work/other.jsonl"), "keep").unwrap();
    files
}

/// Decisions 7 and 8 end to end: a Claude session deleted over HTTP has
/// its transcript removed on its host, `removed`, the context-clear note
/// with it, and nothing names a path (B2).
#[tokio::test]
async fn a_delete_over_http_removes_the_claude_transcript_on_its_host() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    start_host(host_config(collector.addr, &roots, &answering(AGENT_SESSION)));
    connected(&collector, true).await;
    let session = start_session(&collector, "claude", &roots).await;
    let files = claude_files(&roots);
    let (result, body) = deleted(&collector, &session).await;
    assert_eq!(result.host_transcript.state, RemovalState::Removed, "{body}");
    assert!(result.host_transcript.notes[0].contains("context clear"), "{body}");
    for file in files {
        assert!(!file.exists(), "{}", file.display());
    }
    assert!(roots.claude().join("projects/-work/other.jsonl").exists());
    assert!(
        !body.contains(roots.base.to_str().unwrap()) && !body.contains("-work"),
        "{body}"
    );
    assert!(removals(&collector).await.is_empty());
}

/// Decision 5 end to end: deleted while its host is away, pending; at the
/// host's return the transcript goes, and so does the record.
#[tokio::test]
async fn a_claude_transcript_deleted_while_its_host_was_away_goes_at_its_return() {
    let roots = Roots::new();
    let db = roots.base.join("hennery.db");
    let collector = Collector::start(&db).await;
    let cfg = host_config(collector.addr, &roots, &answering(AGENT_SESSION));
    let host = start_host(cfg.clone());
    connected(&collector, true).await;
    let session = start_session(&collector, "claude", &roots).await;
    host.abort();
    connected(&collector, false).await;
    let files = claude_files(&roots);
    let (result, _) = deleted(&collector, &session).await;
    assert_eq!(result.host_transcript.state, RemovalState::Pending);
    assert!(files.iter().all(|f| f.exists()));
    start_host(cfg);
    wait_for("the record removed at the host's return", || async {
        removals(&collector).await.is_empty().then_some(())
    })
    .await;
    for file in files {
        assert!(!file.exists(), "{}", file.display());
    }
}
