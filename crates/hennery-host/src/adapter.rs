//! Adapter process supervision (ACP core §2.3): spawn in its own process
//! group with a scrubbed environment and no inherited descriptor but its
//! stdio, capture a bounded stderr tail, watch for exit, and kill the whole
//! group — never just the direct child.

use std::collections::VecDeque;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::watch;

/// Environment variables that make an agent refuse to start or double-report
/// when hennery itself runs inside an agent session (ACP core §2.3).
pub const NESTING_VARS: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"];

/// The host's own secrets, which no agent may inherit: with the operator's
/// bearer an agent could answer its own permission questions, mint a
/// pairing code and enroll again after a revoke. Named one by one, not by
/// the `HENNERY_` prefix: other `HENNERY_` variables (the fake adapter's
/// script, say) are an agent's to read.
pub const HOST_SECRET_VARS: &[&str] = &["HENNERY_DEV_TOKEN"];

/// Bytes of adapter stderr kept for `adapter_exited` (ACP core §11).
pub const STDERR_TAIL_BYTES: usize = 64 * 1024;

/// How long a group gets between SIGTERM and SIGKILL (ACP core §2.3).
pub const KILL_GRACE: Duration = Duration::from_secs(5);

/// How long to wait for the stderr pipe to drain after the adapter exits.
const STDERR_SETTLE: Duration = Duration::from_millis(500);

/// How to launch an agent's ACP adapter.
#[derive(Debug, Clone)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl AgentCommand {
    /// Parse `"program arg1 arg2"` (whitespace-separated, no quoting).
    pub fn parse(command: &str) -> Option<Self> {
        let mut parts = command.split_whitespace().map(str::to_string);
        let program = parts.next()?;
        Some(Self {
            program,
            args: parts.collect(),
            env: Vec::new(),
        })
    }
}

/// How an adapter process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// The adapter's stdio for the ACP transport.
pub struct AdapterIo {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
}

/// A running (or exited) adapter process and its process group.
pub struct Adapter {
    pgid: i32,
    exit: watch::Receiver<Option<ExitInfo>>,
    stderr: Arc<Mutex<StderrRing>>,
    stderr_done: watch::Receiver<bool>,
    /// Set once this `Adapter` has itself sent (or skipped, because the exit
    /// watcher already did) a group SIGKILL; `kill_group`/`Drop` then do
    /// nothing further.
    group_killed: bool,
}

/// The bounded stderr buffer, plus whether it has ever dropped bytes from
/// the front. Once it has, the byte now at the front may sit mid-line (or
/// mid-token) rather than after a genuine newline.
#[derive(Default)]
struct StderrRing {
    buf: VecDeque<u8>,
    truncated: bool,
}

impl Adapter {
    /// Spawn `agent` in `cwd` as the leader of a new process group.
    pub fn spawn(agent: &AgentCommand, cwd: &Path) -> std::io::Result<(Self, AdapterIo)> {
        let mut command = tokio::process::Command::new(&agent.program);
        command
            .args(&agent.args)
            .envs(agent.env.iter().cloned())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        // After `envs`: a secret is stripped even if the agent's own
        // configuration names it.
        for var in NESTING_VARS.iter().chain(HOST_SECRET_VARS) {
            command.env_remove(var);
        }
        // Read before the fork: getrlimit is not async-signal-safe.
        let limit = fd_limit();
        // SAFETY: the closure runs in the forked child before `exec` and
        // calls only `fcntl` and `close`, which are async-signal-safe; it
        // allocates nothing.
        unsafe {
            command.pre_exec(move || {
                close_inherited(limit);
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let pgid = child.id().expect("a just-spawned child has a pid") as i32;
        let io = AdapterIo {
            stdin: child.stdin.take().expect("piped stdin"),
            stdout: child.stdout.take().expect("piped stdout"),
        };

        let stderr = Arc::new(Mutex::new(StderrRing::default()));
        let (stderr_tx, stderr_done) = watch::channel(false);
        let mut pipe = child.stderr.take().expect("piped stderr");
        let sink = stderr.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut ring = sink.lock().expect("stderr lock");
                ring.buf.extend(&buf[..n]);
                let excess = ring.buf.len().saturating_sub(STDERR_TAIL_BYTES);
                if excess > 0 {
                    ring.buf.drain(..excess);
                    ring.truncated = true;
                }
            }
            let _ = stderr_tx.send(true);
        });

        // The exit watcher owns the child: it is the only code that reaps it.
        let (exit_tx, exit) = watch::channel(None);
        tokio::spawn(async move {
            let info = match child.wait().await {
                Ok(status) => ExitInfo {
                    code: status.code(),
                    signal: status.signal(),
                },
                Err(_) => ExitInfo {
                    code: None,
                    signal: None,
                },
            };
            // Decision #5: after any exit, SIGKILL whatever remains of the
            // group, right here — before anything else can act on the exit.
            // The leader's pid is freed the instant `wait` reaps it, so any
            // gap before this point is a window in which the kernel could
            // recycle that number for an unrelated process group; doing it
            // inline, synchronously, keeps the window at zero.
            signal_group(pgid, libc::SIGKILL);
            let _ = exit_tx.send(Some(info));
        });

        Ok((
            Self {
                pgid,
                exit,
                stderr,
                stderr_done,
                group_killed: false,
            },
            io,
        ))
    }

