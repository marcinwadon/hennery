//! `forget_session` for Codex on the host (plan 9d decisions 9, 10, 12;
//! B3–B6): the session's thread deleted by Codex's own `thread/delete`,
//! through the app-server under the session's recorded `CODEX_HOME` and
//! never another, and, only when that path is unavailable, the fallback:
//! codex-acp's archive, then the rollout files through the descriptor walk.
//!
//! The order:
//! 1. **Check** the root and its `sessions/` and `archived_sessions/` (B3).
//!    One that fails its check (a symlink: a composed home's linked
//!    `sessions/`, decision 12) is reported, and nothing is spawned:
//!    `thread/delete` and the archive would both follow it.
//! 2. **The app-server** (decision 9): the binary's `--version` must be the
//!    one the manifest pins a call shape for; then `app-server`, through
//!    `Adapter::spawn`'s hygiene (B6) with `CODEX_HOME` the root and
//!    `CODEX_SQLITE_HOME` the recorded one or removed, the root its cwd;
//!    then the pinned `initialize`, whose `codexHome` must be the root; then
//!    `thread/delete {threadId}`. Its group is killed after, and the whole
//!    call has one deadline.
//! 3. **The outcome** is decided by checking afterwards that no rollout of
//!    the session is left in `sessions/` or `archived_sessions/` (B4). The
//!    SQLite copies, `thread_history` and `session_index.jsonl` are
//!    `thread/delete`'s contract: hennery does not verify them.
//! 4. **The fallback** (decision 10, B5) runs when the app-server path is
//!    unavailable: no binary or no pin, a spawn that fails, a version with
//!    no pinned shape, an `initialize` that fails, or a `thread/delete`
//!    this Codex does not know. Never after `thread/delete` itself
//!    answered: a refusal, a failure, or no answer in time. Nor when a step
//!    only ran out of time (a first exec macOS scans, say): that is
//!    `timed_out`, retried, since the fallback's result is final.

use crate::adapter::{Adapter, AdapterIo, AgentCommand};
use crate::forget::{
    ADAPTER_GRACE, Forget, ForgetContext, Forgotten, KindDir, Kinds, Tally, check, left, retryable, valid_id,
};
use crate::runtime::manifest::CodexAppServer;
use crate::walk;
use hennery_proto::frames::{ForgetKind, ForgetReason};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::time::Instant;

/// The kind directories under a Codex root (Codex's `SESSIONS_SUBDIR` and
/// `ARCHIVED_SESSIONS_SUBDIR`), both holding rollouts: the transcript.
const CODEX_KINDS: [(ForgetKind, &str); 2] = [
    (ForgetKind::Transcript, "sessions"),
    (ForgetKind::Transcript, "archived_sessions"),
];

/// How deep the walk goes under `sessions/` (Codex's `YYYY/MM/DD`), and
/// under `archived_sessions/` (its top only: Codex archives flat).
const SESSIONS_DEPTH: usize = 3;
const ARCHIVED_DEPTH: usize = 0;

/// The app-server's share of a forget's deadline (B6): its version, its
/// `initialize` and `thread/delete` all within it. The rest is the
/// fallback's and the walk's, so a hung app-server leaves the fallback its
/// time.
fn app_server_share(deadline: Duration) -> Duration {
    deadline * 2 / 5
}

/// The longest line the host reads from the app-server: an `initialize` or
/// `thread/delete` answer is a few hundred bytes.
const MAX_LINE: u64 = 1 << 20;

/// The most a `--version` prints that is read.
const MAX_VERSION: u64 = 4096;

/// Variables a forget's Codex processes never inherit unless recorded
/// (B6): another project directory name is Claude's, and another SQLite
/// home would aim Codex at other database copies.
const CLAUDE_PROJECT_VAR: &str = "CLAUDE_CODE_PROJECT_DIR_NAME";
const SQLITE_VAR: &str = "CODEX_SQLITE_HOME";

/// What the app-server path came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// `thread/delete` answered success, or that there is no such thread:
    /// the check afterwards decides.
    Deleted,
    /// `thread/delete` refused, for this reason: nothing of the thread was
    /// deleted, and the fallback never runs after it (B5).
    Refused(ForgetReason),
    /// `thread/delete` was sent and did not finish: it failed midway, or
    /// gave no answer in time. Retried; no fallback (it may have run).
    Failed(ForgetReason),
    /// The app-server resolved another `CODEX_HOME` than the recorded one:
    /// nothing more was asked of it, and no fallback runs, since codex-acp
    /// would resolve the same home (the parent's rule).
    HomeMismatch,
    /// The app-server path is unavailable, for this reason: the fallback
    /// runs (B5).
    Unavailable(Unavailable),
}

