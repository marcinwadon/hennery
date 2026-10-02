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

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
    let mut child = std::process::Command::new(git)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
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
fn command(git: &Path, cwd: &Path, args: &[&str], vars: impl Iterator<Item = (OsString, OsString)>) -> Command {
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
    // is the repository's code (the second review's B2).
    for var in crate::adapter::NESTING_VARS
        .iter()
        .chain(crate::adapter::HOST_SECRET_VARS)
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
        let out = std::process::Command::new(find_git().unwrap())
            .current_dir(dir)
            .args(["-c", "user.name=test", "-c", "user.email=test@example.invalid"])
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
}
