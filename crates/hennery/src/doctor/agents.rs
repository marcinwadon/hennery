//! The checks that start the agents' programs (distribution spec §7):
//! each adapter answering `initialize` (check 3), each agent's CLI logged
//! in (check 4), and the bundled CLIs against the terminal's (check 13).
//! Every program runs in the environment a service-run host gives its
//! agents (decision 14), in a group of its own that is killed afterwards.
//! Nothing is ever logged in, and nothing a CLI prints reaches the report.

use super::dirs::NO_HOST;
use super::spawn::{self, Cli, Started};
use super::{Doctor, Finding, Verdict};
use hennery_host::AgentCommand;
use hennery_host::runtime::agents;
use hennery_host::runtime::install::{InstalledSet, Layout};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Why the agent checks did not run.
const NO_AGENTS: &str = "no adapter set installed and no --agent";

/// What an agent's checks need: how a host would start it, the version
/// its set pins (none for an `--agent` command), and its CLI.
pub struct Agent {
    pub name: String,
    pub command: AgentCommand,
    pub pinned: Option<String>,
    /// The CLI it runs: the `--use-cli` override, else the set's bundled
    /// one, if any.
    pub cli: Option<Cli>,
}

/// `agent`'s bundled CLI in `set`, unless the set left it out, where 7b
/// puts it: Claude's binary in its platform package, Codex's script run by
/// the set's Node.
pub fn bundled_cli(set: &InstalledSet, agent: &str) -> Option<Cli> {
    let record = set.record.adapters.get(agent)?;
    if record.cli_skipped {
        return None;
    }
    let tree = set.path.join(agent).join("node_modules");
    let cli = match agent {
        "claude" => Cli::plain(
            tree.join(format!("@anthropic-ai/claude-agent-sdk-{}", set.record.platform))
                .join("claude"),
        ),
        "codex" => Cli {
            program: set.node.clone(),
            args: vec![tree.join("@openai/codex/bin/codex.js").to_string_lossy().into_owned()],
        },
        _ => return None,
    };
    let file = cli
        .args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| cli.program.clone());
    file.is_file().then_some(cli)
}

/// The agents a host on `doctor`'s directory would start, exactly as it
/// would start them: the service's `--agent` commands when it gives them,
/// else the current set's (`agents::from_set`). `Err`: why there are none
/// to check.
pub fn agents(doctor: &Doctor) -> Result<Vec<Agent>, &'static str> {
    let Some(host) = &doctor.dirs.host else {
        return Err(NO_HOST);
    };
    if let Some(given) = doctor.given_agents() {
        return Ok(given
            .into_iter()
            .map(|(name, command)| Agent {
                name,
                command,
                pinned: None,
                cli: None,
            })
            .collect());
    }
    let set = Layout::new(host)
        .ok()
        .and_then(|layout| layout.current().ok().flatten());
    let Some(set) = set else {
        return Err(NO_AGENTS);
    };
    let overrides = agents::cli_overrides(host).unwrap_or_default();
    let mut started = agents::from_set(&set, &overrides).agents;
    let mut names: Vec<String> = started.keys().cloned().collect();
    names.sort();
    Ok(names
        .into_iter()
        .map(|name| Agent {
            command: started.remove(&name).expect("listed"),
            pinned: set.record.adapters.get(&name).map(|a| a.version.clone()),
            cli: overrides
                .get(&name)
                .map(|path| Cli::plain(path.clone()))
                .or_else(|| bundled_cli(&set, &name)),
            name,
        })
        .collect())
}

