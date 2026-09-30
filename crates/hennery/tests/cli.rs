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
        let operator = hennery_kernel::operator::Operator::open(&db).unwrap();
        let state = hennery_sessions::AppState::new(store, hosts, operator);
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

/// Plan 3b: the development bearer is gone. `--dev-token` is refused, not
/// ignored, so a service still configured with it fails loudly.
#[test]
fn the_development_token_flag_is_gone() {
    for command in ["collector", "up"] {
        let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args([command, "--dev-token", "dev-token-for-tests"])
            .args(["--data-dir", "/nonexistent/hennery-cli-dev-token"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{command} took --dev-token");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("unexpected argument '--dev-token'"),
            "{command}: {stderr}"
        );
    }
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
    command.args(["up", "--listen", &listen]).arg("--data-dir").arg(&dir);
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

/// The owner's password in these tests' collectors.
const PASSWORD: &str = "correct horse battery";

/// Set up the collector whose data directory is `collector_dir` through its
/// `setup-url` (kernel spec §3.1), with `http://<listen>` as `public_url`,
/// and return the session token the setup signed the owner in with.
fn sign_in(listen: &str, collector_dir: &std::path::Path) -> String {
    let file = collector_dir.join("setup-url");
    wait_until("the setup link", || file.exists());
    let url = std::fs::read_to_string(&file).unwrap();
    // `…/setup#<token>`: the token is the fragment (3b decision 16).
    let token = url.trim_end().rsplit_once('#').unwrap().1;
    let origin = format!("http://{listen}");
    let body = serde_json::json!({ "token": token, "password": PASSWORD, "public_url": origin }).to_string();
    let mut stream = TcpStream::connect(listen).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    write!(
        stream,
        "POST /api/setup HTTP/1.1\r\nHost: {listen}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 201"), "{response}");
    response
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            let value = value.trim().strip_prefix("hennery_session=")?;
            name.eq_ignore_ascii_case("set-cookie")
                .then(|| value.split(';').next().unwrap().to_string())
        })
        .unwrap_or_else(|| panic!("no session cookie: {response}"))
}

