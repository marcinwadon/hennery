//! The host's git probe (ACP core §3.2, §7; plan 6b-ii decision 11): the
//! branch, changes and work tree of a session's cwd, after its start and
//! after each turn.
//!
//! The session actor runs it on a task of its own, bounded to 3 s, and
//! hears its result through its ordered channel, so a probe never delays a
//! turn's end. Hardening (the review's A8–A10):
//! - `git` is the absolute path `find_git` found on `PATH` when the host
//!   started, never a relative entry: a `git` the agent wrote into its cwd
//!   never runs;
//! - every `GIT_*` variable the host inherited is removed, and
//!   `GIT_OPTIONAL_LOCKS=0` keeps `git status` from taking `index.lock`
//!   under the agent's own git;
//! - discovery stops below `$HOME` (`GIT_CEILING_DIRECTORIES`), so a cwd in
//!   no repository never scans a home directory kept in git;
//! - fsmonitor and submodules are off, and git runs in a process group of
//!   its own, killed whole when a probe is given up.
//!
//! A repository can still make `git status` run a command it configures (a
//! clean filter, which `.git/info/attributes` can name). That is an
//! accepted residual risk (umbrella §8.4): the agent working there can run
//! anything already.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

/// The whole probe's bound (ACP core §7).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// What `rev-parse` may print before the probe stops reading it.
const REV_PARSE_MAX_BYTES: u64 = 64 * 1024;

/// What `git status` may print before the probe stops reading it: its
/// headers are short, but a repository can name an upstream of any length
/// (the second review's P3).
const STATUS_MAX_BYTES: u64 = 64 * 1024;

const PROJECT_RETRY_TTL: Duration = Duration::from_secs(1);
const PROJECT_CACHE_CAPACITY: usize = 1024;

/// A discovery result. Retryable failures must not be published as project
/// identity: the UI can use cwd until a later attach discovers the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectLookup {
    Root(PathBuf),
    NotRepository,
    Retry,
}

#[derive(Debug)]
struct CachedProject {
    lookup: ProjectLookup,
    retry_at: Option<tokio::time::Instant>,
}

type ProjectCell = Arc<tokio::sync::Mutex<Option<CachedProject>>>;

/// Successful identities and definitive non-repositories are cached. Other
/// failures expire quickly; each cwd's mutex coalesces concurrent retries.
#[derive(Debug, Default)]
pub struct ProjectRoots(Mutex<HashMap<PathBuf, ProjectCell>>);

impl ProjectRoots {
    pub async fn resolve(&self, git: Option<&Path>, cwd: &Path) -> ProjectLookup {
        self.resolve_with(cwd, || project_root_with_timeout(PROBE_TIMEOUT, project_root(git, cwd)))
            .await
    }

    async fn resolve_with<F, Fut>(&self, cwd: &Path, probe: F) -> ProjectLookup
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ProjectLookup>,
    {
        let cell = {
            let mut roots = self.0.lock().expect("project roots lock");
            if roots.len() >= PROJECT_CACHE_CAPACITY && !roots.contains_key(cwd) {
                // Keep active cells so a cache eviction never duplicates their
                // in-flight probe. If every slot is busy, retry on a later attach.
                let idle = roots
                    .iter()
                    .find(|(_, cell)| Arc::strong_count(cell) == 1)
                    .map(|(cwd, _)| cwd.clone());
                let Some(idle) = idle else { return ProjectLookup::Retry };
                roots.remove(&idle);
            }
            roots.entry(cwd.to_path_buf()).or_default().clone()
        };
        let mut cached = cell.lock().await;
        if let Some(value) = cached.as_ref()
            && value.retry_at.is_none_or(|at| tokio::time::Instant::now() < at)
        {
            return value.lookup.clone();
        }
        let lookup = probe().await;
        let retry_at = matches!(lookup, ProjectLookup::Retry).then(|| tokio::time::Instant::now() + PROJECT_RETRY_TTL);
        *cached = Some(CachedProject {
            lookup: lookup.clone(),
            retry_at,
        });
        lookup
    }
}

