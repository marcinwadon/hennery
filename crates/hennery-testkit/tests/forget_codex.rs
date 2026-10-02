//! The Codex path of a forget on the host (plan 9d decisions 9, 10, 12;
//! B4–B6), run directly against roots of the test's own. The fake
//! app-server (`hennery-fake-codex`) stands in for Codex 0.155.1's CLI and
//! the fake adapter for codex-acp: the contract test checks the exact frames
//! the host sends, and every answer maps to its outcome. These run on Linux
//! CI as on macOS (R4).

use hennery_host::AgentCommand;
use hennery_host::forget::{FORGET_DEADLINE, Forget, ForgetContext, Forgotten, forget};
use hennery_host::runtime::manifest::Manifest;
use hennery_host::walk::Hooks;
use hennery_proto::frames::{AgentHome, ForgetKind, ForgetReason};
use hennery_testkit::{CODEX_SCRIPT_ENV, FakeCodex, FakeDelete, FakeInitialize, FakeScript, SCRIPT_ENV};
use std::collections::HashMap;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A thread id as Codex makes them (a UUID v7, lowercase).
const ID: &str = "019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b";
const STAMP: &str = "2026-10-02T10-00-00";

/// The session's rollouts Codex may have written: plain, a reverted
/// thread's (`_<rollout id>`), compressed, and one archived already.
const OWN: [&str; 3] = [
    "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
    "sessions/2026/10/01/rollout-2026-10-01T09-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b_019a0b1c-2d3e-7f40-8a5b-111111111111.jsonl.zst",
    "archived_sessions/rollout-2026-09-30T08-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
];

/// Entries beside them that are not the session's, or out of the walk's
/// reach, and stay.
const NEIGHBOURS: [&str; 12] = [
    // Another thread.
    "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-000000000000.jsonl",
    // Another thread, whose reverted rollout id is this session's id.
    "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-000000000000_019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
    // The id with a suffix, a prefix, or another extension.
    "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1bf.jsonl",
    "sessions/2026/10/02/xrollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
    "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl.bak",
    // Past `sessions/`'s three levels (a fourth named like a day too), in
    // a directory that is no date, and below `archived_sessions/`'s top
    // (in one named like a year too).
    "sessions/2026/10/02/deeper/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
    "sessions/2026/10/02/03/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
    "sessions/backup/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
    "archived_sessions/old/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
    "archived_sessions/2026/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
    // Codex's other files: never the host's to touch (O12).
    "history.jsonl",
    "session_index.jsonl",
];

/// The umask these tests assume (B3), as `forget_claude.rs` sets it.
fn usual_umask() {
    // SAFETY: umask(2) cannot fail.
    unsafe { libc::umask(0o022) };
}

struct Root {
    _dir: tempfile::TempDir,
    base: PathBuf,
}

impl Root {
    fn new() -> Self {
        usual_umask();
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        for sub in ["codex", "host", "home", "outside", "sqlite"] {
            std::fs::create_dir(base.join(sub)).unwrap();
        }
        Self { _dir: dir, base }
    }

