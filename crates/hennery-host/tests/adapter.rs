//! The adapter supervisor against plain shell processes: group kill reaches
//! grandchildren, SIGTERM escalates to SIGKILL, dropping an adapter kills its
//! group, stderr is bounded and scrubbed, nesting variables are stripped,
//! and no descriptor but its stdio reaches the agent.

use hennery_host::adapter::{Adapter, AgentCommand, STDERR_TAIL_BYTES, scrub};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

fn sh(script: &str) -> AgentCommand {
    AgentCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        env: Vec::new(),
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes for existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Poll until `pid` is gone (a killed orphan is briefly a zombie until init
/// reaps it).
async fn wait_dead(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "pid {pid} is still alive");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn read_pid(path: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|s| s.trim().parse().ok()) {
            return pid;
        }
        assert!(Instant::now() < deadline, "no pid in {}", path.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Poll until `path` exists, instead of sleeping a fixed guess at how long
/// some earlier setup step (e.g. installing a signal trap) takes.
async fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{} never appeared", path.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `sh` that starts a background `sleep` (the grandchild), records its pid,
/// then waits on it.
fn with_grandchild(pid_file: &Path) -> AgentCommand {
    sh(&format!("sleep 600 & echo $! > {}; wait", pid_file.display()))
}

#[tokio::test]
async fn terminate_kills_the_whole_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("gc.pid");
    let (mut adapter, _io) = Adapter::spawn(&with_grandchild(&pid_file), dir.path()).unwrap();
    let grandchild = read_pid(&pid_file).await;
    assert!(alive(grandchild));
    adapter.terminate(Duration::from_secs(2)).await;
    wait_dead(grandchild).await;
}

#[tokio::test]
async fn terminate_escalates_to_sigkill_when_sigterm_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let script = sh(&format!(
        "trap '' TERM; touch {}; while :; do sleep 0.05; done",
        ready.display()
    ));
    let (mut adapter, _io) = Adapter::spawn(&script, dir.path()).unwrap();
    wait_for_file(&ready).await; // poll until the trap is actually installed
    let began = Instant::now();
    let info = adapter.terminate(Duration::from_millis(300)).await;
    assert!(began.elapsed() >= Duration::from_millis(300), "{:?}", began.elapsed());
    assert_eq!(info.signal, Some(libc::SIGKILL), "{info:?}");
}

#[tokio::test]
async fn dropping_an_adapter_kills_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("gc.pid");
    let (adapter, _io) = Adapter::spawn(&with_grandchild(&pid_file), dir.path()).unwrap();
    let grandchild = read_pid(&pid_file).await;
    drop(adapter);
    wait_dead(grandchild).await;
}

#[tokio::test]
async fn a_leaders_own_exit_reaps_its_grandchild_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("gc.pid");
    // The leader starts a grandchild that outlives it and exits right away
    // (no `wait`, unlike `with_grandchild`) — an unattended, "unexpected"
    // exit with no `terminate`/`kill_group`/`Drop` call in sight.
    let script = sh(&format!("sleep 600 & echo $! > {}", pid_file.display()));
    let (mut adapter, _io) = Adapter::spawn(&script, dir.path()).unwrap();
    let grandchild = read_pid(&pid_file).await;
    adapter.exited().await;
    // Decision #5: the exit watcher SIGKILLs the rest of the group itself,
    // the instant it reaps the leader — nobody else has to.
    wait_dead(grandchild).await;
}

#[tokio::test]
async fn an_unexpected_exit_reports_code_and_a_bounded_scrubbed_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    // The explicit `\n` right after the padding guarantees the padding/last
    // line boundary survives the truncation cut (whichever `x` it lands on):
    // once truncated, `stderr_tail` drops through its *own* first newline,
    // so without one there to anchor it, this would drop the last line too.
    let script = "echo 'early Bearer abc.def' >&2; \
                  head -c 100000 /dev/zero | tr '\\0' x >&2; \
                  printf '\\n' >&2; \
                  echo ' late ghp_abcdefghijklmnop' >&2; exit 7";
    let (mut adapter, _io) = Adapter::spawn(&sh(script), dir.path()).unwrap();
    let info = adapter.exited().await;
    assert_eq!((info.code, info.signal), (Some(7), None));
    let tail = adapter.stderr_tail().await;
    assert!(tail.len() <= STDERR_TAIL_BYTES, "{}", tail.len());
    assert_eq!(tail, " late ghp_[redacted]\n");
    assert!(!tail.contains("early"), "the tail keeps only the last bytes");
}

