//! The host's agents, as it reports them (plan 4d-B1-i; ACP core §6
//! "Agent availability"): the static view in `hello`, and the live one,
//! `probe_agents`.
//!
//! A probe runs a fixed set of read-only checks on the agents this host is
//! configured with, and nothing the collector names: each adapter started
//! as a session's is (`Adapter::spawn`: its own guarded group, the host's
//! stripped environment) and sent `initialize` alone (doctor's check 3),
//! then killed; and each agent's CLI asked whether it is logged in
//! (check 4), of which only the exit status is kept. Which CLI and which
//! question is doctor's knowledge, injected through `AgentChecks`: the
//! binary implements it, this crate never depends on the binary. Every
//! check of every agent runs at once, within one budget.

use crate::adapter::{self, Adapter, AgentCommand};
use hennery_proto::agents::{
    AgentAuth, AgentCli, AgentInfo, RuntimeInfo, RuntimeSource, bound_agents, is_readable_version,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

/// How long one probe takes at most, all its checks together: below the
/// collector's 20 s wait, so an answer can still reach it.
pub const PROBE_BUDGET: Duration = Duration::from_secs(15);

/// What the probe sends each adapter: the protocol version and no
/// capability, as doctor's check 3 does. No session, no prompt, no login.
const INITIALIZE: &str =
    r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#;

/// The most of an adapter's output read waiting for its answer.
const MAX_OUTPUT: u64 = 1 << 20;
/// The most lines read waiting for it.
const MAX_LINES: usize = 1000;

/// Said of an adapter that reached `MAX_OUTPUT` or `MAX_LINES` first.
const TOO_MUCH: &str = "its adapter wrote too much before answering `initialize`";

/// Said of a check that ran out of the probe's budget.
const OUT_OF_TIME: &str = "(on macOS a program's first run can wait on an online check: refresh again)";

/// How to ask an agent's CLI whether it is logged in (doctor's check 4).
#[derive(Debug, Clone)]
pub enum LoginCheck {
    /// Run this fixed command; exit 0 means logged in. Its output is never
    /// read.
    Ask(AgentCommand),
    /// Not asked: `auth` stays `unknown`, with this note, if any.
    NotAsked(Option<String>),
}

/// Doctor's knowledge of the agents, which a probe uses (distribution
/// spec §7): implemented by the binary and given to the host in
/// `HostConfig::checks`. Later checks come as methods with default bodies.
pub trait AgentChecks: Send + Sync + std::fmt::Debug + 'static {
    /// How to ask `agent`'s CLI whether it is logged in. By default no CLI
    /// is known.
    fn login(&self, agent: &str) -> LoginCheck {
        let _ = agent;
        LoginCheck::NotAsked(None)
    }
}

/// No knowledge: every `auth` stays `unknown` (tests, and a host given no
/// checks).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoChecks;

impl AgentChecks for NoChecks {}

/// The runtime of a host given `--agent` commands: nothing managed.
pub fn given_runtime() -> RuntimeInfo {
    RuntimeInfo {
        source: RuntimeSource::Given,
        set_id: None,
        pinned: None,
        held: None,
    }
}

/// The static view (`hello`): the managed runtime's `infos` as they are,
/// and every other configured agent as given by `--agent`, launchable as
/// configured. Sorted by name and bounded.
pub fn static_agents(agents: &HashMap<String, AgentCommand>, infos: &[AgentInfo]) -> Vec<AgentInfo> {
    let mut out: Vec<AgentInfo> = infos.to_vec();
    for name in agents.keys() {
        if !out.iter().any(|a| &a.agent == name) {
            out.push(AgentInfo {
                agent: name.clone(),
                available: true,
                auth: AgentAuth::Unknown,
                cli: AgentCli::Given,
                adapter_version: None,
                images: None,
                note: None,
            });
        }
    }
    out.sort_by(|a, b| a.agent.cmp(&b.agent));
    bound_agents(out)
}

/// What an adapter's `initialize` came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// It answered: its `agentInfo.version` and `promptCapabilities.image`.
    Answered { version: Option<String>, images: bool },
    /// It did not: why, in words that quote nothing it printed.
    Failed(String),
}