/// Check 3: each adapter starts as a host would start it, and answers
/// `initialize` within 20 s with the version its set pins.
pub fn adapters_start(doctor: &Doctor) -> Finding {
    let agents = match agents(doctor) {
        Ok(agents) => agents,
        Err(why) => return Finding::NotRun { number: 3, why },
    };
    let env = spawn::agent_env(doctor);
    let mut verdict = Verdict::default();
    for agent in &agents {
        let name = &agent.name;
        match (spawn::initialize(&agent.command, &env, spawn::INITIALIZE_TIMEOUT), &agent.pinned) {
            (Started::Failed(why), _) => verdict.fail(
                format!("{name} does not start: {why}"),
                "run `hennery host adapters update`, then `hennery doctor` again; the host's log has the adapter's own output",
            ),
            (Started::Answered(Some(version)), Some(pinned)) if version == *pinned => {
                verdict.ok(format!("{name} {version} answers"))
            }
            (Started::Answered(version), Some(pinned)) => verdict.warn(
                format!(
                    "{name} answers as {}, not the pinned {pinned}",
                    version.as_deref().unwrap_or("an unnamed version")
                ),
                "run `hennery host adapters update`",
            ),
            (Started::Answered(version), None) => verdict.ok(format!(
                "{name} answers ({}; given by --agent, no pin)",
                version.as_deref().unwrap_or("no version")
            )),
        }
    }
    if agents.is_empty() {
        verdict.warn(
            "the set starts no agent",
            "see checks 12 and 17, then `hennery host adapters update`",
        );
    }
    Finding::Checked(verdict.check(3, "adapters start"))
}

/// How `agent`'s CLI says whether it is logged in, and how to log in.
fn status_of(agent: &str) -> Option<(&'static [&'static str], &'static str)> {
    match agent {
        "claude" => Some((
            &["auth", "status"],
            "run `claude` in a terminal and sign in (`/login`); doctor never logs in",
        )),
        "codex" => Some((
            &["login", "status"],
            "run `codex login` in a terminal; doctor never logs in",
        )),
        _ => None,
    }
}

/// Check 4: each agent's CLI says it is logged in, in the service's
/// environment. Only its exit code is kept: its output, which may name the
/// account, is never read.
pub fn logged_in(doctor: &Doctor) -> Finding {
    let agents = match agents(doctor) {
        Ok(agents) => agents,
        Err(why) => return Finding::NotRun { number: 4, why },
    };
    let env = spawn::agent_env(doctor);
    let mut verdict = Verdict::default();
    for agent in &agents {
        let name = &agent.name;
        let (Some(cli), Some((args, fix))) = (&agent.cli, status_of(name)) else {
            verdict.ok(format!(
                "{name}: its login cannot be checked here (no CLI doctor knows)"
            ));
            continue;
        };
        match cli.run(args, &env, spawn::STATUS_TIMEOUT, false) {
            Some(ran) if ran.ok => verdict.ok(format!("{name} is logged in")),
            Some(_) => verdict.warn(format!("{name} is not logged in"), fix),
            None => {
                let keychain = if doctor.cx.platform == crate::service::Platform::MacOs {
                    " (a keychain prompt may be waiting on the Mac's screen)"
                } else {
                    ""
                };
                verdict.warn(
                    format!("{name}'s CLI did not say whether it is logged in{keychain}"),
                    format!("run `{} {}` in a terminal", cli.shown(), args.join(" ")),
                )
            }
        }
    }
    if doctor.cx.env.contains_key("SSH_CONNECTION") && doctor.cx.platform == crate::service::Platform::MacOs {
        verdict.ok("checked over SSH: a service runs in the GUI login session, whose keychain may differ");
    }
    Finding::Checked(verdict.check(4, "logged in"))
}

/// The first executable `name` on doctor's own PATH (absolute entries
/// only), outside `host`'s directory: the CLI the user's terminal runs.
fn terminal_cli(doctor: &Doctor, name: &str, host: &Path) -> Option<Cli> {
    let path = doctor.cx.env.get("PATH")?;
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute() && !dir.starts_with(host))
        .map(|dir| dir.join(name))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
        .map(Cli::plain)
}

