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
//! - only the join block's explicitly marked port and code placeholders
//!   are replaced; its documented scheme and host must remain unchanged.
//!   The local listener stands in for the SSH tunnel's forwarded endpoint.

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
/// The README's explicitly marked tunnel-port placeholder.
const EXAMPLE_PORT: u16 = 7117;

/// Every block name in the README, in order, with what runs it.
const BLOCKS: [(&str, Runner); 13] = [
    ("release-bootstrap", Runner::Workflow("ci.yml", "checks")),
    ("collector-public", Runner::Here),
    ("clone", Runner::Workflow("ci.yml", "readme")),
    ("build", Runner::Workflow("ci.yml", "readme")),
    ("nix", Runner::Workflow("nix.yml", "flake")),
    ("build-without-nix", Runner::Workflow("ci.yml", "readme-without-nix")),
    ("up", Runner::Here),
    ("up-public", Runner::Here),
    ("join-public", Runner::Here),
    ("join", Runner::Here),
    ("host-run", Runner::Here),
    ("reset-password", Runner::Here),
    ("reset-public-url", Runner::Here),
];

#[derive(Clone, Copy, Debug)]
enum Runner {
    /// A test in this file.
    Here,
    /// This workflow's job, through `readme-blocks.sh README.md <name>`.
    Workflow(&'static str, &'static str),
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
        .env_remove("README_BLOCKS_AWK")
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

/// Replace only the port and code the README explicitly marks as
/// placeholders. Never repair an unsupported URL by changing its origin.
fn pairing_script(script: &str, readme: &str, port: u16, code: &str) -> String {
    assert!(readme.contains(&format!("`{EXAMPLE_PORT}` is a placeholder")));
    assert!(readme.contains(&format!("`{EXAMPLE_CODE}` is a placeholder")));
    let address = |command: &str| {
        reqwest::Url::parse(command.split_whitespace().nth(3).expect("join URL")).expect("join URL parses")
    };
    let before = address(script);
    assert_eq!(
        before.scheme(),
        "http",
        "pairing substitution must not hide a scheme change"
    );
    assert_eq!(
        before.host_str(),
        Some("127.0.0.1"),
        "pairing substitution must not hide a host change"
    );
    assert_eq!(before.port(), Some(EXAMPLE_PORT));
    let port_placeholder = format!(":{EXAMPLE_PORT}");
    assert_eq!(script.matches(&port_placeholder).count(), 1, "{script}");
    assert_eq!(script.matches(EXAMPLE_CODE).count(), 1, "{script}");
    let substituted = script
        .replace(&port_placeholder, &format!(":{port}"))
        .replace(EXAMPLE_CODE, code);
    let after = address(&substituted);
    assert_eq!(after.scheme(), before.scheme(), "substitution changed the scheme");
    assert_eq!(after.host_str(), before.host_str(), "substitution changed the host");
    substituted
}

#[test]
fn pairing_substitution_only_changes_marked_port_and_code() {
    let readme = std::fs::read_to_string(repo().join("README.md")).unwrap();
    assert_eq!(
        pairing_script(&block("join"), &readme, 12345, "EFGH-IJKL"),
        "hennery host join http://127.0.0.1:12345 EFGH-IJKL\n"
    );
    for unmarked in ["`7117` is an example", "`ABCD-EFGH` is an example"] {
        let original = unmarked.replace("is an example", "is a placeholder");
        let changed = readme.replace(&original, unmarked);
        assert!(std::panic::catch_unwind(|| pairing_script(&block("join"), &changed, 12345, "EFGH-IJKL")).is_err());
    }
}

#[test]
fn pairing_substitution_cannot_hide_a_scheme_change() {
    let readme = std::fs::read_to_string(repo().join("README.md")).unwrap();
    let changed = block("join").replace("http://", "https://");
    assert!(std::panic::catch_unwind(|| pairing_script(&changed, &readme, 12345, "EFGH-IJKL")).is_err());
}

#[test]
fn pairing_substitution_cannot_hide_a_host_change() {
    let readme = std::fs::read_to_string(repo().join("README.md")).unwrap();
    let changed = block("join").replace("127.0.0.1", "localhost");
    assert!(std::panic::catch_unwind(|| pairing_script(&changed, &readme, 12345, "EFGH-IJKL")).is_err());
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
    let text = "# t\n\n- a step:\n\n  <!-- check: one -->\n  ```sh\n  echo one\n    echo nested\n  ```\n\n<!-- check: two -->\n```sh title=x\necho two\n```\n";
    assert_eq!(
        blocks_of(text, "one"),
        (Some(0), "echo one\n  echo nested\n".into(), String::new())
    );
    assert_eq!(blocks_of(text, "two"), (Some(0), "echo two\n".into(), String::new()));
    assert_eq!(blocks_of(text, "--list"), (Some(0), "one\ntwo\n".into(), String::new()));
}

/// The languages that are not shell, in any case, need no tag.
#[test]
fn a_block_in_a_language_that_is_not_shell_needs_no_tag() {
    for info in [
        "toml", "json", "jsonc", "yaml", "yml", "rust", "ts", "tsx", "js", "html", "css", "diff", "mermaid", "TOML",
    ] {
        let text = format!("```{info}\nkey = 1\n```\n\n<!-- check: one -->\n```sh\necho one\n```\n");
        assert_eq!(
            blocks_of(&text, "--list"),
            (Some(0), "one\n".into(), String::new()),
            "{info}"
        );
    }
}

#[test]
fn a_shell_block_without_a_tag_is_refused() {
    refused(
        "text\n\n```sh\necho hi\n```\n",
        "a shell block without a <!-- check: <name> --> line",
    );
}

/// Fail closed: a block neither `sh` nor a language that is not shell is
/// refused, tagged or not.
#[test]
fn any_other_block_is_refused() {
    for info in [
        "",
        "SH",
        "bash",
        "shell",
        "console",
        "zsh",
        "fish",
        "text",
        "shell-session",
        "{.sh}",
    ] {
        refused(&format!("```{info}\necho hi\n```\n"), &format!("a block in \"{info}\""));
        refused(
            &format!("<!-- check: one -->\n```{info}\necho hi\n```\n"),
            &format!("a block in \"{info}\""),
        );
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
fn a_malformed_tag_is_refused() {
    for tag in [
        "<!-- check: --list -->",
        "<!-- check: Up -->",
        "<!--check: up-->",
        "<!-- check:up -->",
    ] {
        refused(&format!("{tag}\n```sh\necho a\n```\n"), "a malformed tag");
    }
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
    refused("> ~~~sh\n> echo a\n> ~~~\n", "a ~~~ fence");
}

#[test]
fn a_fence_in_a_block_quote_is_refused() {
    refused("> ```sh\n> echo a\n> ```\n", "a fence in a block quote");
}

#[test]
fn a_fence_indented_with_a_tab_is_refused() {
    refused("-\n\t```sh\n\techo a\n\t```\n", "a fence indented with a tab");
    refused("-\n  \t```sh\n  \techo a\n  \t```\n", "a fence indented with a tab");
}

#[test]
fn a_fence_of_more_backticks_is_refused() {
    refused("````sh\necho a\n````\n", "a fence of more than three backticks");
}

#[test]
fn a_carriage_return_is_refused() {
    refused("<!-- check: one -->\r\n```sh\r\necho a\r\n```\r\n", "a carriage return");
}

#[test]
fn a_missing_name_is_refused() {
    let (status, out, err) = blocks_of("<!-- check: one -->\n```sh\necho a\n```\n", "two");
    assert_eq!((status, out.as_str()), (Some(1), ""));
    assert!(err.contains("no block named two"), "{err}");
}

#[test]
fn wrong_arguments_exit_2() {
    let out = Command::new("sh")
        .arg(repo().join("packaging/readme-blocks.sh"))
        .arg(repo().join("README.md"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
}

#[test]
fn an_unreadable_file_exits_2() {
    let (status, _, err) = blocks(Path::new("/nonexistent-hennery-test/README.md"), "--list");
    assert_eq!(status, Some(2));
    assert!(err.contains("cannot read"), "{err}");
}

/// awk failing is not mistaken for a broken file (1) or bad arguments (2).
#[test]
fn a_failing_awk_exits_3() {
    let awk = Fixture::new("#!/bin/sh\nexit 2\n");
    std::fs::set_permissions(&awk.0, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let readme = Fixture::new("<!-- check: one -->\n```sh\necho a\n```\n");
    let out = Command::new("sh")
        .arg(repo().join("packaging/readme-blocks.sh"))
        .arg(&readme.0)
        .arg("--list")
        .env("README_BLOCKS_AWK", &awk.0)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("awk failed (exit status 2)"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The repository's workflow jobs use two-space indentation. Limit a
/// runner check to its named job, ignoring comments and other jobs.
fn workflow_job(workflow: &str, job: &str) -> String {
    let header = format!("\n  {job}:\n");
    let (_, body) = workflow
        .split_once(&header)
        .unwrap_or_else(|| panic!("no workflow job {job}"));
    body.lines()
        .take_while(|line| {
            !line.starts_with("  ")
                || line.starts_with("   ")
                || line.trim().is_empty()
                || line.trim_start().starts_with('#')
        })
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every block of the README has a runner: these tests, or a workflow
/// that runs the script on it.
#[test]
fn every_readme_block_is_run() {
    let (status, names, err) = blocks(&repo().join("README.md"), "--list");
    assert_eq!(status, Some(0), "{err}");
    let expected: Vec<&str> = BLOCKS.iter().map(|(name, _)| *name).collect();
    assert_eq!(names.lines().collect::<Vec<_>>(), expected);
    let readme = std::fs::read_to_string(repo().join("README.md")).unwrap();
    pairing_script(&block("join"), &readme, 12345, "EFGH-IJKL");
    let here = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/readme.rs")).unwrap();
    for (name, runner) in BLOCKS {
        if let Runner::Here = runner {
            assert!(
                here.contains(&format!("block(\"{name}\")")),
                "no test here runs the block {name}"
            );
        }
        if let Runner::Workflow(file, job) = runner {
            let workflow = std::fs::read_to_string(repo().join(".github/workflows").join(file)).unwrap();
            let workflow = workflow_job(&workflow, job);
            assert!(
                workflow.contains(&format!("readme-blocks.sh README.md {name} ")),
                "{file}'s {job} job does not run the block {name}"
            );
        }
    }
}

#[test]
fn the_non_nix_build_runs_on_linux_prs_and_both_weekly_platforms_without_nix() {
    let workflow = std::fs::read_to_string(repo().join(".github/workflows/ci.yml")).unwrap();
    let job = workflow_job(&workflow, "readme-without-nix");
    assert!(
        job.contains("os: ${{ fromJSON(needs.checks.outputs.recipe_matrix) }}"),
        "{job}"
    );
    let maintenance = std::fs::read_to_string(repo().join(".github/workflows/nix.yml")).unwrap();
    let weekly = workflow_job(&maintenance, "readme-without-nix");
    assert!(
        weekly.contains("- ubuntu-latest") && weekly.contains("- macos-latest"),
        "{weekly}"
    );
    assert!(weekly.contains("build-without-nix"), "{weekly}");
    assert!(job.contains("runs-on: ${{ matrix.os }}"), "{job}");
    assert!(job.contains("timeout-minutes: 60"), "{job}");
    assert!(job.contains("toolchain: stable"), "{job}");
    assert!(job.contains("node-version: '>=24.22.1 <25'"), "{job}");
    assert!(
        !job.contains("install-nix-action") && !job.contains("nix develop"),
        "{job}"
    );
    // Extraction alone must not count as running the block. Its PATH
    // export and the UI check must execute in the same shell on each OS.
    assert!(
        job.contains("readme-blocks.sh README.md build-without-nix > \"$RUNNER_TEMP/build-without-nix.sh\""),
        "{job}"
    );
    assert!(
        job.contains(
            "cat >> \"$RUNNER_TEMP/build-without-nix.sh\" <<'CHECK'\n          sh packaging/check-web-ui.sh \"$(command -v hennery)\"\n          CHECK"
        ),
        "{job}"
    );
    assert!(job.contains("sh -eux \"$RUNNER_TEMP/build-without-nix.sh\""), "{job}");
}

// ---------------------------------------------------------------------
// The blocks run here.

/// The VM recipe runs without a host, both with its flags and with the
/// example config file. Only the explicitly documented backend port changes.
#[test]
fn collector_public_shell_block_and_example_config_start_without_a_host() {
    hennery_testkit::first_exec(env!("CARGO_BIN_EXE_hennery"));
    let readme = std::fs::read_to_string(repo().join("README.md")).unwrap();
    assert!(readme.contains("`7117` is a collector-backend placeholder"));
    let script = block("collector-public");
    assert_eq!(script.matches("127.0.0.1:7117").count(), 1);
    let example = readme
        .split_once("<!-- example: collector-config -->\n```toml\n")
        .unwrap()
        .1
        .split_once("\n```")
        .unwrap()
        .0;
    for from_file in [false, true] {
        let machine = Machine::new(if from_file { "collector-file" } else { "collector-flags" });
        let script = if from_file {
            std::fs::create_dir_all(machine.data()).unwrap();
            std::fs::write(machine.data().join("config.toml"), example.replace(":7117", ":0")).unwrap();
            "hennery collector".to_string()
        } else {
            script.replace("127.0.0.1:7117", "127.0.0.1:0")
        };
        // No shell override may conceal an invalid example config.
        let script = format!("unset HENNERY_LISTEN; exec {script}");
        let mut collector = machine.start(&script, "collector.log");
        let address = collector.listening();
        let setup = machine.data().join("setup-url");
        collector.wait_until("the collector-only setup link", || setup.exists());
        assert!(
            std::fs::read_to_string(&setup)
                .unwrap()
                .starts_with(&format!("{EXAMPLE_URL}/setup#"))
        );
        assert_eq!(request(&address, "GET", "/healthz", &[], "").0, 200);
        assert!(!machine.data().join("host").exists());
        assert!(!machine.data().join("host.toml").exists());
        collector.stop();
    }
}

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
            .env_remove("SSL_CERT_FILE")
            .env_remove("SSL_CERT_DIR")
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
            // pgrep: 0 found, 1 none; anything else, or no pgrep at all,
            // must not pass as "none" (the children would be left behind).
            let out = Command::new("pgrep")
                .args(["-P", &all[next].to_string()])
                .output()
                .expect("run pgrep");
            assert!(matches!(out.status.code(), Some(0 | 1)), "pgrep failed: {out:?}");
            all.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(|line| line.trim().parse::<i32>().ok()),
            );
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
        let mut pids = self.processes();
        // SAFETY: kill(2) on this test's own process group.
        unsafe { libc::kill(-self.pgid(), libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            // Once the shell is reaped its id may be another's: stop asking.
            if let Ok(Some(_)) = self.child.try_wait() {
                pids.retain(|&pid| pid != self.pgid());
            }
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
    /// Kill everything under the shell while it runs, and wait until it is
    /// gone, so that nothing writes into the machine's home after it is
    /// removed. A shell already reaped (after `stop`, or one that exited)
    /// is left alone: its id may be another's.
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(None)) {
            return;
        }
        let pids = self.processes();
        for &pid in &pids {
            // SAFETY: kill(2) on processes this test started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        let _ = self.child.wait();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Self::any_alive(&pids[1..]) && Instant::now() < deadline {
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

/// The session cookie a response sets, as `name=value` to send back:
/// `__Host-hennery_session` on HTTPS, `hennery_session` on loopback.
fn session_of(response: &str) -> String {
    response
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            let value = value.trim();
            let named = value.starts_with("__Host-hennery_session=") || value.starts_with("hennery_session=");
            (named && name.eq_ignore_ascii_case("set-cookie")).then(|| value.split(';').next().unwrap().to_string())
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
    let (status, response) = request(listen, "GET", "/api/hosts", &[("Cookie", session)], "");
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

/// `cmd`'s output, failing the test if it runs longer than `limit`.
fn output_within(cmd: &mut Command, limit: Duration) -> std::process::Output {
    let child = cmd
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let (done, waited) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(child.wait_with_output());
    });
    match waited.recv_timeout(limit) {
        Ok(out) => out.unwrap(),
        Err(_) => {
            // SAFETY: kill(2) on this test's own process group, still running.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            panic!("still running after {limit:?}");
        }
    }
}

/// A command on a terminal, its process group killed on drop: a test that
/// fails mid-prompt leaves nothing waiting on the terminal.
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // Only while it runs: once reaped, its id may be another's.
        if let Ok(None) = self.0.try_wait() {
            // SAFETY: kill(2) on this test's own process group.
            unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }
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
    let mut child = KillOnDrop(
        machine
            .sh(script)
            .process_group(0)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let child = &mut child.0;
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

/// The admin command calls this time-aware method: a live link is reused,
/// but at its expiry it mints and writes a fresh one without a restart.
#[test]
fn an_expired_setup_link_is_reissued_as_the_readme_says() {
    use hennery_kernel::operator::{Operator, SETUP_TOKEN_TTL_SECS, SetupOutcome};

    let dir = tempfile::tempdir().unwrap();
    let operator = Operator::open(&dir.path().join("hennery.db")).unwrap();
    let origin = "http://localhost:7117";
    let now = 1000;
    let old = operator.announce_setup(dir.path(), origin, now).unwrap().unwrap();
    let expiry = now + SETUP_TOKEN_TTL_SECS;
    let live = operator.setup_link(dir.path(), origin, expiry - 1).unwrap().unwrap();
    assert!(live.url == old.url, "a live link was replaced");
    let fresh = operator.setup_link(dir.path(), origin, expiry).unwrap().unwrap();
    assert!(fresh.url != old.url, "the expired link was reused");
    assert!(
        std::fs::read_to_string(&fresh.file).unwrap().trim_end() == fresh.url,
        "the setup-url file does not hold the fresh link"
    );
    let old_token = old.url.rsplit_once('#').unwrap().1;
    let fresh_token = fresh.url.rsplit_once('#').unwrap().1;
    assert_eq!(
        operator.set_up(old_token, PASSWORD, origin, expiry).unwrap(),
        SetupOutcome::InvalidToken
    );
    assert!(matches!(
        operator.set_up(fresh_token, PASSWORD, origin, expiry).unwrap(),
        SetupOutcome::Done { .. }
    ));
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
    let setup_command = output_within(&mut machine.sh("hennery admin setup-url"), Duration::from_secs(60));
    assert!(setup_command.status.success(), "setup-url command failed");
    assert!(
        String::from_utf8(setup_command.stdout).unwrap().trim_end() == link,
        "setup-url did not print the live link"
    );
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
/// join block, given only a tunnel port and a code from "Add host", pairs
/// another machine whose `host run` reconnects through the saved URL.
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
    let cookie = session.clone();
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
    let readme = std::fs::read_to_string(repo().join("README.md")).unwrap();
    let port = listen.rsplit_once(':').unwrap().1.parse().unwrap();
    let join = pairing_script(&block("join"), &readme, port, &code);
    let out = output_within(other.sh(&join).stdin(Stdio::null()), Duration::from_secs(60));
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
    up.wait_until("the stopped host disconnected", || {
        let listed = hosts(&listen, &session).1;
        listed.len() == 2 && listed.iter().filter(|h| h["connected"] == true).count() == 1
    });
    // A later run reads the saved loopback URL, without a URL override.
    let mut host = other.start(&block("host-run"), "host-restarted.log");
    host.wait_until("the restarted host connected through its saved URL", || {
        let listed = hosts(&listen, &session).1;
        listed.len() == 2 && listed.iter().all(|h| h["connected"] == true)
    });
    host.stop();
    up.stop();
}

/// Every changed byte must be an explicitly documented placeholder; HTTPS
/// cannot be repaired into HTTP by the harness.
fn public_pairing_script(script: &str, readme: &str, port: u16, code: &str) -> String {
    for placeholder in ["`hennery.example`", "`443`", "`ABCD-EFGH`"] {
        assert!(readme.contains(&format!("{placeholder} is a public-join placeholder")));
    }
    assert_eq!(script, "hennery host join https://hennery.example:443 ABCD-EFGH\n");
    script
        .replace("hennery.example:443", &format!("localhost:{port}"))
        .replace("ABCD-EFGH", code)
}
#[test]
fn public_pairing_placeholders_cannot_hide_protocol_or_unmarked_changes() {
    let readme = std::fs::read_to_string(repo().join("README.md")).unwrap();
    let script = block("join-public");
    assert_eq!(
        public_pairing_script(&script, &readme, 12345, "EFGH-IJKL"),
        "hennery host join https://localhost:12345 EFGH-IJKL\n"
    );
    for original in ["https://", "hennery.example", "443", "ABCD-EFGH"] {
        let changed = script.replace(original, "unexpected");
        assert!(std::panic::catch_unwind(|| public_pairing_script(&changed, &readme, 12345, "EFGH-IJKL")).is_err());
    }
    for original in ["`hennery.example`", "`443`", "`ABCD-EFGH`"] {
        let changed = readme.replace(&format!("{original} is a public-join placeholder"), "unmarked");
        assert!(std::panic::catch_unwind(|| public_pairing_script(&script, &changed, 12345, "EFGH-IJKL")).is_err());
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn public_join_shell_block_pairs_runs_and_reconnects_over_wss() {
    hennery_testkit::first_exec(env!("CARGO_BIN_EXE_hennery"));
    for kind in ["caddy", "nginx"] {
        let collector = Machine::new(&format!("public-collector-{kind}"));
        let data = collector.data().join("collector");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("config.toml"), "proxy_client_ip_peers = ['127.0.0.1']\n").unwrap();
        let mut up = collector.start(&block("up-public"), "up.log");
        let listen = up.listening();
        let link = setup_url(&mut up, &collector);
        let session = set_up(&listen, &link, EXAMPLE_URL);
        let cookie = session.clone();
        let (status, response) = request(
            &listen,
            "POST",
            "/api/hosts/pairing-codes",
            &[("Origin", EXAMPLE_URL), ("Cookie", &cookie)],
            "",
        );
        assert_eq!(status, 201, "{response}");
        let code =
            serde_json::from_str::<serde_json::Value>(response.split_once("\r\n\r\n").unwrap().1).unwrap()["code"]
                .as_str()
                .unwrap()
                .to_string();
        let ca = hennery_testkit::tls::TestCa::new();
        let mut proxy =
            hennery_testkit::reverse_proxy::RecipeProxy::start(kind, listen.parse().unwrap(), &ca, &collector.home)
                .await;
        let other = Machine::new(&format!("public-host-{kind}"));
        let roots = other.home.join("fixture-roots.pem");
        std::fs::write(&roots, &ca.pem).unwrap();
        let readme = std::fs::read_to_string(repo().join("README.md")).unwrap();
        let script = public_pairing_script(&block("join-public"), &readme, proxy.port(), &code);
        let mut join = other.sh(&script);
        join.env("SSL_CERT_FILE", &roots);
        let out = tokio::task::spawn_blocking(move || output_within(&mut join, Duration::from_secs(60)))
            .await
            .unwrap();
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("paired as host-"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("but the adapter runtime was not installed"));
        let hostdir = other.data();
        let paired = hennery_host::identity::Paired::read(&hostdir).unwrap().unwrap();
        assert_eq!(
            paired.collector_url,
            format!("wss://localhost:{}/api/hosts/ws", proxy.port())
        );
        let mut host = start_with_roots(&other, &block("host-run"), "host.log", &roots);
        host.wait_until("two hosts connected", || {
            hosts(&listen, &session)
                .1
                .iter()
                .filter(|h| h["connected"] == true)
                .count()
                == 2
        });
        proxy.renew_and_reload(&ca).await;
        host.stop();
        let mut restarted = start_with_roots(&other, &block("host-run"), "host-restarted.log", &roots);
        restarted.wait_until("restarted WSS host", || {
            hosts(&listen, &session)
                .1
                .iter()
                .filter(|h| h["connected"] == true)
                .count()
                == 2
        });
        restarted.stop();
        // Configuration must reach the binary's outer layer, before Origin/auth.
        let (status, _) = request(
            &listen,
            "POST",
            "/api/auth/login",
            &[("Hennery-Client-IP", "not-an-ip")],
            "invalid json",
        );
        assert_eq!(status, 400);
        up.stop();
    }
}

fn start_with_roots(machine: &Machine, script: &str, name: &str, roots: &Path) -> Group {
    let log = machine.home.join(name);
    let child = machine
        .sh(script)
        .env("SSL_CERT_FILE", roots)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    Group { child, log }
}