    /// The adapter's process group id (its leader's pid).
    pub fn pgid(&self) -> i32 {
        self.pgid
    }

    /// Resolves when the adapter process has exited (cancel-safe).
    pub async fn exited(&mut self) -> ExitInfo {
        match self.exit.wait_for(Option::is_some).await {
            Ok(info) => info.expect("waited for Some"),
            // The watcher task is gone (runtime shutting down): treat as exited.
            Err(_) => ExitInfo {
                code: None,
                signal: None,
            },
        }
    }

    /// `Some` if the adapter exits within `limit`.
    pub async fn exited_within(&mut self, limit: Duration) -> Option<ExitInfo> {
        tokio::time::timeout(limit, self.exited()).await.ok()
    }

    /// SIGTERM the whole group, wait up to `grace` for the leader to exit,
    /// then SIGKILL the group (which also reaches descendants that ignored
    /// SIGTERM or outlived the leader).
    pub async fn terminate(&mut self, grace: Duration) -> ExitInfo {
        signal_group(self.pgid, libc::SIGTERM);
        let info = match self.exited_within(grace).await {
            Some(info) => info,
            None => {
                signal_group(self.pgid, libc::SIGKILL);
                self.exited().await
            }
        };
        self.kill_group();
        info
    }

    /// SIGKILL whatever is left of the group. Safe to call before the
    /// process has exited (e.g. from `Drop`) — but a no-op once the exit
    /// watcher has already recorded the exit, since it SIGKILLs the group
    /// itself the instant it reaps the leader (decision #5); signalling
    /// again then would only risk hitting a pgid the kernel has since
    /// recycled for an unrelated process group.
    pub fn kill_group(&mut self) {
        if self.group_killed {
            return;
        }
        if self.exit.borrow().is_none() {
            signal_group(self.pgid, libc::SIGKILL);
        }
        self.group_killed = true;
    }

    /// The last `STDERR_TAIL_BYTES` of stderr, scrubbed of token-like
    /// strings. Waits briefly for the pipe to drain if the adapter exited.
    ///
    /// Once the buffer has ever been truncated, its front byte is an
    /// arbitrary offset into the original stream, not necessarily the start
    /// of a line — a token can be cut in half there and only the tail would
    /// leak, unscrubbed, because `scrub` needs the whole prefix to recognise
    /// it. So a truncated buffer is first cut through its own first
    /// newline, trading a little more history for the guarantee that
    /// scrubbing only ever sees genuine line starts.
    pub async fn stderr_tail(&mut self) -> String {
        let _ = tokio::time::timeout(STDERR_SETTLE, self.stderr_done.wait_for(|done| *done)).await;
        let ring = self.stderr.lock().expect("stderr lock");
        let bytes: Vec<u8> = ring.buf.iter().copied().collect();
        let truncated = ring.truncated;
        drop(ring);
        if !truncated {
            return scrub(&String::from_utf8_lossy(&bytes));
        }
        match bytes.iter().position(|&b| b == b'\n') {
            Some(idx) => scrub(&String::from_utf8_lossy(&bytes[idx + 1..])),
            None => "[stderr truncated]".to_string(),
        }
    }
}

