//! `forget_session` on the host (plan 9d decisions 8, 11–13): remove a
//! deleted session's transcript from the agent's own data, under the root
//! the host itself registered for it (B1), and say what was removed and
//! what is left, as kinds and counts (B2).
//!
//! The connection (`connection::forget`) checks the id, the registry and
//! that no live actor has the agent's session (B7), then runs `forget` in a
//! task of its own, holding a marker that refuses any attach of that agent
//! session meanwhile.

use crate::adapter::{Adapter, AgentCommand};
use crate::walk;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{DeleteSessionRequest, InitializeRequest, SessionId};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder, UntypedMessage};
use hennery_proto::frames::{ForgetKind, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat, HostFrame};
use std::collections::{BTreeMap, HashMap};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// One deadline for a whole forget on the host (B6): below the collector's
/// wait (`hennery_sessions::forget::FORGET_WAIT`, 30 s), so its answer
/// comes first.
pub const FORGET_DEADLINE: Duration = Duration::from_secs(20);

/// What a forget needs of the host.
#[derive(Debug, Clone)]
pub struct ForgetContext {
    /// The host's agents, by name: the adapter a forget runs (decision 8).
    pub agents: HashMap<String, AgentCommand>,
    /// The host's data directory: never a root (B3).
    pub data_dir: PathBuf,
    /// The host user's home directory, if known: never a root, nor an
    /// ancestor of it (B3). `HOME` counts too.
    pub home: Option<PathBuf>,
    /// Test seams of the removal (the `test-hooks` feature): none in a
    /// real build.
    pub hooks: crate::walk::Hooks,
    /// The host user (`account`), for the ownership checks (B3).
    pub account: Account,
}

/// One forget, as the collector asked for it, checked against the
/// registry already.
#[derive(Debug, Clone)]
pub struct Forget {
    pub agent: String,
    pub agent_session_id: String,
    pub agent_home: hennery_proto::frames::AgentHome,
}

/// Whether `id` is an id the agent itself writes (decision 8): a UUID in
/// lowercase hex, as Claude's SDK names its files. Nothing else is ever
/// built into a path.
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(b),
        })
}

pub fn left(kind: ForgetKind, count: u32, reason: ForgetReason, retry: bool) -> ForgetRemaining {
    ForgetRemaining {
        what: ForgetWhat { kind, count },
        reason,
        retry,
    }
}

/// The answer for a forget that could not start: the whole session left,
/// for `reason`.
pub fn refused(request_id: String, reason: ForgetReason, retry: bool) -> HostFrame {
    HostFrame::SessionForgotten {
        request_id,
        outcome: ForgetOutcome::Partial,
        removed: Vec::new(),
        remaining: vec![left(ForgetKind::Session, 0, reason, retry)],
    }
}

/// What one forget did.
#[derive(Debug, Clone, PartialEq)]
pub struct Forgotten {
    pub removed: Vec<ForgetWhat>,
    pub remaining: Vec<ForgetRemaining>,
}

impl Forgotten {
    pub fn into_frame(self, request_id: String) -> HostFrame {
        HostFrame::SessionForgotten {
            request_id,
            outcome: if self.remaining.is_empty() {
                ForgetOutcome::Complete
            } else {
                ForgetOutcome::Partial
            },
            removed: self.removed,
            remaining: self.remaining,
        }
    }
}

/// Run one forget (plan 9d decision 8) within `FORGET_DEADLINE`. Only
/// Claude's data is removed so far; any other agent is answered
/// `unsupported_agent`, retryable, for plan 9d-ii to take up.
pub async fn forget(ctx: &ForgetContext, forget: &Forget) -> Forgotten {
    if forget.agent != crate::agent_home::CLAUDE {
        return Forgotten {
            removed: Vec::new(),
            remaining: vec![left(ForgetKind::Session, 0, ForgetReason::UnsupportedAgent, true)],
        };
    }
    if !valid_id(&forget.agent_session_id) {
        // The connection checked it; nothing is built from one that is not.
        return Forgotten {
            removed: Vec::new(),
            remaining: vec![left(ForgetKind::Session, 0, ForgetReason::InvalidId, false)],
        };
    }
    forget_claude(ctx, forget).await
}

