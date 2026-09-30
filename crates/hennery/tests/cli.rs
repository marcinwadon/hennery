use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn help_lists_the_skeleton_commands() {
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    for cmd in ["collector", "host", "up"] {
        assert!(text.contains(cmd), "missing {cmd} in help:\n{text}");
    }
}

#[test]
fn host_help_lists_join_and_run() {
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "--help"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    for cmd in ["join", "run"] {
        assert!(text.contains(cmd), "missing {cmd} in help:\n{text}");
    }
}

/// The pairing code and the host's new public key are the only secrets
/// `host join` still has to spend. `parse_public_url` limits plain `http://`
/// to a loopback address, but a generic HTTP client still honours
/// `HTTP_PROXY`/`ALL_PROXY` by default — handing both to whatever address
/// the environment names, which would defeat that check for anyone able to
/// set it (a captured shell, a CI runner, a compromised dependency's
/// `postinstall`). `pairing::enroll` must call `.no_proxy()` unconditionally.
///
/// Proven end to end: the collector runs in-process (this crate already
/// depends on `hennery-sessions`/`hennery-kernel`), and `host join` runs as a
/// real *subprocess* with the proxy variables set on that child alone — the
/// env vars are per-process, so a subprocess is what lets this test run
/// alongside every other test in the binary without racing their env
/// (`std::env::set_var` on the test process itself would not be safe here).
/// The configured proxy is a port nothing listens on: if the client ever
/// tried to use it, the connection would be refused and the join would fail
/// instead of pairing.
#[test]
fn joining_over_http_ignores_a_configured_proxy() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (addr, code, collector_dir) = rt.block_on(async {
        let dir = std::env::temp_dir().join(format!("hennery-cli-noproxy-collector-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("hennery.db");
        let store = hennery_sessions::store::Store::open(&db).unwrap();
        let hosts = hennery_kernel::hosts::Hosts::open(&db).unwrap();
        let token = hennery_kernel::auth::DevToken::new("dev-token-for-tests").unwrap();
        let state = hennery_sessions::AppState::new(store, hosts, token);
        let code = state
            .hosts
            .mint_pairing_code(hennery_kernel::secret::unix_now())
            .unwrap()
            .code;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(hennery_sessions::serve(listener, state));
        (addr, code, dir)
    });

    let host_dir = std::env::temp_dir().join(format!("hennery-cli-noproxy-host-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&host_dir);

    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "join", &format!("http://{addr}"), &code, "--name", "laptop"])
        .arg("--data-dir")
        .arg(&host_dir)
        // A bogus proxy nobody listens on.
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("http_proxy", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .env("all_proxy", "http://127.0.0.1:1")
        .output()
        .unwrap();

    let _ = std::fs::remove_dir_all(&collector_dir);
    let _ = std::fs::remove_dir_all(&host_dir);

    assert!(
        out.status.success(),
        "join failed with a proxy configured:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("paired as"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn a_malformed_agent_flag_is_rejected() {
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args([
            "host",
            "run",
            "--collector",
            "ws://x",
            "--data-dir",
            "/tmp/x",
            "--dev-token",
            "t",
            "--agent",
            "noequals",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("name=command"));
}

#[test]
fn a_short_dev_token_is_refused_at_start() {
    let dir = std::env::temp_dir().join(format!("hennery-cli-short-token-{}", std::process::id()));
    for command in ["collector", "up"] {
        let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args([command, "--listen", "127.0.0.1:0", "--dev-token", "short"])
            .arg("--data-dir")
            .arg(&dir)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{command} started with a short token");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("at least 16 characters"), "{command}: {stderr}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `collector` must reject a short dev token before it does anything to the
/// data directory: `run_up` already validates first (checked above), and
/// `run_collector` must too, the same way — not only after opening the
/// store and the host registry there.
#[test]
fn a_short_dev_token_stops_the_collector_before_it_touches_the_data_dir() {
    let dir = std::env::temp_dir().join(format!("hennery-cli-token-first-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["collector", "--listen", "127.0.0.1:0", "--dev-token", "short"])
        .arg("--data-dir")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!dir.exists(), "the data dir was created before the token was validated");
}

/// Kills this test's `up` process tree and removes its scratch dir
/// unconditionally, including on an assertion panic mid-test — nothing below
/// is allowed to leave a process running just because a `assert!` fired
/// first.
struct KillTree {
    up: std::process::Child,
    dir: std::path::PathBuf,
    /// `up`'s children, recorded as soon as they're known (`&mut` field set
    /// right after `children_of` finds them). Once `up` exits — whether
    /// gracefully or via the `kill()` below — any child still alive is
    /// immediately reparented away from `up`'s pid, so a *fresh* `pgrep -P
    /// up_pid` at drop time can no longer find it. The recorded list is the
    /// only reliable way to reach it; a fresh query is kept only as a
    /// fallback for a panic that happened before anything was recorded.
    children: Vec<i32>,
}

impl Drop for KillTree {
    fn drop(&mut self) {
        let up_pid = self.up.id() as i32;
        let targets = if self.children.is_empty() {
            children_of(up_pid)
        } else {
            std::mem::take(&mut self.children)
        };
        for pid in targets {
            unsafe {
                // Each child leads its own process group (`run_up` sets
                // `process_group(0)`), so kill both the pid and that group.
                libc::kill(pid, libc::SIGKILL);
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        // Only PIDs this test started: `up` is our direct child, so kill+wait
        // it to avoid leaving a zombie for the rest of the test binary's run.
        let _ = self.up.kill();
        let _ = self.up.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A terminal's Ctrl-C delivers SIGINT to every process in the foreground
/// process group at once. `run_up` pulls its two children out of that group
/// (`process_group(0)` on both spawns) so only the supervisor is signalled
/// and can still forward an ordered shutdown (host, then collector) instead
/// of racing each child's own signal handler.
///
/// This spawns `up` as the leader of a brand-new process group (standing in
/// for a shell job) and sends SIGINT to that whole group, the same delivery
/// a terminal would do. It asserts `up` still exits cleanly and leaves no
/// child process behind. It does not assert the shutdown *order* directly
/// (host before collector): both children shut themselves down promptly on
/// SIGINT regardless of which fix is in place, so "no leftover processes" is
/// the observable signal here, not a strict ordering.
///
/// `guard` (a `KillTree`) is created immediately after spawn, before any
/// assertion that could panic, so a failure anywhere below still SIGKILLs the
/// whole tree and removes the scratch dir on the way out.
#[test]
fn sigint_to_ups_process_group_still_shuts_down_cleanly() {
    let free_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };
    let listen = format!("127.0.0.1:{free_port}");

    let dir = std::env::temp_dir().join(format!("hennery-cli-pgtest-{}-{free_port}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
    command
        .args(["up", "--listen", &listen])
        .arg("--data-dir")
        .arg(&dir)
        .args(["--dev-token", "dev-token-for-tests"]);
    // SAFETY: setpgid(0, 0) in the child, right after fork and before exec,
    // just makes it (and so `up`) the leader of a brand-new process group —
    // async-signal-safe and exactly what a shell does for a foreground job.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let up = command.spawn().unwrap();
    let up_pgid = up.id() as i32;
    let mut guard = KillTree {
        up,
        dir,
        children: Vec::new(),
    };

    // Wait for the collector to be listening.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect(&listen).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "collector never started listening");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Wait for the host child to spawn too (it's launched right after the
    // collector, independent of the collector's readiness). Poll instead of
    // a fixed sleep: fork/exec/setpgid latency under load (this test must
    // stay green with several copies of the binary running concurrently)
    // can exceed any fixed budget short enough to keep the common case fast.
    let deadline = Instant::now() + Duration::from_secs(10);
    let children = loop {
        let children = children_of(guard.up.id() as i32);
        if children.len() >= 2 {
            break children;
        }
        assert!(
            Instant::now() < deadline,
            "expected up to have spawned a collector and a host child within 10s, found {children:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    guard.children = children.clone();

    // Simulate a terminal delivering SIGINT to the whole foreground group.
    let rc = unsafe { libc::kill(-up_pgid, libc::SIGINT) };
    assert_eq!(rc, 0, "kill(-pgid, SIGINT) failed: {}", std::io::Error::last_os_error());

    let status = wait_with_timeout(&mut guard.up, Duration::from_secs(15)).expect("up did not exit after SIGINT");
    assert!(status.success(), "up exited with {status:?}");

    // Give any lingering child a moment to finish reaping before we check.
    std::thread::sleep(Duration::from_millis(300));
    for pid in children {
        assert!(!pid_alive(pid), "child pid {pid} is still alive after up exited");
    }
}

fn children_of(ppid: i32) -> Vec<i32> {
    let out = Command::new("pgrep").args(["-P", &ppid.to_string()]).output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.trim().parse::<i32>().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn pid_alive(pid: i32) -> bool {
    // kill(pid, 0) checks for existence/permission without sending a signal.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