/// Why the app-server path is unavailable: each a fallback trigger (B5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unavailable {
    /// No Codex binary to run (no set, no bundled CLI, no `--use-cli`).
    NoBinary,
    /// The manifest pins no call shape.
    NoPin,
    /// The binary did not spawn, or its `--version` failed.
    Spawn,
    /// Its `--version` is not the one the manifest pins a shape for.
    Unpinned,
    /// `initialize` failed: an error, or the app-server's end.
    Initialize,
    /// This Codex does not know `thread/delete` (`-32601`, or `-32600
    /// Invalid request`: how 0.155.1 answers a method or params it cannot
    /// read, before any handler runs).
    MethodNotFound,
}

/// An answer to one request, as the app-server sent it.
#[derive(Debug, Clone, PartialEq)]
enum Answer {
    Result(serde_json::Value),
    Error { code: i64, message: String },
}

/// How 0.155.1 answers a request it could not deserialize (seen live):
/// an unknown method, a missing field, a field of another type.
const NOT_UNDERSTOOD: [&str; 3] = [
    "Invalid request: unknown variant",
    "Invalid request: missing field",
    "Invalid request: invalid type",
];

/// `thread/delete`'s answer, as Codex 0.155.1 gives it (its
/// `thread_delete.rs`, `delete_thread.rs`, `thread_manager.rs` and
/// `message_processor.rs` at `rust-v0.155.1`). The text only picks the
/// reason: what is left is decided by the check afterwards (B4).
fn classify_delete(answer: &Answer) -> Verdict {
    let (code, message) = match answer {
        Answer::Result(_) => return Verdict::Deleted,
        Answer::Error { code, message } => (*code, message.as_str()),
    };
    match code {
        -32601 => Verdict::Unavailable(Unavailable::MethodNotFound),
        // The request did not deserialize: an unknown method, or params of
        // another shape (the prefixes 0.155.1 was seen to give). No handler
        // ran.
        -32600 if NOT_UNDERSTOOD.iter().any(|p| message.starts_with(p)) => {
            Verdict::Unavailable(Unavailable::MethodNotFound)
        }
        // Nothing of the thread is there (0.155.1 says either).
        -32600 if message.starts_with("no rollout found for thread id") || message.starts_with("thread not found:") => {
            Verdict::Deleted
        }
        -32600 if message.contains("forked history still references it") => {
            Verdict::Refused(ForgetReason::ForkedHistory)
        }
        -32600 if message.starts_with("thread is not persisted") => Verdict::Refused(ForgetReason::Ephemeral),
        // A live internal worker: its owner releases it, so a retry may
        // find it gone.
        -32600 if message.starts_with("live internal threads") => Verdict::Refused(ForgetReason::InProgress),
        _ => Verdict::Failed(ForgetReason::IoError),
    }
}

/// The version a Codex CLI's `--version` prints (`codex-cli 0.155.1`).
fn parse_version(out: &str) -> Option<&str> {
    let version = out.lines().next()?.strip_prefix("codex-cli ")?.trim();
    let plain = version.split('.').count() == 3
        && version
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    plain.then_some(version)
}

/// Whether `name` is one of thread `id`'s rollout files, as Codex's
/// `recorder.rs` and `rollout_file_name.rs` name them, anchored at both
/// ends (the id checked by `valid_id`):
///
/// `^rollout-[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}-[0-9]{2}-[0-9]{2}-<id>(_<uuid>)?\.jsonl(\.zst)?$`
///
/// The `_<uuid>` is a reverted thread's rollout id; the thread id is
/// always the first, so another thread's rollout whose rollout id is `id`
/// does not match.
pub fn is_rollout_of(name: &[u8], id: &str) -> bool {
    let Some(rest) = name.strip_prefix(b"rollout-") else {
        return false;
    };
    let Some((stamp, rest)) = rest.split_at_checked(19) else {
        return false;
    };
    let stamp_ok = stamp.iter().enumerate().all(|(i, b)| match i {
        4 | 7 | 13 | 16 => *b == b'-',
        10 => *b == b'T',
        _ => b.is_ascii_digit(),
    });
    let Some(rest) = rest.strip_prefix(b"-").and_then(|r| r.strip_prefix(id.as_bytes())) else {
        return false;
    };
    let rest = rest.strip_suffix(b".zst").unwrap_or(rest);
    let Some(rest) = rest.strip_suffix(b".jsonl") else {
        return false;
    };
    let rollout_ok = match rest.strip_prefix(b"_") {
        None => rest.is_empty(),
        Some(rollout) => std::str::from_utf8(rollout).is_ok_and(valid_id),
    };
    stamp_ok && valid_id(id) && rollout_ok
}