/// Claude (decision 8, B3–B6, B9): check the root, then its kind
/// directories through descriptors, then run the adapter's own delete
/// (unless `projects/` failed its check: the adapter would follow it), then
/// remove the exact names through the descriptor walk, then check what is
/// left (B4).
async fn forget_claude(ctx: &ForgetContext, forget: &Forget) -> Forgotten {
    let until = Instant::now() + FORGET_DEADLINE;
    let root = PathBuf::from(&forget.agent_home.root);
    let checked = {
        let (ctx, root) = (ctx.clone(), root.clone());
        tokio::task::spawn_blocking(move || check(&ctx, &root)).await
    };
    let kinds = match checked {
        Ok(Ok(kinds)) => kinds,
        Ok(Err(reason)) => {
            return Forgotten {
                removed: Vec::new(),
                remaining: vec![left(ForgetKind::Session, 0, reason, retryable(reason))],
            };
        }
        Err(_) => {
            return Forgotten {
                removed: Vec::new(),
                remaining: vec![left(ForgetKind::Session, 0, ForgetReason::IoError, true)],
            };
        }
    };
    // Not with a `projects/` that failed its check: the adapter would
    // follow it (B3).
    let projects_ok = kinds
        .dirs
        .iter()
        .any(|(kind, _, opened)| *kind == ForgetKind::Transcript && opened.is_ok());
    if projects_ok {
        run_adapter(ctx, forget, &root, until).await;
    }
    let (id, hooks) = (forget.agent_session_id.clone(), ctx.hooks.clone());
    let until = until.into_std();
    match tokio::task::spawn_blocking(move || remove_and_verify(&kinds, &root, &id, &hooks, until)).await {
        Ok(forgotten) => forgotten,
        Err(_) => Forgotten {
            removed: Vec::new(),
            remaining: vec![left(ForgetKind::Session, 0, ForgetReason::IoError, true)],
        },
    }
}

/// Variables a forget's adapter never inherits unless recorded (B6):
/// another project directory name would aim Claude's delete elsewhere, and
/// another SQLite home Codex's.
pub const FORGET_STRIPPED_VARS: &[&str] = &["CLAUDE_CODE_PROJECT_DIR_NAME", "CODEX_SQLITE_HOME"];

/// How long the adapter gets of the forget's deadline (B6); the rest is
/// for stopping it and for the removal.
const ADAPTER_SHARE: Duration = Duration::from_secs(12);

/// The grace a forget's adapter gets between SIGTERM and SIGKILL.
const ADAPTER_GRACE: Duration = Duration::from_secs(2);

/// The agent's own delete (decision 8): its adapter, through
/// `Adapter::spawn`'s hygiene (B6), with `CLAUDE_CONFIG_DIR` set to the
/// root and the root as its cwd, then `initialize`, then `session/delete`
/// if it advertises it. Whatever it answers (not found included) counts
/// for nothing: the check afterwards decides (B4). Outside the no-follow
/// guarantee: the adapter resolves its own paths. Its group is killed
/// after, within the deadline.
async fn run_adapter(ctx: &ForgetContext, forget: &Forget, root: &Path, until: Instant) {
    let Some(mut command) = ctx.agents.get(&forget.agent).cloned() else {
        tracing::info!(agent = %forget.agent, "no adapter configured for a forget; only the exact entries go");
        return;
    };
    command
        .env
        .push(("CLAUDE_CONFIG_DIR".into(), root.to_string_lossy().into_owned()));
    let (mut adapter, io) = match Adapter::spawn_stripped(&command, root, FORGET_STRIPPED_VARS) {
        Ok(spawned) => spawned,
        Err(err) => {
            tracing::warn!(agent = %forget.agent, error = %err, "a forget's adapter did not spawn");
            return;
        }
    };
    let transport = ByteStreams::new(io.stdin.compat_write(), io.stdout.compat());
    let id = forget.agent_session_id.clone();
    let talk = Client
        .builder()
        .name("hennery-host")
        .on_receive_notification(
            async move |_msg: UntypedMessage, _cx| Ok(()),
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |msg: UntypedMessage, responder: Responder<serde_json::Value>, _cx| {
                responder.respond_with_error(agent_client_protocol::Error::method_not_found().data(msg.method))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async move |conn: ConnectionTo<Agent>| {
            let init = conn
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            if init.agent_capabilities.session_capabilities.delete.is_some() {
                // Not found is success (the SDK throws for it); any answer
                // is only logged.
                if let Err(err) = conn
                    .send_request(DeleteSessionRequest::new(SessionId::new(id)))
                    .block_task()
                    .await
                {
                    tracing::info!(error = %err, "the adapter's session/delete answered an error");
                }
            }
            Ok(())
        });
    let deadline = until.min(Instant::now() + ADAPTER_SHARE);
    match tokio::time::timeout_at(deadline, talk).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::info!(error = %err, "a forget's adapter connection ended"),
        Err(_) => tracing::warn!("a forget's adapter ran out of time"),
    }
    adapter.terminate(ADAPTER_GRACE).await;
}

