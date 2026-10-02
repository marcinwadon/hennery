//! The programs doctor runs, bounded: each in a process group of its own,
//! killed whole when it is done, out of time, or doctor itself is stopped
//! (`kill_all`), with an environment doctor builds rather than its own and
//! the home directory as their working directory (decision 14). What they
//! print is parsed by the check that ran them, never printed or logged.

use super::Doctor;
use crate::service::Ran;
use hennery_host::AgentCommand;
use std::collections::BTreeSet;
use std::io::{BufRead, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

/// The most of each output read.
pub const MAX_OUTPUT: u64 = 4096;

/// How long an adapter has to answer `initialize` (distribution §7, check 3).
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(20);

/// How long `auth status` / `login status`, or a CLI's `--version`, may
/// take (checks 4, 13, 17). A CLI's first run after an install can wait
/// on the machine's scan of a new executable.
pub const CLI_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the output of a program that has exited is waited for.
const DRAIN: Duration = Duration::from_secs(5);

/// The environment a host run by the installed service gives its agents
/// (decision 14): what launchd and systemd give a user service (`HOME`,
/// `USER`, `LOGNAME`, `SHELL`), on Linux the user manager's
/// `XDG_RUNTIME_DIR` and `DBUS_SESSION_BUS_ADDRESS` (a keyring-backed
/// login needs them), and the service's PATH (or with no service doctor's
/// own). Built, not inherited; and `command` drops what the host's spawn
/// drops (`stripped`).
pub fn agent_env(doctor: &Doctor) -> Vec<(String, String)> {
    let cx = doctor.cx;
    let linux = cx.platform == crate::service::Platform::Linux;
    let kept: &[&str] = if linux {
        &["HOME", "USER", "LOGNAME", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"]
    } else {
        &["HOME", "USER", "LOGNAME"]
    };
    let mut env: Vec<(String, String)> = kept
        .iter()
        .filter_map(|name| cx.env.get(*name).map(|v| (name.to_string(), v.clone())))
        .collect();
    env.push(("SHELL".to_string(), cx.shell.display().to_string()));
    let path = super::service::service_path_of(doctor)
        .or_else(|| cx.env.get("PATH").cloned())
        .unwrap_or_else(|| "/usr/bin:/bin".to_string());
    env.push(("PATH".to_string(), path));
    env
}

/// What a host never passes to an agent, even when the agent's own
/// configuration names it (`Adapter::spawn`): the nesting variables, the
/// host's secrets, and how a service-run host picks its log.
fn stripped(name: &str) -> bool {
    use hennery_host::adapter::{HOST_LOG_VARS, HOST_SECRET_VARS, NESTING_VARS};
    NESTING_VARS
        .iter()
        .chain(HOST_SECRET_VARS)
        .chain(HOST_LOG_VARS)
        .any(|v| *v == name)
}

/// The process groups doctor has started and not yet killed; `None` once
/// `kill_all` ran, after which nothing more is started.
static GROUPS: Mutex<Option<BTreeSet<libc::pid_t>>> = Mutex::new(Some(BTreeSet::new()));

/// SIGKILL every group doctor started that is still there: when doctor is
/// interrupted, since its children, in groups of their own, do not see the
/// terminal's Ctrl-C. Any later spawn is refused.
pub fn kill_all() {
    let groups = GROUPS.lock().unwrap_or_else(|e| e.into_inner()).take();
    for pgid in groups.into_iter().flatten() {
        // SAFETY: kill(2) of a group doctor made (`process_group(0)`).
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
}

/// Spawn `cmd`, its group registered for `kill_all`; refused once
/// `kill_all` ran.
fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    let mut groups = GROUPS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(groups) = groups.as_mut() else {
        return Err(std::io::ErrorKind::Interrupted.into());
    };
    let child = cmd.spawn()?;
    if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
        groups.insert(pgid);
    }
    Ok(child)
}

/// SIGKILL `child`'s whole process group, then reap it.
fn kill_group(child: &mut Child) {
    if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill(2) of the group this function's caller made the
        // child the leader of (`process_group(0)`).
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
        if let Some(groups) = GROUPS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            groups.remove(&pgid);
        }
    }
    let _ = child.wait();
}

