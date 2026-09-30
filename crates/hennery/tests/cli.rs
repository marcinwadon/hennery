use std::io::{Read, Write};
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
        .args(["host", "run", "--data-dir", "/tmp/x", "--agent", "noequals"])
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

/// `GET path` on the collector with the development bearer: the JSON body
/// of a 200, else `None`.
fn get_json(listen: &str, path: &str, token: &str) -> Option<serde_json::Value> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {listen}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let (head, body) = response.split_once("\r\n\r\n")?;
    if !head.starts_with("HTTP/1.1 200") {
        return None;
    }
    serde_json::from_str(body).ok()
}

fn free_listen() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    format!("127.0.0.1:{port}")
}

/// Removes a scratch directory on drop, however the test ends.
struct RemoveDir(std::path::PathBuf);

impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Start `up`, wait until its host is connected, and return the connected
/// host ids. The returned guard stops the whole tree (and leaves `dir`).
fn up_until_connected(listen: &str, dir: &std::path::Path) -> (KillTree, Vec<String>) {
    let up = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["up", "--listen", listen])
        .arg("--data-dir")
        .arg(dir)
        .args(["--dev-token", "dev-token-for-tests"])
        .spawn()
        .unwrap();
    let guard = KillTree {
        up,
        dir: std::path::PathBuf::new(),
        children: Vec::new(),
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(serde_json::Value::Array(hosts)) = get_json(listen, "/api/hosts", "dev-token-for-tests")
            && hosts.iter().any(|h| h["connected"] == true)
        {
            let ids = hosts
                .iter()
                .filter_map(|h| h["host_id"].as_str().map(str::to_string))
                .collect();
            return (guard, ids);
        }
        assert!(Instant::now() < deadline, "the all-in-one host never connected");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `hennery up` pairs its own host on first start, through the pipe the
/// supervisor hands both children (kernel spec §4.2), and a restart reuses
/// that pairing instead of minting another, also on another port.
#[test]
fn up_pairs_its_own_host_once() {
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-pair-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let _cleanup = RemoveDir(dir.clone());
    let host_dir = dir.join("host");

    let (mut first, ids) = up_until_connected(&listen, &dir);
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert!(ids[0].starts_with("host-"), "{ids:?}");
    let key = std::fs::read(host_dir.join("host.key")).unwrap();
    // SIGTERM, not a kill: `up` stops the host, then the collector.
    unsafe { libc::kill(first.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut first.up, Duration::from_secs(15)).is_some());

    let (_second, again) = up_until_connected(&free_listen(), &dir);
    assert_eq!(again, ids, "the restart paired a second host");
    assert_eq!(std::fs::read(host_dir.join("host.key")).unwrap(), key);
}

/// `DELETE path` on the collector with the development bearer: the status.
fn delete(listen: &str, path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok()?;
    write!(
        stream,
        "DELETE {path} HTTP/1.1\r\nHost: {listen}\r\nAuthorization: Bearer dev-token-for-tests\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.split(' ').nth(1)?.parse().ok()
}

/// Start `up` with its log in `log`.
fn up_logging_to(listen: &str, dir: &std::path::Path, log: &std::path::Path) -> KillTree {
    let up = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["up", "--listen", listen])
        .arg("--data-dir")
        .arg(dir)
        .args(["--dev-token", "dev-token-for-tests"])
        .stdout(std::fs::File::create(log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    KillTree {
        up,
        dir: std::path::PathBuf::new(),
        children: Vec::new(),
    }
}

fn wait_until(what: &str, mut probe: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !probe() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Operator recovery: revoking the all-in-one host stops the host child
/// only. `up` keeps the collector serving, says how to pair the host
/// again, and does the same on every later start until that is done.
#[test]
fn a_revoked_all_in_one_host_leaves_the_collector_serving() {
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-revoke-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let revoked_logged = |log: &std::path::Path| {
        std::fs::read_to_string(log).is_ok_and(|text| text.contains("the all-in-one host was revoked"))
    };

    let log = dir.join("first.log");
    let mut first = up_logging_to(&listen, &dir.join("data"), &log);
    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", "dev-token-for-tests") else {
            return false;
        };
        match hosts.first() {
            Some(h) if h["connected"] == true => {
                host_id = h["host_id"].as_str().unwrap().to_string();
                true
            }
            _ => false,
        }
    });
    assert_eq!(delete(&listen, &format!("/api/hosts/{host_id}")), Some(200));
    wait_until("the revoke logged", || revoked_logged(&log));
    assert!(first.up.try_wait().unwrap().is_none(), "up exited with its host");
    let hosts = get_json(&listen, "/api/hosts", "dev-token-for-tests").expect("the collector still serves");
    assert!(hosts[0]["revoked_at"].is_string(), "{hosts}");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("host.key") && text.contains("host.toml"), "{text}");
    unsafe { libc::kill(first.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut first.up, Duration::from_secs(15)).is_some());

    // Started again: the host is refused again, and the collector serves.
    let log = dir.join("second.log");
    let mut second = up_logging_to(&listen, &dir.join("data"), &log);
    wait_until("the revoke logged again", || revoked_logged(&log));
    assert!(get_json(&listen, "/api/hosts", "dev-token-for-tests").is_some());
    assert!(second.up.try_wait().unwrap().is_none());
}