/// The kind directories under a Claude root, each with the kind it holds
/// (decision 8).
const CLAUDE_KINDS: [(ForgetKind, &str); 5] = [
    (ForgetKind::Transcript, "projects"),
    (ForgetKind::FileHistory, "file-history"),
    (ForgetKind::SessionEnv, "session-env"),
    (ForgetKind::Tasks, "tasks"),
    (ForgetKind::Debug, "debug"),
];

/// Whether `name` is one of the transcript's own names in a project
/// directory (B9): exactly `<id>.jsonl`, `<id>`, `<id>.ccr-tip.json`,
/// `<id>.precompact.json`, `<id>.cast`, `<id>.dir-sync.json`,
/// `<id>.dir-sync-empty.json`, or one of the prefixes
/// `<id>.jsonl.superseded-` and `<id>.jsonl.compact.tmp.`. The id is a
/// checked one (`valid_id`); nothing else is ever matched.
pub fn in_transcript_family(name: &[u8], id: &str) -> bool {
    let Some(rest) = name.strip_prefix(id.as_bytes()) else {
        return false;
    };
    matches!(
        rest,
        b"" | b".jsonl"
            | b".ccr-tip.json"
            | b".precompact.json"
            | b".cast"
            | b".dir-sync.json"
            | b".dir-sync-empty.json"
    ) || rest.starts_with(b".jsonl.superseded-")
        || rest.starts_with(b".jsonl.compact.tmp.")
}

/// The one entry of a non-transcript kind (decision 8): `<id>` in
/// `file-history`, `session-env` and `tasks`, `<id>.txt` in `debug`.
fn kind_entry(kind: ForgetKind, id: &str) -> String {
    match kind {
        ForgetKind::Debug => format!("{id}.txt"),
        _ => id.to_string(),
    }
}

/// The host user, as the safety checks need them (B3): the effective uid,
/// and its account's primary gid and name (`getpwuid_r`). With no account
/// the name is empty and the gid matches nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
    pub name: Vec<u8>,
}

/// A group, as `getgrgid_r` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub gid: libc::gid_t,
    pub name: Vec<u8>,
    pub members: Vec<Vec<u8>>,
}

/// Whether a root or kind directory with `st` is safe to remove from (B3;
/// the review's item 5): the host user's, never writable by others, and
/// writable by its group only when that group is the user's own private
/// one: the account's primary group, named as the user, with no member
/// but the user (a umask of 002 makes every directory so). A shared group
/// (`staff`) does not count.
pub fn safe_mode(st: &libc::stat, account: &Account, group: Option<&Group>) -> bool {
    if st.st_uid != account.uid || st.st_mode & 0o002 != 0 {
        return false;
    }
    if st.st_mode & 0o020 == 0 {
        return true;
    }
    let Some(group) = group else {
        return false;
    };
    !account.name.is_empty()
        && st.st_gid == account.gid
        && group.gid == st.st_gid
        && group.name == account.name
        && (group.members.is_empty() || group.members == [account.name.clone()])
}