    fn root(&self) -> PathBuf {
        self.base.join("codex")
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    fn outside(&self) -> PathBuf {
        self.base.join("outside")
    }

    fn codex_log(&self) -> PathBuf {
        self.base.join("codex.log")
    }

    fn archive_log(&self) -> PathBuf {
        self.base.join("archive.log")
    }

    /// The fake app-server answering as `delete` says, version 0.155.1.
    fn fake(&self, delete: FakeDelete) -> FakeCodex {
        FakeCodex {
            version: "0.155.1".into(),
            log: self.codex_log().to_str().unwrap().into(),
            delete,
            ..FakeCodex::default()
        }
    }

    /// The forget's context: `codex` as the fake app-server, and as the
    /// fake adapter in codex-acp's archive mode. Each also carries
    /// variables of its own that the forget must override or strip (B6).
    fn ctx(&self, codex: FakeCodex) -> ForgetContext {
        let mut app_server = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-codex")).unwrap();
        app_server
            .env
            .push((CODEX_SCRIPT_ENV.into(), serde_json::to_string(&codex).unwrap()));
        let script = FakeScript {
            codex_archive_log: Some(self.archive_log().to_str().unwrap().into()),
            ..FakeScript::default()
        };
        let mut adapter = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
        adapter
            .env
            .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
        let outside = self.outside().to_str().unwrap().to_string();
        for command in [&mut app_server, &mut adapter] {
            command.env.push(("CODEX_HOME".into(), outside.clone()));
            command.env.push(("CODEX_SQLITE_HOME".into(), outside.clone()));
            command
                .env
                .push(("CLAUDE_CODE_PROJECT_DIR_NAME".into(), "elsewhere".into()));
        }
        ForgetContext {
            agents: HashMap::from([("codex".to_string(), adapter)]),
            data_dir: self.base.join("host"),
            home: Some(self.base.join("home")),
            hooks: Hooks::default(),
            account: None,
            codex_app_server: Some(app_server),
            codex_pin: Manifest::embedded().codex_app_server,
            deadline: FORGET_DEADLINE,
        }
    }

    fn forget_at(&self, root: &Path, sqlite_root: Option<&Path>) -> Forget {
        Forget {
            agent: "codex".into(),
            agent_session_id: ID.into(),
            agent_home: AgentHome {
                root: root.to_str().unwrap().into(),
                sqlite_root: sqlite_root.map(|p| p.to_str().unwrap().into()),
            },
            fallback: false,
        }
    }

    async fn forget_with(&self, ctx: &ForgetContext) -> Forgotten {
        forget(ctx, &self.forget_at(&self.root(), None)).await
    }

    /// The session's rollouts and the neighbours that stay.
    fn populate(&self) {
        for rel in OWN.iter().chain(NEIGHBOURS.iter()) {
            let path = self.at(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "rollout").unwrap();
        }
    }

    fn exists(&self, rel: &str) -> bool {
        std::fs::symlink_metadata(self.at(rel)).is_ok()
    }

    fn own_left(&self) -> usize {
        OWN.iter().filter(|rel| self.exists(rel)).count()
    }

    fn neighbours_kept(&self) {
        for rel in NEIGHBOURS {
            assert!(self.exists(rel), "{rel} was removed");
        }
    }

    /// The fake app-server's log: its spawns, and the lines it read.
    fn codex_lines(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.codex_log())
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn spawns(&self) -> Vec<serde_json::Value> {
        self.codex_lines()
            .into_iter()
            .filter_map(|l| l.get("spawn").cloned())
            .collect()
    }

    fn received(&self) -> Vec<String> {
        self.codex_lines()
            .into_iter()
            .filter_map(|l| l.get("recv").and_then(|r| r.as_str().map(str::to_string)))
            .collect()
    }

    fn archived(&self) -> Option<String> {
        std::fs::read_to_string(self.archive_log()).ok()
    }
}

fn reasons(forgotten: &Forgotten) -> Vec<(ForgetKind, ForgetReason, bool)> {
    forgotten
        .remaining
        .iter()
        .map(|r| (r.what.kind, r.reason, r.retry))
        .collect()
}

fn count_left(forgotten: &Forgotten, kind: ForgetKind) -> u32 {
    forgotten
        .remaining
        .iter()
        .filter(|r| r.what.kind == kind)
        .map(|r| r.what.count)
        .sum()
}

fn removed(forgotten: &Forgotten, kind: ForgetKind) -> u32 {
    forgotten
        .removed
        .iter()
        .filter(|w| w.kind == kind)
        .map(|w| w.count)
        .sum()
}

/// The exact frames the host sends Codex 0.155.1 (decision 9): the pinned
/// `initialize`, then `thread/delete` with the thread id. One JSON object
/// per line, no `jsonrpc` field, as codex-acp sends them.
fn pinned_frames() -> [String; 2] {
    [
        r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"hennery","title":"hennery","version":"1"}}}"#
            .to_string(),
        format!(r#"{{"id":2,"method":"thread/delete","params":{{"threadId":"{ID}"}}}}"#),
    ]
}

/// The spawn record the fake wrote for `args`, as the forget ran it (B6):
/// `CODEX_HOME` the recorded root whatever the command's own said, the
/// project-name override stripped, the root its cwd.
fn assert_spawned(spawn: &serde_json::Value, args: &[&str], root: &Path, sqlite: &str) {
    let root = root.to_str().unwrap();
    assert_eq!(spawn["args"], serde_json::json!(args), "{spawn}");
    assert_eq!(spawn["CODEX_HOME"], root, "{spawn}");
    assert_eq!(spawn["CODEX_SQLITE_HOME"], sqlite, "{spawn}");
    assert_eq!(spawn["CLAUDE_CODE_PROJECT_DIR_NAME"], "-", "{spawn}");
    assert_eq!(spawn["cwd"], root, "{spawn}");
}