/// Start `agent` as a session's adapter is started, in `cwd`, send
/// `initialize` and wait for its answer until `deadline` (check 3). Its
/// whole group is killed as this returns: `Adapter`'s drop does it.
pub async fn initialize(agent: &AgentCommand, cwd: &Path, deadline: Instant) -> Started {
    let (_adapter, io) = match Adapter::spawn(agent, cwd) {
        Ok(spawned) => spawned,
        Err(err) => return Started::Failed(format!("its adapter cannot be started: {}", err.kind())),
    };
    let mut stdin = io.stdin;
    let answer = async {
        if stdin.write_all(format!("{INITIALIZE}\n").as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            return Started::Failed("its adapter exited before it read `initialize`".into());
        }
        let mut lines = tokio::io::BufReader::new(io.stdout.take(MAX_OUTPUT)).lines();
        for _ in 0..MAX_LINES {
            let Ok(Some(line)) = lines.next_line().await else {
                // The byte cap ends the output as an end of file would.
                if lines.get_ref().get_ref().limit() == 0 {
                    return Started::Failed(TOO_MUCH.into());
                }
                return Started::Failed("its adapter exited without answering `initialize`".into());
            };
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if message.get("id") != Some(&serde_json::json!(0)) {
                continue;
            }
            return match message.get("result") {
                Some(result) => Started::Answered {
                    version: result
                        .pointer("/agentInfo/version")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    images: result
                        .pointer("/agentCapabilities/promptCapabilities/image")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                },
                None => Started::Failed("its adapter answered `initialize` with an error".into()),
            };
        }
        Started::Failed(TOO_MUCH.into())
    };
    match tokio::time::timeout_at(deadline, answer).await {
        Ok(started) => started,
        Err(_) => Started::Failed(format!(
            "its adapter did not answer `initialize` in the probe's time {OUT_OF_TIME}"
        )),
    }
}

/// What check 4 came to: `auth`, and a note when it says why it is
/// `unknown`.
pub async fn logged_in(check: LoginCheck, cwd: &Path, deadline: Instant) -> (AgentAuth, Option<String>) {
    let command = match check {
        LoginCheck::Ask(command) => command,
        LoginCheck::NotAsked(note) => return (AgentAuth::Unknown, note),
    };
    let left = deadline.saturating_duration_since(Instant::now());
    match adapter::exit_status(&command, cwd, left).await {
        Ok(Some(ended)) if ended.code == Some(0) => (AgentAuth::Ok, None),
        // Ended by a signal, it said nothing (the review's O1).
        Ok(Some(ended)) if ended.code.is_none() => (
            AgentAuth::Unknown,
            Some("its CLI was ended by a signal before it said whether it is logged in".into()),
        ),
        Ok(Some(_)) => (AgentAuth::Missing, None),
        Ok(None) => (
            AgentAuth::Unknown,
            Some(format!(
                "its CLI did not say whether it is logged in in the probe's time (a keychain prompt may be waiting on the Mac's screen) {OUT_OF_TIME}"
            )),
        ),
        Err(err) => (
            AgentAuth::Unknown,
            Some(format!("its CLI cannot be started: {}", err.kind())),
        ),
    }
}

/// One agent, live: launchable only if its adapter answered; `auth` from
/// its CLI; the version it answered with, else the one the set records.
async fn probe_one(
    info: AgentInfo,
    command: Option<AgentCommand>,
    checks: Arc<dyn AgentChecks>,
    cwd: PathBuf,
    deadline: Instant,
) -> AgentInfo {
    // Not launchable as configured: nothing to start.
    let Some(command) = command else { return info };
    let login = checks.login(&info.agent);
    let (started, (auth, login_note)) =
        tokio::join!(initialize(&command, &cwd, deadline), logged_in(login, &cwd, deadline));
    let (available, version, images, start_note) = match started {
        Started::Answered { version, images } => (true, version.filter(|v| is_readable_version(v)), Some(images), None),
        Started::Failed(why) => (false, None, None, Some(why)),
    };
    let note = match (start_note, login_note) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    };
    AgentInfo {
        available,
        auth,
        adapter_version: version.or(info.adapter_version),
        images,
        note,
        ..info
    }
}

/// The live view: every agent of `report` probed at once, within
/// `budget` (`PROBE_BUDGET` but in tests), in `cwd`. Bounded like the
/// static view.
pub async fn probe(
    report: &[AgentInfo],
    agents: &HashMap<String, AgentCommand>,
    checks: Arc<dyn AgentChecks>,
    cwd: &Path,
    budget: Duration,
) -> Vec<AgentInfo> {
    let deadline = Instant::now() + budget;
    let each = report.iter().map(|info| {
        probe_one(
            info.clone(),
            agents.get(&info.agent).cloned(),
            checks.clone(),
            cwd.to_path_buf(),
            deadline,
        )
    });
    bound_agents(futures::future::join_all(each).await)
}

/// At most one probe at a time on a host, across its connections (hazard
/// (e)): a probe still running when its connection dropped holds it.
#[derive(Debug, Default, Clone)]
pub struct OneProbe(Arc<AtomicBool>);

/// Held while a probe runs.
pub struct ProbeSlot(Arc<AtomicBool>);

impl OneProbe {
    /// The slot, unless a probe holds it.
    pub fn try_start(&self) -> Option<ProbeSlot> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| ProbeSlot(self.0.clone()))
    }
}

impl Drop for ProbeSlot {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
