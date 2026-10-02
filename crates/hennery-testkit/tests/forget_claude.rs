//! The Claude path of a forget on the host (plan 9d decision 8, B3–B6, B9,
//! R1–R4), run directly against roots of the test's own: what it removes,
//! what it leaves and never follows, and the roots it refuses. The fake
//! adapter stands in for `claude-agent-acp`. These run on Linux CI as on
//! macOS (R4).

use hennery_host::AgentCommand;
use hennery_host::forget::{Forget, ForgetContext, Forgotten, forget};
use hennery_host::walk::Hooks;
use hennery_proto::frames::{AgentHome, ForgetKind, ForgetOutcome, ForgetReason, ForgetWhat, HostFrame};
use hennery_testkit::{FakeScript, SCRIPT_ENV};
use std::collections::HashMap;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ID: &str = "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";

/// The transcript family in one project directory (B9).
const FAMILY: [&str; 9] = [
    ".jsonl",
    "",
    ".ccr-tip.json",
    ".precompact.json",
    ".cast",
    ".dir-sync.json",
    ".dir-sync-empty.json",
    ".jsonl.superseded-1700000000",
    ".jsonl.compact.tmp.abc",
];

/// Names beside them that are not the session's and stay.
const NEIGHBOURS: [&str; 4] = [
    "1b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3.jsonl",
    "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3x.jsonl",
    "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3.jsonl.bak",
    "notes.md",
];

struct Root {
    _dir: tempfile::TempDir,
    base: PathBuf,
}

impl Root {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        for sub in ["claude", "host", "home", "outside"] {
            std::fs::create_dir(base.join(sub)).unwrap();
        }
        Self { _dir: dir, base }
    }

    fn root(&self) -> PathBuf {
        self.base.join("claude")
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    fn outside(&self) -> PathBuf {
        self.base.join("outside")
    }

    fn log(&self) -> PathBuf {
        self.base.join("delete.log")
    }

    fn ctx(&self, hooks: Hooks) -> ForgetContext {
        let script = FakeScript {
            delete_log: Some(self.log().to_str().unwrap().into()),
            ..FakeScript::default()
        };
        let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
        fake.env
            .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
        // The forget sets `CLAUDE_CONFIG_DIR` itself (decision 8); one in
        // the agent's own configuration, or one to strip, must not count.
        fake.env
            .push(("CLAUDE_CONFIG_DIR".into(), self.outside().to_str().unwrap().into()));
        fake.env
            .push(("CLAUDE_CODE_PROJECT_DIR_NAME".into(), "elsewhere".into()));
        ForgetContext {
            agents: HashMap::from([("claude".to_string(), fake)]),
            data_dir: self.base.join("host"),
            home: Some(self.base.join("home")),
            hooks,
        }
    }

    fn forget_at(&self, root: &Path) -> Forget {
        Forget {
            agent: "claude".into(),
            agent_session_id: ID.into(),
            agent_home: AgentHome {
                root: root.to_str().unwrap().into(),
                sqlite_root: None,
            },
        }
    }

    async fn forget(&self) -> Forgotten {
        forget(&self.ctx(Hooks::default()), &self.forget_at(&self.root())).await
    }

    /// The session's entries in every kind, and the neighbours that stay.
    fn populate(&self) {
        for project in ["projects/-p-one", "projects/-p-two"] {
            std::fs::create_dir_all(self.at(project)).unwrap();
            for suffix in FAMILY {
                let path = self.at(&format!("{project}/{ID}{suffix}"));
                if suffix.is_empty() {
                    std::fs::create_dir_all(path.join("subagents")).unwrap();
                    std::fs::write(path.join("subagents/agent-1.jsonl"), "x").unwrap();
                } else {
                    std::fs::write(path, "transcript").unwrap();
                }
            }
            for name in NEIGHBOURS {
                std::fs::write(self.at(&format!("{project}/{name}")), "keep").unwrap();
            }
        }
        for kind in ["file-history", "session-env", "tasks"] {
            std::fs::create_dir_all(self.at(&format!("{kind}/{ID}/nested"))).unwrap();
            std::fs::write(self.at(&format!("{kind}/{ID}/nested/f")), "x").unwrap();
            std::fs::create_dir_all(self.at(&format!("{kind}/other"))).unwrap();
        }
        std::fs::create_dir_all(self.at("debug")).unwrap();
        std::fs::write(self.at(&format!("debug/{ID}.txt")), "x").unwrap();
        std::fs::write(self.at("debug/other.txt"), "keep").unwrap();
        std::fs::write(self.at("history.jsonl"), format!("{{\"sessionId\":\"{ID}\"}}")).unwrap();
    }

    /// Every path under the root, relative to it, sorted.
    fn tree(&self) -> Vec<String> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                out.push(path.strip_prefix(base).unwrap().to_string_lossy().into_owned());
                if std::fs::symlink_metadata(&path).unwrap().is_dir() {
                    walk(base, &path, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root(), &self.root(), &mut out);
        out.sort();
        out
    }

    fn adapter_ran(&self) -> Option<String> {
        std::fs::read_to_string(self.log()).ok()
    }
}