/// The contract test (decision 9): the version is asked first, then the
/// app-server gets exactly the pinned frames; Codex's answer and its
/// unsolicited notifications are read past; the rollouts are gone, the
/// check afterwards finds none (B4), and nothing else is touched. No
/// fallback runs.
#[tokio::test]
async fn the_app_server_gets_exactly_the_pinned_frames_and_the_thread_goes() {
    let root = Root::new();
    root.populate();
    let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::Delete))).await;
    assert_eq!(reasons(&forgotten), [], "{forgotten:?}");
    assert_eq!(removed(&forgotten, ForgetKind::Transcript), OWN.len() as u32);
    assert_eq!(root.own_left(), 0);
    root.neighbours_kept();
    let spawns = root.spawns();
    assert_eq!(spawns.len(), 2, "{spawns:?}");
    assert_spawned(&spawns[0], &["--version"], &root.root(), "-");
    assert_spawned(&spawns[1], &["app-server"], &root.root(), "-");
    assert_eq!(root.received(), pinned_frames());
    assert_eq!(root.archived(), None, "no fallback");
    // A second forget finds nothing: Codex's "no rollout found" counts for
    // nothing, the check afterwards decides.
    let again = root.forget_with(&root.ctx(root.fake(FakeDelete::Delete))).await;
    assert_eq!(reasons(&again), [], "{again:?}");
    assert_eq!(removed(&again, ForgetKind::Transcript), 0);
}

/// B6: a recorded `CODEX_SQLITE_HOME` is the one the app-server gets; with
/// none recorded, the command's own is stripped (above: `-`).
#[tokio::test]
async fn a_recorded_sqlite_home_is_the_app_servers() {
    let root = Root::new();
    root.populate();
    let sqlite = root.base.join("sqlite");
    let forgotten = forget(
        &root.ctx(root.fake(FakeDelete::Delete)),
        &root.forget_at(&root.root(), Some(&sqlite)),
    )
    .await;
    assert_eq!(reasons(&forgotten), [], "{forgotten:?}");
    let spawns = root.spawns();
    assert_eq!(spawns.len(), 2, "{spawns:?}");
    for (spawn, args) in spawns.iter().zip([["--version"], ["app-server"]]) {
        assert_spawned(spawn, &args, &root.root(), sqlite.to_str().unwrap());
    }
}

/// Decision 9, B5: each of `thread/delete`'s refusals maps to its own
/// reason, retryable only for a live worker; nothing is removed, and the
/// fallback never runs after one.
#[tokio::test]
async fn each_refusal_of_thread_delete_maps_to_its_reason_and_never_falls_back() {
    for (delete, reason, retry) in [
        (FakeDelete::ForkedHistory, ForgetReason::ForkedHistory, false),
        (FakeDelete::Ephemeral, ForgetReason::Ephemeral, false),
        (FakeDelete::LiveWorker, ForgetReason::InProgress, true),
    ] {
        let root = Root::new();
        root.populate();
        let forgotten = root.forget_with(&root.ctx(root.fake(delete))).await;
        assert_eq!(
            reasons(&forgotten),
            [
                (ForgetKind::Session, reason, retry),
                (ForgetKind::Transcript, reason, retry)
            ],
            "{delete:?}: {forgotten:?}"
        );
        assert_eq!(
            count_left(&forgotten, ForgetKind::Transcript),
            OWN.len() as u32,
            "{delete:?}"
        );
        assert_eq!(root.own_left(), OWN.len(), "{delete:?}");
        assert_eq!(root.received(), pinned_frames(), "{delete:?}");
        assert_eq!(root.archived(), None, "{delete:?}: no fallback after a refusal");
    }
}

/// B5: an error `thread/delete` answered that is none of its known
/// refusals (a failure midway) is retried, and never followed by the
/// fallback either.
#[tokio::test]
async fn an_unknown_failure_of_thread_delete_is_retried_and_never_falls_back() {
    let root = Root::new();
    root.populate();
    let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::Internal))).await;
    assert_eq!(
        reasons(&forgotten),
        [
            (ForgetKind::Session, ForgetReason::IoError, true),
            (ForgetKind::Transcript, ForgetReason::IoError, true)
        ],
        "{forgotten:?}"
    );
    assert_eq!(root.archived(), None);
}