/// Whether `name` is a date directory at `depth` under `sessions/`: `YYYY`
/// at 1, then `MM` and `DD`. Nothing else there is Codex's.
fn is_date_dir(name: &[u8], depth: usize) -> bool {
    let len = if depth == 1 { 4 } else { 2 };
    name.len() == len && name.iter().all(u8::is_ascii_digit)
}

/// One forget of a Codex session (decisions 9 and 10).
pub async fn forget_codex(ctx: &ForgetContext, forget: &Forget) -> Forgotten {
    let start = Instant::now();
    let until = start + ctx.deadline;
    let root = PathBuf::from(&forget.agent_home.root);
    let checked = {
        let (ctx, root) = (ctx.clone(), root.clone());
        tokio::task::spawn_blocking(move || check(&ctx, &root, &CODEX_KINDS)).await
    };
    let kinds = match checked {
        Ok(Ok(kinds)) => Arc::new(kinds),
        Ok(Err(reason)) => return whole(reason),
        Err(_) => return whole(ForgetReason::IoError),
    };
    // B3: a kind directory that failed its check stops everything here.
    let refused: Vec<(ForgetKind, ForgetReason)> = kinds
        .dirs
        .iter()
        .filter_map(|(kind, _, opened)| opened.as_ref().err().map(|reason| (*kind, *reason)))
        .collect();
    if !refused.is_empty() {
        let mut tally = Tally::default();
        for (kind, reason) in refused {
            tally.left(kind, reason);
        }
        return tally.into_forgotten();
    }
    let (env, strip) = environment(forget, &root);
    // What is there before, so what `thread/delete` removes can be
    // counted: only if the app-server may run.
    let before = if ctx.codex_pin.is_some() && ctx.codex_app_server.is_some() {
        let (kinds, id, hooks) = (kinds.clone(), forget.agent_session_id.clone(), ctx.hooks.clone());
        let until = until.into_std();
        tokio::task::spawn_blocking(move || present(&kinds, &id, &hooks, until))
            .await
            .unwrap_or(None)
    } else {
        Some(0)
    };
    let share = until.min(start + app_server_share(ctx.deadline));
    let verdict = app_server(ctx, forget, &root, &env, &strip, share).await;
    let id = forget.agent_session_id.clone();
    match verdict {
        Verdict::Unavailable(why) => {
            tracing::info!(
                ?why,
                "Codex's app-server is unavailable for a forget; the fallback runs"
            );
            crate::forget::run_adapter(ctx, forget, &root, env, &strip, until).await;
            let hooks = ctx.hooks.clone();
            let until = until.into_std();
            let walked = tokio::task::spawn_blocking(move || {
                let mut tally = rollouts(&kinds, &id, &hooks, true, ForgetReason::StillPresent, until);
                // Decision 10: what only `thread/delete` reaches.
                tally.left_whole(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly);
                tally
            })
            .await;
            match walked {
                Ok(tally) => tally.into_forgotten(),
                Err(_) => whole(ForgetReason::IoError),
            }
        }
        Verdict::HomeMismatch => whole(ForgetReason::HomeMismatch),
        Verdict::Deleted | Verdict::Refused(_) | Verdict::Failed(_) => {
            let (left_as, the_delete) = match verdict {
                Verdict::Refused(reason) | Verdict::Failed(reason) => (reason, Some(reason)),
                _ => (ForgetReason::StillPresent, None),
            };
            let hooks = ctx.hooks.clone();
            let until = until.into_std();
            let checked = tokio::task::spawn_blocking(move || {
                let mut tally = rollouts(&kinds, &id, &hooks, false, left_as, until);
                // Counted only from two whole counts: a walk the deadline
                // cut proves nothing gone.
                if let (Some(before), Some(after)) = (before, present(&kinds, &id, &hooks, until))
                    && before > after
                {
                    tally.removed.insert(ForgetKind::Transcript, before - after);
                }
                if let Some(reason) = the_delete {
                    tally.left_whole(ForgetKind::Session, reason);
                }
                tally
            })
            .await;
            match checked {
                Ok(tally) => tally.into_forgotten(),
                Err(_) => whole(ForgetReason::IoError),
            }
        }
    }
}

/// The whole forget left, for `reason`.
fn whole(reason: ForgetReason) -> Forgotten {
    Forgotten {
        removed: Vec::new(),
        remaining: vec![left(ForgetKind::Session, 0, reason, retryable(reason))],
    }
}

/// What a forget's Codex processes run with (B6, the parent's rule):
/// `CODEX_HOME` the recorded root, `CODEX_SQLITE_HOME` the recorded one;
/// and what is removed: that one when none is recorded, and Claude's
/// project-name override always.
fn environment(forget: &Forget, root: &Path) -> (Vec<(String, String)>, Vec<&'static str>) {
    let mut env = vec![("CODEX_HOME".to_string(), root.to_string_lossy().into_owned())];
    let mut strip = vec![CLAUDE_PROJECT_VAR];
    match &forget.agent_home.sqlite_root {
        Some(sqlite) => env.push((SQLITE_VAR.to_string(), sqlite.clone())),
        None => strip.push(SQLITE_VAR),
    }
    (env, strip)
}