async fn project_root_with_timeout(
    bound: Duration,
    probe: impl std::future::Future<Output = ProjectLookup>,
) -> ProjectLookup {
    tokio::time::timeout(bound, probe).await.unwrap_or(ProjectLookup::Retry)
}

async fn project_root(git: Option<&Path>, cwd: &Path) -> ProjectLookup {
    // rev-parse only reads local metadata: no shell, hooks, status filters or network.
    let Some(git) = git else { return ProjectLookup::Retry };
    let mut cmd = command(
        git,
        cwd,
        &[
            "rev-parse",
            "--is-bare-repository",
            "--path-format=absolute",
            "--git-common-dir",
        ],
        std::env::vars_os(),
    );
    // Git's definitive discovery diagnostic must be recognized independently
    // of the host locale; every other error is retried after the short TTL.
    cmd.env("LC_ALL", "C").stderr(Stdio::piped());
    let Ok(mut child) = cmd.spawn() else {
        return ProjectLookup::Retry;
    };
    let mut group = Group(child.id().and_then(|id| i32::try_from(id).ok()));
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return ProjectLookup::Retry;
    };
    let (mut out, mut err) = (String::new(), String::new());
    let mut stdout = stdout.take(REV_PARSE_MAX_BYTES + 1);
    let mut stderr = stderr.take(REV_PARSE_MAX_BYTES + 1);
    let (read_out, read_err) = tokio::join!(stdout.read_to_string(&mut out), stderr.read_to_string(&mut err));
    if read_out.is_err() || read_err.is_err() {
        return ProjectLookup::Retry;
    }
    let Ok(status) = child.wait().await else {
        return ProjectLookup::Retry;
    };
    group.0 = None;
    classify_project_output(status.success(), status.code(), &out, &err)
}

fn classify_project_output(success: bool, code: Option<i32>, out: &str, err: &str) -> ProjectLookup {
    if success {
        parse_project_root(out)
            .map(ProjectLookup::Root)
            .unwrap_or(ProjectLookup::Retry)
    } else if code == Some(128) && not_repository(err) {
        ProjectLookup::NotRepository
    } else {
        ProjectLookup::Retry
    }
}

fn not_repository(err: &str) -> bool {
    if err.trim() == "fatal: not a git repository (or any of the parent directories): .git" {
        return true;
    }
    // Discovery can also stop at a mount boundary (notably a tmpfs TMPDIR).
    let mut lines = err.trim().lines();
    matches!((lines.next(), lines.next(), lines.next()),
        (Some(first), Some("Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set)."), None)
        if first.starts_with("fatal: not a git repository (or any parent up to mount point /") && first.ends_with(')'))
}

fn parse_project_root(out: &str) -> Option<PathBuf> {
    if out.len() > REV_PARSE_MAX_BYTES as usize {
        return None;
    }
    let mut lines = out.strip_suffix('\n').unwrap_or(out).split('\n');
    if !matches!(lines.next()?, "false" | "true") {
        return None;
    }
    let common_text = lines.next()?;
    if !hennery_proto::rest::valid_project_root(common_text) {
        return None;
    }
    let common = Path::new(common_text);
    if lines.next().is_some()
        || !common.is_absolute()
        || common
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    let root = if common.file_name().is_some_and(|name| name == ".git") {
        common.parent()?
    } else {
        common
    };
    hennery_proto::rest::valid_project_root(root.to_str()?).then(|| root.to_path_buf())
}

/// What a probe found: the fields of a `git_state`, but `base_commit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitState {
    /// The branch checked out; `None` when detached.
    pub branch: Option<String>,
    /// Any staged, unstaged or untracked change.
    pub dirty: bool,
    /// The cwd is in a linked work tree, not the repository's main one.
    pub worktree: bool,
    /// The commit checked out; `None` on a branch with no commit yet.
    pub head: Option<String>,
}