/// B5: an app-server that ends once `thread/delete` was sent, answering
/// nothing, is retried and never followed by the fallback: the delete may
/// have run.
#[tokio::test]
async fn an_app_server_that_ends_after_the_delete_was_sent_never_falls_back() {
    let root = Root::new();
    root.populate();
    let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::Exit))).await;
    assert_eq!(
        reasons(&forgotten),
        [
            (ForgetKind::Session, ForgetReason::IoError, true),
            (ForgetKind::Transcript, ForgetReason::IoError, true)
        ],
        "{forgotten:?}"
    );
    assert_eq!(root.received(), pinned_frames());
    assert_eq!(root.archived(), None);
}

/// B4: an app-server that answers success but leaves a rollout makes the
/// result partial, retryable: the check afterwards decides, not the answer.
#[tokio::test]
async fn a_rollout_left_behind_makes_the_result_partial() {
    let root = Root::new();
    root.populate();
    let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::AnswerButKeep))).await;
    assert_eq!(
        reasons(&forgotten),
        [(ForgetKind::Transcript, ForgetReason::StillPresent, true)],
        "{forgotten:?}"
    );
    assert_eq!(count_left(&forgotten, ForgetKind::Transcript), OWN.len() as u32);
    assert_eq!(root.archived(), None);
}

/// The parent's rule: an app-server that resolved another `CODEX_HOME`
/// than the recorded one is asked nothing more, and no fallback runs (its
/// archive would resolve the same wrong home).
#[tokio::test]
async fn an_app_server_on_another_home_is_asked_nothing_and_nothing_falls_back() {
    let root = Root::new();
    root.populate();
    let mut fake = root.fake(FakeDelete::Delete);
    fake.codex_home = Some(root.outside().to_str().unwrap().into());
    let forgotten = root.forget_with(&root.ctx(fake)).await;
    assert_eq!(
        reasons(&forgotten),
        [(ForgetKind::Session, ForgetReason::HomeMismatch, false)],
        "{forgotten:?}"
    );
    assert_eq!(root.received(), pinned_frames()[..1]);
    assert_eq!(root.own_left(), OWN.len());
    assert_eq!(root.archived(), None);
}

/// What the fallback leaves (decision 10): the archive ran under the
/// recorded root (B6), then the rollouts went through the descriptor walk,
/// and Codex's own database copies are reported, final.
fn assert_fell_back(root: &Root, forgotten: &Forgotten, why: &str) {
    assert_eq!(
        reasons(forgotten),
        [(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)],
        "{why}: {forgotten:?}"
    );
    assert_eq!(removed(forgotten, ForgetKind::Transcript), OWN.len() as u32, "{why}");
    assert_eq!(root.own_left(), 0, "{why}");
    root.neighbours_kept();
    let r = root.root();
    let r = r.to_str().unwrap();
    assert_eq!(
        root.archived().as_deref(),
        Some(format!("CODEX_HOME={r}\ncwd={r}\nCODEX_SQLITE_HOME=-\n").as_str()),
        "{why}"
    );
}