/// The app-server path (decision 9), within `until`.
async fn app_server(
    ctx: &ForgetContext,
    forget: &Forget,
    root: &Path,
    env: &[(String, String)],
    strip: &[&str],
    until: Instant,
) -> Verdict {
    let Some(pin) = &ctx.codex_pin else {
        return Verdict::Unavailable(Unavailable::NoPin);
    };
    let Some(binary) = &ctx.codex_app_server else {
        return Verdict::Unavailable(Unavailable::NoBinary);
    };
    let command = |arg: &str| {
        let mut command = binary.clone();
        command.args.push(arg.to_string());
        command.env.extend(env.iter().cloned());
        command
    };
    match version(&command("--version"), root, strip, until).await {
        Ok(Some(version)) if version == pin.codex_version => {}
        Ok(version) => {
            tracing::info!(?version, pinned = %pin.codex_version, "Codex's version has no pinned delete");
            return Verdict::Unavailable(Unavailable::Unpinned);
        }
        Err(verdict) => return verdict,
    }
    let (mut process, io) = match Adapter::spawn_stripped(&command("app-server"), root, strip) {
        Ok(spawned) => spawned,
        Err(err) => {
            tracing::warn!(error = %err, "Codex's app-server did not spawn");
            return Verdict::Unavailable(Unavailable::Spawn);
        }
    };
    let verdict = talk(io, pin, root, &forget.agent_session_id, until).await;
    process.terminate(ADAPTER_GRACE).await;
    verdict
}

/// A Codex CLI's `--version`, through the same hygiene, within `until`:
/// the version it printed, if any; the verdict if it did not run, or ran
/// out of time.
async fn version(
    command: &AgentCommand,
    root: &Path,
    strip: &[&str],
    until: Instant,
) -> Result<Option<String>, Verdict> {
    let (mut process, io) = match Adapter::spawn_stripped(command, root, strip) {
        Ok(spawned) => spawned,
        Err(err) => {
            tracing::warn!(error = %err, "Codex's CLI did not spawn");
            return Err(Verdict::Unavailable(Unavailable::Spawn));
        }
    };
    let AdapterIo { stdin, stdout } = io;
    drop(stdin);
    let mut out = Vec::new();
    let read = tokio::time::timeout_at(until, stdout.take(MAX_VERSION).read_to_end(&mut out)).await;
    process.terminate(ADAPTER_GRACE).await;
    match read {
        Err(_) => Err(Verdict::Failed(ForgetReason::TimedOut)),
        Ok(Err(_)) => Err(Verdict::Unavailable(Unavailable::Spawn)),
        Ok(Ok(_)) => Ok(parse_version(&String::from_utf8_lossy(&out)).map(str::to_string)),
    }
}

/// `initialize`, its `codexHome` checked, then `thread/delete` (decision
/// 9): one JSON object per line, no `jsonrpc` field, as codex-acp speaks
/// to it. Notifications and requests from the app-server are read past.
async fn talk(io: AdapterIo, pin: &CodexAppServer, root: &Path, id: &str, until: Instant) -> Verdict {
    let AdapterIo { mut stdin, stdout } = io;
    let mut reader = BufReader::new(stdout);
    let initialize = serde_json::json!({ "id": 1, "method": "initialize", "params": pin.initialize });
    if send(&mut stdin, &initialize).await.is_err() {
        return Verdict::Unavailable(Unavailable::Initialize);
    }
    match tokio::time::timeout_at(until, answer(&mut reader, 1)).await {
        Ok(Some(Answer::Result(result))) => {
            // The home it resolved is the recorded one, or nothing more is
            // asked of it.
            if result.get("codexHome").and_then(|h| h.as_str()) != Some(root.to_string_lossy().as_ref()) {
                tracing::warn!("Codex's app-server resolved another CODEX_HOME than the recorded one");
                return Verdict::HomeMismatch;
            }
        }
        Ok(Some(Answer::Error { code, message })) => {
            tracing::info!(code, %message, "Codex's app-server refused initialize");
            return Verdict::Unavailable(Unavailable::Initialize);
        }
        Ok(None) => return Verdict::Unavailable(Unavailable::Initialize),
        Err(_) => return Verdict::Failed(ForgetReason::TimedOut),
    }
    let delete = serde_json::json!({ "id": 2, "method": pin.delete_method, "params": pin.params(id) });
    if send(&mut stdin, &delete).await.is_err() {
        return Verdict::Failed(ForgetReason::IoError);
    }
    match tokio::time::timeout_at(until, answer(&mut reader, 2)).await {
        Ok(Some(answer)) => {
            let verdict = classify_delete(&answer);
            if let Answer::Error { code, message } = &answer {
                tracing::info!(code, %message, ?verdict, "Codex's thread/delete answered an error");
            }
            verdict
        }
        Ok(None) => Verdict::Failed(ForgetReason::IoError),
        Err(_) => Verdict::Failed(ForgetReason::TimedOut),
    }
}

