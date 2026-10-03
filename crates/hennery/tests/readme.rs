//! The README's shell examples (plan 4f). Every one is a fenced block with
//! a name (`packaging/readme-blocks.sh`), and every name is run: here, or by
//! a workflow, which these tests check names it.
//!
//! The blocks run here run as written, through `sh -c`, with this build's
//! binary first on `PATH` as `hennery`, each in a scratch home of its own
//! (a machine of its own), never the real one. Only these differ from a
//! tester's machine, and the plan records why:
//! - `HENNERY_LISTEN=127.0.0.1:0`: `up` listens on a free port, not 7117;
//! - the adapter mirrors point at a port nothing listens on, so nothing is
//!   downloaded: `host join` pairs and then fails on the download;
//! - the join block's example address and code are replaced by the
//!   collector's loopback address and a code minted for the test.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where the managed runtime would be fetched from: nothing listens there.
const OFFLINE: &str = "http://127.0.0.1:1/";
/// The owner's password at setup.
const PASSWORD: &str = "correct horse battery";
/// The password `reset-password` sets.
const NEW_PASSWORD: &str = "a new password, typed twice";
/// The README's example public address.
const EXAMPLE_URL: &str = "https://hennery.example";
/// The README's example pairing code.
const EXAMPLE_CODE: &str = "ABCD-EFGH";

/// Every block name in the README, in order, with what runs it.
const BLOCKS: [(&str, Runner); 9] = [
    ("clone", Runner::Workflow("ci.yml")),
    ("build", Runner::Workflow("ci.yml")),
    ("nix", Runner::Workflow("nix.yml")),
    ("up", Runner::Here),
    ("up-public", Runner::Here),
    ("join", Runner::Here),
    ("host-run", Runner::Here),
    ("reset-password", Runner::Here),
    ("reset-public-url", Runner::Here),
];