/// The size of a buffer for `getpwuid_r` / `getgrgid_r`, and its growth.
const LOOKUP_BUFFER: usize = 1024;
const LOOKUP_BUFFER_MAX: usize = 1 << 20;

/// The host user's account (`getpwuid_r` of the effective uid), only the
/// reentrant call.
pub fn account() -> Account {
    // SAFETY: geteuid(2) cannot fail.
    let uid = unsafe { libc::geteuid() };
    let mut size = LOOKUP_BUFFER;
    while size <= LOOKUP_BUFFER_MAX {
        let mut buf = vec![0 as libc::c_char; size];
        // SAFETY: an all-zero `passwd` is a valid value for the call to fill.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: getpwuid_r(3) into local storage of the given size.
        let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut found) };
        match rc {
            0 if !found.is_null() => {
                // SAFETY: `pw_name` points into `buf`, NUL-terminated.
                let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }.to_bytes().to_vec();
                return Account {
                    uid,
                    gid: pwd.pw_gid,
                    name,
                };
            }
            libc::ERANGE => size *= 2,
            libc::EINTR => {}
            _ => break,
        }
    }
    Account {
        uid,
        gid: libc::gid_t::MAX,
        name: Vec::new(),
    }
}

/// The group `gid` (`getgrgid_r`), if there is one.
pub fn group(gid: libc::gid_t) -> Option<Group> {
    let mut size = LOOKUP_BUFFER;
    while size <= LOOKUP_BUFFER_MAX {
        let mut buf = vec![0 as libc::c_char; size];
        // SAFETY: as in `account`.
        let mut grp: libc::group = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::group = std::ptr::null_mut();
        // SAFETY: getgrgid_r(3) into local storage of the given size.
        let rc = unsafe { libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr(), buf.len(), &mut found) };
        match rc {
            0 if !found.is_null() => {
                // SAFETY: the name and the NULL-terminated member list point
                // into `buf`.
                let name = unsafe { std::ffi::CStr::from_ptr(grp.gr_name) }.to_bytes().to_vec();
                let mut members = Vec::new();
                let mut at = grp.gr_mem;
                // SAFETY: as above; the list ends with a NULL pointer.
                unsafe {
                    while !at.is_null() && !(*at).is_null() {
                        members.push(std::ffi::CStr::from_ptr(*at).to_bytes().to_vec());
                        at = at.add(1);
                    }
                }
                return Some(Group {
                    gid: grp.gr_gid,
                    name,
                    members,
                });
            }
            0 => return None,
            libc::ERANGE => size *= 2,
            libc::EINTR => {}
            _ => return None,
        }
    }
    None
}

/// `safe_mode` for `st`, looking its group up only if it matters.
fn safely_owned(st: &libc::stat, account: &Account) -> bool {
    let group = if st.st_mode & 0o020 != 0 {
        group(st.st_gid)
    } else {
        None
    };
    safe_mode(st, account, group.as_ref())
}

/// `(st_dev, st_ino)` of `path`, following links.
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// The root, checked (decision 8, B3), open: absolute and canonical, still
/// resolving to itself; not `/`, the host user's home or an ancestor of
/// it, nor the host's data directory or an ancestor of it; a real
/// directory of the host user's, not writable by others. With its device.
fn open_checked_root(ctx: &ForgetContext, root: &Path) -> Result<(OwnedFd, libc::dev_t), ForgetReason> {
    if !root.is_absolute() {
        return Err(ForgetReason::UnsafeRoot);
    }
    match std::fs::canonicalize(root) {
        Ok(canonical) if canonical == root => {}
        Ok(_) => return Err(ForgetReason::UnsafeRoot),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(ForgetReason::RootMissing),
        Err(_) => return Err(ForgetReason::UnsafeRoot),
    }
    let fd = walk::open_root(root).map_err(|_| ForgetReason::UnsafeRoot)?;
    let st = walk::stat_fd(fd.as_raw_fd()).map_err(|_| ForgetReason::UnsafeRoot)?;
    // The field types differ by platform (`st_dev` is `i32` on macOS).
    #[allow(clippy::unnecessary_cast)]
    let opened = (st.st_dev as u64, st.st_ino as u64);
    // The directory opened is the one the path names now (the review's
    // item 9): nothing was swapped in between the checks and the open.
    if identity(root) != Some(opened) {
        return Err(ForgetReason::UnsafeRoot);
    }
    // Not the host's data directory, nor the user's home, nor one of
    // their ancestors (`/` too), compared by device and inode.
    let mut guarded: Vec<PathBuf> = vec![ctx.data_dir.clone()];
    guarded.extend(ctx.home.clone());
    guarded.extend(std::env::var_os("HOME").map(PathBuf::from));
    for guarded in guarded {
        let guarded = std::fs::canonicalize(&guarded).unwrap_or(guarded);
        if guarded.ancestors().any(|dir| identity(dir) == Some(opened)) {
            return Err(ForgetReason::UnsafeRoot);
        }
    }
    if !walk::is_dir(&st) || !safely_owned(&st, &ctx.account) {
        return Err(ForgetReason::UnsafeRoot);
    }
    Ok((fd, st.st_dev))
}