async fn send(stdin: &mut tokio::process::ChildStdin, frame: &serde_json::Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(frame).expect("a frame serialises");
    line.push(b'\n');
    stdin.write_all(&line).await?;
    stdin.flush().await
}

/// The answer to request `want`: lines read until one carries that id and
/// a result or an error. `None` at the end of the stream, on a read error
/// or past `MAX_LINE`.
async fn answer<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R, want: u64) -> Option<Answer> {
    loop {
        let mut line = Vec::new();
        let n = (&mut *reader).take(MAX_LINE).read_until(b'\n', &mut line).await.ok()?;
        if n == 0 || (line.last() != Some(&b'\n') && n as u64 >= MAX_LINE) {
            return None;
        }
        let Ok(message) = serde_json::from_slice::<serde_json::Value>(&line) else {
            continue;
        };
        if message.get("method").is_some() || message.get("id").and_then(|i| i.as_u64()) != Some(want) {
            continue;
        }
        if let Some(error) = message.get("error") {
            return Some(Answer::Error {
                code: error.get("code").and_then(|c| c.as_i64()).unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        if let Some(result) = message.get("result") {
            return Some(Answer::Result(result.clone()));
        }
    }
}

/// The session's rollouts under the checked kind directories (decision 10,
/// B5): with `remove`, each regular file is unlinked first (never a
/// symlink, never through one); then they are counted again, what is still
/// there left for `left_as` (or why its removal failed), a symlink for
/// `symlink`. The check afterwards decides (B4).
fn rollouts(
    kinds: &Kinds,
    id: &str,
    hooks: &walk::Hooks,
    remove: bool,
    left_as: ForgetReason,
    until: std::time::Instant,
) -> Tally {
    let mut tally = Tally::default();
    // Why a removal failed, by the directory's device and inode and name.
    let mut failed: BTreeMap<(u64, u64, Vec<u8>), ForgetReason> = BTreeMap::new();
    if remove {
        let mut scratch = Tally::default();
        for (dir, path, depth) in kind_dirs(kinds) {
            let walker = Walk {
                id,
                dev: kinds.dev,
                hooks,
                max: depth,
                until,
            };
            walker.dir(
                dir,
                &path,
                0,
                &mut |dir, name, st, tally| {
                    if walk::is_file(st) {
                        match walk::unlink_file_at(dir, name) {
                            Ok(()) => *tally.removed.entry(ForgetKind::Transcript).or_default() += 1,
                            Err(_) => {
                                failed.insert(key(dir, name), ForgetReason::IoError);
                            }
                        }
                    }
                },
                &mut scratch,
            );
        }
        tally.removed = scratch.removed;
    }
    for (dir, path, depth) in kind_dirs(kinds) {
        let walker = Walk {
            id,
            dev: kinds.dev,
            hooks,
            max: depth,
            until,
        };
        walker.dir(
            dir,
            &path,
            0,
            &mut |dir, name, st, tally| {
                let reason = if walk::is_link(st) {
                    ForgetReason::Symlink
                } else if walk::is_file(st) {
                    failed.get(&key(dir, name)).copied().unwrap_or(left_as)
                } else {
                    ForgetReason::StillPresent
                };
                tally.left(ForgetKind::Transcript, reason);
            },
            &mut tally,
        );
    }
    tally
}

/// How many of the session's rollout files (regular files) are there:
/// `None` if the deadline cut the count short.
fn present(kinds: &Kinds, id: &str, hooks: &walk::Hooks, until: std::time::Instant) -> Option<u32> {
    let mut count = 0;
    let mut scratch = Tally::default();
    for (dir, path, depth) in kind_dirs(kinds) {
        let walker = Walk {
            id,
            dev: kinds.dev,
            hooks,
            max: depth,
            until,
        };
        walker.dir(
            dir,
            &path,
            0,
            &mut |_, _, st, _| {
                if walk::is_file(st) {
                    count += 1;
                }
            },
            &mut scratch,
        );
    }
    (!scratch.has_left(ForgetReason::TimedOut)).then_some(count)
}

/// The open kind directories, each with its path (for the hooks only) and
/// how deep the walk goes in it.
fn kind_dirs(kinds: &Kinds) -> Vec<(RawFd, PathBuf, usize)> {
    kinds
        .dirs
        .iter()
        .filter_map(|(_, name, opened): &KindDir| {
            let Ok(Some(fd)) = opened else {
                return None;
            };
            let depth = if *name == "sessions" {
                SESSIONS_DEPTH
            } else {
                ARCHIVED_DEPTH
            };
            Some((fd.as_raw_fd(), PathBuf::from(name), depth))
        })
        .collect()
}

/// A rollout's identity for the second pass: its directory's device and
/// inode, and its name.
fn key(dir: RawFd, name: &CString) -> (u64, u64, Vec<u8>) {
    // The field types differ by platform (`st_dev` is `i32` on macOS).
    #[allow(clippy::unnecessary_cast)]
    let (dev, ino) = walk::stat_fd(dir).map_or((0, 0), |st| (st.st_dev as u64, st.st_ino as u64));
    (dev, ino, name.as_bytes().to_vec())
}

/// One walk of a kind directory, through descriptors only (B3, R1, R2):
/// each date directory opened with `O_NOFOLLOW` relative to its parent,
/// on the root's file system, never past `max` levels.
struct Walk<'a> {
    id: &'a str,
    dev: libc::dev_t,
    hooks: &'a walk::Hooks,
    max: usize,
    /// The forget's one deadline (the review's item 4): past it the walk
    /// stops, and what it did not reach is left `timed_out`.
    until: std::time::Instant,
}

/// Called for each entry named as one of the session's rollouts, with its
/// directory, its name and what `fstatat(AT_SYMLINK_NOFOLLOW)` found.
type OnRollout<'f> = dyn FnMut(RawFd, &CString, &libc::stat, &mut Tally) + 'f;

impl Walk<'_> {
    fn dir(&self, dir: RawFd, path: &Path, depth: usize, on: &mut OnRollout<'_>, tally: &mut Tally) {
        if std::time::Instant::now() >= self.until {
            return tally.left(ForgetKind::Transcript, crate::forget::stop_reason(walk::Stop::Deadline));
        }
        let names = match self.hooks.list(dir, path) {
            Ok(names) => names,
            Err(_) => return tally.left(ForgetKind::Transcript, ForgetReason::IoError),
        };
        self.hooks.listed(path, &names);
        for name in names {
            let st = match walk::stat_at(dir, &name) {
                Ok(Some(st)) => st,
                Ok(None) => continue,
                Err(_) => {
                    tally.left(ForgetKind::Transcript, ForgetReason::IoError);
                    continue;
                }
            };
            if is_rollout_of(name.as_bytes(), self.id) {
                on(dir, &name, &st, tally);
                continue;
            }
            if depth >= self.max || !is_date_dir(name.as_bytes(), depth + 1) {
                continue;
            }
            // A date directory that is a symlink is reported, never entered.
            if walk::is_link(&st) {
                tally.left(ForgetKind::Transcript, ForgetReason::Symlink);
                continue;
            }
            if !walk::is_dir(&st) {
                continue;
            }
            let child_path = path.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
            self.hooks.stated(&child_path);
            let child = match walk::open_dir_at(dir, &name) {
                Ok(fd) => fd,
                // Swapped for a link since it was looked at (R3).
                Err(libc::ELOOP | libc::ENOTDIR) => {
                    if matches!(walk::stat_at(dir, &name), Ok(Some(st)) if walk::is_link(&st)) {
                        tally.left(ForgetKind::Transcript, ForgetReason::Symlink);
                    }
                    continue;
                }
                Err(libc::ENOENT) => continue,
                Err(_) => {
                    tally.left(ForgetKind::Transcript, ForgetReason::IoError);
                    continue;
                }
            };
            match walk::stat_fd(child.as_raw_fd()) {
                Ok(st) if st.st_dev == self.dev => self.dir(child.as_raw_fd(), &child_path, depth + 1, on, tally),
                Ok(_) => tally.left(ForgetKind::Transcript, ForgetReason::MountPoint),
                Err(_) => tally.left(ForgetKind::Transcript, ForgetReason::IoError),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b";

    fn error(code: i64, message: &str) -> Answer {
        Answer::Error {
            code,
            message: message.into(),
        }
    }

    /// Decision 9, B5, the fleet's per-outcome rule: each answer
    /// `thread/delete` can give maps to its verdict.
    #[test]
    fn each_answer_of_thread_delete_has_its_verdict() {
        assert_eq!(
            classify_delete(&Answer::Result(serde_json::json!({}))),
            Verdict::Deleted
        );
        assert_eq!(
            classify_delete(&error(-32600, &format!("no rollout found for thread id {ID}"))),
            Verdict::Deleted
        );
        assert_eq!(
            classify_delete(&error(-32600, &format!("thread not found: {ID}"))),
            Verdict::Deleted
        );
        assert_eq!(
            classify_delete(&error(
                -32600,
                &format!("cannot delete thread {ID}: forked history still references it")
            )),
            Verdict::Refused(ForgetReason::ForkedHistory)
        );
        assert_eq!(
            classify_delete(&error(
                -32600,
                &format!("thread is not persisted and cannot be deleted: {ID}")
            )),
            Verdict::Refused(ForgetReason::Ephemeral)
        );
        assert_eq!(
            classify_delete(&error(
                -32600,
                "live internal threads can only be removed by their owner"
            )),
            Verdict::Refused(ForgetReason::InProgress)
        );
        assert_eq!(
            classify_delete(&error(-32601, "thread/delete is not supported yet")),
            Verdict::Unavailable(Unavailable::MethodNotFound)
        );
        assert_eq!(
            classify_delete(&error(
                -32600,
                "Invalid request: unknown variant `thread/delete`, expected one of `initialize`"
            )),
            Verdict::Unavailable(Unavailable::MethodNotFound)
        );
        assert_eq!(
            classify_delete(&error(-32600, "Invalid request: missing field `threadId`")),
            Verdict::Unavailable(Unavailable::MethodNotFound)
        );
        assert_eq!(
            classify_delete(&error(
                -32600,
                "Invalid request: invalid type: integer `1`, expected a string"
            )),
            Verdict::Unavailable(Unavailable::MethodNotFound)
        );
        // The review's item 2: only the prefixes seen live mean "not
        // understood"; another `Invalid request:` is a failure, retried.
        assert_eq!(
            classify_delete(&error(-32600, "Invalid request: something new")),
            Verdict::Failed(ForgetReason::IoError)
        );
        for other in [
            error(-32603, "failed to delete thread: database is locked"),
            error(-32600, "failed to locate thread id x: permission denied"),
            error(-32001, "Server overloaded; retry later."),
        ] {
            assert_eq!(
                classify_delete(&other),
                Verdict::Failed(ForgetReason::IoError),
                "{other:?}"
            );
        }
    }

    /// Every reason the Codex path gives is retried only when a retry can
    /// change it (decision 4).
    #[test]
    fn only_a_live_worker_a_failure_or_a_deadline_is_retried() {
        for (reason, retried) in [
            (ForgetReason::ForkedHistory, false),
            (ForgetReason::Ephemeral, false),
            (ForgetReason::HomeMismatch, false),
            (ForgetReason::FallbackOnly, false),
            (ForgetReason::InProgress, true),
            (ForgetReason::IoError, true),
            (ForgetReason::TimedOut, true),
            (ForgetReason::StillPresent, true),
        ] {
            assert_eq!(retryable(reason), retried, "{reason:?}");
        }
    }

    #[test]
    fn the_version_is_codex_clis_own_line() {
        assert_eq!(parse_version("codex-cli 0.155.1\n"), Some("0.155.1"));
        assert_eq!(parse_version("codex-cli 0.155.1"), Some("0.155.1"));
        for bad in [
            "",
            "codex 0.155.1",
            "codex-cli 0.155",
            "codex-cli 0.155.1-alpha",
            "codex-cli  \n",
        ] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    /// B5: the anchored match, built from `recorder.rs`'s naming.
    #[test]
    fn only_the_threads_own_rollout_names_match() {
        let rollout = "019a0b1c-2d3e-7f40-8a5b-111111111111";
        for good in [
            format!("rollout-2026-10-02T10-00-00-{ID}.jsonl"),
            format!("rollout-2026-10-02T10-00-00-{ID}.jsonl.zst"),
            format!("rollout-2026-10-02T10-00-00-{ID}_{rollout}.jsonl"),
            format!("rollout-2026-10-02T10-00-00-{ID}_{rollout}.jsonl.zst"),
        ] {
            assert!(is_rollout_of(good.as_bytes(), ID), "{good}");
        }
        for bad in [
            // Another thread, and another thread whose rollout id is `ID`.
            format!("rollout-2026-10-02T10-00-00-{rollout}.jsonl"),
            format!("rollout-2026-10-02T10-00-00-{rollout}_{ID}.jsonl"),
            // A prefix, a suffix, another extension.
            format!("xrollout-2026-10-02T10-00-00-{ID}.jsonl"),
            format!("rollout-2026-10-02T10-00-00-{ID}f.jsonl"),
            format!("rollout-2026-10-02T10-00-00-{ID}.jsonl.bak"),
            format!("rollout-2026-10-02T10-00-00-{ID}.json"),
            format!("rollout-2026-10-02T10-00-00-{ID}_.jsonl"),
            format!("rollout-2026-10-02T10-00-00-{ID}_{rollout}x.jsonl"),
            format!("rollout-2026-10-02T10-00-00-{ID}"),
            // A timestamp out of shape.
            format!("rollout-2026-10-02 10-00-00-{ID}.jsonl"),
            format!("rollout-2026-10-0210-00-00-{ID}.jsonl"),
            format!("rollout-{ID}.jsonl"),
            // A path, not a name.
            format!("x/rollout-2026-10-02T10-00-00-{ID}.jsonl"),
        ] {
            assert!(!is_rollout_of(bad.as_bytes(), ID), "{bad}");
        }
        // An id that is not one never matches, even itself.
        let name = "rollout-2026-10-02T10-00-00-../../x.jsonl";
        assert!(!is_rollout_of(name.as_bytes(), "../../x"));
    }

    /// R2: a date directory on another file system is not entered, and
    /// counted (`dev ^ 1` stands for another device, as 9d-i's tests do).
    #[test]
    fn a_date_directory_on_another_file_system_is_not_entered() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions/2026/10/02");
        std::fs::create_dir_all(&day).unwrap();
        let rollout = day.join(format!("rollout-2026-10-02T10-00-00-{ID}.jsonl"));
        std::fs::write(&rollout, "x").unwrap();
        let sessions = walk::open_root(&dir.path().join("sessions")).unwrap();
        let dev = walk::stat_fd(sessions.as_raw_fd()).unwrap().st_dev;
        let hooks = walk::Hooks::default();
        let mut found = 0;
        for (dev, want) in [(dev ^ 1, 0), (dev, 1)] {
            let walker = Walk {
                id: ID,
                dev,
                hooks: &hooks,
                max: SESSIONS_DEPTH,
                until: std::time::Instant::now() + Duration::from_secs(60),
            };
            let mut tally = Tally::default();
            found = 0;
            walker.dir(
                sessions.as_raw_fd(),
                Path::new("sessions"),
                0,
                &mut |_, _, _, _| found += 1,
                &mut tally,
            );
            assert_eq!(found, want);
            let left = tally.into_forgotten().remaining;
            assert_eq!(
                left.iter().any(|r| r.reason == ForgetReason::MountPoint),
                want == 0,
                "{left:?}"
            );
        }
        assert_eq!(found, 1);
    }

    /// The review's item 4: the walk keeps the forget's one deadline. Past
    /// it, nothing is listed or reported found; what is left is
    /// `timed_out`, retried.
    #[test]
    fn a_walk_past_its_deadline_stops_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions/2026/10/02");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join(format!("rollout-2026-10-02T10-00-00-{ID}.jsonl")), "x").unwrap();
        let sessions = walk::open_root(&dir.path().join("sessions")).unwrap();
        let dev = walk::stat_fd(sessions.as_raw_fd()).unwrap().st_dev;
        let hooks = walk::Hooks::default();
        let walker = Walk {
            id: ID,
            dev,
            hooks: &hooks,
            max: SESSIONS_DEPTH,
            until: std::time::Instant::now(),
        };
        let (mut tally, mut found) = (Tally::default(), 0);
        walker.dir(
            sessions.as_raw_fd(),
            Path::new("sessions"),
            0,
            &mut |_, _, _, _| found += 1,
            &mut tally,
        );
        assert_eq!(found, 0);
        assert_eq!(
            tally.into_forgotten().remaining,
            [left(ForgetKind::Transcript, 1, ForgetReason::TimedOut, true)]
        );
    }

    /// The review's item 4: a count the deadline cut is no count, so no
    /// removal is claimed from it.
    #[test]
    fn a_count_past_its_deadline_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let day = root.join("sessions/2026/10/02");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join(format!("rollout-2026-10-02T10-00-00-{ID}.jsonl")), "x").unwrap();
        let ctx = ForgetContext {
            agents: std::collections::HashMap::new(),
            data_dir: PathBuf::from("/nonexistent-data"),
            home: None,
            hooks: walk::Hooks::default(),
            account: None,
            codex_app_server: None,
            codex_pin: None,
            deadline: crate::forget::FORGET_DEADLINE,
        };
        let kinds = check(&ctx, &root, &CODEX_KINDS).unwrap();
        let hooks = walk::Hooks::default();
        let later = std::time::Instant::now() + Duration::from_secs(60);
        assert_eq!(present(&kinds, ID, &hooks, later), Some(1));
        assert_eq!(present(&kinds, ID, &hooks, std::time::Instant::now()), None);
    }

    #[test]
    fn date_directories_are_codexs_own_shape() {
        assert!(is_date_dir(b"2026", 1) && is_date_dir(b"10", 2) && is_date_dir(b"02", 3));
        for (name, depth) in [(&b"26"[..], 1), (b"2026", 2), (b"1", 2), (b"0a", 3), (b"..", 2)] {
            assert!(!is_date_dir(name, depth), "{name:?} at {depth}");
        }
    }
}