/// Task 4 review (controller ruling): `up` must compute and validate its
/// host's collector URL (`collector_ws_url`) *before* spawning the collector
/// child, so a `--listen` address that `collector_ws_url` cannot make sense
/// of fails at once, with nothing started — not after a collector is already
/// bound and serving on it.
#[test]
fn a_non_loopback_listen_fails_up_before_any_child_starts() {
    let dir = std::env::temp_dir().join(format!("hennery-cli-badlisten-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());

    // TEST-NET-3 (RFC 5737): never routable, and not loopback either, so
    // `collector_ws_url` refuses it (plain http off loopback) before `up`
    // spawns anything that could bind or listen on it.
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["up", "--listen", "203.0.113.5:7117"])
        .arg("--data-dir")
        .arg(&dir)
        .args(["--dev-token", "dev-token-for-tests"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("loopback"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Neither child ever touched its data directory.
    assert!(!dir.join("collector").exists(), "the collector was started anyway");
    assert!(!dir.join("host").exists(), "the host was started anyway");
}

/// Task 4 review (controller ruling): a failed pairing-code write (the host
/// child died, or otherwise never attached the other end of the pipe) is the
/// host's problem, not the collector's — it must be logged, without the code
/// itself, and the collector must keep serving every other route regardless.
///
/// Reproduced directly against `hennery collector --pairing-code-fd`,
/// without `up`: the read end of the pipe is dropped and never given to any
/// process, so the collector's write is a broken pipe every time, not just
/// on some unlucky timing.
#[test]
fn a_failed_pairing_code_write_is_logged_without_the_code_and_does_not_kill_the_collector() {
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-badpipe-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let log = dir.join("collector.log");

    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader); // Nobody will ever read: every write is EPIPE.
    let writer_fd = std::os::fd::AsRawFd::as_raw_fd(&writer);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hennery"));
    cmd.args(["collector", "--listen", &listen])
        .arg("--data-dir")
        .arg(dir.join("data"))
        .args(["--dev-token", "dev-token-for-tests"])
        .args(["--pairing-code-fd", "3"])
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap());
    // SAFETY: dup2 in the forked child, before exec, of a descriptor this
    // process owns; async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(writer_fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let collector = cmd.spawn().unwrap();
    drop(writer); // This process's own copy; the child dup'd its own.
    // A `KillTree` guard, as `up_logging_to` uses for `up` itself: an
    // assertion below that panics must still not leak this process.
    let mut guard = KillTree {
        up: collector,
        dir: std::path::PathBuf::new(),
        children: Vec::new(),
    };

    wait_until("the collector serving", || {
        get_json(&listen, "/api/hosts", "dev-token-for-tests").is_some()
    });
    assert!(
        guard.up.try_wait().unwrap().is_none(),
        "the collector died on a failed pairing-code write"
    );
    // tracing's default writer is stdout, not stderr.
    let stdout = std::fs::read_to_string(&log).unwrap();
    assert!(stdout.contains("could not hand the pairing code"), "stdout: {stdout}");
    // The code minted for the write that failed is a secret; it must never
    // reach the log.
    let code_line = stdout
        .lines()
        .find(|l| l.contains("could not hand the pairing code"))
        .unwrap();
    assert!(!code_line.contains("code="), "{code_line}");

    unsafe { libc::kill(guard.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut guard.up, Duration::from_secs(15)).is_some());
}