/// `git` on the host's `PATH`, as an absolute path: found once, when the
/// host starts. Relative entries are skipped, and so is a `git` older than
/// 2.31 (no `--path-format`) or one that does not answer `--version` (the
/// second review's P5: on macOS, `/usr/bin/git` without the developer tools
/// would otherwise offer to install them at every probe). It is asked once
/// here, and the host says so if it is not used.
pub fn find_git() -> Option<PathBuf> {
    let git = find_git_in(std::env::var_os("PATH"))?;
    let found = version_of(&git);
    if usable(found) {
        Some(git)
    } else {
        tracing::info!(git = %git.display(), ?found, "git is older than 2.31 or does not run: sessions report no git state");
        None
    }
}

/// What `git --version` says, within `PROBE_TIMEOUT`: a git that hangs
/// must not hold the host's start (the second review, after B1–B4).
fn version_of(git: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new(git)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                // Whatever it started too (an `xcrun`, say).
                if let Ok(pgid) = i32::try_from(child.id()) {
                    // SAFETY: killpg(2) on the group of a child not yet
                    // reaped, so its id is not reused.
                    unsafe {
                        libc::killpg(pgid, libc::SIGKILL);
                    }
                }
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    child.stdout.take()?.take(4096).read_to_string(&mut out).ok()?;
    if status.success() { parse_version(&out) } else { None }
}

/// `git --version`'s major and minor version.
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let mut numbers = text
        .trim()
        .strip_prefix("git version ")?
        .split(|c: char| !c.is_ascii_digit());
    Some((numbers.next()?.parse().ok()?, numbers.next()?.parse().ok()?))
}

/// Whether the probe can use a `git` of this version.
fn usable(version: Option<(u32, u32)>) -> bool {
    version.is_some_and(|version| version >= (2, 31))
}

fn find_git_in(path: Option<OsString>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("git"))
        .find(|git| std::fs::metadata(git).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// `git <args>` in `cwd`, kept from the host's own git environment (`vars`,
/// the host's variables): see the module's doc.
pub(crate) fn command(
    git: &Path,
    cwd: &Path,
    args: &[&str],
    vars: impl Iterator<Item = (OsString, OsString)>,
) -> Command {
    let mut cmd = Command::new(git);
    cmd.current_dir(cwd)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    // What an agent never inherits, git does not either: a filter it runs
    // is the repository's code (the second review's B2), and the host's log
    // variables (plan 7c-iii) are not its own.
    for var in crate::adapter::NESTING_VARS
        .iter()
        .chain(crate::adapter::HOST_SECRET_VARS)
        .chain(crate::adapter::HOST_LOG_VARS)
    {
        cmd.env_remove(var);
    }
    let mut home = None;
    for (name, value) in vars {
        if name.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(&name);
        } else if name == "HOME" {
            home = Some(value);
        }
    }
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(home) = home.filter(|home| Path::new(home).is_absolute()) {
        cmd.env("GIT_CEILING_DIRECTORIES", home);
    }
    cmd
}