#[derive(Clone, Copy, Debug)]
enum Runner {
    /// A test in this file.
    Here,
    /// This workflow, through `readme-blocks.sh README.md <name>`.
    Workflow(&'static str),
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `readme-blocks.sh <file> <arg>`: its exit status, output and errors.
fn blocks(file: &Path, arg: &str) -> (Option<i32>, String, String) {
    let out = Command::new("sh")
        .arg(repo().join("packaging/readme-blocks.sh"))
        .arg(file)
        .arg(arg)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// The README's block `name`.
fn block(name: &str) -> String {
    let (status, out, err) = blocks(&repo().join("README.md"), name);
    assert_eq!(status, Some(0), "{err}");
    out
}

/// A scratch file holding `text`, removed on drop.
struct Fixture(PathBuf);

impl Fixture {
    fn new(text: &str) -> Self {
        let file = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
        let (_, path) = file.keep().unwrap();
        std::fs::write(&path, text).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `readme-blocks.sh` on a file holding `text`.
fn blocks_of(text: &str, arg: &str) -> (Option<i32>, String, String) {
    let fixture = Fixture::new(text);
    blocks(&fixture.0, arg)
}

/// The script's refusal of `text`: exit 1, and `message` said.
fn refused(text: &str, message: &str) {
    let (status, out, err) = blocks_of(text, "--list");
    assert_eq!(status, Some(1), "out: {out}\nerr: {err}");
    assert!(err.contains(message), "{err}");
}

#[test]
fn a_named_block_is_printed_without_its_indent() {
    let text = "# t\n\n- a step:\n\n  <!-- check: one -->\n  ```sh\n  echo one\n    echo nested\n  ```\n\n<!-- check: two -->\n```bash\necho two\n```\n";
    assert_eq!(
        blocks_of(text, "one"),
        (Some(0), "echo one\n  echo nested\n".into(), String::new())
    );
    assert_eq!(blocks_of(text, "two"), (Some(0), "echo two\n".into(), String::new()));
    assert_eq!(blocks_of(text, "--list"), (Some(0), "one\ntwo\n".into(), String::new()));
}

#[test]
fn a_block_in_another_language_needs_no_tag() {
    let text = "```toml\nkey = 1\n```\n\n<!-- check: one -->\n```sh\necho one\n```\n";
    assert_eq!(blocks_of(text, "--list"), (Some(0), "one\n".into(), String::new()));
}

#[test]
fn a_shell_block_without_a_tag_is_refused() {
    refused(
        "text\n\n```sh\necho hi\n```\n",
        "a shell block without a <!-- check: <name> --> line",
    );
    // Every shell info string, and none, is a shell block.
    for info in ["", "bash", "shell", "console", "zsh"] {
        refused(&format!("```{info}\necho hi\n```\n"), "a shell block without");
    }
}

#[test]
fn a_tag_must_be_just_before_a_shell_block() {
    refused(
        "<!-- check: one -->\n\n```sh\necho one\n```\n",
        "the tag one is not followed by a shell block",
    );
    refused("<!-- check: one -->\n", "the tag one is not followed by a shell block");
    refused(
        "<!-- check: one -->\n```toml\nk = 1\n```\n",
        "the tag one is on a block in toml",
    );
}

#[test]
fn a_name_used_twice_is_refused() {
    refused(
        "<!-- check: one -->\n```sh\necho a\n```\n<!-- check: one -->\n```sh\necho b\n```\n",
        "the name one is used twice",
    );
}

#[test]
fn a_block_never_closed_is_refused() {
    refused("<!-- check: one -->\n```sh\necho a\n", "a block never closed");
}

#[test]
fn a_tilde_fence_is_refused() {
    refused("~~~sh\necho a\n~~~\n", "a ~~~ fence");
}

#[test]
fn a_missing_name_is_refused() {
    let (status, out, err) = blocks_of("<!-- check: one -->\n```sh\necho a\n```\n", "two");
    assert_eq!((status, out.as_str()), (Some(1), ""));
    assert!(err.contains("no block named two"), "{err}");
}

#[test]
fn wrong_arguments_and_an_unreadable_file_exit_2() {
    let out = Command::new("sh")
        .arg(repo().join("packaging/readme-blocks.sh"))
        .arg(repo().join("README.md"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
    let (status, _, err) = blocks(Path::new("/nonexistent-hennery-test/README.md"), "--list");
    assert_eq!(status, Some(2));
    assert!(err.contains("cannot read"), "{err}");
}

/// Every block of the README has a runner: these tests, or a workflow
/// that runs the script on it.
#[test]
fn every_readme_block_is_run() {
    let (status, names, err) = blocks(&repo().join("README.md"), "--list");
    assert_eq!(status, Some(0), "{err}");
    let expected: Vec<&str> = BLOCKS.iter().map(|(name, _)| *name).collect();
    assert_eq!(names.lines().collect::<Vec<_>>(), expected);
    for (name, runner) in BLOCKS {
        if let Runner::Workflow(file) = runner {
            let workflow = std::fs::read_to_string(repo().join(".github/workflows").join(file)).unwrap();
            assert!(
                workflow.contains(&format!("readme-blocks.sh README.md {name} ")),
                "{file} does not run the block {name}"
            );
        }
    }
}

// ---------------------------------------------------------------------
// The blocks run here.

/// A scratch machine: a home under `/tmp` (short, for the admin socket's
/// path), removed on drop.
struct Machine {
    home: PathBuf,
}

impl Machine {
    fn new(name: &str) -> Self {
        let home = PathBuf::from(format!("/tmp/hennery-readme-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        Self { home }
    }

    /// The platform's data directory under this home (distribution §8).
    fn data(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home.join("Library/Application Support/hennery")
        } else {
            self.home.join(".local/share/hennery")
        }
    }

    /// `sh -c script` on this machine, with `hennery` on `PATH`.
    fn sh(&self, script: &str) -> Command {
        let bin = Path::new(env!("CARGO_BIN_EXE_hennery")).parent().unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin.to_path_buf()];
        paths.extend(std::env::split_paths(&path));
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script).current_dir(&self.home);
        for (var, _) in std::env::vars_os() {
            if var.to_string_lossy().starts_with("HENNERY_") {
                cmd.env_remove(var);
            }
        }
        cmd.env("PATH", std::env::join_paths(paths).unwrap())
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("CODEX_SQLITE_HOME")
            .env("HENNERY_NPM_REGISTRY", OFFLINE)
            .env("HENNERY_NODE_MIRROR", OFFLINE)
            .env("HENNERY_LISTEN", "127.0.0.1:0");
        cmd
    }

    /// `script` started in a process group of its own, its output in
    /// `<home>/<log>`.
    fn start(&self, script: &str, log: &str) -> Group {
        let log = self.home.join(log);
        let child = self
            .sh(script)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
            .spawn()
            .unwrap();
        Group { child, log }
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// A long-running block's process group: killed on drop.
struct Group {
    child: std::process::Child,
    log: PathBuf,
}

impl Group {
    fn pgid(&self) -> i32 {
        self.child.id() as i32
    }

    fn output(&self) -> String {
        let out = std::fs::read_to_string(&self.log).unwrap_or_default();
        let err = std::fs::read_to_string(self.log.with_extension("err")).unwrap_or_default();
        format!("{out}{err}")
    }

    /// Wait for `what` to hold, failing with the group's output after 60 s.
    fn wait_until(&mut self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done() {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("it exited ({status}) before {what}:\n{}", self.output());
            }
            assert!(Instant::now() < deadline, "no {what} within 60 s:\n{}", self.output());
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The collector's address, from its "collector listening" line.
    fn listening(&mut self) -> String {
        let mut address = String::new();
        let log = self.log.clone();
        self.wait_until("collector listening", || {
            let text = strip_ansi(&complete_lines(&log));
            match text
                .lines()
                .filter(|line| line.contains("collector listening"))
                .find_map(|line| line.split_whitespace().find_map(|f| f.strip_prefix("address=")))
            {
                Some(found) => {
                    address = found.to_string();
                    true
                }
                None => false,
            }
        });
        address
    }

    /// Every process started under this group's shell, as far down as they
    /// go: `up` starts its children in process groups of their own.
    fn processes(&self) -> Vec<i32> {
        let mut all = vec![self.pgid()];
        let mut next = 0;
        while next < all.len() {
            let out = Command::new("pgrep").args(["-P", &all[next].to_string()]).output();
            if let Ok(out) = out {
                all.extend(
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .filter_map(|line| line.trim().parse::<i32>().ok()),
                );
            }
            next += 1;
        }
        all
    }

    /// Whether any of `pids` is still running.
    fn any_alive(pids: &[i32]) -> bool {
        // SAFETY: kill(2) with signal 0 only checks the process exists.
        pids.iter().any(|&pid| unsafe { libc::kill(pid, 0) } == 0)
    }

    /// SIGTERM to the group, as Ctrl-C's SIGINT would be: it and every
    /// process under it must be gone within 20 s.
    fn stop(&mut self) {
        let pids = self.processes();
        assert!(pids.len() > 1, "nothing ran under the shell:\n{}", self.output());
        // SAFETY: kill(2) on this test's own process group.
        unsafe { libc::kill(-self.pgid(), libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let _ = self.child.try_wait();
            if !Self::any_alive(&pids) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "still running 20 s after SIGTERM:\n{}",
                self.output()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Group {
    /// Kill everything under the shell, and wait until it is gone, so that
    /// nothing writes into the machine's home after it is removed.
    fn drop(&mut self) {
        let pids = self.processes();
        for &pid in &pids {
            // SAFETY: kill(2) on processes this test started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        let _ = self.child.wait();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Self::any_alive(&pids) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// The complete lines of `log`: a line still being written could end
/// mid-port.
fn complete_lines(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    match text.rfind('\n') {
        Some(end) => text[..=end].to_string(),
        None => String::new(),
    }
}

/// `line` without terminal colour codes (`ESC [ … m`).
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// One HTTP/1.1 request to `listen`: the status and the whole response.
fn request(listen: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(listen).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {listen}\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    stream.write_all(head.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let status = response.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, response)
}

/// The `hennery_session` cookie a response sets.
fn session_of(response: &str) -> String {
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

/// What the setup page does: the link's token, a password and the origin
/// the browser is at. The owner's session.
fn set_up(listen: &str, setup_url: &str, origin: &str) -> String {
    let token = setup_url.trim_end().rsplit_once('#').unwrap().1;
    let body = serde_json::json!({ "token": token, "password": PASSWORD, "public_url": origin }).to_string();
    let (status, response) = request(listen, "POST", "/api/setup", &[("Origin", origin)], &body);
    assert_eq!(status, 201, "{response}");
    session_of(&response)
}

/// Sign in with `password` from `origin`: the status.
fn log_in(listen: &str, origin: &str, password: &str) -> u16 {
    let body = serde_json::json!({ "password": password }).to_string();
    request(listen, "POST", "/api/auth/login", &[("Origin", origin)], &body).0
}

/// `GET /api/hosts` with `session`: the status and the hosts.
fn hosts(listen: &str, session: &str) -> (u16, Vec<serde_json::Value>) {
    let cookie = format!("hennery_session={session}");
    let (status, response) = request(listen, "GET", "/api/hosts", &[("Cookie", &cookie)], "");
    let body = response.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    let hosts = serde_json::from_str::<Vec<serde_json::Value>>(body).unwrap_or_default();
    (status, hosts)
}

/// The setup link `up` wrote, once it has.
fn setup_url(group: &mut Group, machine: &Machine) -> String {
    let file = machine.data().join("collector/setup-url");
    let mut url = String::new();
    group.wait_until("the setup link", || {
        url = std::fs::read_to_string(&file).unwrap_or_default();
        url.contains('#')
    });
    url.trim_end().to_string()
}

/// Run `script` with a terminal on its standard input and error, as a
/// person would: each `(prompt, answer)` answered once `prompt` shows. Its
/// exit status, its standard output, and what the terminal showed.
fn on_a_terminal(machine: &Machine, script: &str, answers: &[(&str, &str)]) -> (ExitStatus, String, String) {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty(3) into two descriptors, with no name, settings or size.
    let made = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(made, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: both descriptors were just opened, and are owned here alone.
    let (master, slave) = unsafe { (std::fs::File::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let mut child = machine
        .sh(script)
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let shown = Arc::new(Mutex::new(String::new()));
    let reader = {
        let shown = shown.clone();
        let mut master = master.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = master.read(&mut buf) {
                if n == 0 {
                    break;
                }
                shown.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        })
    };
    let mut writer = master;
    let mut seen = 0;
    for (prompt, answer) in answers {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let text = shown.lock().unwrap().clone();
            if let Some(at) = text[seen..].find(prompt) {
                seen += at + prompt.len();
                break;
            }
            if let Ok(Some(status)) = child.try_wait() {
                panic!("it exited ({status}) before asking {prompt:?}:\n{text}");
            }
            assert!(Instant::now() < deadline, "no {prompt:?} within 30 s:\n{text}");
            std::thread::sleep(Duration::from_millis(50));
        }
        writer.write_all(format!("{answer}\n").as_bytes()).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("still running after 30 s:\n{}", shown.lock().unwrap());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stdout = String::new();
    child.stdout.take().unwrap().read_to_string(&mut stdout).unwrap();
    drop(writer);
    // The reader ends at the terminal's end, once no process holds it;
    // not joined, so a descriptor some grandchild kept cannot hang the test.
    drop(reader);
    let shown = shown.lock().unwrap().clone();
    (status, stdout, shown)
}

/// Quickstart step 2 and the recovery commands: `hennery up` writes a setup
/// link that sets the collector up; `reset-password` sets a password that
/// signs in where the old one no longer does; `reset-public-url` moves the
/// origin sign-ins are accepted from.
#[test]
fn up_and_the_recovery_commands_work_as_the_readme_says() {
    let machine = Machine::new("up");
    let mut up = machine.start(&block("up"), "up.log");
    let listen = up.listening();
    let link = setup_url(&mut up, &machine);
    // The link's origin is the browser's, and the one sign-ins come from.
    let port = listen.rsplit_once(':').unwrap().1;
    let origin = format!("http://localhost:{port}");
    assert!(link.starts_with(&format!("{origin}/setup#")), "{link}");
    let session = set_up(&listen, &link, &origin);
    assert_eq!(hosts(&listen, &session).0, 200);

    let (status, stdout, shown) = on_a_terminal(
        &machine,
        &block("reset-password"),
        &[
            ("New password: ", NEW_PASSWORD),
            ("The same again: ", NEW_PASSWORD),
            ("Type yes to go on: ", "yes"),
        ],
    );
    assert!(status.success(), "{status}: {stdout}\n{shown}");
    assert!(
        stdout.starts_with("The password is reset; 1 session(s) signed out"),
        "{stdout}"
    );
    assert!(!shown.contains(NEW_PASSWORD), "the password was echoed: {shown}");
    assert_eq!(hosts(&listen, &session).0, 401, "the old session still works");
    assert_eq!(log_in(&listen, &origin, PASSWORD), 401);
    assert_eq!(log_in(&listen, &origin, NEW_PASSWORD), 204);

    let script = block("reset-public-url");
    assert!(script.contains(EXAMPLE_URL), "{script}");
    let (status, stdout, shown) = on_a_terminal(&machine, &script, &[("Type yes to go on: ", "yes")]);
    assert!(status.success(), "{status}: {stdout}\n{shown}");
    assert!(
        stdout.starts_with(&format!("public_url is now {EXAMPLE_URL};")),
        "{stdout}"
    );
    assert_eq!(log_in(&listen, EXAMPLE_URL, NEW_PASSWORD), 204);
    assert_eq!(
        log_in(&listen, &origin, NEW_PASSWORD),
        403,
        "the old origin is still accepted"
    );

    up.stop();
}

/// Quickstart step 3: `up --public-url` sets up at that address, and the
/// join block, given the collector's address and a code from "Add host",
/// pairs another machine whose `host run` then connects.
#[test]
fn pairing_another_machine_works_as_the_readme_says() {
    let collector = Machine::new("collector");
    let script = block("up-public");
    assert!(script.contains(EXAMPLE_URL), "{script}");
    let mut up = collector.start(&script, "up.log");
    let listen = up.listening();
    let link = setup_url(&mut up, &collector);
    assert!(link.starts_with(&format!("{EXAMPLE_URL}/setup#")), "{link}");
    let session = set_up(&listen, &link, EXAMPLE_URL);
    up.wait_until("up's own host", || hosts(&listen, &session).1.len() == 1);

    // "Add host": what the Hosts screen mints, just after setup.
    let cookie = format!("hennery_session={session}");
    let (status, response) = request(
        &listen,
        "POST",
        "/api/hosts/pairing-codes",
        &[("Origin", EXAMPLE_URL), ("Cookie", &cookie)],
        "",
    );
    assert_eq!(status, 201, "{response}");
    let body = response.split_once("\r\n\r\n").unwrap().1;
    let code = serde_json::from_str::<serde_json::Value>(body).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();

    let other = Machine::new("other");
    let join = block("join");
    assert_eq!(join.matches(EXAMPLE_URL).count(), 1, "{join}");
    assert_eq!(join.matches(EXAMPLE_CODE).count(), 1, "{join}");
    let join = join
        .replace(EXAMPLE_URL, &format!("http://{listen}"))
        .replace(EXAMPLE_CODE, &code);
    let out = other.sh(&join).stdin(Stdio::null()).output().unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // Paired; then, offline, the adapters' download fails, and only that.
    assert!(stdout.contains("paired as host-"), "{stdout}\n{stderr}");
    assert_eq!(out.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(stderr.contains("but the adapter runtime was not installed"), "{stderr}");

    let mut host = other.start(&block("host-run"), "host.log");
    host.wait_until("both hosts connected", || {
        let listed = hosts(&listen, &session).1;
        listed.len() == 2 && listed.iter().all(|h| h["connected"] == true)
    });
    host.stop();
    up.stop();
}