#[tokio::test]
async fn a_token_split_by_the_truncation_boundary_never_leaks_even_a_fragment() {
    let dir = tempfile::tempdir().unwrap();
    // A single unbroken run of padding (no newlines) big enough that the
    // rolling 64 KiB window must cut through it — and, since nothing before
    // the final line has a newline, through the token too, wherever exactly
    // the cut lands. A real token cut this way would keep a suffix like
    // `p_abcdefghijklmnopqrstuvwxyz0123456789` (missing its `ghp_` prefix),
    // which `scrub` cannot recognise without the prefix.
    let script = "head -c 70000 /dev/zero | tr '\\0' x >&2; \
                  printf 'ghp_abcdefghijklmnopqrstuvwxyz0123456789\\nsafe-tail-marker\\n' >&2; exit 0";
    let (mut adapter, _io) = Adapter::spawn(&sh(script), dir.path()).unwrap();
    adapter.exited().await;
    let tail = adapter.stderr_tail().await;
    assert_eq!(
        tail, "safe-tail-marker\n",
        "a truncated tail must drop through its first newline, never scrub a slice of a token"
    );
}

#[tokio::test]
async fn nesting_variables_are_removed_from_the_adapter_environment() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("env.txt");
    let mut cmd = sh(&format!("env > {}", out.display()));
    cmd.env.push(("CLAUDECODE".into(), "1".into()));
    cmd.env.push(("HENNERY_KEEP".into(), "yes".into()));
    let (mut adapter, _io) = Adapter::spawn(&cmd, dir.path()).unwrap();
    adapter.exited().await;
    let env = std::fs::read_to_string(&out).unwrap();
    assert!(env.contains("HENNERY_KEEP=yes"), "{env}");
    assert!(!env.contains("CLAUDECODE="), "{env}");
}

/// Final review I1: the operator's bearer never reaches an agent, which
/// could otherwise answer its own permission questions, mint a pairing code
/// and enroll again after a revoke. Other `HENNERY_` variables still pass.
#[tokio::test]
async fn the_operator_token_is_removed_from_the_adapter_environment() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("env.txt");
    let mut cmd = sh(&format!("env > {}", out.display()));
    cmd.env
        .push(("HENNERY_DEV_TOKEN".into(), "operator-token-never-for-agents".into()));
    cmd.env.push(("HENNERY_KEEP".into(), "yes".into()));
    let (mut adapter, _io) = Adapter::spawn(&cmd, dir.path()).unwrap();
    adapter.exited().await;
    let env = std::fs::read_to_string(&out).unwrap();
    assert!(env.contains("HENNERY_KEEP=yes"), "{env}");
    assert!(!env.contains("HENNERY_DEV_TOKEN="), "{env}");
    assert!(!env.contains("operator-token-never-for-agents"), "{env}");
}

#[test]
fn scrub_redacts_token_like_strings_and_leaves_words_alone() {
    let cases = [
        ("Authorization: Bearer abc.DEF-123", "Authorization: Bearer [redacted]"),
        ("key=sk-ant-api03-abcdefgh end", "key=sk-[redacted] end"),
        (
            "tokens ghp_1234567890abcdef and github_pat_11ABCDEFG_xyz",
            "tokens ghp_[redacted] and github_pat_[redacted]",
        ),
        ("slack xoxb-1234-5678-abcdefgh", "slack xoxb-[redacted]"),
        (
            "use sk-learn and a task-sk-12345678",
            "use sk-learn and a task-sk-12345678",
        ),
        ("zażółć gęślą jaźń", "zażółć gęślą jaźń"),
    ];
    for (input, expected) in cases {
        assert_eq!(scrub(input), expected, "input {input:?}");
    }
}

/// Closes a raw descriptor on drop, however the test ends.
struct CloseOnDrop(i32);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        // SAFETY: close(2) on a descriptor this test opened.
        unsafe { libc::close(self.0) };
    }
}