fn reasons(forgotten: &Forgotten) -> Vec<(ForgetKind, ForgetReason, bool)> {
    forgotten
        .remaining
        .iter()
        .map(|r| (r.what.kind, r.reason, r.retry))
        .collect()
}

fn removed(forgotten: &Forgotten, kind: ForgetKind) -> u32 {
    forgotten
        .removed
        .iter()
        .filter(|w| w.kind == kind)
        .map(|w| w.count)
        .sum()
}

/// Decision 8, B9, B4, B6: the adapter's own delete runs, with
/// `CLAUDE_CONFIG_DIR` the root, the root its cwd and the project-name
/// override stripped; then exactly the session's names go, in every
/// project directory and kind; nothing else does. A second forget finds
/// nothing and is complete again (the adapter's not-found counts for
/// nothing).
#[tokio::test]
async fn a_forget_removes_exactly_the_sessions_entries() {
    let root = Root::new();
    root.populate();
    let forgotten = root.forget().await;
    assert_eq!(reasons(&forgotten), [], "{forgotten:?}");
    // The adapter took `<id>.jsonl` and `<id>/` of the first project
    // directory it found; the walk took the rest.
    assert_eq!(removed(&forgotten, ForgetKind::Transcript), 2 * FAMILY.len() as u32 - 2);
    for kind in [
        ForgetKind::FileHistory,
        ForgetKind::SessionEnv,
        ForgetKind::Tasks,
        ForgetKind::Debug,
    ] {
        assert_eq!(removed(&forgotten, kind), 1, "{kind:?}");
    }
    let mut kept = vec![
        "debug".to_string(),
        "debug/other.txt".into(),
        "history.jsonl".into(),
        "projects".into(),
    ];
    for project in ["projects/-p-one", "projects/-p-two"] {
        kept.push(project.into());
        kept.extend(NEIGHBOURS.iter().map(|n| format!("{project}/{n}")));
    }
    for kind in ["file-history", "session-env", "tasks"] {
        kept.push(kind.into());
        kept.push(format!("{kind}/other"));
    }
    kept.sort();
    assert_eq!(root.tree(), kept);
    let root_s = root.root().to_str().unwrap().to_string();
    assert_eq!(
        root.adapter_ran().unwrap(),
        format!("CLAUDE_CONFIG_DIR={root_s}\ncwd={root_s}\nCLAUDE_CODE_PROJECT_DIR_NAME=-\nCODEX_SQLITE_HOME=-\n")
    );
    // B2: what goes back names kinds and counts, never a path.
    let frame = serde_json::to_string(&forgotten.clone().into_frame("r".into())).unwrap();
    assert!(
        !frame.contains("-p-one") && !frame.contains(&root_s) && !frame.contains(ID),
        "{frame}"
    );
    let HostFrame::SessionForgotten { outcome, .. } = forgotten.into_frame("r".into()) else {
        unreachable!()
    };
    assert_eq!(outcome, ForgetOutcome::Complete);

    let again = root.forget().await;
    assert_eq!(
        (reasons(&again), again.removed.clone()),
        (vec![], Vec::<ForgetWhat>::new())
    );
}

/// R3, decision 8: a symlink at a named entry is reported and left, and
/// what it points at is untouched.
#[tokio::test]
async fn a_symlink_at_a_named_entry_is_reported_and_never_followed() {
    let root = Root::new();
    std::fs::create_dir_all(root.outside().join("target")).unwrap();
    std::fs::write(root.outside().join("target/precious"), "keep").unwrap();
    std::fs::create_dir_all(root.at("file-history")).unwrap();
    symlink(root.outside().join("target"), root.at(&format!("file-history/{ID}"))).unwrap();
    std::fs::create_dir_all(root.at("projects/-p")).unwrap();
    symlink(
        root.outside().join("target/precious"),
        root.at(&format!("projects/-p/{ID}.cast")),
    )
    .unwrap();
    let forgotten = root.forget().await;
    // What the links point at first: untouched (R3).
    assert_eq!(
        std::fs::read_to_string(root.outside().join("target/precious")).unwrap(),
        "keep"
    );
    assert_eq!(
        reasons(&forgotten),
        [
            (ForgetKind::Transcript, ForgetReason::Symlink, false),
            (ForgetKind::FileHistory, ForgetReason::Symlink, false),
        ]
    );
    assert!(std::fs::symlink_metadata(root.at(&format!("file-history/{ID}"))).is_ok());
    assert!(std::fs::symlink_metadata(root.at(&format!("projects/-p/{ID}.cast"))).is_ok());
}