/// B5: the fallback runs when the app-server path is unavailable, and
/// only then: no binary, a binary that does not spawn, a version the
/// manifest pins no shape for (or no pin at all), an `initialize` that
/// fails or ends, and a `thread/delete` this Codex does not know (0.155.1
/// answers an unknown method `-32600 Invalid request`, and JSON-RPC's own
/// is `-32601`).
#[tokio::test]
async fn the_fallback_runs_only_when_the_app_server_is_unavailable() {
    struct Case {
        why: &'static str,
        set: fn(&Root, &mut ForgetContext),
        /// The app-server's spawns (`--version`, `app-server`) expected.
        spawns: usize,
    }
    fn fake_with(root: &Root, ctx: &mut ForgetContext, edit: fn(&mut FakeCodex)) {
        let mut fake = root.fake(FakeDelete::Delete);
        edit(&mut fake);
        *ctx = root.ctx(fake);
    }
    let cases = [
        Case {
            why: "no binary",
            set: |_, ctx| ctx.codex_app_server = None,
            spawns: 0,
        },
        Case {
            why: "a binary that does not spawn",
            set: |root, ctx| {
                ctx.codex_app_server = Some(AgentCommand::parse(root.base.join("missing").to_str().unwrap()).unwrap())
            },
            spawns: 0,
        },
        Case {
            why: "no pinned shape",
            set: |_, ctx| ctx.codex_pin = None,
            spawns: 0,
        },
        Case {
            why: "an unpinned version",
            set: |root, ctx| fake_with(root, ctx, |f| f.version = "0.156.0".into()),
            spawns: 1,
        },
        Case {
            why: "initialize refused",
            set: |root, ctx| fake_with(root, ctx, |f| f.initialize = FakeInitialize::Error),
            spawns: 2,
        },
        Case {
            why: "initialize ends the app-server",
            set: |root, ctx| fake_with(root, ctx, |f| f.initialize = FakeInitialize::Exit),
            spawns: 2,
        },
        Case {
            why: "thread/delete unknown to this Codex",
            set: |root, ctx| fake_with(root, ctx, |f| f.delete = FakeDelete::UnknownMethod),
            spawns: 2,
        },
        Case {
            why: "method not found",
            set: |root, ctx| fake_with(root, ctx, |f| f.delete = FakeDelete::MethodNotFound),
            spawns: 2,
        },
    ];
    for case in cases {
        let root = Root::new();
        root.populate();
        let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
        (case.set)(&root, &mut ctx);
        let forgotten = root.forget_with(&ctx).await;
        assert_fell_back(&root, &forgotten, case.why);
        assert_eq!(root.spawns().len(), case.spawns, "{}: {:?}", case.why, root.spawns());
    }
}

/// B6: a `--version`, an `initialize` or a `thread/delete` that never answers is cut at
/// the app-server's share of the forget's deadline, retryable; the
/// app-server's whole group is killed, though it ignores SIGTERM; and no
/// fallback runs: the delete may have been under way, and a slow start (a
/// first exec macOS scans) is no reason to settle for the fallback's final
/// result.
#[tokio::test]
async fn an_app_server_that_never_answers_is_cut_at_the_deadline_and_its_group_killed() {
    for (hang, frames) in [("thread/delete", 2), ("initialize", 1), ("--version", 0)] {
        let root = Root::new();
        root.populate();
        let pid_file = root.base.join("app-server.pid");
        let mut fake = root.fake(FakeDelete::Hang);
        match hang {
            "initialize" => fake.initialize = FakeInitialize::Hang,
            "--version" => fake.hang_version = true,
            _ => {}
        }
        fake.pid_file = Some(pid_file.to_str().unwrap().into());
        fake.ignore_term = true;
        let mut ctx = root.ctx(fake);
        ctx.deadline = Duration::from_secs(10);
        let started = Instant::now();
        let forgotten = tokio::time::timeout(Duration::from_secs(40), root.forget_with(&ctx))
            .await
            .expect("the forget keeps its deadline");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{hang}: {:?}",
            started.elapsed()
        );
        // Before the delete was written, the app-server's own timeout (the
        // hybrid's count, B5 as ruled); after, the delete's.
        let reason = if frames == 2 {
            ForgetReason::TimedOut
        } else {
            ForgetReason::AppServerTimedOut
        };
        assert_eq!(
            reasons(&forgotten),
            [
                (ForgetKind::Session, reason, true),
                (ForgetKind::Transcript, reason, true)
            ],
            "{hang}: {forgotten:?}"
        );
        assert_eq!(root.received(), pinned_frames()[..frames], "{hang}");
        assert_eq!(root.archived(), None, "{hang}: no fallback");
        assert_gone(&pid_file).await;
    }
}

/// B6 (the review's item 6): the app-server's own child, in its process
/// group and deaf to SIGTERM, is gone after the forget too: the group is
/// killed, not only the app-server. Its pid is polled for, never read
/// straight after the spawn.
#[tokio::test]
async fn the_app_servers_grandchild_dies_with_its_group() {
    let root = Root::new();
    root.populate();
    let grandchild = root.base.join("grandchild.pid");
    let mut fake = root.fake(FakeDelete::Hang);
    fake.grandchild_pid_file = Some(grandchild.to_str().unwrap().into());
    let mut ctx = root.ctx(fake);
    // Room for a first exec macOS scans before the app-server starts.
    ctx.deadline = Duration::from_secs(15);
    let run = tokio::spawn(async move {
        root.forget_with(&ctx).await;
        root
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let pid: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(&grandchild)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "the grandchild never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        hennery_testkit::pid_alive(pid),
        "the grandchild runs while the forget waits"
    );
    let _root = tokio::time::timeout(Duration::from_secs(40), run)
        .await
        .expect("the forget keeps its deadline")
        .unwrap();
    assert_gone(&grandchild).await;
}

/// The process whose pid `pid_file` holds is gone (its group killed).
async fn assert_gone(pid_file: &Path) {
    let pid: i32 = std::fs::read_to_string(pid_file).unwrap().trim().parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while hennery_testkit::pid_alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!hennery_testkit::pid_alive(pid), "the app-server outlived its forget");
}