/// The descriptor numbers `ls /dev/fd` printed.
fn listed(out: &[u8]) -> Vec<i32> {
    String::from_utf8_lossy(out)
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// A descriptor the host holds open across `exec` (inherited from `hennery
/// up`, a service manager or a shell, or opened by another thread without
/// close-on-exec) never reaches an agent: it gets its stdio and nothing
/// else of the host's.
#[tokio::test]
async fn an_adapter_inherits_no_descriptor_but_its_stdio() {
    let file = std::fs::File::open("/dev/null").unwrap();
    // SAFETY: fcntl(2) on an open descriptor: a copy at 64 or above,
    // without close-on-exec, as a leaked one would be.
    let leaked = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 64) };
    assert!(leaked >= 64, "{}", std::io::Error::last_os_error());
    let _close = CloseOnDrop(leaked);
    // Control: a plain spawn passes it on.
    let control = std::process::Command::new("ls").arg("/dev/fd").output().unwrap();
    assert!(listed(&control.stdout).contains(&leaked), "{control:?}");

    let dir = tempfile::tempdir().unwrap();
    let ls = AgentCommand {
        program: "ls".into(),
        args: vec!["/dev/fd".into()],
        env: Vec::new(),
    };
    let (mut adapter, mut io) = Adapter::spawn(&ls, dir.path()).unwrap();
    let mut out = Vec::new();
    io.stdout.read_to_end(&mut out).await.unwrap();
    adapter.exited().await;
    let fds = listed(&out);
    assert!(fds.contains(&0) && fds.contains(&1), "{fds:?}");
    assert!(
        !fds.contains(&leaked),
        "the agent inherited descriptor {leaked}: {fds:?}"
    );
}

/// Set in the copy of this binary that
/// `a_descriptor_above_a_lowered_soft_limit_is_closed_too` runs.
const LOWERED_SOFT_LIMIT: &str = "HENNERY_TEST_LOWERED_SOFT_LIMIT";

/// Held open across `exec` above the lowered soft limit.
const ABOVE_THE_LIMIT: i32 = 60;

/// A descriptor opened while the soft `RLIMIT_NOFILE` was higher stays open
/// once it is lowered, so the spawn closes up to the hard limit. The test
/// runs itself again in a copy of this binary, holding descriptor 60 open
/// with the soft limit lowered to 50: the limit is process-wide, and would
/// starve the other tests here.
#[tokio::test]
async fn a_descriptor_above_a_lowered_soft_limit_is_closed_too() {
    if std::env::var_os(LOWERED_SOFT_LIMIT).is_some() {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit(2) and fcntl(2) into and on local values.
        unsafe {
            assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit), 0);
            assert!(libc::fcntl(ABOVE_THE_LIMIT, libc::F_GETFD) >= 0, "not inherited");
        }
        assert!(limit.rlim_cur < ABOVE_THE_LIMIT as libc::rlim_t, "{}", limit.rlim_cur);
        let dir = tempfile::tempdir().unwrap();
        let ls = AgentCommand {
            program: "ls".into(),
            args: vec!["/dev/fd".into()],
            env: Vec::new(),
        };
        let (mut adapter, mut io) = Adapter::spawn(&ls, dir.path()).unwrap();
        let mut out = Vec::new();
        io.stdout.read_to_end(&mut out).await.unwrap();
        adapter.exited().await;
        let fds = listed(&out);
        assert!(fds.contains(&1), "{fds:?}");
        assert!(!fds.contains(&ABOVE_THE_LIMIT), "the agent inherited it: {fds:?}");
        return;
    }
    let file = std::fs::File::open("/dev/null").unwrap();
    let fd = file.as_raw_fd();
    let mut hard = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit(2) into a local struct.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut hard) }, 0);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", "a_descriptor_above_a_lowered_soft_limit_is_closed_too"])
        .env(LOWERED_SOFT_LIMIT, "1");
    // SAFETY: dup2(2) and setrlimit(2) in the forked child, before exec;
    // nothing is allocated.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut child, move || {
            if libc::dup2(fd, ABOVE_THE_LIMIT) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let lowered = libc::rlimit {
                rlim_cur: 50,
                rlim_max: hard.rlim_max,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &lowered) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let out = child.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("1 passed"), "{text}");
}