/// R3, R1: a symlink inside a directory being removed is unlinked as an
/// entry, never followed: what it points at is untouched.
#[tokio::test]
async fn a_symlink_inside_a_removed_directory_is_unlinked_not_followed() {
    let root = Root::new();
    std::fs::create_dir_all(root.outside().join("target")).unwrap();
    std::fs::write(root.outside().join("target/precious"), "keep").unwrap();
    std::fs::create_dir_all(root.at(&format!("tasks/{ID}/deeper"))).unwrap();
    symlink(
        root.outside().join("target"),
        root.at(&format!("tasks/{ID}/deeper/link")),
    )
    .unwrap();
    let forgotten = root.forget().await;
    assert_eq!(
        std::fs::read_to_string(root.outside().join("target/precious")).unwrap(),
        "keep"
    );
    assert_eq!(reasons(&forgotten), []);
    assert!(std::fs::symlink_metadata(root.at(&format!("tasks/{ID}"))).is_err());
}

/// R3, R1: a directory swapped for a symlink after its parent was listed
/// (the test hook) is not followed when the walk reaches it.
#[tokio::test]
async fn a_directory_swapped_for_a_symlink_after_listing_is_not_followed() {
    let root = Root::new();
    std::fs::create_dir_all(root.outside().join("target")).unwrap();
    std::fs::write(root.outside().join("target/precious"), "keep").unwrap();
    std::fs::create_dir_all(root.at(&format!("session-env/{ID}/sub"))).unwrap();
    std::fs::write(root.at(&format!("session-env/{ID}/sub/f")), "x").unwrap();
    let listed = root.at(&format!("session-env/{ID}"));
    let (target, aside) = (root.outside().join("target"), root.outside().join("aside"));
    let swapped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = swapped.clone();
    let hooks = Hooks {
        listed: Some(Arc::new(move |path: &Path, _entries: &[std::ffi::OsString]| {
            if path == listed && !seen.swap(true, std::sync::atomic::Ordering::SeqCst) {
                std::fs::rename(path.join("sub"), &aside).unwrap();
                symlink(&target, path.join("sub")).unwrap();
            }
        })),
    };
    let forgotten = forget(&root.ctx(hooks), &root.forget_at(&root.root())).await;
    assert!(swapped.load(std::sync::atomic::Ordering::SeqCst), "the hook never ran");
    assert_eq!(
        std::fs::read_to_string(root.outside().join("target/precious")).unwrap(),
        "keep"
    );
    assert_eq!(reasons(&forgotten), []);
    assert!(std::fs::symlink_metadata(root.at(&format!("session-env/{ID}"))).is_err());
}

/// B3: a kind directory that is a symlink is skipped and reported, and
/// with `projects/` so, the adapter does not run (it would follow it).
#[tokio::test]
async fn a_symlinked_kind_directory_is_skipped_and_the_adapter_not_run() {
    let root = Root::new();
    std::fs::create_dir_all(root.outside().join("projects/-p")).unwrap();
    std::fs::write(root.outside().join(format!("projects/-p/{ID}.jsonl")), "keep").unwrap();
    symlink(root.outside().join("projects"), root.at("projects")).unwrap();
    std::fs::create_dir_all(root.at(&format!("tasks/{ID}"))).unwrap();
    let forgotten = root.forget().await;
    assert_eq!(
        reasons(&forgotten),
        [(ForgetKind::Transcript, ForgetReason::Symlink, false)]
    );
    assert_eq!(removed(&forgotten, ForgetKind::Tasks), 1);
    assert_eq!(root.adapter_ran(), None);
    assert_eq!(
        std::fs::read_to_string(root.outside().join(format!("projects/-p/{ID}.jsonl"))).unwrap(),
        "keep"
    );
}

/// B3: a kind directory writable by others, or one that is no directory,
/// is skipped and reported.
#[tokio::test]
async fn a_kind_directory_writable_by_others_is_skipped() {
    let root = Root::new();
    std::fs::create_dir_all(root.at("debug")).unwrap();
    std::fs::write(root.at(&format!("debug/{ID}.txt")), "keep").unwrap();
    std::fs::set_permissions(root.at("debug"), std::fs::Permissions::from_mode(0o777)).unwrap();
    std::fs::write(root.at("tasks"), "not a directory").unwrap();
    let forgotten = root.forget().await;
    assert_eq!(
        reasons(&forgotten),
        [
            (ForgetKind::Tasks, ForgetReason::NotADirectory, false),
            (ForgetKind::Debug, ForgetReason::UnsafeDirectory, false),
        ]
    );
    assert!(root.at(&format!("debug/{ID}.txt")).exists());
}