/// Kills a git's process group when dropped, unless it finished: an
/// abandoned probe takes git and whatever it started with it.
struct Group(Option<i32>);

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pgid) = self.0 {
            // SAFETY: killpg(2) on the group of a git this probe spawned
            // and has not reaped. ESRCH (already gone) is harmless.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

/// The git state of `cwd`, if it is in a work tree and `git` answers
/// within `PROBE_TIMEOUT`; `None` otherwise, which is not an error.
pub async fn probe(git: &Path, cwd: &Path) -> Option<GitState> {
    tokio::time::timeout(PROBE_TIMEOUT, probe_unbounded(git, cwd))
        .await
        .ok()
        .flatten()
}

fn spawn(git: &Path, cwd: &Path, args: &[&str]) -> Option<(tokio::process::Child, Group)> {
    let child = command(git, cwd, args, std::env::vars_os()).spawn().ok()?;
    let group = Group(child.id().and_then(|id| i32::try_from(id).ok()));
    Some((child, group))
}

async fn probe_unbounded(git: &Path, cwd: &Path) -> Option<GitState> {
    // Absolute, so the main checkout's subdirectories, where git prints a
    // relative common dir, are not taken for linked work trees (the
    // review's A8). Needs git 2.31; an older one gives no state.
    let (mut child, mut group) = spawn(
        git,
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"],
    )?;
    let mut dirs = String::new();
    child
        .stdout
        .take()?
        .take(REV_PARSE_MAX_BYTES)
        .read_to_string(&mut dirs)
        .await
        .ok()?;
    let finished = child.wait().await.ok()?;
    group.0 = None;
    if !finished.success() {
        return None;
    }
    let mut dirs = dirs.lines();
    let worktree = dirs.next()? != dirs.next()?;

    // The headers come first; the first other line is a change, and the
    // probe stops reading there rather than buffering every change.
    let (mut child, mut group) = spawn(
        git,
        cwd,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--ignore-submodules=all",
            "-unormal",
        ],
    )?;
    let mut status = BufReader::new(child.stdout.take()?.take(STATUS_MAX_BYTES));
    let (mut branch, mut head, mut dirty) = (None, None, false);
    let mut line = Vec::new();
    loop {
        line.clear();
        if status.read_until(b'\n', &mut line).await.ok()? == 0 {
            break;
        }
        match line.strip_prefix(b"# ") {
            Some(text) => header(String::from_utf8_lossy(text).trim_end(), &mut branch, &mut head),
            None => {
                dirty = true;
                break;
            }
        }
    }
    if !dirty {
        // Closed first: a git still writing (past the cap) fails at once
        // rather than at the probe's bound.
        drop(status);
        let finished = child.wait().await.ok()?;
        group.0 = None;
        if !finished.success() {
            return None;
        }
    }
    Some(GitState {
        branch,
        dirty,
        worktree,
        head,
    })
}

