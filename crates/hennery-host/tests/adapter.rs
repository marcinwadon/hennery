//! The adapter supervisor against plain shell processes: group kill reaches
//! grandchildren, SIGTERM escalates to SIGKILL, dropping an adapter kills its
//! group, stderr is bounded and scrubbed, nesting variables are stripped.

use hennery_host::adapter::{Adapter, AgentCommand, STDERR_TAIL_BYTES, scrub};
use std::path::Path;
use std::time::{Duration, Instant};

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
    let (mut adapter, _io) = Adapter::spawn(&sh("trap '' TERM; while :; do sleep 0.05; done"), dir.path()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await; // let the trap install
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
async fn an_unexpected_exit_reports_code_and_a_bounded_scrubbed_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let script = "echo 'early Bearer abc.def' >&2; \
                  head -c 100000 /dev/zero | tr '\\0' x >&2; \
                  echo ' late ghp_abcdefghijklmnop' >&2; exit 7";
    let (mut adapter, _io) = Adapter::spawn(&sh(script), dir.path()).unwrap();
    let info = adapter.exited().await;
    assert_eq!((info.code, info.signal), (Some(7), None));
    let tail = adapter.stderr_tail().await;
    assert!(tail.len() <= STDERR_TAIL_BYTES, "{}", tail.len());
    assert!(
        tail.ends_with(" late ghp_[redacted]\n"),
        "{:?}",
        &tail[tail.len() - 40..]
    );
    assert!(!tail.contains("early"), "the tail keeps only the last bytes");
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