/// The review's item 1: before a home's first archive there is no
/// `archived_sessions/`; codex-acp's archive creates it and moves the
/// rollouts there. The walk after the archive reopens the kind
/// directories (with every B3 check) and removes them: nothing is left.
#[tokio::test]
async fn a_fallback_into_a_new_archived_sessions_leaves_nothing() {
    let root = Root::new();
    for rel in OWN.iter().filter(|r| r.starts_with("sessions/")) {
        let path = root.at(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "rollout").unwrap();
    }
    assert!(!root.at("archived_sessions").exists());
    let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
    ctx.codex_app_server = None;
    let forgotten = root.forget_with(&ctx).await;
    assert!(root.archived().is_some(), "the archive ran");
    assert_eq!(
        reasons(&forgotten),
        [(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)],
        "{forgotten:?}"
    );
    assert_eq!(removed(&forgotten, ForgetKind::Transcript), 2);
    let left: Vec<_> = std::fs::read_dir(root.at("archived_sessions")).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

/// The review's item 1: the directories reopened after the archive pass
/// every B3 check again. A root replaced while the adapter ran (another
/// directory at the path, its device and inode not the first open's) is
/// refused, `unsafe_root`, and nothing is removed from either; an
/// `archived_sessions/` the archive left as a symlink is reported, never
/// followed.
#[tokio::test]
async fn the_directories_reopened_after_the_archive_are_checked_again() {
    fn with_script(root: &Root, edit: impl FnOnce(&mut FakeScript)) -> ForgetContext {
        let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
        ctx.codex_app_server = None;
        let mut script = FakeScript {
            codex_archive_log: Some(root.archive_log().to_str().unwrap().into()),
            ..FakeScript::default()
        };
        edit(&mut script);
        let adapter = ctx.agents.get_mut("codex").unwrap();
        adapter.env.retain(|(k, _)| k != SCRIPT_ENV);
        adapter
            .env
            .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
        ctx
    }
    // A root replaced.
    let root = Root::new();
    root.populate();
    let ctx = with_script(&root, |s| s.codex_archive_replaces_home = true);
    let forgotten = root.forget_with(&ctx).await;
    assert_eq!(
        reasons(&forgotten),
        [(ForgetKind::Session, ForgetReason::UnsafeRoot, false)],
        "{forgotten:?}"
    );
    let aside = root.root().with_extension("aside");
    let rollouts_in = |dir: &Path| {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("rollout-")
            })
            .count()
    };
    assert_eq!(
        rollouts_in(&aside.join("archived_sessions")),
        OWN.len(),
        "nothing was removed from the moved-aside root"
    );
    // `archived_sessions/` left as a symlink.
    let root = Root::new();
    root.populate();
    let target = root.outside().join("archived");
    let ctx = with_script(&root, |s| {
        s.codex_archive_links_archived = Some(target.to_str().unwrap().into())
    });
    let forgotten = root.forget_with(&ctx).await;
    assert_eq!(
        reasons(&forgotten),
        [
            (ForgetKind::Transcript, ForgetReason::Symlink, false),
            (ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)
        ],
        "{forgotten:?}"
    );
    assert_eq!(rollouts_in(&target), OWN.len(), "the link was followed");
}

/// The review's item 4: a fallback whose adapter never answers is cut its
/// grace before the forget's deadline, so stopping it still ends within
/// the deadline (and the host's answer inside the collector's wait). Its
/// archive ran first, so the walk still leaves nothing.
#[tokio::test]
async fn a_fallback_adapter_that_never_answers_is_stopped_within_the_deadline() {
    let root = Root::new();
    root.populate();
    let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
    ctx.codex_app_server = None;
    ctx.deadline = Duration::from_secs(6);
    let script = FakeScript {
        codex_archive_log: Some(root.archive_log().to_str().unwrap().into()),
        delete_waits_for_file: Some(root.base.join("never").to_str().unwrap().into()),
        ..FakeScript::default()
    };
    let adapter = ctx.agents.get_mut("codex").unwrap();
    adapter.env.retain(|(k, _)| k != SCRIPT_ENV);
    adapter
        .env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let started = Instant::now();
    let forgotten = root.forget_with(&ctx).await;
    // Cut at 4 s (6 s less the 2 s grace), not at 6 s.
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_fell_back(&root, &forgotten, "a hung adapter");
}

