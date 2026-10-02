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
    for cmd in ["collector", "host", "up", "admin"] {
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

/// Plan 6c decision 6 (Task 2's review): a bad `--workspace-root` fails the
/// start before anything else, so an unpaired host spends no pairing code.
#[test]
fn a_bad_workspace_root_fails_before_pairing() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "run", "--data-dir"])
        .arg(dir.path())
        .args(["--workspace-root", "relative/dir"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not absolute"), "{stderr}");
    assert!(!stderr.contains("holds no pairing"), "{stderr}");
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

/// What `collector`, `up` and `host run` log once when `HENNERY_DEV_TOKEN`
/// is still set.
const DEV_TOKEN_WARNING: &str = "HENNERY_DEV_TOKEN is set but no longer used";

/// Plan 3b: an operator whose shell or service still sets the old bearer is
/// told once, at start, that it does nothing now, and its value is never
/// printed. Each command here stops early (no data directory can be made,
/// the listen address is refused, no pairing is stored), so none binds a
/// port: the warning comes before any of that.
#[test]
fn the_development_token_in_the_environment_is_warned_about_and_never_printed() {
    const TOKEN: &str = "old-dev-token-still-exported";
    let dir = std::env::temp_dir().join(format!("hennery-cli-devtokenwarn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let host_dir = dir.join("host");
    std::fs::create_dir_all(&host_dir).unwrap();
    // A data directory under a regular file cannot be made by any user,
    // root included (ENOTDIR), so the collector stops before it binds.
    let not_a_dir = dir.join("file");
    std::fs::write(&not_a_dir, b"").unwrap();
    let runs: [(&str, Vec<std::ffi::OsString>); 3] = [
        (
            "collector",
            vec!["collector".into(), "--data-dir".into(), not_a_dir.join("data").into()],
        ),
        (
            "up",
            vec![
                "up".into(),
                "--listen".into(),
                "203.0.113.5:7117".into(),
                "--data-dir".into(),
                dir.join("up").into(),
            ],
        ),
        (
            "host run",
            vec!["host".into(), "run".into(), "--data-dir".into(), host_dir.into()],
        ),
    ];
    for (name, args) in runs {
        let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args(args)
            .env("HENNERY_DEV_TOKEN", TOKEN)
            .env("RUST_LOG", "info")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "{name} was meant to stop early: {text}");
        assert_eq!(text.matches(DEV_TOKEN_WARNING).count(), 1, "{name}: {text}");
        assert!(!text.contains(TOKEN), "{name} printed the token: {text}");
    }
}

/// Stops this test's `up` process tree and removes its scratch dir
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
    /// only reliable way to reach it once `up` is gone; a fresh query is made
    /// only while `up` is stopped and still alive (drop's fallback).
    children: Vec<i32>,
    /// Where `up`'s standard output goes, with its standard error next to it
    /// (`.err`): printed when it dies while a test waits on it.
    log: Option<std::path::PathBuf>,
}

impl KillTree {
    /// `process`, whose standard output goes to `log` (and its standard
    /// error to `log`'s `.err`).
    fn new(process: std::process::Child, log: &std::path::Path) -> Self {
        Self {
            up: process,
            dir: std::path::PathBuf::new(),
            children: Vec::new(),
            log: Some(log.to_path_buf()),
        }
    }

    /// Fail at once, with its output, if the process has exited: a test
    /// waiting on a dead collector would otherwise time out and not say why.
    fn assert_running(&mut self, what: &str) {
        let Ok(Some(status)) = self.up.try_wait() else {
            return;
        };
        let read = |path: std::path::PathBuf| std::fs::read_to_string(&path).unwrap_or_else(|err| format!("({err})"));
        let (stdout, stderr) = match &self.log {
            Some(log) => (read(log.clone()), read(log.with_extension("err"))),
            None => ("(not captured)".into(), "(not captured)".into()),
        };
        panic!("waiting for {what}, the process exited ({status}):\nstdout:\n{stdout}\nstderr:\n{stderr}");
    }

    /// Poll `probe` until it holds, failing at once if the process exits
    /// first, and after 20 seconds.
    fn wait_until(&mut self, what: &str, mut probe: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !probe() {
            self.assert_running(what);
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The address the collector (itself, or `up`'s child) listens on, from
    /// its "collector listening" log line: the tests start it on port 0, so
    /// no other process can take the port between choosing and binding it.
    fn listening(&mut self) -> String {
        self.listening_on(1).remove(0)
    }

    /// The addresses of the collector's first `n` "collector listening"
    /// lines, one per listener, in the order it was given them.
    fn listening_on(&mut self, n: usize) -> Vec<String> {
        let log = self.log.clone().expect("the process's output is captured");
        let mut addresses = Vec::new();
        self.wait_until("the collector listening", || {
            addresses = std::fs::read_to_string(&log)
                .map(|text| listening_addresses(&text))
                .unwrap_or_default();
            addresses.len() >= n
        });
        addresses.truncate(n);
        addresses
    }
}

/// The `address` of every complete "collector listening" line in `log`.
fn listening_addresses(log: &str) -> Vec<String> {
    // Complete lines only: a line still being written could end mid-port.
    let Some(end) = log.rfind('\n') else {
        return Vec::new();
    };
    log[..end]
        .lines()
        .map(strip_ansi)
        .filter_map(|line| {
            let fields = line.split_once("collector listening")?.1;
            let address = fields
                .split_whitespace()
                .find_map(|field| field.strip_prefix("address="))?;
            Some(address.to_string())
        })
        .collect()
}

/// `line` without its terminal colour codes (`ESC [ … m`), which the log
/// carries also when written to a file.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Drop for KillTree {
    fn drop(&mut self) {
        // SIGTERM first, and wait: `up` (and a collector) catches it from
        // before it starts any child, so it stops its children itself, host
        // then collector, also one it spawns after this signal. Killing `up`
        // outright cannot: a test may end before `up` spawns its host, which
        // a `pgrep` made first would miss and `up`'s death would orphan.
        // Never signalled once reaped (a test may have stopped it already):
        // its pid may be another process's by now. The bound is past `up`'s
        // own (10 s for each child).
        if matches!(self.up.try_wait(), Ok(None)) {
            let up_pid = self.up.id() as i32;
            unsafe { libc::kill(up_pid, libc::SIGTERM) };
            if wait_with_timeout(&mut self.up, Duration::from_secs(30)).is_none() {
                // Stopped, `up` spawns no child after the `pgrep` below.
                unsafe { libc::kill(up_pid, libc::SIGSTOP) };
                self.children.extend(children_of(up_pid));
            }
        }
        for pid in std::mem::take(&mut self.children) {
            unsafe {
                // Each child leads its own process group (`UpChildren` sets
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
/// process group at once. `up` pulls its two children out of that group
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
    let dir = scratch_dir("pgtest");
    let log = dir.join("up.log");

    let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
    command
        .args(["up", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(dir.join("data"))
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap());
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
    let guard_pid = up_pgid;
    let mut guard = KillTree::new(up, &log);
    guard.dir = dir;

    // Wait for the collector to be listening.
    let listen = guard.listening();
    guard.wait_until("the collector serving", || TcpStream::connect(&listen).is_ok());
    // Wait for the host child to spawn too (it's launched right after the
    // collector, independent of the collector's readiness). Poll instead of
    // a fixed sleep: fork/exec/setpgid latency under load (this test must
    // stay green with several copies of the binary running concurrently)
    // can exceed any fixed budget short enough to keep the common case fast.
    let mut children = Vec::new();
    guard.wait_until("up's collector and host children", || {
        children = children_of(guard_pid);
        children.len() >= 2
    });
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

/// Set up the collector `process` runs (or `up` runs), whose data directory
/// is `collector_dir`, through its
/// `setup-url` (kernel spec §3.1), with `http://<listen>` as `public_url`,
/// and return the session token the setup signed the owner in with.
fn sign_in(process: &mut KillTree, listen: &str, collector_dir: &std::path::Path) -> String {
    let file = collector_dir.join("setup-url");
    process.wait_until("the setup link", || file.exists());
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

/// A fresh scratch directory for the test `name`; the caller removes it.
fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("hennery-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Removes a scratch directory on drop, however the test ends.
struct RemoveDir(std::path::PathBuf);

impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Start `up` on `dir`, logging to `log`, wait until its host is connected,
/// and return the address it listens on, the connected host ids and the
/// owner's session: `session`, or else a new one from setting the collector
/// up. The returned guard stops
/// the whole tree (and leaves `dir`).
fn up_until_connected(
    dir: &std::path::Path,
    log: &std::path::Path,
    session: Option<&str>,
) -> (KillTree, String, Vec<String>, String) {
    let mut guard = up_logging_to(dir, log);
    let listen = guard.listening();
    let session = match session {
        Some(session) => session.to_string(),
        None => sign_in(&mut guard, &listen, &dir.join("collector")),
    };
    let mut ids = Vec::new();
    guard.wait_until("the all-in-one host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", &session) else {
            return false;
        };
        ids = hosts
            .iter()
            .filter_map(|h| h["host_id"].as_str().map(str::to_string))
            .collect();
        hosts.iter().any(|h| h["connected"] == true)
    });
    (guard, listen, ids, session)
}

/// `hennery up` pairs its own host on first start, through the pipe the
/// supervisor hands both children (kernel spec §4.2), and a restart reuses
/// that pairing instead of minting another, also on another port.
#[test]
fn up_pairs_its_own_host_once() {
    let dir = scratch_dir("pair");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let host_dir = data.join("host");

    let (mut first, first_listen, ids, session) = up_until_connected(&data, &dir.join("first.log"), None);
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert!(ids[0].starts_with("host-"), "{ids:?}");
    let key = std::fs::read(host_dir.join("host.key")).unwrap();
    // SIGTERM, not a kill: `up` stops the host, then the collector.
    unsafe { libc::kill(first.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut first.up, Duration::from_secs(15)).is_some());
    // Held, so the restart cannot get the same port back by chance.
    let _held = std::net::TcpListener::bind(&first_listen);

    // Set up already: the session from the first run still holds.
    let (_second, second_listen, again, _) = up_until_connected(&data, &dir.join("second.log"), Some(&session));
    assert_ne!(second_listen, first_listen, "the restart was not on another port");
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

/// Start `up` on port 0 with its log in `log`.
fn up_logging_to(dir: &std::path::Path, log: &std::path::Path) -> KillTree {
    up_logging_to_with(Command::new(env!("CARGO_BIN_EXE_hennery")), dir, log, &[])
}

/// Like `up_logging_to`, from `command` (`hennery` itself, or a shell in
/// front of it), with `extra` arguments appended (e.g. `--agent`).
fn up_logging_to_with(mut command: Command, dir: &std::path::Path, log: &std::path::Path, extra: &[&str]) -> KillTree {
    let up = command
        .args(["up", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(dir)
        .args(extra)
        .stdout(std::fs::File::create(log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    KillTree::new(up, log)
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
    let dir = scratch_dir("revoke");
    let _cleanup = RemoveDir(dir.clone());
    let revoked_logged = |log: &std::path::Path| {
        std::fs::read_to_string(log).is_ok_and(|text| text.contains("the all-in-one host was revoked"))
    };

    let log = dir.join("first.log");
    let mut first = up_logging_to(&dir.join("data"), &log);
    let listen = first.listening();
    let session = sign_in(&mut first, &listen, &dir.join("data").join("collector"));
    let mut host_id = String::new();
    first.wait_until("the host connected", || {
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
    first.wait_until("the revoke logged", || revoked_logged(&log));
    assert!(first.up.try_wait().unwrap().is_none(), "up exited with its host");
    let hosts = get_json(&listen, "/api/hosts", &session).expect("the collector still serves");
    assert!(hosts[0]["revoked_at"].is_string(), "{hosts}");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("host.key") && text.contains("host.toml"), "{text}");
    unsafe { libc::kill(first.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut first.up, Duration::from_secs(15)).is_some());

    // Started again: the host is refused again, and the collector serves.
    let log = dir.join("second.log");
    let mut second = up_logging_to(&dir.join("data"), &log);
    let listen = second.listening();
    second.wait_until("the revoke logged again", || revoked_logged(&log));
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
    let dir = scratch_dir("badpipe");
    let _cleanup = RemoveDir(dir.clone());
    let log = dir.join("collector.log");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hennery"));
    cmd.args(["collector", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(dir.join("data"))
        .args(["--pairing-code-fd", "3"])
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap());
    // The pipe is made in the forked child, not here: on macOS a pipe is
    // made close-on-exec only after it exists (`pipe`, then `fcntl`), so a
    // process another test spawns in between inherits its read end and
    // keeps it open, and the collector's write then succeeds.
    //
    // SAFETY: pipe, dup2 and close in the forked child, before exec;
    // async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            let mut fds = [0; 2];
            if libc::pipe(fds.as_mut_ptr()) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let [reader, writer] = fds;
            // Nobody will ever read: every write is EPIPE. Placed onto the
            // reader's number, the writer closes it too.
            hennery_testkit::place_fd(writer, 3)?;
            for fd in [reader, writer] {
                if fd != 3 {
                    libc::close(fd);
                }
            }
            Ok(())
        });
    }
    // A `KillTree` guard, as `up_logging_to` uses for `up` itself: an
    // assertion below that panics must still not leak this process.
    let mut guard = KillTree::new(cmd.spawn().unwrap(), &log);

    let listen = guard.listening();
    let session = sign_in(&mut guard, &listen, &dir.join("data"));
    guard.wait_until("the collector serving", || {
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
    let dir = scratch_dir("slowstart");
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
        Command::new(env!("CARGO_BIN_EXE_hennery")),
        &dir.join("data"),
        &log,
        &["--agent", &format!("slow=/bin/sh {}", script.display())],
    );
    let listen = up.listening();
    let session = sign_in(&mut up, &listen, &dir.join("data").join("collector"));

    let mut host_id = String::new();
    up.wait_until("the host connected", || {
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
    let mut grandchild = None;
    up.wait_until("the slow adapter started", || {
        grandchild = pid_from(&grandchild_pid_file);
        grandchild.is_some()
    });
    let grandchild = grandchild.unwrap();
    assert!(pid_alive(grandchild), "the grandchild died before the revoke");

    assert_eq!(delete(&listen, &format!("/api/hosts/{host_id}"), &session), Some(200));
    // Generous: a reconnect (up to ~1s of backoff) plus `shut_down`'s ~6s
    // bound, with slack for four parallel copies of this binary.
    let deadline = Instant::now() + Duration::from_secs(40);
    while !std::fs::read_to_string(&log).is_ok_and(|text| text.contains("the all-in-one host was revoked")) {
        up.assert_running("the revoke logged");
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
        up.assert_running("the still-starting adapter reaped");
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
/// The agent is a shell script that dumps its environment, and which of the
/// descriptors 3 to 9 it holds (3 is the pairing pipe's number in the host
/// child, 4 the listening socket's in the collector child), then exits.
///
/// `up` itself is started holding descriptor 7 open across `exec`, as a
/// service manager or a shell can leave one: the host's adapter spawn must
/// close it too. The control starts with no inherited descriptor above 2:
/// on macOS, std makes a pipe or socket close-on-exec only after creating
/// it, so under parallel tests one another thread of this binary is making
/// can leak into a spawn. A fresh data directory, so this run pairs through
/// that pipe.
#[test]
fn ups_agents_never_see_the_operator_token_or_the_pairing_pipe() {
    const TOKEN: &str = "operator-token-from-the-environment";
    let dir = scratch_dir("agentenv");
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
            "for n in 3 4 5 6 7 8 9; do if ( eval \": <&$n\" ) 2>/dev/null; then echo $n; fi; done > {fd}.tmp\n\
             env > {env}.tmp\nmv {fd}.tmp {fd}\nmv {env}.tmp {env}\n",
            fd = report("fds.txt").display(),
            env = report("env.txt").display(),
        ),
    )
    .unwrap();

    // Control: the probe does see descriptors 3 and 4 that are open, and
    // only those.
    let mut control = Command::new("/bin/sh");
    control.arg(&script);
    // SAFETY: fcntl, close and dup2 in the forked child, before exec;
    // async-signal-safe.
    unsafe {
        control.pre_exec(|| {
            close_leaked_descriptors();
            hennery_testkit::place_fd(1, 3)?;
            hennery_testkit::place_fd(1, 4)?;
            Ok(())
        });
    }
    assert!(control.status().unwrap().success());
    assert_eq!(std::fs::read_to_string(report("fds.txt")).unwrap(), "3\n4\n");
    std::fs::remove_file(report("fds.txt")).unwrap();
    std::fs::remove_file(report("env.txt")).unwrap();

    let log = dir.join("up.log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
    command
        .env("HENNERY_DEV_TOKEN", TOKEN)
        .env("HENNERY_AGENT_MAY_SEE", "yes")
        .env("RUST_LOG", "info");
    // SAFETY: as for the control above.
    unsafe {
        command.pre_exec(|| {
            close_leaked_descriptors();
            hennery_testkit::place_fd(2, 7)?;
            Ok(())
        });
    }
    let mut up = up_logging_to_with(
        command,
        &dir.join("data"),
        &log,
        &["--agent", &format!("envdump=/bin/sh {}", script.display())],
    );

    let listen = up.listening();
    let session = sign_in(&mut up, &listen, &dir.join("data").join("collector"));
    let mut host_id = String::new();
    up.wait_until("the host connected", || {
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
    up.wait_until("the agent's report", || report("env.txt").exists());

    let env = std::fs::read_to_string(report("env.txt")).unwrap();
    // Positive controls: the agent got an environment, `up`'s included.
    assert!(env.contains("PATH="), "{env}");
    assert!(env.contains("HENNERY_AGENT_MAY_SEE=yes"), "{env}");
    assert!(
        !env.contains("HENNERY_DEV_TOKEN="),
        "the agent inherited the operator token"
    );
    assert!(!env.contains(TOKEN), "the agent inherited the operator token");
    assert_eq!(
        std::fs::read_to_string(report("fds.txt")).unwrap(),
        "",
        "the agent inherited descriptors"
    );

    unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut up.up, Duration::from_secs(15)).is_some());
    // `up` warns once; its collector child is not handed the variable, so
    // it does not warn again; and neither prints the value.
    let output = std::fs::read_to_string(&log).unwrap() + &std::fs::read_to_string(log.with_extension("err")).unwrap();
    assert_eq!(output.matches(DEV_TOKEN_WARNING).count(), 1, "{output}");
    assert!(!output.contains(TOKEN), "up printed the operator token: {output}");
}

/// In a forked child before `exec`: close every descriptor above 2 that is
/// not close-on-exec, which only a leak from another thread can be here
/// (std's own descriptors for the spawn are close-on-exec). Only fcntl and
/// close: async-signal-safe.
fn close_leaked_descriptors() {
    for fd in 3..1024 {
        // SAFETY: fcntl and close on a descriptor number in this process.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::close(fd);
            }
        }
    }
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
/// listens. Returns the address it listens on.
fn collector_under_umask_022(data: &std::path::Path, log: &std::path::Path) -> (KillTree, String) {
    let collector = under_umask_022()
        .args(["collector", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(data)
        .stdout(std::fs::File::create(log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut guard = KillTree::new(collector, log);
    let listen = guard.listening();
    guard.wait_until("the collector serving", || {
        data.join("setup-url").exists() && TcpStream::connect(&listen).is_ok()
    });
    (guard, listen)
}

/// Final review I2: pairing-code hashes (decision 5) and everything else in
/// `hennery.db` rely on nobody but the collector's user reading it. Its data
/// directory is created 0700 (every missing parent too) and the database
/// and its WAL files 0600, whatever the umask.
#[test]
fn the_collectors_data_is_private_to_its_user() {
    let dir = scratch_dir("private");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("root").join("collector");

    let (mut collector, _) = collector_under_umask_022(&data, &dir.join("collector.log"));
    // Checked while it serves: a clean shutdown checkpoints the WAL and
    // removes the `-wal` and `-shm`.
    assert_eq!(mode_of(&dir.join("root")), 0o700);
    assert_eq!(mode_of(&data), 0o700);
    for file in ["hennery.db", "hennery.db-wal", "hennery.db-shm", "admin.sock"] {
        assert_eq!(mode_of(&data.join(file)), 0o600, "{file}");
    }
    unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());
    assert!(
        !data.join("admin.sock").exists(),
        "the admin socket outlived the collector"
    );
}

/// Review I1: `the_collectors_data_is_private_to_its_user` only sends its
/// SIGTERM once `setup-url` exists and the port accepts a connection — by
/// then the old code (the shutdown task spawned on a bare `terminated()`,
/// with no `Signals` created up front) was already well past the only
/// window a SIGTERM could hit it in, so that test's own revert-probe never
/// caught the regression `Signals::new()` fixes. This test sends SIGTERM the
/// instant `admin.sock` exists, which is created right after `bind`, before
/// the database is opened, migrated or a setup link is written — pinning the
/// window directly instead of relying on how long start-up happens to take.
#[test]
fn a_sigterm_during_the_start_leaves_no_admin_socket() {
    let dir = scratch_dir("sigterm-start");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let log = dir.join("collector.log");

    let collector = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["collector", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(&data)
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    // Created before any assertion that could panic: a failure below must
    // still SIGKILL the collector and remove the scratch dir on the way out.
    let mut guard = KillTree::new(collector, &log);
    guard.dir = dir.clone();

    let socket = data.join("admin.sock");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !socket.exists() {
        guard.assert_running("admin.sock to appear");
        assert!(Instant::now() < deadline, "timed out waiting for admin.sock");
        std::thread::sleep(Duration::from_micros(200));
    }

    unsafe { libc::kill(guard.up.id() as i32, libc::SIGTERM) };
    let status =
        wait_with_timeout(&mut guard.up, Duration::from_secs(15)).expect("the collector did not exit after SIGTERM");
    assert!(status.success(), "the collector exited with {status:?}");
    assert!(!socket.exists(), "the admin socket outlived the collector");
}

/// Final review I2, an install from before the fix: a database others can
/// read is made private when the collector opens it. A data directory others
/// can enter is the operator's to fix: it is named in a warning, and left
/// as it is.
#[test]
fn an_existing_readable_database_is_made_private_and_a_loose_directory_is_warned_about() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir("loose");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
    // An empty file is a new, empty SQLite database.
    std::fs::write(data.join("hennery.db"), b"").unwrap();
    std::fs::set_permissions(data.join("hennery.db"), std::fs::Permissions::from_mode(0o644)).unwrap();

    let log = dir.join("collector.log");
    let (mut collector, _) = collector_under_umask_022(&data, &log);
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
    let dir = scratch_dir("uproot");
    let _cleanup = RemoveDir(dir.clone());
    let root = dir.join("data");

    let log = dir.join("up.log");
    let mut up = up_logging_to_with(under_umask_022(), &root, &log, &[]);
    let listen = up.listening();
    let session = sign_in(&mut up, &listen, &root.join("collector"));
    up.wait_until("the host connected", || {
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
    let dir = scratch_dir("looseroot");
    let _cleanup = RemoveDir(dir.clone());
    let root = dir.join("data");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

    let log = dir.join("up.log");
    let mut up = up_logging_to(&root, &log);
    up.wait_until("the warning about the data root", || {
        std::fs::read_to_string(&log).is_ok_and(|text| {
            text.lines()
                .any(|line| line.contains("readable by other users") && line.contains(&root.display().to_string()))
        })
    });
    assert_eq!(mode_of(&root), 0o755, "the operator's directory was changed");
    unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut up.up, Duration::from_secs(15)).is_some());
}

/// The processes (pid and command line) whose command line names `dir`: an
/// `up` on `dir` and its children, found also once they are orphaned.
fn processes_naming(dir: &std::path::Path) -> Vec<(i32, String)> {
    let out = Command::new("ps")
        .args(["-A", "-ww", "-o", "pid=,command="])
        .output()
        .unwrap();
    assert!(out.status.success(), "ps failed: {out:?}");
    let dir = dir.display().to_string();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| line.contains(&dir))
        .filter_map(|line| {
            let (pid, command) = line.trim().split_once(' ')?;
            Some((pid.parse().ok()?, command.to_string()))
        })
        .collect()
}

/// SIGKILLs, on drop, every process whose command line names this
/// directory: only processes this test started name it.
struct KillNaming(std::path::PathBuf);

impl Drop for KillNaming {
    fn drop(&mut self) {
        for (pid, _) in processes_naming(&self.0) {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

/// `up` catches SIGTERM from before it starts a child: one that comes just
/// after the collector's spawn must still stop the collector, not kill `up`
/// by the default action and leave its collector running with nobody to
/// stop it.
///
/// Each attempt sends SIGTERM a little later (0 to 1 ms) after `up`'s
/// warning about its loose data root, which it logs just before it spawns.
/// Deterministic: `up` must exit 0, having caught the signal, on every
/// attempt; a `up` that installs its handlers only later dies of the
/// signal whenever it comes first. And no process naming the data root may
/// outlive `up`, by a `ps` scan first shown to see such a process.
#[test]
fn a_sigterm_as_up_starts_leaves_no_child_running() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir("earlyterm");
    let _cleanup = RemoveDir(dir.clone());
    // The scan below must not pass for want of seeing anything: it does see
    // a live process whose command line names a directory here. Checked
    // apart from `up`, as a `ps` between the warning and the signal would
    // take longer than the window.
    let probe_dir = dir.join("probe");
    let mut probe = Command::new("/bin/sh")
        // Not a lone command, which the shell would exec, losing its $0.
        .args(["-c", "sleep 30; exit 0"])
        .arg(&probe_dir)
        .process_group(0)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    // Polled: until it has exec'd, the probe's command line is this binary's.
    while !seen.iter().any(|&(pid, _)| pid == probe.id() as i32) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        seen = processes_naming(&probe_dir);
    }
    // The whole group: the shell's `sleep` too.
    unsafe { libc::kill(-(probe.id() as i32), libc::SIGKILL) };
    let _ = probe.wait();
    assert!(
        seen.iter().any(|&(pid, _)| pid == probe.id() as i32),
        "ps does not see the probe: {seen:?}"
    );
    for attempt in 0..5 {
        let root = dir.join(format!("data-{attempt}"));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _orphans = KillNaming(root.clone());
        let log = dir.join(format!("up-{attempt}.log"));
        let mut up = up_logging_to(&root, &log);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !std::fs::read_to_string(&log).is_ok_and(|text| text.contains("readable by other users")) {
            up.assert_running("the warning about the data root");
            assert!(Instant::now() < deadline, "timed out waiting for the warning");
            std::thread::sleep(Duration::from_micros(100));
        }
        let offset = Duration::from_micros(250 * attempt);
        let start = Instant::now();
        while start.elapsed() < offset {
            std::hint::spin_loop();
        }
        unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
        let status = wait_with_timeout(&mut up.up, Duration::from_secs(30)).expect("up did not exit");
        assert!(
            status.success(),
            "attempt {attempt}: up did not catch the SIGTERM: {status}"
        );
        // `up` waits for its children before it exits: none may be left.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut left = processes_naming(&root);
        while !left.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            left = processes_naming(&root);
        }
        assert!(
            left.is_empty(),
            "attempt {attempt}: still running after up exited: {left:?}"
        );
    }
}

/// `--listen-fd` (`up`'s hand-over) adopts only a listening TCP socket: a
/// closed descriptor, a file, a UDP socket, a TCP socket that does not
/// listen (never bound, or bound and never listened on) and a listening
/// Unix socket are refused at once with a message naming it, before the
/// data directory is made, not adopted to abort, hang or serve later. Also
/// refused: a standard stream's number, `--listen` with it, and one
/// descriptor given twice.
#[test]
fn the_collector_refuses_a_listen_fd_that_is_not_a_listening_socket() {
    use std::os::fd::{AsRawFd, OwnedFd};
    let dir = scratch_dir("badlistenfd");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let file: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
    let udp: OwnedFd = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().into();
    // Close-on-exec, so it leaks into no child another test spawns: at
    // once on Linux, right after socket(2) on macOS (no SOCK_CLOEXEC).
    // SAFETY: socket(2) and fcntl(2) on the new descriptor, owned at once.
    let stream_socket = || -> OwnedFd {
        #[cfg(target_os = "linux")]
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        #[cfg(not(target_os = "linux"))]
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(fd >= 0);
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) }, 0);
        unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) }
    };
    let tcp = stream_socket();
    // Bound to a port (port 0: the system picks one), never listened on:
    // macOS used to take this one.
    // SAFETY: bind(2) on a local address.
    let bound: OwnedFd = unsafe {
        let socket = stream_socket();
        let fd = socket.as_raw_fd();
        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
        let rc = libc::bind(
            fd,
            (&raw const addr).cast(),
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        );
        assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
        socket
    };
    let unix: OwnedFd = std::os::unix::net::UnixListener::bind(dir.join("s")).unwrap().into();
    let listening: OwnedFd = std::net::TcpListener::bind("127.0.0.1:0").unwrap().into();
    let collector = |fd: Option<&OwnedFd>, args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hennery"));
        cmd.arg("collector").args(args).arg("--data-dir").arg(&data);
        if fd.is_none() {
            // Another test's spawn can leak a descriptor into this child for
            // a moment (macOS makes a pipe or socket close-on-exec only after
            // it exists), and it may sit at 50: closed here, so what is
            // tested is a closed descriptor.
            // SAFETY: close(2) in the forked child, before exec; async-signal-safe.
            unsafe {
                cmd.pre_exec(|| {
                    libc::close(50);
                    Ok(())
                });
            }
        }
        if let Some(fd) = fd {
            let fd = fd.as_raw_fd();
            // SAFETY: dup2 in the forked child, before exec; async-signal-safe.
            unsafe {
                // Already 50, maybe: a busy test binary has that many open.
                cmd.pre_exec(move || hennery_testkit::place_fd(fd, 50));
            }
        }
        let mut child = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let status = wait_with_timeout(&mut child, Duration::from_secs(15));
        let _ = child.kill();
        let _ = child.wait();
        let mut stderr = String::new();
        child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
        (status, stderr)
    };
    for (what, fd, expected) in [
        ("a closed descriptor", None, "is not an open descriptor"),
        ("a file", Some(&file), "is not a socket"),
        ("a UDP socket", Some(&udp), "is not a stream (TCP) socket"),
        (
            "a TCP socket that does not listen",
            Some(&tcp),
            "is not a listening socket",
        ),
        (
            "a TCP socket bound and never listened on",
            Some(&bound),
            "is not a listening socket",
        ),
        ("a listening Unix socket", Some(&unix), "is not a TCP socket"),
    ] {
        // 50: well above what the collector's own runtime opens at start.
        let (status, stderr) = collector(fd, &["--listen-fd", "50"]);
        let status = status.unwrap_or_else(|| panic!("{what}: the collector hung"));
        assert!(!status.success(), "{what}: adopted");
        assert!(stderr.contains(expected), "{what}: {stderr}");
        assert!(!data.exists(), "{what}: the data directory was made");
    }
    for args in [
        &["--listen-fd", "2"][..],
        &["--listen-fd", "50", "--listen", "127.0.0.1:0"],
    ] {
        let (status, stderr) = collector(Some(&listening), args);
        assert!(!status.unwrap().success(), "{args:?} was taken: {stderr}");
    }
    // A descriptor given twice would be adopted as two listeners sharing one
    // socket: refused before anything is created (decision 14, A6). Without
    // the guard this stays green on Linux (the second `TcpListener::from_std`
    // still fails, but only after `private_data_dir` ran), so both the
    // message and "nothing was made" are pinned here, not just `!success()`.
    let (status, stderr) = collector(Some(&listening), &["--listen-fd", "50", "--listen-fd", "50"]);
    assert!(!status.unwrap().success(), "was taken: {stderr}");
    assert!(stderr.contains("--listen-fd 50 is given twice"), "{stderr}");
    assert!(!data.exists(), "the data directory was made");
}

/// 3a/3b's deferred check: the pairing pipe's ends `up` hands its children,
/// `collector --pairing-code-fd` and `host run --join-code-fd`, must be open
/// pipes, as `--listen-fd` must be a listening socket. A closed descriptor,
/// a file (`/dev/null`) and a socket are refused at once with a message
/// naming the flag and the descriptor, before the data directory is made;
/// so is a standard stream's number. A paired host checks it too, before it
/// closes the descriptor as not needed.
#[test]
fn the_code_descriptors_must_be_open_pipes() {
    use std::os::fd::{AsRawFd, OwnedFd};
    let dir = scratch_dir("badcodefd");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let file: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
    // The other end is held open: a read from this one would block.
    let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let socket: OwnedFd = socket.into();
    // `--listen 127.0.0.1:0`: should a check let it through, this collector
    // must not take a port another one may be serving.
    let collector = |data: &std::path::Path, fd: &str| -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hennery"));
        cmd.args(["collector", "--listen", "127.0.0.1:0", "--pairing-code-fd", fd])
            .arg("--data-dir")
            .arg(data);
        cmd
    };
    let host = |data: &std::path::Path, fd: &str| -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hennery"));
        cmd.args(["host", "run", "--join-url", "http://127.0.0.1:1", "--join-code-fd", fd])
            .arg("--data-dir")
            .arg(data);
        cmd
    };
    let run = |mut cmd: Command, fd: Option<&OwnedFd>| {
        for var in [
            "HENNERY_LISTEN",
            "HENNERY_DATA_DIR",
            "HENNERY_HOST_DATA_DIR",
            "HENNERY_PUBLIC_URL",
        ] {
            cmd.env_remove(var);
        }
        let fd = fd.map(|fd| fd.as_raw_fd());
        // SAFETY: dup2 and close in the forked child, before exec;
        // async-signal-safe.
        unsafe {
            cmd.pre_exec(move || match fd {
                Some(fd) => hennery_testkit::place_fd(fd, 50),
                // Closed for certain: something another thread opened
                // without close-on-exec may sit at 50.
                None => {
                    libc::close(50);
                    Ok(())
                }
            });
        }
        let mut child = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let status = wait_with_timeout(&mut child, Duration::from_secs(15));
        let _ = child.kill();
        let _ = child.wait();
        let mut stderr = String::new();
        child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
        (status, stderr)
    };
    type Make<'a> = &'a dyn Fn(&std::path::Path, &str) -> Command;
    for (flag, make) in [("--pairing-code-fd", &collector as Make), ("--join-code-fd", &host)] {
        for (what, fd, expected) in [
            ("a closed descriptor", None, "is not an open descriptor"),
            ("a file", Some(&file), "is not a pipe"),
            ("a socket", Some(&socket), "is not a pipe"),
        ] {
            // 50: well above what the child's own runtime opens at start.
            let (status, stderr) = run(make(&data, "50"), fd);
            let status = status.unwrap_or_else(|| panic!("{flag}, {what}: the child hung"));
            assert!(!status.success(), "{flag}, {what}: taken");
            assert!(
                stderr.contains(&format!("{flag} 50 {expected}")),
                "{flag}, {what}: {stderr}"
            );
            assert!(!data.exists(), "{flag}, {what}: the data directory was made");
        }
        // Standard error is a pipe here, so only the range refuses it: a
        // collector would write the code to it, a host would take it over.
        let (status, stderr) = run(make(&data, "2"), None);
        assert!(!status.unwrap().success(), "{flag} 2 was taken: {stderr}");
        assert!(
            stderr.contains(&format!("invalid value '2' for '{flag}")),
            "{flag} 2: {stderr}"
        );
        assert!(!data.exists(), "{flag} 2: the data directory was made");
    }

    // A paired host does not need the code, but still refuses a bad
    // descriptor rather than closing whatever is at that number.
    let (_rt, addr, code) = collector_with_a_pairing_code(&dir);
    let paired = dir.join("paired");
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "join", &format!("http://{addr}"), &code, "--name", "laptop"])
        .arg("--data-dir")
        .arg(&paired)
        .env_remove("HENNERY_HOST_DATA_DIR")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for (what, fd, expected) in [
        ("a closed descriptor", None, "is not an open descriptor"),
        ("a socket", Some(&socket), "is not a pipe"),
    ] {
        let (status, stderr) = run(host(&paired, "50"), fd);
        let status = status.unwrap_or_else(|| panic!("paired, {what}: the host ran"));
        assert!(!status.success(), "paired, {what}: taken");
        assert!(
            stderr.contains(&format!("--join-code-fd 50 {expected}")),
            "paired, {what}: {stderr}"
        );
    }
}

/// Plan 3a's deferred EOF fail-safe (kernel spec §4.2): the collector
/// child dies before it writes the pairing code, so the host's read of
/// `--join-code-fd` gets end-of-file. The host must fail, saying so, and
/// pair nothing: no pairing stored, no host enrolled, although a live code
/// and a collector to spend it on are there.
///
/// The shell makes the pipe and runs `:` as the collector, which exits
/// without writing: no end of it is ever open in this (multithreaded) test
/// process, so no spawn on another thread can inherit the writer and hold
/// the end-of-file back. Only the host's side is pinned here: that `up`
/// drops its own copies of both ends is what lets the end-of-file arrive.
#[test]
fn the_host_fails_and_pairs_nothing_when_the_collector_dies_before_the_code() {
    let dir = scratch_dir("codeeof");
    let _cleanup = RemoveDir(dir.clone());
    let (_rt, addr, _code) = collector_with_a_pairing_code(&dir);
    let host_dir = dir.join("host");
    let mut child = Command::new("/bin/sh")
        .args([
            "-c",
            ": | exec \"$0\" \"$@\" 3<&0 </dev/null",
            env!("CARGO_BIN_EXE_hennery"),
        ])
        .args([
            "host",
            "run",
            "--join-url",
            &format!("http://{addr}"),
            "--join-code-fd",
            "3",
        ])
        .arg("--data-dir")
        .arg(&host_dir)
        .env_remove("HENNERY_HOST_DATA_DIR")
        .env_remove("HENNERY_DEV_TOKEN")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        // Its own group: on a hang, the whole pipeline is killed below.
        .process_group(0)
        .spawn()
        .unwrap();
    let status = wait_with_timeout(&mut child, Duration::from_secs(30));
    if status.is_none() {
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    }
    let _ = child.wait();
    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    let status = status.unwrap_or_else(|| panic!("the host hung on the closed pipe: {stderr}"));
    assert!(!status.success(), "the host ran without a code: {stderr}");
    assert!(
        stderr.contains("the collector exited without handing over a pairing code"),
        "{stderr}"
    );
    for file in ["host.key", "host.toml"] {
        assert!(!host_dir.join(file).exists(), "{file} was stored");
    }
    let hosts = hennery_kernel::hosts::Hosts::open(&dir.join("hennery.db")).unwrap();
    assert!(hosts.list().unwrap().is_empty(), "a host was enrolled");
}

/// Everything `pid`'s command line and environment hold, as text: from
/// `/proc` on Linux, and from `ps -E` elsewhere (macOS), which prints the
/// environment after the command line.
fn argv_and_environment(pid: i32) -> String {
    #[cfg(target_os = "linux")]
    {
        let read = |what: &str| {
            let bytes =
                std::fs::read(format!("/proc/{pid}/{what}")).unwrap_or_else(|err| panic!("{pid} {what}: {err}"));
            String::from_utf8_lossy(&bytes).replace('\0', " ")
        };
        format!("{}\n{}", read("cmdline"), read("environ"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = Command::new("ps")
            .args(["-E", "-ww", "-o", "command=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        assert!(out.status.success(), "ps failed for {pid}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

/// Plan 3a's deferred check (kernel spec §4.2): the pairing code travels
/// from `up`'s collector child to its host child over the pipe only, never
/// on either child's command line or in its environment, where any process
/// of the user's (and, for the command line, any user) could read it.
///
/// `up` runs with an environment of its own making, so nothing inherited
/// is shaped like a code. The probe is first shown to see a code's shape in
/// a process's arguments and in its environment, and then to see both
/// children's own flags and `up`'s marker variable. Read once the host has
/// paired, so the code has been handed over by then.
#[test]
fn the_pairing_code_never_reaches_ups_childrens_argv_or_environment() {
    let dir = scratch_dir("codeargv");
    let _cleanup = RemoveDir(dir.clone());

    // Control: the probe reads a process's arguments and its environment.
    // `hennery` itself, not a shell: macOS shows no platform binary's
    // environment (`/bin/sh`'s, say) to `ps -E`. `host join` with the code
    // left out waits on standard input, and gives up once that closes.
    let mut control = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "join", "http://127.0.0.1:1", "--name", "ABCD-EFGH"])
        .arg("--data-dir")
        .arg(dir.join("control"))
        .env("HENNERY_PROBE_CONTROL", "WXYZ-2345")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Right after `spawn` the child may not have finished `exec` yet, and
    // Linux shows it with an empty command line: read until it has.
    let deadline = Instant::now() + Duration::from_secs(10);
    let seen = loop {
        let seen = argv_and_environment(control.id() as i32);
        if seen.contains("ABCD-EFGH") || Instant::now() > deadline {
            break seen;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(control.stdin.take());
    assert!(wait_with_timeout(&mut control, Duration::from_secs(15)).is_some());
    let _ = control.kill();
    let _ = control.wait();
    assert!(seen.contains("ABCD-EFGH"), "the probe missed an argument: {seen}");
    assert!(
        seen.contains("HENNERY_PROBE_CONTROL=WXYZ-2345"),
        "the probe missed the environment: {seen}"
    );
    assert!(contains_a_pairing_code_shape(&seen));

    let data = dir.join("data");
    let log = dir.join("up.log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HENNERY_PROBE_MARKER", "yes");
    let mut up = up_logging_to_with(command, &data, &log, &[]);
    let paired = data.join("host").join("host.toml");
    up.wait_until("the host paired", || paired.exists());
    let children = children_of(up.up.id() as i32);
    up.children.extend(&children);
    assert_eq!(children.len(), 2, "{children:?}");
    let seen: Vec<String> = children.iter().map(|&pid| argv_and_environment(pid)).collect();
    // The probe sees both children's own arguments and environment.
    for flag in ["--pairing-code-fd", "--join-code-fd"] {
        assert!(
            seen.iter().any(|text| text.contains(flag)),
            "no child has {flag}: {seen:?}"
        );
    }
    for text in &seen {
        assert!(text.contains("HENNERY_PROBE_MARKER=yes"), "{text}");
        assert!(
            !contains_a_pairing_code_shape(text),
            "a pairing code reached a child's argv or environment: {text}"
        );
    }

    unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut up.up, Duration::from_secs(15)).is_some());
}

/// Kernel spec §3.1: a collector that is not set up writes its one-time
/// setup link to `setup-url` (0600, under `umask 022` too) and, its output
/// not being a terminal, logs only that file's path: the token itself must
/// never reach a log collector.
#[test]
fn an_unset_collector_writes_its_setup_link_to_a_private_file_and_never_to_its_output() {
    let dir = scratch_dir("setup");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let log = dir.join("collector.log");

    let (mut collector, listen) = collector_under_umask_022(&data, &log);
    let file = data.join("setup-url");
    // The file is written before the line naming it is logged; that line
    // must be in the log before the collector is stopped.
    collector.wait_until("the setup link announced", || {
        std::fs::read_to_string(&log).is_ok_and(|text| text.contains(&file.display().to_string()))
    });
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

/// A collector served in this process on `dir`'s database, with one pairing
/// code minted: the runtime serving it (keep it alive), its address and the
/// code.
fn collector_with_a_pairing_code(dir: &std::path::Path) -> (tokio::runtime::Runtime, std::net::SocketAddr, String) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (addr, code) = rt.block_on(async {
        let db = dir.join("hennery.db");
        let state = hennery_sessions::AppState::new(
            hennery_sessions::store::Store::open(&db).unwrap(),
            hennery_kernel::hosts::Hosts::open(&db).unwrap(),
            hennery_kernel::operator::Operator::open(&db).unwrap(),
        );
        let code = state
            .hosts
            .mint_pairing_code(hennery_kernel::secret::unix_now())
            .unwrap()
            .code;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(hennery_sessions::serve(listener, state));
        (addr, code)
    });
    (rt, addr, code)
}

/// `hennery host join` to the collector at `addr`, into `host`, with the
/// code left out (so read from standard input) and every stream piped.
fn join_from_stdin(addr: std::net::SocketAddr, host: &std::path::Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "join", &format!("http://{addr}"), "--name", "laptop"])
        .arg("--data-dir")
        .arg(host)
        .env_remove("HENNERY_HOST_DATA_DIR")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

/// 3a's deferred M5: a pairing code on the command line is in the process
/// list and the shell history, so `host join` also takes it on standard
/// input when it is left out, and refuses an empty one.
#[test]
fn join_reads_the_code_from_standard_input_when_it_is_left_out() {
    let dir = scratch_dir("stdin-code");
    let _cleanup = RemoveDir(dir.clone());
    let (_rt, addr, code) = collector_with_a_pairing_code(&dir);
    let join = |stdin: &str| {
        let mut child = join_from_stdin(addr, &dir.join("host"));
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    };

    let empty = join("\n");
    assert!(!empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr).contains("no pairing code"));

    let out = join(&format!("{code}\n"));
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("paired as"));
}

/// 3a/3b's deferred bound on that read: a line of standard input longer than
/// any code is refused as soon as the bound is hit, without waiting for a
/// newline or the end, without echoing it and without pairing; the code it
/// began with still pairs afterwards.
#[test]
fn join_refuses_an_overlong_line_on_standard_input_at_once() {
    let dir = scratch_dir("stdin-overlong");
    let _cleanup = RemoveDir(dir.clone());
    let (_rt, addr, code) = collector_with_a_pairing_code(&dir);
    let host = dir.join("host");
    let mut child = join_from_stdin(addr, &host);
    let mut stdin = child.stdin.take().unwrap();
    // The real code, then 1 MiB with no newline, and standard input held
    // open: an unbounded read waits for more forever. The child stops
    // reading early, so the write may well fail (EPIPE); that is the point.
    let overlong = format!("{code}{}", "x".repeat(1 << 20));
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(overlong.as_bytes());
        stdin
    });
    let status = wait_with_timeout(&mut child, Duration::from_secs(15));
    let _ = child.kill();
    let _ = child.wait();
    drop(writer.join().unwrap());
    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    let status = status.unwrap_or_else(|| panic!("join kept reading: {stderr}"));
    assert!(!status.success(), "{stderr}");
    assert!(
        stderr.contains("the pairing code on standard input is longer than 256 bytes"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&code) && !stderr.contains("xxxxxxxx"),
        "echoed: {stderr}"
    );
    assert!(!host.exists(), "a pairing was stored");

    let mut child = join_from_stdin(addr, &host);
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{code}\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// `POST path` with no body on the collector at `listen`, with the owner's
/// `session` and `Origin: origin`: the status.
fn post_from(listen: &str, path: &str, session: &str, origin: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok()?;
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {listen}\r\nCookie: hennery_session={session}\r\nOrigin: {origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.split(' ').nth(1)?.parse().ok()
}

/// `GET path` on the collector at `listen`, without a session: the status
/// and the body.
fn get_plain(listen: &str, path: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {listen}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let (head, body) = response.split_once("\r\n\r\n")?;
    Some((head.split(' ').nth(1)?.parse().ok()?, body.to_string()))
}

/// Kernel spec §7: a collector given several addresses serves the same
/// routes and the same state on each, with the same browser rules, and
/// browser access stays bound to `public_url`: a state-changing request on
/// the second listener needs the first's origin, the one set up as
/// `public_url`, not the second's own.
#[test]
fn every_listen_address_serves_the_same_collector() {
    let dir = scratch_dir("listeners");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let log = dir.join("collector.log");
    let collector = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["collector", "--listen", "127.0.0.1:0", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(&data)
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut collector = KillTree::new(collector, &log);
    let addresses = collector.listening_on(2);
    assert_ne!(addresses[0], addresses[1]);
    let (first, second) = (&addresses[0], &addresses[1]);
    // `public_url` is `http://<first>`.
    let session = sign_in(&mut collector, first, &data);
    for address in &addresses {
        assert_eq!(get_plain(address, "/healthz"), Some((200, "ok".into())), "{address}");
        assert!(get_json(address, "/api/hosts", &session).is_some(), "{address}");
    }
    assert_eq!(
        post_from(second, "/api/auth/logout", &session, &format!("http://{second}")),
        Some(403),
        "the second listener's own origin was taken for public_url"
    );
    assert_eq!(
        post_from(second, "/api/auth/logout", &session, &format!("http://{first}")),
        Some(204)
    );
    // The logout on the second listener ended the session on the first.
    assert!(get_json(first, "/api/hosts", &session).is_none());
}

/// Kernel spec §7, §11: start fails when any one of several addresses is
/// taken, for `collector` and for `up`, before either touches its data
/// directory.
#[test]
fn a_taken_listen_address_fails_the_start_before_the_data_dir_is_touched() {
    let dir = scratch_dir("taken");
    let _cleanup = RemoveDir(dir.clone());
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = taken.local_addr().unwrap().to_string();
    for command in ["collector", "up"] {
        let data = dir.join(command);
        let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args([command, "--listen", "127.0.0.1:0", "--listen", &taken])
            .arg("--data-dir")
            .arg(&data)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{command} started: {stderr}");
        assert!(stderr.contains(&format!("bind {taken}")), "{command}: {stderr}");
        assert!(!data.exists(), "{command} made its data directory");
    }
}

/// `up` binds every address and hands each to its collector child (kernel
/// spec §7): its host connects over the first, and the second serves the
/// same collector. A `HENNERY_LISTEN` in `up`'s environment (which its
/// flags override) does not reach the child, which would otherwise refuse
/// it beside `--listen-fd`.
#[test]
fn up_hands_every_listen_address_to_its_collector() {
    let dir = scratch_dir("uplisteners");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let log = dir.join("up.log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
    command.env("HENNERY_LISTEN", "127.0.0.1:0");
    let mut up = up_logging_to_with(command, &data, &log, &["--listen", "127.0.0.1:0"]);
    let addresses = up.listening_on(2);
    let session = sign_in(&mut up, &addresses[0], &data.join("collector"));
    up.wait_until("the host connected, seen on the second listener", || {
        matches!(
            get_json(&addresses[1], "/api/hosts", &session),
            Some(serde_json::Value::Array(hosts)) if hosts.iter().any(|h| h["connected"] == true)
        )
    });
}

/// Kernel spec §2: `config.toml` gives `listen` and `public_url` when
/// neither a flag nor the environment does; `HENNERY_LISTEN` and
/// `HENNERY_PUBLIC_URL` win over the file, and flags over both. Counted by
/// the collector's listeners (the file names three, the environment two, a
/// flag one) and by the setup link, which names `public_url`.
#[test]
fn config_toml_yields_to_the_environment_and_the_environment_to_flags() {
    let dir = scratch_dir("config");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("config.toml"),
        "listen = [\"127.0.0.1:0\", \"127.0.0.1:0\", \"127.0.0.1:0\"]\npublic_url = \"https://file.example\"\n",
    )
    .unwrap();
    // Private whatever the umask: a file others can write is refused.
    std::fs::set_permissions(
        data.join("config.toml"),
        std::os::unix::fs::PermissionsExt::from_mode(0o644),
    )
    .unwrap();
    let run = |env: &[(&str, &str)], args: &[&str], name: &str| {
        let log = dir.join(format!("{name}.log"));
        let collector = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .arg("collector")
            .args(args)
            .arg("--data-dir")
            .arg(&data)
            .envs(env.iter().copied())
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
            .spawn()
            .unwrap();
        let mut collector = KillTree::new(collector, &log);
        let file = data.join("setup-url");
        // Written after every listener's line is logged.
        collector.wait_until("the setup link", || file.exists());
        let link = std::fs::read_to_string(&file).unwrap();
        let listeners = listening_addresses(&std::fs::read_to_string(&log).unwrap()).len();
        unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
        assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());
        let _ = std::fs::remove_file(&file);
        let origin = link.split("/setup#").next().unwrap().to_string();
        (listeners, origin)
    };
    assert_eq!(run(&[], &[], "file"), (3, "https://file.example".to_string()));
    let env = [
        ("HENNERY_LISTEN", "127.0.0.1:0,127.0.0.1:0"),
        ("HENNERY_PUBLIC_URL", "https://env.example"),
    ];
    assert_eq!(run(&env, &[], "env"), (2, "https://env.example".to_string()));
    assert_eq!(
        run(
            &env,
            &["--listen", "127.0.0.1:0", "--public-url", "https://flag.example"],
            "flag"
        ),
        (1, "https://flag.example".to_string())
    );
}

/// A `config.toml` that does not parse, names a key hennery does not know,
/// or that other users can write, stops the start with the file's name,
/// before anything is bound or created.
#[test]
fn a_bad_config_toml_stops_the_start() {
    let dir = scratch_dir("badconfig");
    let _cleanup = RemoveDir(dir.clone());
    for (name, text, mode, expected) in [
        ("typo", "listens = [\"127.0.0.1:0\"]\n", 0o644, "unknown field"),
        (
            "public_url",
            "public_url = \"http://hennery.example\"\n",
            0o644,
            "public_url must be https://",
        ),
        ("writable", "listen = [\"127.0.0.1:0\"]\n", 0o666, "chmod go-w"),
    ] {
        use std::os::unix::fs::PermissionsExt;
        let data = dir.join(name);
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("config.toml"), text).unwrap();
        std::fs::set_permissions(data.join("config.toml"), std::fs::Permissions::from_mode(mode)).unwrap();
        // On port 0, and bounded: a collector that ignored the file would
        // serve rather than stop, and never on the default port.
        let mut child = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args(["collector", "--listen", "127.0.0.1:0", "--data-dir"])
            .arg(&data)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let status = wait_with_timeout(&mut child, Duration::from_secs(15));
        let _ = child.kill();
        let _ = child.wait();
        let mut stderr = String::new();
        child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
        assert!(status.is_some_and(|s| !s.success()), "{name}: started: {stderr}");
        assert!(stderr.contains(expected), "{name}: {stderr}");
        assert!(stderr.contains("config.toml"), "{name}: {stderr}");
        assert!(!data.join("hennery.db").exists(), "{name}: the database was made");
    }
}

/// `up --public-url` reaches its collector child, whose setup link names it
/// (kernel spec §3.1).
#[test]
fn up_hands_its_public_url_to_its_collector() {
    let dir = scratch_dir("uppublicurl");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let mut up = up_logging_to_with(
        Command::new(env!("CARGO_BIN_EXE_hennery")),
        &data,
        &dir.join("up.log"),
        &["--public-url", "https://up.example"],
    );
    let file = data.join("collector").join("setup-url");
    up.wait_until("the setup link", || file.exists());
    let link = std::fs::read_to_string(&file).unwrap();
    assert!(link.starts_with("https://up.example/setup#"), "{link}");
}

/// A collector on port 0 in `data`, logging to `log`, once it serves: the
/// guard, and the address it listens on.
fn collector_on(data: &std::path::Path, log: &std::path::Path) -> (KillTree, String) {
    let collector = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["collector", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(data)
        .stdout(std::fs::File::create(log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut guard = KillTree::new(collector, log);
    let listen = guard.listening();
    guard.wait_until("the admin socket", || data.join("admin.sock").exists());
    (guard, listen)
}

/// `hennery admin --data-dir <data> <args…>` with no terminal: standard
/// input is empty.
fn admin(data: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("admin")
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

/// Like `admin`, with a terminal (a pty) as standard input, into which
/// `typed` is typed ahead: the confirmations' path. A command still
/// waiting for input after 20 seconds (typed-ahead input lost, say) is
/// killed and fails the test.
fn admin_on_a_terminal(data: &std::path::Path, args: &[&str], typed: &str) -> std::process::Output {
    let (master, slave) = open_pty();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("admin")
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .stdin(std::process::Stdio::from(slave))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    (&master).write_all(typed.as_bytes()).unwrap();
    let Some(status) = wait_with_timeout(&mut child, Duration::from_secs(20)) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("`hennery admin {args:?}` still waited for input");
    };
    drop(master);
    // Its output is a few lines: it fit the pipes while it ran.
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    child.stdout.take().unwrap().read_to_end(&mut stdout).unwrap();
    child.stderr.take().unwrap().read_to_end(&mut stderr).unwrap();
    std::process::Output { status, stdout, stderr }
}

/// Like `admin_on_a_terminal`, typing each `(prompt, typed)`'s `typed` only
/// once `prompt` is on the command's standard error, as a person would.
/// Also returns everything the terminal showed (its echo).
fn admin_conversation(data: &std::path::Path, args: &[&str], steps: &[(&str, &str)]) -> (std::process::Output, String) {
    use std::sync::{Arc, Mutex};
    let (master, slave) = open_pty();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("admin")
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .stdin(std::process::Stdio::from(slave))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Read on throughout: what the terminal shows, and the prompts.
    let collect = |mut from: Box<dyn Read + Send>| {
        let text = Arc::new(Mutex::new(Vec::new()));
        let sink = text.clone();
        let thread = std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n) = from.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        (text, thread)
    };
    let (shown, shown_thread) = collect(Box::new(master.try_clone().unwrap()));
    let (stderr, stderr_thread) = collect(Box::new(child.stderr.take().unwrap()));
    for (prompt, typed) in steps {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !String::from_utf8_lossy(&stderr.lock().unwrap()).contains(prompt) {
            if Instant::now() > deadline || matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("no {prompt:?}: {}", String::from_utf8_lossy(&stderr.lock().unwrap()));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        (&master).write_all(typed.as_bytes()).unwrap();
    }
    let Some(status) = wait_with_timeout(&mut child, Duration::from_secs(20)) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("`hennery admin {args:?}` still waited for input");
    };
    drop(master);
    let mut stdout = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut stdout).unwrap();
    // Both end once the command has exited: the terminal with its last
    // holder, standard error with the command.
    stderr_thread.join().unwrap();
    shown_thread.join().unwrap();
    let stderr = stderr.lock().unwrap().clone();
    let shown = String::from_utf8_lossy(&shown.lock().unwrap()).into_owned();
    (std::process::Output { status, stdout, stderr }, shown)
}

/// A new pty: its master end, and its slave end for a child's standard
/// input.
fn open_pty() -> (std::fs::File, std::os::fd::OwnedFd) {
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty(3) into two local ints, with no name and the default
    // settings. The `fcntl(F_SETFD)` calls below are separate syscalls, not
    // atomic with the open: a child spawned by another test thread in
    // between inherits both ends, and holds them open until it exits (the
    // unbounded `shown_thread.join()` above then waits that long too). The
    // fds are otherwise unused until returned, and owned.
    unsafe {
        let rc = libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
        for fd in [master, slave] {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        (std::fs::File::from_raw_fd(master), OwnedFd::from_raw_fd(slave))
    }
}

/// `POST /api/auth/login` with `password`, from `origin`: the status.
///
/// The request goes out in one write. A refused origin is answered before
/// the body is read, and the collector then closes: a body still unread, or
/// still on its way (`write!` sends each piece apart), resets the
/// connection, and the 403 already received is lost to the reset.
fn login(listen: &str, origin: &str, password: &str) -> Option<u16> {
    let body = serde_json::json!({ "password": password }).to_string();
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok()?;
    let request = format!(
        "POST /api/auth/login HTTP/1.1\r\nHost: {listen}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.split(' ').nth(1)?.parse().ok()
}

/// Kernel spec §4.2: the commands that change state or hand out a
/// credential ask for confirmation on a terminal, and without one they
/// refuse and send nothing. Printing the setup link and listing hosts
/// need none; the setup link printed is the one in `setup-url`.
#[test]
fn admin_commands_that_change_state_need_a_terminal() {
    let dir = scratch_dir("adminnotty");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (mut collector, listen) = collector_on(&data, &dir.join("collector.log"));
    collector.wait_until("the setup link", || data.join("setup-url").exists());
    let out = admin(&data, &["setup-url"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        std::fs::read_to_string(data.join("setup-url")).unwrap()
    );
    let session = sign_in(&mut collector, &listen, &data);

    for args in [
        &["reset-password"][..],
        &["pairing-code"],
        &["reset-public-url", "https://moved.example"],
    ] {
        let out = admin(&data, args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} ran without a terminal");
        assert!(stderr.contains("on a terminal"), "{args:?}: {stderr}");
        assert!(!contains_a_pairing_code_shape(&String::from_utf8_lossy(&out.stdout)));
    }
    // Nothing reached the collector: the session and the origin still hold.
    assert!(get_json(&listen, "/api/hosts", &session).is_some());
    assert_eq!(
        post_from(&listen, "/api/auth/logout", &session, &format!("http://{listen}")),
        Some(204)
    );

    let out = admin(&data, &["hosts"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let out = admin(&data, &["setup-url"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("set up already"));
}

/// 3b decision 4's lock-out, recovered live: a collector set up at one
/// origin and then reached at another refuses every login from the new one.
/// `hennery admin reset-public-url`, confirmed on a terminal, moves it
/// without a restart: the new origin signs in, the old one no longer does,
/// and the sessions of the old are gone.
#[test]
fn a_moved_collector_is_recovered_over_the_admin_socket() {
    let dir = scratch_dir("adminmove");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (mut collector, listen) = collector_on(&data, &dir.join("collector.log"));
    // Set up at `http://127.0.0.1:<port>`, then reached as `localhost`.
    let session = sign_in(&mut collector, &listen, &data);
    let old = format!("http://{listen}");
    let new = format!("http://localhost:{}", listen.rsplit(':').next().unwrap());
    assert_eq!(login(&listen, &new, PASSWORD), Some(403), "not locked out");

    let refused = admin_on_a_terminal(&data, &["reset-public-url", &new], "no\n");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("not confirmed"));
    // Kernel spec §3.2: it says passkeys stop working before it asks.
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("Passkeys stop working if the host name changes"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(login(&listen, &new, PASSWORD), Some(403), "moved without a yes");

    let out = admin_on_a_terminal(&data, &["reset-public-url", &new], "yes\n");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(&format!("public_url is now {new}")),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("0 passkey(s) removed"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(login(&listen, &new, PASSWORD), Some(204));
    assert_eq!(login(&listen, &old, PASSWORD), Some(403));
    assert!(
        get_json(&listen, "/api/hosts", &session).is_none(),
        "an old session survived"
    );
}

/// `hennery admin reset-password` reads the new password twice from the
/// terminal and, confirmed, replaces it and signs every session out. Two
/// different entries change nothing.
#[test]
fn a_password_reset_over_the_admin_socket_signs_everyone_out() {
    const NEW: &str = "a new long password";
    let dir = scratch_dir("adminpassword");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (mut collector, listen) = collector_on(&data, &dir.join("collector.log"));
    let session = sign_in(&mut collector, &listen, &data);
    let origin = format!("http://{listen}");

    let differ = admin_on_a_terminal(&data, &["reset-password"], &format!("{NEW}\nsomething else\n"));
    assert!(!differ.status.success());
    assert!(String::from_utf8_lossy(&differ.stderr).contains("differ"));
    // 3c review, A1: it says, before it asks for the password, that every
    // passkey goes.
    let stderr = String::from_utf8_lossy(&differ.stderr);
    let warned = stderr.find("This removes every passkey").expect("the passkey warning");
    assert!(warned < stderr.find("New password: ").unwrap(), "{stderr}");
    assert!(get_json(&listen, "/api/hosts", &session).is_some());

    // Typed only once each prompt is shown, as a person would: none of it
    // may be echoed.
    let line = format!("{NEW}\n");
    let (out, shown) = admin_conversation(
        &data,
        &["reset-password"],
        &[
            ("New password: ", &line),
            ("The same again: ", &line),
            ("Type yes", "yes\n"),
        ],
    );
    // Echo is back on at the confirmation, so the reader having seen
    // nothing at all here would let the assertion below pass vacuously:
    // prove it actually captured the terminal first.
    assert!(shown.contains("yes"), "{shown:?}");
    assert!(!shown.contains(NEW), "the terminal echoed the password: {shown:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        stdout.contains("1 session(s) signed out, 0 passkey(s) removed"),
        "{stdout}"
    );
    assert!(!stdout.contains(NEW) && !String::from_utf8_lossy(&out.stderr).contains(NEW));
    assert!(
        get_json(&listen, "/api/hosts", &session).is_none(),
        "a session survived"
    );
    assert_eq!(login(&listen, &origin, PASSWORD), Some(401));
    assert_eq!(login(&listen, &origin, NEW), Some(204));
}

/// 3b-ii's deferred item: with no collector serving the data directory,
/// the commands that prompt fail before they ask for anything, so no
/// password is typed into a dead end.
#[test]
fn admin_commands_that_prompt_check_the_socket_first() {
    let dir = scratch_dir("adminnone");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    std::fs::create_dir_all(&data).unwrap();
    for args in [
        &["reset-password"][..],
        &["pairing-code"],
        &["reset-public-url", "https://moved.example"],
    ] {
        let out = admin_on_a_terminal(&data, args, "a new long password\na new long password\nyes\n");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}: {stderr}");
        assert!(stderr.contains("is the collector running?"), "{args:?}: {stderr}");
        assert!(
            !stderr.contains("New password") && !stderr.contains("Type yes"),
            "{args:?} prompted first: {stderr}"
        );
    }
}

/// A collector whose admin socket takes the connection and never answers:
/// `hennery admin` gives up, saying the command's outcome is unknown,
/// rather than waiting for ever. The bound is shortened for the test.
#[test]
fn admin_gives_up_on_a_collector_that_never_answers() {
    let dir = scratch_dir("adminmute");
    let _cleanup = RemoveDir(dir.clone());
    let socket = dir.join("admin.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let mute = std::thread::spawn(move || {
        // Taken, held unanswered until the test is done, then closed.
        let held = listener.accept().map(|(stream, _)| stream);
        let _ = wait.recv();
        drop(held);
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("admin")
        .arg("--data-dir")
        .arg(&dir)
        .arg("hosts")
        .env("HENNERY_ADMIN_TIMEOUT_MS", "500")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let status = wait_with_timeout(&mut child, Duration::from_secs(15));
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    done.send(()).unwrap();
    // Unblocks `accept` if the client never connected, so the join cannot
    // hang the test.
    let _ = std::os::unix::net::UnixStream::connect(&socket);
    mute.join().unwrap();
    let status = status.expect("`hennery admin hosts` still waited for an answer");
    let mut stderr = Vec::new();
    child.stderr.take().unwrap().read_to_end(&mut stderr).unwrap();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("did not answer within 500ms"), "{stderr}");
    assert!(stderr.contains("outcome is unknown"), "{stderr}");
}

/// The longest path a Unix socket can be bound at, in bytes.
fn max_socket_path_bytes() -> usize {
    // SAFETY: all-zero bytes are a valid `sockaddr_un`.
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_path.len() - 1
}

/// 3b-ii's O7: when the data directory's `admin.sock` path is too long for
/// a Unix socket (the collector then runs without one), `hennery admin`
/// says so, naming the path and the limit, before it prompts. Also for
/// `hennery up`'s data directory whose own `admin.sock` would fit, but
/// whose collector's, one directory down, does not.
#[test]
fn admin_names_a_socket_path_too_long_for_a_unix_socket() {
    // Under /tmp, not the temporary directory: a long `TMPDIR` could leave
    // no room to pad the paths below to exactly one byte too long.
    let dir = std::path::Path::new("/tmp").join(format!("hennery-cli-adminlong-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let max = max_socket_path_bytes();
    let long = dir.join("d".repeat(max));
    std::fs::create_dir_all(&long).unwrap();
    // Padded so that `<ups>/collector/admin.sock` is one byte too long.
    let base = dir.join("u").as_os_str().len() + "/collector/admin.sock".len();
    let ups = dir.join(format!("u{}", "u".repeat(max + 1 - base)));
    std::fs::create_dir_all(ups.join("collector")).unwrap();
    assert!(ups.join("admin.sock").as_os_str().len() <= max);
    let too_long = format!("longer than a Unix socket path can be ({max} bytes at most)");
    for (data, socket) in [
        (&long, long.join("admin.sock")),
        (&ups, ups.join("collector").join("admin.sock")),
    ] {
        assert!(socket.as_os_str().len() > max);
        let out = admin(data, &["hosts"]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{stderr}");
        assert!(stderr.contains(&too_long), "{stderr}");
        assert!(stderr.contains(&socket.display().to_string()), "{stderr}");
        let out = admin_on_a_terminal(data, &["reset-password"], "a new long password\n");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{stderr}");
        assert!(stderr.contains(&too_long), "{stderr}");
        assert!(!stderr.contains("New password"), "it prompted first: {stderr}");
    }
}

/// `hennery admin pairing-code`, confirmed on a terminal, prints a code
/// that `host join` pairs with; `hennery admin hosts` then lists the host,
/// also given `hennery up`'s data directory rather than the collector's.
#[test]
fn a_pairing_code_from_the_admin_socket_pairs_a_host() {
    let dir = scratch_dir("admincode");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (_collector, listen) = collector_on(&data, &dir.join("collector.log"));
    let out = admin_on_a_terminal(&data, &["pairing-code"], "yes\n");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(contains_a_pairing_code_shape(&code), "{code}");
    let join = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "join", &format!("http://{listen}"), &code, "--name", "laptop"])
        .arg("--data-dir")
        .arg(dir.join("host"))
        .output()
        .unwrap();
    assert!(join.status.success(), "{}", String::from_utf8_lossy(&join.stderr));
    let out = admin(&dir, &["hosts"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let listed = String::from_utf8_lossy(&out.stdout);
    assert!(listed.contains("\tlaptop\t") && listed.contains("\tpaired"), "{listed}");
}

/// `hennery collector healthcheck` (distribution spec §1, §4.1), as the
/// image's `HEALTHCHECK` runs it: the address from `--listen` or
/// `HENNERY_LISTEN` (the image sets the variable), with `HENNERY_DATA_DIR`
/// set too. 0 while the collector serves, 1 once it is gone.
#[test]
fn the_healthcheck_passes_while_the_collector_serves_and_fails_once_it_stops() {
    let dir = scratch_dir("healthcheck");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let log = dir.join("collector.log");
    let collector = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["collector", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(&data)
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut collector = KillTree::new(collector, &log);
    let address = collector.listening();
    let healthcheck = |how: &dyn Fn(&mut Command)| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
        command
            .args(["collector", "healthcheck"])
            .env_remove("HENNERY_LISTEN")
            .env_remove("HENNERY_DATA_DIR");
        how(&mut command);
        command.output().unwrap()
    };
    let by_flag = |command: &mut Command| {
        command.arg("--listen").arg(&address);
    };
    let by_environment = |command: &mut Command| {
        command.env("HENNERY_LISTEN", &address).env("HENNERY_DATA_DIR", &data);
    };
    for (how, set) in [
        ("--listen", &by_flag as &dyn Fn(&mut Command)),
        ("HENNERY_LISTEN", &by_environment),
    ] {
        let out = healthcheck(set);
        assert!(
            out.status.success(),
            "{how}: {:?} {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    drop(collector);
    for (how, set) in [
        ("--listen", &by_flag as &dyn Fn(&mut Command)),
        ("HENNERY_LISTEN", &by_environment),
    ] {
        let out = healthcheck(set);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{how}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("unhealthy"),
            "{how}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// `collector` still needs its data directory; `collector healthcheck`
/// takes none of the collector's own flags.
#[test]
fn the_collector_without_a_data_dir_or_with_the_healthcheck_and_its_flags_is_refused() {
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args(args)
            .env_remove("HENNERY_DATA_DIR")
            .env_remove("HENNERY_LISTEN")
            .output()
            .unwrap()
    };
    let out = run(&["collector"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--data-dir"));
    let out = run(&["collector", "--data-dir", "/nonexistent", "healthcheck"]);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
}

/// `host.lock` (distribution spec §8): while `up`'s host child runs on
/// `<data>/host`, a second `hennery host run` there refuses to start, names
/// the holder's pid, and touches nothing first: not even a pairing left
/// half-done (`host.key.pending`, `host.toml.pending`), which reading the
/// pairing would roll forward over the first host's. The first host keeps
/// the lock, and stays connected.
#[test]
fn a_second_host_on_one_data_directory_refuses_to_start() {
    let dir = scratch_dir("hostlock");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let (mut up, listen, ids, session) = up_until_connected(&data, &dir.join("up.log"), None);
    let lock = data.join("host").join("host.lock");
    let holder = std::fs::read_to_string(&lock).unwrap();
    let holder = holder.trim().to_string();
    assert!(holder.parse::<i32>().is_ok_and(pid_alive), "{holder:?}");
    // A pairing staged and not yet put in place, as a crash in the middle
    // of a join leaves one: whoever reads the pairing renames both files
    // into place. The second host must refuse before it reads it.
    let key = hennery_host::identity::HostKey::generate();
    let key_pending = data.join("host").join("host.key.pending");
    key.save(&key_pending).unwrap();
    let pending = data.join("host").join("host.toml.pending");
    let staged = format!(
        "collector = \"ws://127.0.0.1:9/api/hosts/ws\"\nhost_id = \"host-staged\"\npublic_key = \"{}\"\n",
        key.public_key_hex()
    );
    std::fs::write(&pending, &staged).unwrap();

    let mut second = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "run"])
        .arg("--data-dir")
        .arg(data.join("host"))
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(dir.join("second.err")).unwrap())
        .spawn()
        .unwrap();
    let status = wait_with_timeout(&mut second, Duration::from_secs(20));
    if status.is_none() {
        let _ = second.kill();
        let _ = second.wait();
    }
    let stderr = std::fs::read_to_string(dir.join("second.err")).unwrap();
    let status = status.unwrap_or_else(|| panic!("a second host kept running: {stderr}"));
    assert!(!status.success(), "{stderr}");
    assert!(
        stderr.contains("host.lock") && stderr.contains(&format!("pid {holder}")),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&pending).unwrap(), staged);
    assert!(key_pending.exists(), "the staged key was put in place");
    std::fs::remove_file(&pending).unwrap();
    std::fs::remove_file(&key_pending).unwrap();
    up.assert_running("the first host");
    assert_eq!(std::fs::read_to_string(&lock).unwrap().trim(), holder);
    assert!(holder.parse::<i32>().is_ok_and(pid_alive), "{holder:?}");
    let hosts = get_json(&listen, "/api/hosts", &session).unwrap();
    assert_eq!(hosts[0]["host_id"], ids[0].as_str());
    assert_eq!(hosts[0]["connected"], true, "{hosts}");
}

/// `up.lock`: a second `hennery up` on one data root refuses to start,
/// before it starts any child, and the first keeps serving.
#[test]
fn a_second_up_on_one_data_root_refuses_to_start() {
    let dir = scratch_dir("uplock");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let (mut first, listen, _, session) = up_until_connected(&data, &dir.join("first.log"), None);
    let log = dir.join("second.log");
    let mut second = up_logging_to(&data, &log);
    let status = wait_with_timeout(&mut second.up, Duration::from_secs(20)).expect("the second up kept running");
    let stderr = std::fs::read_to_string(log.with_extension("err")).unwrap();
    assert!(!status.success(), "{stderr}");
    assert!(
        stderr.contains("up.lock") && stderr.contains(&format!("pid {}", first.up.id())),
        "{stderr}"
    );
    first.assert_running("the first up");
    assert!(get_json(&listen, "/api/hosts", &session).is_some());
}

/// `up`'s report of its children (`supervisor.json`), as JSON.
fn supervisor_state(data: &std::path::Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(data.join("supervisor.json")).ok()?).ok()
}

/// Distribution spec §5.2: a child killed outright, once past its start, is
/// started again: the host reconnects with its pairing, and the collector
/// serves on the same port, with the sessions it had.
#[test]
fn a_killed_child_is_started_again() {
    let dir = scratch_dir("restart");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let (mut up, listen, ids, session) = up_until_connected(&data, &dir.join("up.log"), None);
    let up_pid = up.up.id() as i32;
    // Past the startup grace (5 s), counted from now, when both children
    // run already: the kill counts as a crash, not as a failed start.
    std::thread::sleep(Duration::from_secs(6));
    let lock = data.join("host").join("host.lock");
    let host = pid_from(&lock).expect("the host's pid");
    // Both children, as `pgrep` sees them: polled, not read once.
    let mut collector = 0;
    up.wait_until("up's two children", || {
        let children = children_of(up_pid);
        collector = children.iter().copied().find(|&pid| pid != host).unwrap_or(0);
        children.len() == 2 && children.contains(&host) && collector != 0
    });
    up.children = vec![host, collector];

    unsafe { libc::kill(host, libc::SIGKILL) };
    let mut again = 0;
    up.wait_until("the host started again", || {
        again = pid_from(&lock).unwrap_or(host);
        again != host && pid_alive(again)
    });
    up.children.push(again);
    up.wait_until("the host reported running again", || {
        supervisor_state(&data).is_some_and(|s| s["host"]["restarts"] == 1 && s["host"]["state"] == "running")
    });
    up.wait_until("the host connected again", || {
        get_json(&listen, "/api/hosts", &session).is_some_and(|hosts| {
            hosts.as_array().is_some_and(|hosts| {
                hosts.len() == 1 && hosts[0]["host_id"] == ids[0].as_str() && hosts[0]["connected"] == true
            })
        })
    });

    unsafe { libc::kill(collector, libc::SIGKILL) };
    up.wait_until("the collector reported running again", || {
        supervisor_state(&data).is_some_and(|s| s["collector"]["restarts"] == 1 && s["collector"]["state"] == "running")
    });
    up.children.extend(children_of(up_pid));
    // The same port, and the session from before the crash.
    up.wait_until("the collector serving again", || {
        get_json(&listen, "/api/hosts", &session).is_some_and(|hosts| hosts[0]["connected"] == true)
    });
    let state = supervisor_state(&data).unwrap();
    assert_eq!(state["pid"], up_pid, "{state}");
    assert_eq!(state["host"]["last_exit"], "signal: 9 (SIGKILL)", "{state}");
}

/// `hennery service …` in a scratch home: HOME and the XDG directories in
/// `dir`, and PATH starting with stand-ins for `launchctl`, `systemctl` and
/// `loginctl` that only record that they ran. Every command these tests
/// run refuses before it would reach the service manager, and the
/// stand-ins prove it.
fn service(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;
    let stubs = dir.join("stubs");
    std::fs::create_dir_all(&stubs).unwrap();
    for name in ["launchctl", "systemctl", "loginctl"] {
        let stub = stubs.join(name);
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\necho {name} \"$@\" >> \"{}\"\nexit 1\n",
                dir.join("ran").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("service")
        .args(args)
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("PATH", format!("{}:/usr/bin:/bin", stubs.display()))
        .output()
        .unwrap()
}

/// The service file of `role` in `service`'s scratch home.
fn service_file(dir: &std::path::Path, role: &str) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        dir.join("home/Library/LaunchAgents")
            .join(format!("dev.hennery.{role}.plist"))
    } else {
        let unit = match role {
            "up" => "hennery.service".to_string(),
            other => format!("hennery-{other}.service"),
        };
        dir.join("config/systemd/user").join(unit)
    }
}

/// `--role host` on a data directory with no pairing, and a second role
/// beside an installed one, are refused before anything is written or run.
#[test]
fn service_install_refuses_an_unpaired_host_and_a_second_role() {
    let dir = scratch_dir("svcinstall");
    let _cleanup = RemoveDir(dir.clone());
    let empty = dir.join("empty");
    let empty = empty.to_str().unwrap();
    let out = service(&dir, &["install", "--role", "host", "--data-dir", empty]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("holds no pairing"), "{stderr}");
    assert!(!service_file(&dir, "host").exists());

    let up = service_file(&dir, "up");
    std::fs::create_dir_all(up.parent().unwrap()).unwrap();
    std::fs::write(&up, "").unwrap();
    let out = service(&dir, &["install", "--role", "collector"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("uninstall --role up"), "{stderr}");
    assert!(!service_file(&dir, "collector").exists());
    assert!(
        !dir.join("ran").exists(),
        "{:?}",
        std::fs::read_to_string(dir.join("ran"))
    );
}

#[test]
fn service_help_lists_install_uninstall_and_status() {
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["service", "--help"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    for command in ["install", "uninstall", "status"] {
        assert!(text.contains(command), "{text}");
    }
}

/// Nothing installed: `status` says so and exits 1, `uninstall` has nothing
/// to do; neither asks the service manager.
#[test]
fn service_status_with_nothing_installed_fails_without_asking_the_manager() {
    let dir = scratch_dir("svcstatus");
    let _cleanup = RemoveDir(dir.clone());
    let out = service(&dir, &["status"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("no hennery service is installed"));
    let out = service(&dir, &["uninstall"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        !dir.join("ran").exists(),
        "{:?}",
        std::fs::read_to_string(dir.join("ran"))
    );
}