/// A kind directory, checked before anything runs (B3): open through no
/// symlink, the host user's, not writable by others, on the root's file
/// system. `Ok(None)` if there is none.
fn open_kind(root: RawFd, name: &str, dev: libc::dev_t, account: &Account) -> Result<Option<OwnedFd>, ForgetReason> {
    let c_name = walk::c_name(name.as_bytes());
    let fd = match walk::open_dir_at(root, &c_name) {
        Ok(fd) => fd,
        Err(libc::ENOENT) => return Ok(None),
        // A symlink is refused as `ELOOP` on some systems and `ENOTDIR`
        // on others (macOS): which it is, is read without following it.
        Err(libc::ELOOP | libc::ENOTDIR) => {
            return Err(match walk::stat_at(root, &c_name) {
                Ok(Some(st)) if walk::is_link(&st) => ForgetReason::Symlink,
                _ => ForgetReason::NotADirectory,
            });
        }
        Err(_) => return Err(ForgetReason::IoError),
    };
    let st = walk::stat_fd(fd.as_raw_fd()).map_err(|_| ForgetReason::IoError)?;
    if st.st_dev != dev {
        return Err(ForgetReason::MountPoint);
    }
    if !safely_owned(&st, account) {
        return Err(ForgetReason::UnsafeDirectory);
    }
    Ok(Some(fd))
}

/// A retry can change what is left for `reason` (decision 4): an entry
/// still there, or a removal that failed midway (B3). A symlink, a mount
/// point, an unsafe directory stay as they are.
fn retryable(reason: ForgetReason) -> bool {
    matches!(
        reason,
        ForgetReason::StillPresent | ForgetReason::IoError | ForgetReason::TimedOut | ForgetReason::InProgress
    )
}

/// What is counted per kind.
#[derive(Default)]
struct Tally {
    removed: BTreeMap<ForgetKind, u32>,
    left: BTreeMap<(ForgetKind, ForgetReason), u32>,
}

impl Tally {
    fn left(&mut self, kind: ForgetKind, reason: ForgetReason) {
        *self.left.entry((kind, reason)).or_default() += 1;
    }

    fn into_forgotten(self) -> Forgotten {
        Forgotten {
            removed: self
                .removed
                .into_iter()
                .map(|(kind, count)| ForgetWhat { kind, count })
                .collect(),
            remaining: self
                .left
                .into_iter()
                .map(|((kind, reason), count)| left(kind, count, reason, retryable(reason)))
                .collect(),
        }
    }
}

fn stop_reason(stop: walk::Stop) -> ForgetReason {
    match stop {
        walk::Stop::MountPoint => ForgetReason::MountPoint,
        walk::Stop::TooDeep => ForgetReason::TooDeep,
        walk::Stop::Io(_) | walk::Stop::Swapped => ForgetReason::IoError,
        walk::Stop::Deadline => ForgetReason::TimedOut,
    }
}