/// One `git status --porcelain=v2 --branch` header, without its `# `.
fn header(text: &str, branch: &mut Option<String>, head: &mut Option<String>) {
    if let Some(oid) = text.strip_prefix("branch.oid ") {
        *head = (oid != "(initial)").then(|| oid.to_string());
    } else if let Some(name) = text.strip_prefix("branch.head ") {
        *branch = (name != "(detached)").then(|| name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsStr;

    /// `git` run by a test to set a repository up, not by the probe.
    fn sh_git(dir: &Path, args: &[&str]) -> String {
        let mut cmd = std::process::Command::new(find_git().unwrap());
        // Never the repository a git hook of the runner's names.
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(name);
            }
        }
        let out = cmd
            .current_dir(dir)
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// The review's A9 and decision 11: every `GIT_*` the host inherited is
    /// removed, and so is what an agent never inherits either (the second
    /// review's B2); `GIT_OPTIONAL_LOCKS=0` keeps `git status` off
    /// `index.lock`, and discovery stops below an absolute `HOME`.
    #[test]
    fn a_command_is_isolated_from_the_hosts_git_environment() {
        let host: Vec<(OsString, OsString)> = [
            ("GIT_DIR", "/elsewhere/.git"),
            ("GIT_INDEX_FILE", "/elsewhere/index"),
            ("GIT_CONFIG_PARAMETERS", "'core.fsmonitor=evil'"),
            ("HOME", "/home/someone"),
            ("PATH", "/usr/bin"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let cmd = command(
            Path::new("/usr/bin/git"),
            Path::new("/tmp"),
            &["status"],
            host.into_iter(),
        );
        let envs: HashMap<OsString, Option<OsString>> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned())))
            .collect();
        for removed in ["GIT_DIR", "GIT_INDEX_FILE", "GIT_CONFIG_PARAMETERS"] {
            assert_eq!(envs.get(OsStr::new(removed)), Some(&None), "{removed}");
        }
        for removed in crate::adapter::NESTING_VARS
            .iter()
            .chain(crate::adapter::HOST_SECRET_VARS)
            .chain(crate::adapter::HOST_LOG_VARS)
        {
            assert_eq!(envs.get(OsStr::new(removed)), Some(&None), "{removed}");
        }
        assert_eq!(envs[OsStr::new("GIT_OPTIONAL_LOCKS")], Some("0".into()));
        assert_eq!(
            envs[OsStr::new("GIT_CEILING_DIRECTORIES")],
            Some("/home/someone".into())
        );
        assert!(!envs.contains_key(OsStr::new("PATH")));
        let args: Vec<&OsStr> = cmd.as_std().get_args().collect();
        assert_eq!(args, ["-c", "core.fsmonitor=false", "status"]);
        // A relative `HOME` sets no ceiling.
        let relative = [(OsString::from("HOME"), OsString::from("home"))];
        let cmd = command(Path::new("/usr/bin/git"), Path::new("/tmp"), &[], relative.into_iter());
        assert!(cmd.as_std().get_envs().all(|(k, _)| k != "GIT_CEILING_DIRECTORIES"));
    }

    /// The review's A9: only an absolute `PATH` entry is searched, so a
    /// `git` the agent writes into its cwd (a relative entry such as `.`)
    /// never runs.
    #[test]
    fn git_is_found_on_absolute_path_entries_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let git = bin.join("git");
        std::fs::write(&git, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = |entries: &[&Path]| Some(std::env::join_paths(entries).unwrap());
        // `bin` again, but relative to the working directory: it holds a
        // `git`, and is skipped.
        let here = std::env::current_dir().unwrap();
        let up: PathBuf = here.components().skip(1).map(|_| "..").collect();
        let relative = up.join(bin.strip_prefix("/").unwrap());
        assert!(
            relative.is_relative() && relative.join("git").exists(),
            "{}",
            relative.display()
        );
        assert_eq!(find_git_in(path(&[&relative])), None);
        assert_eq!(find_git_in(path(&[&relative, &bin])), Some(git.clone()));
        assert_eq!(find_git_in(Some("".into())), None);
        assert_eq!(find_git_in(None), None);
        // Not executable: skipped.
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(find_git_in(path(&[&bin])), None);
    }

    /// The second review's P5: a `git` the probe cannot use (older than
    /// 2.31, or one that does not answer `--version`, as a missing
    /// developer tool does) is not used at all.
    #[test]
    fn only_git_2_31_or_newer_is_used() {
        assert_eq!(parse_version("git version 2.39.5 (Apple Git-154)\n"), Some((2, 39)));
        assert_eq!(parse_version("git version 2.31.0"), Some((2, 31)));
        assert_eq!(parse_version("git version 2.30.2"), Some((2, 30)));
        assert_eq!(parse_version("git version 3.0"), Some((3, 0)));
        assert_eq!(parse_version("xcrun: error: invalid active developer path"), None);
        assert_eq!(parse_version(""), None);
        assert!(usable(Some((2, 31))) && usable(Some((3, 0))));
        assert!(!usable(Some((2, 30))) && !usable(Some((1, 99))) && !usable(None));
    }

    #[test]
    fn status_headers_give_the_branch_and_the_head() {
        let (mut branch, mut head) = (None, None);
        header("branch.oid c0ffee", &mut branch, &mut head);
        header("branch.head main", &mut branch, &mut head);
        header("branch.upstream origin/main", &mut branch, &mut head);
        assert_eq!((branch.as_deref(), head.as_deref()), (Some("main"), Some("c0ffee")));
        header("branch.oid (initial)", &mut branch, &mut head);
        header("branch.head (detached)", &mut branch, &mut head);
        assert_eq!((branch, head), (None, None));
    }

    /// Decision 11 and the review's A8: a linked work tree is reported as
    /// one, the main checkout is not, even probed from a subdirectory
    /// (where git would print a relative common dir); changes make it
    /// dirty; outside a work tree there is no state.
    #[tokio::test]
    async fn a_probe_reports_the_branch_changes_and_linked_work_trees() {
        let Some(git) = find_git() else {
            eprintln!("no git on PATH: skipped");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(main.join("sub")).unwrap();
        sh_git(&main, &["init", "-q", "-b", "trunk"]);
        sh_git(&main, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let head = sh_git(&main, &["rev-parse", "HEAD"]);
        let state = probe(&git, &main.join("sub")).await.unwrap();
        assert_eq!(
            state,
            GitState {
                branch: Some("trunk".into()),
                dirty: false,
                worktree: false,
                head: Some(head.clone()),
            }
        );
        std::fs::write(main.join("sub/new.txt"), "x").unwrap();
        assert!(probe(&git, &main).await.unwrap().dirty);
        sh_git(&main, &["worktree", "add", "-q", "-b", "side", "../linked"]);
        let linked = probe(&git, &dir.path().join("linked")).await.unwrap();
        assert_eq!(
            (linked.branch.as_deref(), linked.worktree, linked.dirty),
            (Some("side"), true, false)
        );
        sh_git(&main, &["checkout", "-q", "--detach"]);
        let detached = probe(&git, &main).await.unwrap();
        assert_eq!((detached.branch, detached.head), (None, Some(head)));
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(probe(&git, outside.path()).await, None);
    }

    #[tokio::test]
    async fn project_roots_join_normal_subdirectory_and_linked_worktree() {
        let git = find_git().expect("git in dev shell");
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(main.join("sub")).unwrap();
        sh_git(&main, &["init", "-q", "-b", "trunk"]);
        sh_git(&main, &["commit", "-q", "--allow-empty", "-m", "first"]);
        sh_git(&main, &["worktree", "add", "-q", "-b", "side", "../linked"]);
        let roots = ProjectRoots::default();
        let expected = main.canonicalize().unwrap();
        for cwd in [&main, &main.join("sub"), &dir.path().join("linked")] {
            assert_eq!(
                roots.resolve(Some(&git), cwd).await,
                ProjectLookup::Root(expected.clone())
            );
        }
        // Successful results are cached per cwd, even after the directory goes away.
        std::fs::rename(&main, dir.path().join("moved")).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            roots.resolve(Some(&git), &main.join("sub")).await,
            ProjectLookup::Root(expected)
        );
    }

    #[tokio::test]
    async fn project_roots_keep_bare_repositories_and_their_worktrees_distinct() {
        let git = find_git().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let seed = tmp.path().join("seed");
        std::fs::create_dir(&seed).unwrap();
        sh_git(&seed, &["init", "-q", "-b", "main"]);
        sh_git(&seed, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        let roots = ProjectRoots::default();
        let mut identities = Vec::new();
        for name in ["first.git", "second.git"] {
            sh_git(tmp.path(), &["clone", "--bare", "-q", seed.to_str().unwrap(), name]);
            let bare = tmp.path().join(name).canonicalize().unwrap();
            let linked = tmp.path().join(format!("{name}-worktree"));
            sh_git(&bare, &["worktree", "add", "-q", linked.to_str().unwrap(), "main"]);
            assert_eq!(
                roots.resolve(Some(&git), &bare).await,
                ProjectLookup::Root(bare.clone())
            );
            assert_eq!(
                roots.resolve(Some(&git), &linked).await,
                ProjectLookup::Root(bare.clone())
            );
            identities.push(bare);
        }
        assert_ne!(identities[0], identities[1]);
    }

    #[tokio::test]
    async fn project_roots_keep_two_submodules_distinct() {
        let git = find_git().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let seed = tmp.path().join("seed");
        let parent = tmp.path().join("parent");
        std::fs::create_dir(&seed).unwrap();
        std::fs::create_dir(&parent).unwrap();
        sh_git(&seed, &["init", "-q"]);
        sh_git(&seed, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        sh_git(&parent, &["init", "-q"]);
        let roots = ProjectRoots::default();
        for name in ["first", "second"] {
            sh_git(
                &parent,
                &[
                    "-c",
                    "protocol.file.allow=always",
                    "submodule",
                    "add",
                    "-q",
                    seed.to_str().unwrap(),
                    name,
                ],
            );
            let module = parent.join(name);
            let expected = parent.join(".git/modules").join(name).canonicalize().unwrap();
            assert_eq!(roots.resolve(Some(&git), &module).await, ProjectLookup::Root(expected));
        }
    }

    #[tokio::test]
    async fn project_roots_keep_two_separate_git_dirs_distinct() {
        let git = find_git().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let roots = ProjectRoots::default();
        for name in ["first", "second"] {
            let checkout = tmp.path().join(format!("checkout-{name}"));
            let separate = tmp.path().join(format!("metadata-{name}"));
            sh_git(
                tmp.path(),
                &[
                    "init",
                    "-q",
                    "--separate-git-dir",
                    separate.to_str().unwrap(),
                    checkout.to_str().unwrap(),
                ],
            );
            assert_eq!(
                roots.resolve(Some(&git), &checkout).await,
                ProjectLookup::Root(separate.canonicalize().unwrap())
            );
        }
    }

    #[tokio::test]
    async fn project_roots_retry_transient_spawn_errors_after_short_cache_ttl() {
        let git = find_git().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        sh_git(tmp.path(), &["init", "-q"]);
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let roots = ProjectRoots::default();
        let missing = tmp.path().join("missing-git");
        assert_eq!(roots.resolve(Some(&missing), &sub).await, ProjectLookup::Retry);
        assert_eq!(
            roots.resolve(Some(&git), &sub).await,
            ProjectLookup::Retry,
            "short TTL coalesces failures"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            roots.resolve(Some(&git), &sub).await,
            ProjectLookup::Root(tmp.path().canonicalize().unwrap())
        );
    }

    #[tokio::test]
    async fn project_roots_retry_git_errors_instead_of_caching_them_as_non_repositories() {
        let git = find_git().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        sh_git(tmp.path(), &["init", "-q"]);
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let config = tmp.path().join(".git/config");
        let original = std::fs::read(&config).unwrap();
        std::fs::write(&config, "[core]\nrepositoryformatversion = 999\n").unwrap();
        let roots = ProjectRoots::default();
        assert_eq!(roots.resolve(Some(&git), &sub).await, ProjectLookup::Retry);
        std::fs::write(&config, original).unwrap();
        assert_eq!(roots.resolve(Some(&git), &sub).await, ProjectLookup::Retry);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            roots.resolve(Some(&git), &sub).await,
            ProjectLookup::Root(tmp.path().canonicalize().unwrap())
        );
    }

    #[test]
    fn project_root_classifier_distinguishes_success_discovery_and_transient_failures() {
        assert_eq!(
            classify_project_output(true, Some(0), "false\n/repo/.git\n", ""),
            ProjectLookup::Root("/repo".into())
        );
        assert_eq!(
            classify_project_output(true, Some(0), "true\n/repos/bare.git\n", ""),
            ProjectLookup::Root("/repos/bare.git".into())
        );
        let not_repo = "fatal: not a git repository (or any of the parent directories): .git\n";
        assert_eq!(
            classify_project_output(false, Some(128), "", not_repo),
            ProjectLookup::NotRepository
        );
        assert_eq!(
            classify_project_output(
                false,
                Some(128),
                "",
                "fatal: not a git repository (or any parent up to mount point /)\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).\n"
            ),
            ProjectLookup::NotRepository
        );
        for (success, code, out, err) in [
            (false, Some(128), "", "fatal: detected dubious ownership"),
            (false, Some(1), "", not_repo),
            (false, None, "", ""),
            (true, Some(0), "bad output", ""),
        ] {
            assert_eq!(classify_project_output(success, code, out, err), ProjectLookup::Retry);
        }
    }

    #[tokio::test]
    async fn project_roots_coalesce_timeout_retries_and_bound_the_cache() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let roots = ProjectRoots::default();
        let count = AtomicUsize::new(0);
        let cwd = Path::new("/repository/sub");
        let timeout = || async {
            count.fetch_add(1, Ordering::SeqCst);
            project_root_with_timeout(Duration::from_millis(20), std::future::pending()).await
        };
        let (first, second) = tokio::join!(roots.resolve_with(cwd, timeout), roots.resolve_with(cwd, timeout));
        assert_eq!((first, second), (ProjectLookup::Retry, ProjectLookup::Retry));
        assert_eq!(count.load(Ordering::SeqCst), 1, "one in-flight lookup");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let retry = || async {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            ProjectLookup::Root("/repository".into())
        };
        let (first, second) = tokio::join!(roots.resolve_with(cwd, retry), roots.resolve_with(cwd, retry));
        assert_eq!(
            (first, second),
            (
                ProjectLookup::Root("/repository".into()),
                ProjectLookup::Root("/repository".into())
            )
        );
        assert_eq!(count.load(Ordering::SeqCst), 2, "one retry after expiry");
        assert_eq!(
            roots
                .resolve_with(cwd, || async { panic!("success must stay cached") })
                .await,
            ProjectLookup::Root("/repository".into())
        );
        for i in 0..=PROJECT_CACHE_CAPACITY {
            roots
                .resolve_with(&PathBuf::from(format!("/cwd/{i}")), || async {
                    ProjectLookup::NotRepository
                })
                .await;
        }
        assert_eq!(roots.0.lock().unwrap().len(), PROJECT_CACHE_CAPACITY);
    }

    #[tokio::test]
    async fn project_roots_cache_definitive_non_git_and_fall_back_on_unavailable_git() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let git = find_git().expect("git in dev shell");
        let roots = ProjectRoots::default();
        let sub = cwd.join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert_eq!(project_root(Some(&git), &sub).await, ProjectLookup::NotRepository);
        assert_eq!(roots.resolve(Some(&git), &sub).await, ProjectLookup::NotRepository);
        sh_git(&cwd, &["init", "-q"]);
        tokio::time::sleep(Duration::from_secs(2)).await;
        // The fallback too is cached: do not re-run git on every session.
        assert_eq!(roots.resolve(Some(&git), &sub).await, ProjectLookup::NotRepository);
        let missing = cwd.join("missing");
        assert_eq!(
            ProjectRoots::default().resolve(Some(&missing), &cwd).await,
            ProjectLookup::Retry
        );
        assert_eq!(ProjectRoots::default().resolve(None, &cwd).await, ProjectLookup::Retry);
        sh_git(&cwd, &["init", "--bare", "-q", "bare"]);
        let bare = cwd.join("bare");
        assert_eq!(
            ProjectRoots::default().resolve(Some(&git), &bare).await,
            ProjectLookup::Root(bare)
        );
    }

    #[tokio::test]
    async fn project_root_timeout_is_nonblocking_and_falls_back() {
        // A pipe holds a real child open without a shell or a sleep race.
        // `cat` is warmed before applying the short deadline (Gatekeeper).
        let cat = Path::new("/bin/cat");
        assert!(
            std::process::Command::new(cat)
                .arg("/dev/null")
                .status()
                .unwrap()
                .success()
        );
        let mut cmd = Command::new(cat);
        cmd.arg("/dev/zero")
            .stdout(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let child = cmd.spawn().unwrap();
        let group = Group(child.id().and_then(|id| i32::try_from(id).ok()));
        let bound = Duration::from_millis(30);
        let mut result = Box::pin(project_root_with_timeout(bound, async {
            let _group = group;
            let mut child = child;
            let _ = child.wait().await;
            ProjectLookup::Root(PathBuf::from("/unexpected"))
        }));
        let heartbeat = tokio::spawn(async {
            tokio::task::yield_now().await;
            7
        });
        tokio::select! {
            root = &mut result => panic!("lookup completed before the runtime heartbeat: {root:?}"),
            value = heartbeat => assert_eq!(value.unwrap(), 7),
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), result)
                .await
                .expect("lookup stayed bounded"),
            ProjectLookup::Retry
        );
    }

    #[test]
    fn project_root_output_rejects_malformed_relative_bare_and_oversized_paths() {
        assert_eq!(parse_project_root("false\n/repo/.git\n"), Some(PathBuf::from("/repo")));
        for text in [
            "invalid\n/repo/.git\n",
            "false\nrelative/.git\n",
            "false\n/repo/.git\nextra\n",
            "false\n/repo\u{0}/.git\n",
            "false\n/repo/../gitdir\n",
            "false\n",
        ] {
            assert_eq!(parse_project_root(text), None, "{text:?}");
        }
        assert_eq!(
            parse_project_root(&format!("false\n/{}.git\n", "x".repeat(REV_PARSE_MAX_BYTES as usize))),
            None
        );
    }
}