/// Check 13: each bundled CLI against the one the user's terminal runs. A
/// session resumed from the terminal meets the format of the terminal's.
pub fn bundled_and_terminal(doctor: &Doctor) -> Finding {
    let Some(host) = &doctor.dirs.host else {
        return Finding::NotRun {
            number: 13,
            why: NO_HOST,
        };
    };
    let set = Layout::new(host)
        .ok()
        .and_then(|layout| layout.current().ok().flatten());
    let Some(set) = set else {
        return Finding::NotRun {
            number: 13,
            why: NO_AGENTS,
        };
    };
    let env = spawn::agent_env(doctor);
    let mut terminal_env = env.clone();
    if let Some(path) = doctor.cx.env.get("PATH") {
        terminal_env.retain(|(k, _)| k != "PATH");
        terminal_env.push(("PATH".to_string(), path.clone()));
    }
    let mut verdict = Verdict::default();
    let mut names: Vec<&String> = set.record.adapters.keys().collect();
    names.sort();
    for name in names {
        let Some(bundled) = bundled_cli(&set, name) else {
            verdict.ok(format!("{name}: no bundled CLI in the set"));
            continue;
        };
        let Some(ours) = spawn::cli_version(&bundled, &env) else {
            verdict.warn(
                format!("{name}'s bundled CLI does not say its version"),
                "run `hennery host adapters update`",
            );
            continue;
        };
        let Some(theirs_cli) = terminal_cli(doctor, name, host) else {
            verdict.ok(format!(
                "{name}: bundled {}.{}.{}, none on PATH",
                ours.0, ours.1, ours.2
            ));
            continue;
        };
        match spawn::cli_version(&theirs_cli, &terminal_env) {
            None => verdict.warn(
                format!("{} does not say its version", theirs_cli.shown()),
                format!("run `{} --version` in a terminal", theirs_cli.shown()),
            ),
            Some(theirs) if spawn::far_apart(ours, theirs) => verdict.warn(
                format!(
                    "{name}: bundled {}.{}.{}, terminal {}.{}.{}: sessions resumed from the terminal may meet another format",
                    ours.0, ours.1, ours.2, theirs.0, theirs.1, theirs.2
                ),
                format!(
                    "bring {} near {}.{}, or resume sessions only in hennery",
                    theirs_cli.shown(),
                    ours.0,
                    ours.1
                ),
            ),
            Some(theirs) => verdict.ok(format!(
                "{name}: bundled {}.{}.{}, terminal {}.{}.{}",
                ours.0, ours.1, ours.2, theirs.0, theirs.1, theirs.2
            )),
        }
    }
    Finding::Checked(verdict.check(13, "bundled and terminal CLIs"))
}

/// Check 17's version gap: `agent`'s override (`cli`) against the set's
/// bundled CLI, when the set still has it.
pub fn override_gap(doctor: &Doctor, host: &Path, agent: &str, cli: &Path, verdict: &mut Verdict) {
    let set = Layout::new(host)
        .ok()
        .and_then(|layout| layout.current().ok().flatten());
    let Some(bundled) = set.as_ref().and_then(|set| bundled_cli(set, agent)) else {
        verdict.ok(format!("{agent}: the pinned CLI is not installed here to compare with"));
        return;
    };
    let env = spawn::agent_env(doctor);
    match (
        spawn::cli_version(&Cli::plain(cli), &env),
        spawn::cli_version(&bundled, &env),
    ) {
        (Some(theirs), Some(ours)) if spawn::far_apart(ours, theirs) => verdict.warn(
            format!(
                "{agent}: your CLI is {}.{}.{}, the pinned one {}.{}.{}",
                theirs.0, theirs.1, theirs.2, ours.0, ours.1, ours.2
            ),
            format!(
                "use a {agent} near {}.{}, or `--use-cli {agent}=bundled`",
                ours.0, ours.1
            ),
        ),
        (Some(theirs), Some(ours)) => verdict.ok(format!(
            "{agent}: your CLI {}.{}.{}, the pinned {}.{}.{}",
            theirs.0, theirs.1, theirs.2, ours.0, ours.1, ours.2
        )),
        _ => verdict.warn(
            format!("{agent}: a CLI's version cannot be read"),
            format!("run `{} --version` in a terminal", cli.display()),
        ),
    }
}