/// `program args` in a group of its own, with only `env` (less what a
/// host strips), in its `HOME` (never doctor's working directory: a
/// repository there could configure the CLI with code of its own).
fn command(program: &Path, args: &[String], env: &[(String, String)]) -> Command {
    let home = env
        .iter()
        .find(|(k, _)| k == "HOME")
        .map(|(_, v)| v.as_str())
        .unwrap_or("/");
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env_clear()
        .envs(env.iter().filter(|(k, _)| !stripped(k)).cloned())
        .current_dir(home)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// Read at most `MAX_OUTPUT` bytes of `from` on a thread of its own.
fn reader(from: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = from.take(MAX_OUTPUT).read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    rx
}

/// Run `program args` with `env`, killed with its whole group after
/// `timeout`, and in any case once it has exited (so nothing it started is
/// left holding its pipes). `output`: its standard output and error are
/// read, at most `MAX_OUTPUT` bytes each; else they go nowhere. `None` when
/// it could not be started or did not finish in time.
pub fn run(program: &Path, args: &[&str], env: &[(String, String)], timeout: Duration, output: bool) -> Option<Ran> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let mut cmd = command(program, &args, env);
    if output {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let mut child = spawn(&mut cmd).ok()?;
    let streams = output.then(|| {
        (
            reader(child.stdout.take().expect("piped")),
            reader(child.stderr.take().expect("piped")),
        )
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => break None,
        }
    };
    kill_group(&mut child);
    let status = status?;
    let (stdout, stderr) = match streams {
        // The group is gone, so each pipe reaches its end once the reader
        // has drained it; only a process that left the group could hold one.
        Some((out, err)) => (
            out.recv_timeout(DRAIN).unwrap_or_default(),
            err.recv_timeout(DRAIN).unwrap_or_default(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    Some(Ran {
        ok: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// What an adapter answered `initialize` with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// It answered; its `agentInfo.version`, if it gave one.
    Answered(Option<String>),
    /// It did not: why, in words that quote nothing it printed.
    Failed(String),
}

/// The `initialize` doctor sends: the protocol version and no capability.
const INITIALIZE: &str =
    r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#;

/// The most lines read waiting for `initialize`'s answer.
const MAX_LINES: usize = 1000;

/// Start `agent` as a host would, with `env` and its own variables, send
/// `initialize`, and wait for the answer within `timeout` (distribution
/// §7, check 3). Then its whole group is killed: no session is opened, no
/// prompt sent, no authentication asked for.
pub fn initialize(agent: &AgentCommand, env: &[(String, String)], timeout: Duration) -> Started {
    let mut all = env.to_vec();
    all.extend(agent.env.iter().cloned());
    let mut cmd = command(Path::new(&agent.program), &agent.args, &all);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = match spawn(&mut cmd) {
        Ok(child) => child,
        Err(err) => return Started::Failed(format!("it cannot be started: {}", err.kind())),
    };
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().expect("piped");
    let lines = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout.take(MAX_OUTPUT * 256))
            .lines()
            .take(MAX_LINES)
        {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdin = child.stdin.take().expect("piped");
    let sent = stdin
        .write_all(format!("{INITIALIZE}\n").as_bytes())
        .and_then(|()| stdin.flush());
    let deadline = Instant::now() + timeout;
    let started = if sent.is_err() {
        Started::Failed("it exited before it read `initialize`".to_string())
    } else {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(line) => {
                    let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    if message.get("id") != Some(&serde_json::json!(0)) {
                        continue;
                    }
                    break match message.get("result") {
                        Some(result) => Started::Answered(
                            result
                                .pointer("/agentInfo/version")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                        ),
                        None => Started::Failed("it answered `initialize` with an error".to_string()),
                    };
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    break Started::Failed(format!("it did not answer `initialize` within {timeout:?}"));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Started::Failed("it exited without answering `initialize`".to_string());
                }
            }
        }
    };
    drop(stdin);
    kill_group(&mut child);
    let _ = lines.join();
    started
}

/// The first `x.y.z` in `text`, as numbers.
pub fn version_in(text: &str) -> Option<(u64, u64, u64)> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find_map(|word| {
            let mut parts = word.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            let patch = parts.next()?.parse().ok()?;
            Some((major, minor, patch))
        })
}

/// A large gap between two CLI versions (checks 13, 17): another major, or
/// another minor.
pub fn far_apart(a: (u64, u64, u64), b: (u64, u64, u64)) -> bool {
    a.0 != b.0 || a.1 != b.1
}

/// A CLI as a host's agent runs it: a program, and the arguments before
/// its own (`node <set>/codex/…/codex.js`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
}

impl Cli {
    pub fn plain(program: impl Into<std::path::PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    /// Run it with `extra` after its own arguments, within `CLI_TIMEOUT`,
    /// with `env` and its auto-updater off (a CLI doctor runs installs
    /// nothing).
    pub fn run(&self, extra: &[&str], env: &[(String, String)], output: bool) -> Option<Ran> {
        let mut args: Vec<&str> = self.args.iter().map(String::as_str).collect();
        args.extend_from_slice(extra);
        let mut env = env.to_vec();
        env.push(("DISABLE_AUTOUPDATER".to_string(), "1".to_string()));
        run(&self.program, &args, &env, CLI_TIMEOUT, output)
    }

    /// What the report calls it.
    pub fn shown(&self) -> String {
        std::iter::once(self.program.display().to_string())
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// `cli --version`, with `env`: the version it prints.
pub fn cli_version(cli: &Cli, env: &[(String, String)]) -> Option<(u64, u64, u64)> {
    let ran = cli.run(&["--version"], env, true)?;
    if !ran.ok {
        return None;
    }
    version_in(&ran.stdout)
}