/// `GET path` on the collector with the owner's `session`: the JSON body of
/// a 200, else `None`.
fn get_json(listen: &str, path: &str, session: &str) -> Option<serde_json::Value> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {listen}\r\nCookie: hennery_session={session}\r\nConnection: close\r\n\r\n"
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
/// host ids and the owner's session: `session`, or else a new one from
/// setting the collector up. The returned guard stops the whole tree (and
/// leaves `dir`).
fn up_until_connected(listen: &str, dir: &std::path::Path, session: Option<&str>) -> (KillTree, Vec<String>, String) {
    let up = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["up", "--listen", listen])
        .arg("--data-dir")
        .arg(dir)
        .spawn()
        .unwrap();
    let guard = KillTree {
        up,
        dir: std::path::PathBuf::new(),
        children: Vec::new(),
    };
    let session = match session {
        Some(session) => session.to_string(),
        None => sign_in(listen, &dir.join("collector")),
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(serde_json::Value::Array(hosts)) = get_json(listen, "/api/hosts", &session)
            && hosts.iter().any(|h| h["connected"] == true)
        {
            let ids = hosts
                .iter()
                .filter_map(|h| h["host_id"].as_str().map(str::to_string))
                .collect();
            return (guard, ids, session);
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

    let (mut first, ids, session) = up_until_connected(&listen, &dir, None);
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert!(ids[0].starts_with("host-"), "{ids:?}");
    let key = std::fs::read(host_dir.join("host.key")).unwrap();
    // SIGTERM, not a kill: `up` stops the host, then the collector.
    unsafe { libc::kill(first.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut first.up, Duration::from_secs(15)).is_some());

    // Set up already: the session from the first run still holds.
    let (_second, again, _) = up_until_connected(&free_listen(), &dir, Some(&session));
    assert_eq!(again, ids, "the restart paired a second host");
    assert_eq!(std::fs::read(host_dir.join("host.key")).unwrap(), key);
}

/// `POST path` with a JSON body, on the collector with the owner's
/// `session`, from its `public_url` (`http://<listen>`). The response is never read past a short timeout: a session start
/// that never finishes (the point of the slow-starting-adapter test below)
/// may hold the request open, and the pid files it writes are this test's
/// real readiness signal, not the HTTP response.
fn post_json(listen: &str, path: &str, session: &str, body: &str) {
    let Ok(mut stream) = TcpStream::connect(listen) else {
        return;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {listen}\r\nCookie: hennery_session={session}\r\nOrigin: http://{listen}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut discard = [0u8; 1];
    let _ = stream.read(&mut discard);
}

fn pid_from(path: &std::path::Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// `DELETE path` on the collector with the owner's `session`, from its
/// `public_url`: the status.
fn delete(listen: &str, path: &str, session: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok()?;
    write!(
        stream,
        "DELETE {path} HTTP/1.1\r\nHost: {listen}\r\nCookie: hennery_session={session}\r\nOrigin: http://{listen}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.split(' ').nth(1)?.parse().ok()
}

/// Start `up` with its log in `log`.
fn up_logging_to(listen: &str, dir: &std::path::Path, log: &std::path::Path) -> KillTree {
    up_logging_to_with(listen, dir, log, &[])
}

/// Like `up_logging_to`, with extra CLI arguments appended (e.g. `--agent`).
fn up_logging_to_with(listen: &str, dir: &std::path::Path, log: &std::path::Path, extra: &[&str]) -> KillTree {
    let up = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["up", "--listen", listen])
        .arg("--data-dir")
        .arg(dir)
        .args(extra)
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

/// Whether `text` contains a run shaped like a pairing code
/// (`[0-9A-Z]{4}-[0-9A-Z]{4}`, kernel spec §4.1) anywhere, not just next to a
/// particular label: a log line could quote the code under a different key,
/// or with no key at all, and either would still be a leak.
fn contains_a_pairing_code_shape(text: &str) -> bool {
    let is_code_char = |b: u8| b.is_ascii_uppercase() || b.is_ascii_digit();
    let bytes = text.as_bytes();
    (0..bytes.len().saturating_sub(8)).any(|i| {
        bytes[i..i + 4].iter().all(|&b| is_code_char(b))
            && bytes[i + 4] == b'-'
            && bytes[i + 5..i + 9].iter().all(|&b| is_code_char(b))
    })
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
    let session = sign_in(&listen, &dir.join("data").join("collector"));
    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", &session) else {
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
    assert_eq!(delete(&listen, &format!("/api/hosts/{host_id}"), &session), Some(200));
    wait_until("the revoke logged", || revoked_logged(&log));
    assert!(first.up.try_wait().unwrap().is_none(), "up exited with its host");
    let hosts = get_json(&listen, "/api/hosts", &session).expect("the collector still serves");
    assert!(hosts[0]["revoked_at"].is_string(), "{hosts}");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("host.key") && text.contains("host.toml"), "{text}");
    unsafe { libc::kill(first.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut first.up, Duration::from_secs(15)).is_some());

    // Started again: the host is refused again, and the collector serves.
    let log = dir.join("second.log");
    let mut second = up_logging_to(&listen, &dir.join("data"), &log);
    wait_until("the revoke logged again", || revoked_logged(&log));
    assert!(get_json(&listen, "/api/hosts", &session).is_some());
    assert!(second.up.try_wait().unwrap().is_none());
}

/// Task 4 review (controller ruling): `up` must compute and validate its
/// host's collector URL (`collector_ws_url`) *before* spawning the collector
/// child, so a `--listen` address that `collector_ws_url` cannot make sense
/// of fails cleanly, with neither data directory ever touched.
///
/// This does *not* prove the ordering itself (that the validation runs
/// strictly before `collector_cmd.spawn()`): that is a fact about the source,
/// checked by reading `run_up`, not one a black-box process test can pin
/// reliably. A revert-probe against the old (validate-after-spawn) code
/// still passed this exact test, because the parent's own string-only
/// validation is so much faster than the freshly forked child's own
/// exec-and-parse that the parent's early return (and the `kill_on_drop`
/// that comes with it) almost always wins the race regardless of which
/// order the source uses. So this is real coverage of the *outcome* `up`
/// must have either way, not a regression guard for the ordering fix.
#[test]
fn a_non_loopback_listen_is_refused_and_touches_neither_data_dir() {
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

    let session = sign_in(&listen, &dir.join("data"));
    wait_until("the collector serving", || {
        get_json(&listen, "/api/hosts", &session).is_some()
    });
    assert!(
        guard.up.try_wait().unwrap().is_none(),
        "the collector died on a failed pairing-code write"
    );
    // tracing's default writer is stdout, not stderr.
    let stdout = std::fs::read_to_string(&log).unwrap();
    assert!(stdout.contains("could not hand the pairing code"), "stdout: {stdout}");
    // The code minted for the write that failed is a secret; it must never
    // reach the log — checked against its actual shape (`XXXX-XXXX`, kernel
    // spec §4.1), anywhere in the log, not just a `code=`-labelled field a
    // future log line might not use.
    assert!(!contains_a_pairing_code_shape(&stdout), "stdout: {stdout}");

    unsafe { libc::kill(guard.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut guard.up, Duration::from_secs(15)).is_some());
}

/// Kills an adapter's whole process group on drop, from its pid file. Unlike
/// `KillTree` (which reaches only `up`'s own two children and their groups),
/// an adapter the host child spawns leads its *own* process group — this is
/// the only thing standing between a bug here and a real leaked `sleep`.
struct KillAdapter(std::path::PathBuf);

impl Drop for KillAdapter {
    fn drop(&mut self) {
        if let Some(pid) = pid_from(&self.0) {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

/// Task 7 review, fix round 1: a revoked host must stop *every* adapter it
/// runs (ACP core §3.5), including one whose session actor is still stuck
/// "starting" — negotiating the adapter's own handshake (session.rs's
/// `drive`, the `tokio::select!` around `negotiate`) has no arm that reads a
/// shutdown or command-channel signal, so such an actor does not react to
/// `run_until`'s shutdown on its own. Cleanup for it depends entirely on
/// `shut_down`'s bound being exceeded (it logs "did not stop in time" and
/// gives up on that actor) and, from there, on the actor's *task* being
/// dropped when the `#[tokio::main]` runtime itself finally drops — which is
/// exactly what a `std::process::exit` inside that runtime would skip,
/// orphaning the adapter's process group for good. Driven through the real
/// `hennery` binary via `up`, not `hennery_host::run` in-process, because the
/// bug is specifically in `main`'s own exit path.
#[test]
fn a_revoked_hosts_still_starting_adapter_is_reaped_past_shut_downs_bound() {
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-slowstart-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());

    // A shell script, not `hennery-fake-acp` (a binary of another crate this
    // one has no dependency on): it records its own pid and a grandchild's,
    // then never answers `initialize` — the adapter that never finishes
    // starting.
    let adapter_pid_file = dir.join("adapter.pid");
    let grandchild_pid_file = dir.join("grandchild.pid");
    let script = dir.join("slow.sh");
    std::fs::write(
        &script,
        format!(
            "echo $$ > {}\nsleep 7117 &\necho $! > {}\nwait\n",
            adapter_pid_file.display(),
            grandchild_pid_file.display()
        ),
    )
    .unwrap();
    // A drop guard for the adapter's own group: declared before any
    // assertion below, so a panic mid-test still reaps it.
    let _kill_adapter = KillAdapter(adapter_pid_file.clone());

    let log = dir.join("up.log");
    let mut up = up_logging_to_with(
        &listen,
        &dir.join("data"),
        &log,
        &["--agent", &format!("slow=/bin/sh {}", script.display())],
    );
    let session = sign_in(&listen, &dir.join("data").join("collector"));

    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", &session) else {
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

    post_json(
        &listen,
        "/api/sessions",
        &session,
        &serde_json::json!({ "host_id": host_id, "agent": "slow", "cwd": dir }).to_string(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let grandchild = loop {
        if let Some(pid) = pid_from(&grandchild_pid_file) {
            break pid;
        }
        assert!(Instant::now() < deadline, "the slow adapter never started");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(pid_alive(grandchild), "the grandchild died before the revoke");

    assert_eq!(delete(&listen, &format!("/api/hosts/{host_id}"), &session), Some(200));
    // Generous: a reconnect (up to ~1s of backoff) plus `shut_down`'s ~6s
    // bound, with slack for four parallel copies of this binary.
    let deadline = Instant::now() + Duration::from_secs(40);
    while !std::fs::read_to_string(&log).is_ok_and(|text| text.contains("the all-in-one host was revoked")) {
        assert!(Instant::now() < deadline, "timed out waiting for the revoke logged");
        std::thread::sleep(Duration::from_millis(50));
    }
    let text = std::fs::read_to_string(&log).unwrap();
    // Proof this exercised the runtime-drop path, not the graceful one: the
    // actor genuinely never reacted to the shutdown on its own.
    assert!(
        text.contains("session actors did not stop in time"),
        "the bound was never exceeded, so this never reached the code path under test: {text}"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while pid_alive(grandchild) {
        assert!(
            Instant::now() < deadline,
            "the still-starting adapter's grandchild outlived the revoked host"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut up.up, Duration::from_secs(15)).is_some());
}

/// Final review I1: an operator whose shell still exports the old bearer
/// (`HENNERY_DEV_TOKEN`, from before 3b removed it) must not hand it to the
/// host child, nor through it to any agent.
///
/// The agent is a shell script that dumps its environment, and whether it
/// holds a descriptor 3 (the pairing pipe's number in the host child), then
/// exits. A fresh data directory, so this run pairs through that pipe.
#[test]
fn ups_agents_never_see_the_operator_token_or_the_pairing_pipe() {
    const TOKEN: &str = "operator-token-from-the-environment";
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-agentenv-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());

    // Written to temporary names and moved into place, so a reader never
    // sees half a file. The probe runs in a subshell: a failed redirection
    // on a special builtin (`:`) ends a POSIX shell such as dash (Debian's
    // and Ubuntu's `/bin/sh`), which would then never write its report.
    let script = dir.join("envdump.sh");
    let report = |name: &str| dir.join(name);
    std::fs::write(
        &script,
        format!(
            "if ( : <&3 ) 2>/dev/null; then echo open > {fd}.tmp; else echo closed > {fd}.tmp; fi\n\
             env > {env}.tmp\nmv {fd}.tmp {fd}\nmv {env}.tmp {env}\n",
            fd = report("fd3.txt").display(),
            env = report("env.txt").display(),
        ),
    )
    .unwrap();

    // Control: the probe does see a descriptor 3 that is open.
    let mut control = Command::new("/bin/sh");
    control.arg(&script);
    // SAFETY: dup2 in the forked child, before exec; async-signal-safe.
    unsafe {
        control.pre_exec(|| {
            if libc::dup2(1, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    assert!(control.status().unwrap().success());
    assert_eq!(std::fs::read_to_string(report("fd3.txt")).unwrap().trim(), "open");
    std::fs::remove_file(report("fd3.txt")).unwrap();
    std::fs::remove_file(report("env.txt")).unwrap();

    let log = dir.join("up.log");
    let up = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["up", "--listen", &listen])
        .arg("--data-dir")
        .arg(dir.join("data"))
        .arg("--agent")
        .arg(format!("envdump=/bin/sh {}", script.display()))
        .env("HENNERY_DEV_TOKEN", TOKEN)
        .env("HENNERY_AGENT_MAY_SEE", "yes")
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut up = KillTree {
        up,
        dir: std::path::PathBuf::new(),
        children: Vec::new(),
    };

    let session = sign_in(&listen, &dir.join("data").join("collector"));
    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", &session) else {
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
    post_json(
        &listen,
        "/api/sessions",
        &session,
        &serde_json::json!({ "host_id": host_id, "agent": "envdump", "cwd": dir }).to_string(),
    );
    wait_until("the agent's report", || report("env.txt").exists());

    let env = std::fs::read_to_string(report("env.txt")).unwrap();
    // Positive controls: the agent got an environment, `up`'s included.
    assert!(env.contains("PATH="), "{env}");
    assert!(env.contains("HENNERY_AGENT_MAY_SEE=yes"), "{env}");
    assert!(
        !env.contains("HENNERY_DEV_TOKEN="),
        "the agent inherited the operator token"
    );
    assert!(!env.contains(TOKEN), "the agent inherited the operator token");
    assert_eq!(std::fs::read_to_string(report("fd3.txt")).unwrap().trim(), "closed");

    unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut up.up, Duration::from_secs(15)).is_some());
}

/// `hennery` under `umask 022`, the usual default, which would leave a new
/// directory 0755 and a new file 0644. Set in a shell in front of it, never
/// in this process: the umask is process-wide.
fn under_umask_022() -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "umask 022; exec \"$0\" \"$@\"", env!("CARGO_BIN_EXE_hennery")]);
    cmd
}

fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

/// Start `hennery collector` under `umask 022` on `data`, logging to `log`,
/// and wait until it serves: a new collector writes its setup link once it
/// listens.
fn collector_under_umask_022(listen: &str, data: &std::path::Path, log: &std::path::Path) -> KillTree {
    let collector = under_umask_022()
        .args(["collector", "--listen", listen])
        .arg("--data-dir")
        .arg(data)
        .stdout(std::fs::File::create(log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let guard = KillTree {
        up: collector,
        dir: std::path::PathBuf::new(),
        children: Vec::new(),
    };
    wait_until("the collector serving", || {
        data.join("setup-url").exists() && TcpStream::connect(listen).is_ok()
    });
    guard
}

/// Final review I2: pairing-code hashes (decision 5) and everything else in
/// `hennery.db` rely on nobody but the collector's user reading it. Its data
/// directory is created 0700 (every missing parent too) and the database
/// and its WAL files 0600, whatever the umask.
#[test]
fn the_collectors_data_is_private_to_its_user() {
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-private-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("root").join("collector");

    let mut collector = collector_under_umask_022(&listen, &data, &dir.join("collector.log"));
    // Checked while it serves: a clean shutdown checkpoints the WAL and
    // removes the `-wal` and `-shm`.
    assert_eq!(mode_of(&dir.join("root")), 0o700);
    assert_eq!(mode_of(&data), 0o700);
    for file in ["hennery.db", "hennery.db-wal", "hennery.db-shm"] {
        assert_eq!(mode_of(&data.join(file)), 0o600, "{file}");
    }
    unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());
}

/// Final review I2, an install from before the fix: a database others can
/// read is made private when the collector opens it. A data directory others
/// can enter is the operator's to fix: it is named in a warning, and left
/// as it is.
#[test]
fn an_existing_readable_database_is_made_private_and_a_loose_directory_is_warned_about() {
    use std::os::unix::fs::PermissionsExt;
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-loose-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
    // An empty file is a new, empty SQLite database.
    std::fs::write(data.join("hennery.db"), b"").unwrap();
    std::fs::set_permissions(data.join("hennery.db"), std::fs::Permissions::from_mode(0o644)).unwrap();

    let log = dir.join("collector.log");
    let mut collector = collector_under_umask_022(&listen, &data, &log);
    assert_eq!(mode_of(&data.join("hennery.db")), 0o600);
    assert_eq!(mode_of(&data), 0o755, "the operator's directory was changed");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("readable by other users") && text.contains(&data.display().to_string()),
        "{text}"
    );
    unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());
}

/// Final review I2: `up`'s data root, and both children's directories in
/// it, are private. This pins the outcome, not `up`'s own creation of the
/// root: each child also creates any missing parent 0700, so a revert-probe
/// of that line alone still passes; it fails without the collector's.
#[test]
fn ups_data_root_is_private_to_its_user() {
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-uproot-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let root = dir.join("data");

    let log = dir.join("up.log");
    let up = under_umask_022()
        .args(["up", "--listen", &listen])
        .arg("--data-dir")
        .arg(&root)
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut up = KillTree {
        up,
        dir: std::path::PathBuf::new(),
        children: Vec::new(),
    };
    let session = sign_in(&listen, &root.join("collector"));
    wait_until("the host connected", || {
        get_json(&listen, "/api/hosts", &session).is_some_and(|hosts| {
            hosts
                .as_array()
                .is_some_and(|h| h.iter().any(|h| h["connected"] == true))
        })
    });
    for dir in [root.clone(), root.join("collector"), root.join("host")] {
        assert_eq!(mode_of(&dir), 0o700, "{}", dir.display());
    }
    assert_eq!(mode_of(&root.join("collector").join("hennery.db")), 0o600);
    unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut up.up, Duration::from_secs(15)).is_some());
}

/// Review of the fix wave: `up` names an existing data root that other
/// users can reach into, and leaves it as it is, like the collector does
/// for its own directory.
#[test]
fn up_warns_about_a_loose_existing_data_root() {
    use std::os::unix::fs::PermissionsExt;
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-looseroot-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let root = dir.join("data");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

    let log = dir.join("up.log");
    let mut up = up_logging_to(&listen, &root, &log);
    wait_until("the warning about the data root", || {
        std::fs::read_to_string(&log).is_ok_and(|text| {
            text.lines()
                .any(|line| line.contains("readable by other users") && line.contains(&root.display().to_string()))
        })
    });
    assert_eq!(mode_of(&root), 0o755, "the operator's directory was changed");
    unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut up.up, Duration::from_secs(15)).is_some());
}

/// Kernel spec §3.1: a collector that is not set up writes its one-time
/// setup link to `setup-url` (0600, under `umask 022` too) and, its output
/// not being a terminal, logs only that file's path: the token itself must
/// never reach a log collector.
#[test]
fn an_unset_collector_writes_its_setup_link_to_a_private_file_and_never_to_its_output() {
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-setup-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let log = dir.join("collector.log");

    let mut collector = collector_under_umask_022(&listen, &data, &log);
    let file = data.join("setup-url");
    assert_eq!(mode_of(&file), 0o600);
    let url = std::fs::read_to_string(&file).unwrap();
    let port = listen.rsplit(':').next().unwrap();
    let token = url
        .trim_end()
        .strip_prefix(&format!("http://localhost:{port}/setup#"))
        .unwrap_or_else(|| panic!("{url}"));
    assert_eq!(token.len(), 64, "{url}");
    unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());

    let stdout = std::fs::read_to_string(&log).unwrap();
    let stderr = std::fs::read_to_string(log.with_extension("err")).unwrap();
    assert!(
        !stdout.contains(token) && !stderr.contains(token),
        "the setup token was logged"
    );
    assert!(stdout.contains(&file.display().to_string()), "{stdout}");
}