/// Decision 8, B3: the root must be canonical, a real directory of the
/// host user's, not writable by others, and none of `/`, the host user's
/// home or one of its ancestors, the host's data directory or one of its
/// ancestors. Refused, nothing runs and nothing goes.
#[tokio::test]
async fn unsafe_roots_are_refused_before_anything_runs() {
    let root = Root::new();
    std::fs::create_dir_all(root.base.join("home/projects/-p")).unwrap();
    std::fs::write(root.base.join(format!("home/projects/-p/{ID}.jsonl")), "keep").unwrap();
    symlink(root.root(), root.base.join("linked")).unwrap();
    let cases: Vec<(PathBuf, ForgetReason)> = vec![
        (PathBuf::from("/"), ForgetReason::UnsafeRoot),
        (root.base.join("home"), ForgetReason::UnsafeRoot),
        (root.base.clone(), ForgetReason::UnsafeRoot),
        (root.base.join("host"), ForgetReason::UnsafeRoot),
        (root.base.join("linked"), ForgetReason::UnsafeRoot),
        (root.base.join("claude/../claude"), ForgetReason::UnsafeRoot),
        (root.base.join("gone"), ForgetReason::RootMissing),
    ];
    let ctx = root.ctx(Hooks::default());
    for (path, reason) in cases {
        let forgotten = forget(&ctx, &root.forget_at(&path)).await;
        assert_eq!(
            reasons(&forgotten),
            [(ForgetKind::Session, reason, false)],
            "{}",
            path.display()
        );
    }
    std::fs::set_permissions(root.root(), std::fs::Permissions::from_mode(0o775)).unwrap();
    let forgotten = root.forget().await;
    assert_eq!(
        reasons(&forgotten),
        [(ForgetKind::Session, ForgetReason::UnsafeRoot, false)]
    );
    assert_eq!(root.adapter_ran(), None);
    assert!(root.base.join(format!("home/projects/-p/{ID}.jsonl")).exists());
}

/// R2: a tree deeper than the walk's bound stops there, reported.
#[tokio::test]
async fn a_tree_past_the_depth_bound_is_left_reported() {
    let root = Root::new();
    let mut deep = root.at(&format!("tasks/{ID}"));
    for _ in 0..hennery_host::walk::MAX_DEPTH + 2 {
        deep = deep.join("d");
    }
    std::fs::create_dir_all(&deep).unwrap();
    let forgotten = root.forget().await;
    assert_eq!(reasons(&forgotten), [(ForgetKind::Tasks, ForgetReason::TooDeep, false)]);
}

/// Decision 8: other agents are not forgotten yet (9d-ii).
#[tokio::test]
async fn a_codex_forget_is_unsupported_for_now() {
    let root = Root::new();
    let mut codex = root.forget_at(&root.root());
    codex.agent = "codex".into();
    let forgotten = forget(&root.ctx(Hooks::default()), &codex).await;
    assert_eq!(
        reasons(&forgotten),
        [(ForgetKind::Session, ForgetReason::UnsupportedAgent, true)]
    );
}

/// Decision 8: a project directory that is a symlink is skipped by the
/// walk, never followed. (Here with no adapter configured: the adapter's
/// own delete is outside the no-follow guarantee, B3.)
#[tokio::test]
async fn a_symlinked_project_directory_is_not_walked() {
    let root = Root::new();
    std::fs::create_dir_all(root.outside().join("proj")).unwrap();
    std::fs::write(root.outside().join(format!("proj/{ID}.jsonl")), "keep").unwrap();
    std::fs::create_dir_all(root.at("projects")).unwrap();
    symlink(root.outside().join("proj"), root.at("projects/-linked")).unwrap();
    let mut ctx = root.ctx(Hooks::default());
    ctx.agents.clear();
    let forgotten = forget(&ctx, &root.forget_at(&root.root())).await;
    assert_eq!(reasons(&forgotten), []);
    assert_eq!(
        std::fs::read_to_string(root.outside().join(format!("proj/{ID}.jsonl"))).unwrap(),
        "keep"
    );
}

/// B4, B3: an entry the removal could not finish is found by the check
/// afterwards and reported, retryable (here a directory the host user may
/// not write; the check is skipped for a root user, who may).
#[tokio::test]
async fn an_entry_the_removal_could_not_finish_is_reported_for_a_retry() {
    // SAFETY: geteuid(2) cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let root = Root::new();
    let locked = root.at(&format!("tasks/{ID}/locked"));
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("f"), "x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let forgotten = root.forget().await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(reasons(&forgotten), [(ForgetKind::Tasks, ForgetReason::IoError, true)]);
    assert!(locked.join("f").exists());
}