/// B5 as ruled (the hybrid): a forget the collector flags `fallback`
/// spawns no Codex at all, and goes straight to the archive and the walk
/// under the recorded home, after every B3 check: the same result as an
/// unavailable app-server, final. A flag only downgrades this session's
/// own deletion: the other thread's rollouts and everything else stay,
/// and a failed check still spawns nothing.
#[tokio::test]
async fn a_forget_flagged_fallback_spawns_no_codex_and_removes_only_its_own() {
    let root = Root::new();
    root.populate();
    let mut flagged = root.forget_at(&root.root(), None);
    flagged.fallback = true;
    let forgotten = forget(&root.ctx(root.fake(FakeDelete::Delete)), &flagged).await;
    assert_fell_back(&root, &forgotten, "flagged");
    assert_eq!(root.spawns().len(), 0, "{:?}", root.spawns());
    // A flagged forget whose kind directory fails its check: nothing runs.
    let root = Root::new();
    let target = root.outside().join("real");
    std::fs::create_dir_all(&target).unwrap();
    symlink(&target, root.at("sessions")).unwrap();
    let mut flagged = root.forget_at(&root.root(), None);
    flagged.fallback = true;
    let forgotten = forget(&root.ctx(root.fake(FakeDelete::Delete)), &flagged).await;
    assert_eq!(
        reasons(&forgotten),
        [(ForgetKind::Transcript, ForgetReason::Symlink, false)],
        "{forgotten:?}"
    );
    assert_eq!((root.spawns().len(), root.archived()), (0, None));
}

/// B3, decision 12: `sessions/` or `archived_sessions/` that is a symlink
/// (a composed home's linked `sessions/`, say) is reported, never
/// followed: nothing is spawned, since `thread/delete` and the archive
/// would both follow it, and its target is untouched.
#[tokio::test]
async fn a_symlinked_sessions_directory_spawns_nothing() {
    for kind_dir in ["sessions", "archived_sessions"] {
        let root = Root::new();
        let target = root.outside().join("real");
        std::fs::create_dir_all(&target).unwrap();
        let theirs = target.join(format!("rollout-{STAMP}-{ID}.jsonl"));
        std::fs::write(&theirs, "keep").unwrap();
        symlink(&target, root.at(kind_dir)).unwrap();
        let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::Delete))).await;
        assert_eq!(
            reasons(&forgotten),
            [(ForgetKind::Transcript, ForgetReason::Symlink, false)],
            "{kind_dir}: {forgotten:?}"
        );
        assert!(theirs.exists(), "{kind_dir}: the target was touched");
        assert_eq!(root.spawns().len(), 0, "{kind_dir}");
        assert_eq!(root.archived(), None, "{kind_dir}");
    }
}

/// Decision 11's spirit on the host: a root that is not there (any more)
/// spawns nothing.
#[tokio::test]
async fn a_missing_root_spawns_nothing() {
    let root = Root::new();
    let gone = root.base.join("gone");
    let forgotten = forget(&root.ctx(root.fake(FakeDelete::Delete)), &root.forget_at(&gone, None)).await;
    assert_eq!(
        reasons(&forgotten),
        [(ForgetKind::Session, ForgetReason::RootMissing, false)],
        "{forgotten:?}"
    );
    assert_eq!(root.spawns().len(), 0);
    assert_eq!(root.archived(), None);
}