impl Drop for Adapter {
    /// A dropped actor (host shutdown, panic) must not leave the agent CLI's
    /// subtree running: `kill_on_drop` alone reaches only the direct child.
    fn drop(&mut self) {
        self.kill_group();
    }
}

/// `close_inherited` checks no descriptor at or above this, whatever the
/// soft limit: a soft `RLIMIT_NOFILE` of a million (a container's default)
/// would cost every adapter spawn a million `fcntl` calls.
pub const MAX_CLOSED_FD: libc::c_int = 65_536;

/// The first descriptor number `close_inherited` does not check: the hard
/// `RLIMIT_NOFILE`, at most `MAX_CLOSED_FD`. Not the soft limit: a
/// descriptor opened while it was higher stays open once it is lowered.
fn fd_limit() -> libc::c_int {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit(2) into a local struct of the right type.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return MAX_CLOSED_FD;
    }
    limit.rlim_max.min(MAX_CLOSED_FD as libc::rlim_t) as libc::c_int
}

/// In the forked child, before `exec`: close every descriptor from 3 up to
/// `limit` that would stay open in the agent. Whatever the host inherited
/// without close-on-exec (from `hennery up`, a service manager or a shell)
/// and whatever another thread opened without it would otherwise reach
/// every agent. Descriptors that are close-on-exec already are left alone:
/// std reports a failed `exec` over one of them.
fn close_inherited(limit: libc::c_int) {
    for fd in 3..limit {
        // SAFETY: fcntl(2) and close(2) on a descriptor number of this
        // (forked) process; both are async-signal-safe.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::close(fd);
            }
        }
    }
}

fn signal_group(pgid: i32, signal: i32) {
    // SAFETY: killpg(2) on a process group this host created. ESRCH (group
    // already gone) is expected and harmless.
    unsafe {
        libc::killpg(pgid, signal);
    }
}

const TOKEN_PREFIXES: &[&str] = &[
    "github_pat_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "sk-",
    "xoxa-",
    "xoxb-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
];
const REDACTED: &str = "[redacted]";

/// Replace token-like strings (`Bearer …`, `sk-…`, `ghp_…`, `github_pat_…`,
/// `xox?-…`) with `[redacted]`, keeping the prefix so the kind of secret is
/// still visible. Applied to stderr tails and host notes, never to ACP
/// payloads (ACP core §2.3).
pub fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_word = false;
    while i < text.len() {
        let rest = &text[i..];
        if !in_word && let Some((keep, total)) = match_secret(rest) {
            out.push_str(&rest[..keep]);
            out.push_str(REDACTED);
            i += total;
            in_word = true;
            continue;
        }
        let c = rest.chars().next().expect("non-empty rest");
        out.push(c);
        in_word = c.is_ascii_alphanumeric() || c == '_' || c == '-';
        i += c.len_utf8();
    }
    out
}

/// `(bytes to keep, bytes consumed)` if `s` starts with a secret.
fn match_secret(s: &str) -> Option<(usize, usize)> {
    for bearer in ["Bearer ", "bearer "] {
        if let Some(rest) = s.strip_prefix(bearer) {
            let n = token_len(rest);
            if n > 0 {
                return Some((bearer.len(), bearer.len() + n));
            }
        }
    }
    for prefix in TOKEN_PREFIXES {
        if let Some(rest) = s.strip_prefix(prefix) {
            let n = token_len(rest);
            // Short runs are words (`sk-learn`), not keys.
            if n >= 8 {
                return Some((prefix.len(), prefix.len() + n));
            }
        }
    }
    None
}

fn token_len(s: &str) -> usize {
    s.bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'=' | b'+' | b'/'))
        .count()
}