/// One kind directory as checked: its kind, its name, and it open (none
/// there), or why it is skipped.
type KindDir = (ForgetKind, &'static str, Result<Option<OwnedFd>, ForgetReason>);

/// The checked root's kind directories, as opened before the adapter ran.
struct Kinds {
    root: OwnedFd,
    dev: libc::dev_t,
    dirs: Vec<KindDir>,
}

/// The root and its kind directories, checked (B3). `Err` for the root.
fn check(ctx: &ForgetContext, root: &Path) -> Result<Kinds, ForgetReason> {
    let (root_fd, dev) = open_checked_root(ctx, root)?;
    let dirs = CLAUDE_KINDS
        .iter()
        .map(|&(kind, name)| (kind, name, open_kind(root_fd.as_raw_fd(), name, dev, &ctx.account)))
        .collect();
    Ok(Kinds {
        root: root_fd,
        dev,
        dirs,
    })
}

/// The project directories under `projects` that are real directories on
/// `dev`, each open, with its name; a symlinked one is counted and never
/// followed (decision 8; the review's item 4); one on another file system
/// is counted; a file there is no project directory.
/// `base` is the directory's path, for the logs only: nothing is looked up
/// by it.
fn project_dirs(
    projects: RawFd,
    base: &Path,
    dev: libc::dev_t,
    tally: &mut Tally,
) -> Vec<(OwnedFd, std::ffi::CString)> {
    let names = match walk::list(projects) {
        Ok(names) => names,
        Err(_) => {
            tally.left(ForgetKind::Transcript, ForgetReason::IoError);
            return Vec::new();
        }
    };
    let mut dirs = Vec::new();
    for name in names {
        let _path = base.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
        match walk::open_dir_at(projects, &name) {
            Ok(fd) => match walk::stat_fd(fd.as_raw_fd()) {
                Ok(st) if st.st_dev == dev => dirs.push((fd, name)),
                Ok(_) => tally.left(ForgetKind::Transcript, ForgetReason::MountPoint),
                Err(_) => tally.left(ForgetKind::Transcript, ForgetReason::IoError),
            },
            Err(libc::ELOOP | libc::ENOTDIR) => {
                if matches!(walk::stat_at(projects, &name), Ok(Some(st)) if walk::is_link(&st)) {
                    tally.left(ForgetKind::Transcript, ForgetReason::Symlink);
                }
            }
            Err(libc::ENOENT) => {}
            Err(_) => tally.left(ForgetKind::Transcript, ForgetReason::IoError),
        }
    }
    dirs
}

/// The exact entries of `kinds` (B9), each through descriptors (R1, R2),
/// then the check afterwards (B4): what is still there is what is left,
/// whatever the removal reported.
fn remove_and_verify(
    kinds: &Kinds,
    root: &Path,
    id: &str,
    hooks: &walk::Hooks,
    until: std::time::Instant,
) -> Forgotten {
    let mut tally = Tally::default();
    // Project directories whose listing failed, in either pass: each counts
    // once, as left for a retry (the review's item 1).
    let mut unlisted: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
    // Why an entry was not removed, by kind and name, for the check after.
    let mut why: BTreeMap<(ForgetKind, Vec<u8>), ForgetReason> = BTreeMap::new();
    let mut each = |kind: ForgetKind, dir: RawFd, path: PathBuf, name: &[u8], tally: &mut Tally| {
        let c = walk::c_name(name);
        match walk::remove_entry(dir, &c, kinds.dev, &path, hooks, until) {
            walk::Removal::Absent => {}
            walk::Removal::Removed => *tally.removed.entry(kind).or_default() += 1,
            walk::Removal::Symlink => {
                why.insert((kind, name.to_vec()), ForgetReason::Symlink);
            }
            walk::Removal::Stopped(stop) => {
                why.insert((kind, name.to_vec()), stop_reason(stop));
            }
        }
    };
    for (kind, dir_name, opened) in &kinds.dirs {
        let dir = match opened {
            Ok(Some(fd)) => fd.as_raw_fd(),
            Ok(None) => continue,
            Err(reason) => {
                tally.left(*kind, *reason);
                continue;
            }
        };
        let base = root.join(dir_name);
        if *kind == ForgetKind::Transcript {
            // What the first pass finds of the project directories is found
            // again, and counted, by the check after.
            let mut scratch = Tally::default();
            for (project, project_name) in project_dirs(dir, &base, kinds.dev, &mut scratch) {
                let project_path = base.join(std::ffi::OsStr::from_bytes(project_name.as_bytes()));
                let names = match hooks.list(project.as_raw_fd(), &project_path) {
                    Ok(names) => names,
                    Err(_) => {
                        if unlisted.insert(project_name.as_bytes().to_vec()) {
                            tally.left(ForgetKind::Transcript, ForgetReason::IoError);
                        }
                        continue;
                    }
                };
                for name in names.iter().filter(|n| in_transcript_family(n.as_bytes(), id)) {
                    let path = project_path.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
                    each(*kind, project.as_raw_fd(), path, name.as_bytes(), &mut tally);
                }
            }
        } else {
            let name = kind_entry(*kind, id);
            each(*kind, dir, base.join(&name), name.as_bytes(), &mut tally);
        }
    }
    // The check afterwards (B4).
    let verify =
        |kind: ForgetKind, dir: RawFd, name: &[u8], tally: &mut Tally| match walk::stat_at(dir, &walk::c_name(name)) {
            Ok(None) => {}
            Ok(Some(st)) if walk::is_link(&st) => tally.left(kind, ForgetReason::Symlink),
            Ok(Some(_)) => {
                let reason = why
                    .get(&(kind, name.to_vec()))
                    .copied()
                    .unwrap_or(ForgetReason::StillPresent);
                tally.left(kind, reason);
            }
            Err(_) => tally.left(kind, ForgetReason::IoError),
        };
    for (kind, dir_name, opened) in &kinds.dirs {
        let Ok(Some(fd)) = opened else {
            continue;
        };
        if *kind == ForgetKind::Transcript {
            let base = root.join(dir_name);
            for (project, project_name) in project_dirs(fd.as_raw_fd(), &base, kinds.dev, &mut tally) {
                let project_path = base.join(std::ffi::OsStr::from_bytes(project_name.as_bytes()));
                let names = match hooks.list(project.as_raw_fd(), &project_path) {
                    Ok(names) => names,
                    Err(_) => {
                        if unlisted.insert(project_name.as_bytes().to_vec()) {
                            tally.left(ForgetKind::Transcript, ForgetReason::IoError);
                        }
                        continue;
                    }
                };
                for name in names.iter().filter(|n| in_transcript_family(n.as_bytes(), id)) {
                    verify(*kind, project.as_raw_fd(), name.as_bytes(), &mut tally);
                }
            }
        } else {
            verify(*kind, fd.as_raw_fd(), kind_entry(*kind, id).as_bytes(), &mut tally);
        }
    }
    let _ = &kinds.root;
    tally.into_forgotten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Account {
        Account {
            uid: 501,
            gid: 501,
            name: b"me".to_vec(),
        }
    }

    fn dir(uid: libc::uid_t, gid: libc::gid_t, mode: libc::mode_t) -> libc::stat {
        // SAFETY: an all-zero `stat` is a valid value.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        st.st_uid = uid;
        st.st_gid = gid;
        st.st_mode = libc::S_IFDIR | mode;
        st
    }

    fn grp(gid: libc::gid_t, name: &str, members: &[&str]) -> Group {
        Group {
            gid,
            name: name.as_bytes().to_vec(),
            members: members.iter().map(|m| m.as_bytes().to_vec()).collect(),
        }
    }

    /// The review's item 5.
    #[test]
    fn only_the_users_private_group_may_write_a_directory_besides_the_user() {
        let me = me();
        // Not group-writable: only the owner counts.
        assert!(safe_mode(&dir(501, 20, 0o755), &me, None));
        assert!(!safe_mode(&dir(502, 501, 0o755), &me, None));
        // A private group (umask 002): the user's own, empty or just them.
        let private = grp(501, "me", &[]);
        assert!(safe_mode(&dir(501, 501, 0o775), &me, Some(&private)));
        assert!(safe_mode(&dir(501, 501, 0o775), &me, Some(&grp(501, "me", &["me"]))));
        // A shared group, `staff` (gid 20).
        assert!(!safe_mode(
            &dir(501, 20, 0o775),
            &me,
            Some(&grp(20, "staff", &["me", "you"]))
        ));
        // A group named as the user and only theirs, but not their
        // primary one.
        assert!(!safe_mode(&dir(501, 600, 0o775), &me, Some(&grp(600, "me", &[]))));
        // The primary gid, but named otherwise, or with another member.
        assert!(!safe_mode(&dir(501, 501, 0o775), &me, Some(&grp(501, "dev", &[]))));
        assert!(!safe_mode(&dir(501, 501, 0o775), &me, Some(&grp(501, "me", &["you"]))));
        assert!(!safe_mode(
            &dir(501, 501, 0o775),
            &me,
            Some(&grp(501, "me", &["me", "you"]))
        ));
        // A group it could not look up, or an account with no name.
        assert!(!safe_mode(&dir(501, 501, 0o775), &me, None));
        let nameless = Account {
            name: Vec::new(),
            ..me.clone()
        };
        assert!(!safe_mode(&dir(501, 501, 0o775), &nameless, Some(&grp(501, "", &[]))));
        // Writable by others: never.
        assert!(!safe_mode(&dir(501, 501, 0o777), &me, Some(&private)));
        assert!(!safe_mode(&dir(501, 501, 0o757), &me, Some(&private)));
    }

    /// `account` and `group` find the host user (the reentrant calls).
    #[test]
    fn the_host_user_and_their_group_are_found() {
        let account = account();
        // SAFETY: geteuid(2) cannot fail.
        assert_eq!(account.uid, unsafe { libc::geteuid() });
        assert!(!account.name.is_empty());
        assert_eq!(super::group(account.gid).map(|g| g.gid), Some(account.gid));
    }

    /// The review's item 7: a kind directory on another device is refused.
    #[test]
    fn a_kind_directory_on_another_file_system_is_refused() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("tasks")).unwrap();
        let fd = walk::open_root(root.path()).unwrap();
        let dev = walk::stat_fd(fd.as_raw_fd()).unwrap().st_dev;
        let me = account();
        assert!(matches!(open_kind(fd.as_raw_fd(), "tasks", dev, &me), Ok(Some(_))));
        assert_eq!(
            open_kind(fd.as_raw_fd(), "tasks", dev ^ 1, &me).err(),
            Some(ForgetReason::MountPoint)
        );
    }

    /// The review's item 3: what the deadline left, a retry may finish.
    #[test]
    fn a_removal_cut_by_its_deadline_is_retried() {
        assert_eq!(stop_reason(walk::Stop::Deadline), ForgetReason::TimedOut);
        assert!(retryable(ForgetReason::TimedOut));
        assert!(!retryable(ForgetReason::Symlink));
    }

    #[test]
    fn only_the_transcripts_own_names_are_its_family() {
        let id = "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";
        for suffix in [
            ".jsonl",
            "",
            ".ccr-tip.json",
            ".precompact.json",
            ".cast",
            ".dir-sync.json",
            ".dir-sync-empty.json",
            ".jsonl.superseded-1",
            ".jsonl.compact.tmp.x",
        ] {
            assert!(
                in_transcript_family(format!("{id}{suffix}").as_bytes(), id),
                "{suffix:?}"
            );
        }
        for name in [
            format!("{id}x.jsonl"),
            format!("{id}.jsonl.bak"),
            format!("{id}.json"),
            format!("x{id}.jsonl"),
            "other.jsonl".to_string(),
        ] {
            assert!(!in_transcript_family(name.as_bytes(), id), "{name:?}");
        }
    }

    #[test]
    fn only_a_lowercase_uuid_is_an_agent_session_id() {
        assert!(valid_id("0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3"));
        for bad in [
            "0B9C1D2E-3F40-4A5B-8C6D-7E8F90A1B2C3",
            "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c",
            "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3x",
            "0b9c1d2e_3f40-4a5b-8c6d-7e8f90a1b2c3",
            "../../../../../../etc/passwd/aaaaaaa",
            "fake-session-1",
            "",
            "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2g3",
        ] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }
}