/// Decision 10, B5, R3: the fallback's own walk (here with no adapter to
/// archive first) removes the session's rollouts in `sessions/` at most
/// three levels down and at the top of `archived_sessions/`, regular files
/// only: a rollout-named symlink is reported and left, its target too; a
/// symlinked date directory is never entered.
#[tokio::test]
async fn the_fallback_walk_removes_only_the_sessions_own_rollout_files() {
    let root = Root::new();
    root.populate();
    let linked_target = root.outside().join("rollout");
    std::fs::write(&linked_target, "keep").unwrap();
    symlink(
        &linked_target,
        root.at(&format!("archived_sessions/rollout-{STAMP}-{ID}.jsonl.zst")),
    )
    .unwrap();
    let day = root.outside().join("day");
    std::fs::create_dir_all(&day).unwrap();
    let in_linked_day = day.join(format!("rollout-{STAMP}-{ID}.jsonl"));
    std::fs::write(&in_linked_day, "keep").unwrap();
    symlink(&day, root.at("sessions/2026/10/03")).unwrap();
    let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
    ctx.codex_app_server = None;
    ctx.agents.clear();
    let forgotten = root.forget_with(&ctx).await;
    assert_eq!(
        reasons(&forgotten),
        [
            (ForgetKind::Transcript, ForgetReason::Symlink, false),
            (ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)
        ],
        "{forgotten:?}"
    );
    // The rollout-named link and the linked day, each counted.
    assert_eq!(count_left(&forgotten, ForgetKind::Transcript), 2, "{forgotten:?}");
    assert_eq!(removed(&forgotten, ForgetKind::Transcript), OWN.len() as u32);
    assert_eq!(root.own_left(), 0);
    root.neighbours_kept();
    assert!(linked_target.exists() && in_linked_day.exists(), "a link was followed");
    assert!(root.exists(&format!("archived_sessions/rollout-{STAMP}-{ID}.jsonl.zst")));
}

/// R3 for the fallback's walk: a date directory swapped for a symlink
/// after it was found to be a directory, before it is opened, is never
/// entered, and its target stays.
#[tokio::test]
async fn a_date_directory_swapped_for_a_symlink_is_never_entered() {
    let root = Root::new();
    root.populate();
    let target = root.outside().join("swapped");
    std::fs::create_dir_all(&target).unwrap();
    let theirs = target.join(format!("rollout-{STAMP}-{ID}.jsonl"));
    std::fs::write(&theirs, "keep").unwrap();
    let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
    ctx.codex_app_server = None;
    ctx.agents.clear();
    let day = root.at("sessions/2026/10/02");
    let (day_hook, target_hook) = (day.clone(), target.clone());
    let swapped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = swapped.clone();
    ctx.hooks.stated = Some(std::sync::Arc::new(move |path: &Path| {
        let real_dir = std::fs::symlink_metadata(&day_hook).is_ok_and(|m| m.is_dir());
        if path.ends_with("sessions/2026/10/02") && real_dir {
            std::fs::rename(&day_hook, day_hook.with_extension("moved")).unwrap();
            symlink(&target_hook, &day_hook).unwrap();
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }));
    let forgotten = root.forget_with(&ctx).await;
    assert!(swapped.load(std::sync::atomic::Ordering::SeqCst), "the hook never ran");
    assert!(theirs.exists(), "the swapped-in target was touched: {forgotten:?}");
    assert!(std::fs::symlink_metadata(&day).unwrap().file_type().is_symlink());
    assert!(
        reasons(&forgotten).contains(&(ForgetKind::Transcript, ForgetReason::Symlink, false)),
        "{forgotten:?}"
    );
}

/// B3 for the fallback's walk: a date directory it cannot list, and a
/// rollout it cannot unlink (its directory read-only), are left for a
/// retry, and counted.
#[tokio::test]
async fn what_the_walk_could_not_list_or_unlink_is_retried() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid(2) cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        // Root unlinks in a read-only directory: nothing to see.
        eprintln!("skipped: running as root");
        return;
    }
    let root = Root::new();
    root.populate();
    let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
    ctx.codex_app_server = None;
    ctx.agents.clear();
    ctx.hooks.fail_listing = Some(std::sync::Arc::new(|path: &Path| path.ends_with("sessions/2026/10/01")));
    let locked = root.at("sessions/2026/10/02");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let forgotten = root.forget_with(&ctx).await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        reasons(&forgotten),
        [
            (ForgetKind::Transcript, ForgetReason::IoError, true),
            (ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)
        ],
        "{forgotten:?}"
    );
    // The unlisted day's rollout and the locked day's.
    assert_eq!(count_left(&forgotten, ForgetKind::Transcript), 2, "{forgotten:?}");
    assert_eq!(removed(&forgotten, ForgetKind::Transcript), 1, "only the archived one");
    assert!(root.exists(OWN[0]) && root.exists(OWN[1]));
}
