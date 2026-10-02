//! The checks that start the agents' programs (distribution spec §7):
//! each adapter answering `initialize` (check 3), and each agent's CLI
//! logged in (check 4).
//! Every program runs in the environment a service-run host gives its
//! agents (decision 14), in a group of its own that is killed afterwards.
//! Nothing is ever logged in, and nothing a CLI prints reaches the report.

use super::dirs::NO_HOST;
use super::runtime::writable_by_others;
use super::spawn::{self, Cli, Started};
use super::{Doctor, Finding, Verdict};
use hennery_host::AgentCommand;
use hennery_host::runtime::agents;
use hennery_host::runtime::install::{InstalledSet, Layout};
use std::path::{Path, PathBuf};

/// Why the agent checks did not run.
const NO_AGENTS: &str = "no adapter set installed and no --agent";

/// What a timeout on macOS may be: the first run of a program written
/// since, which macOS checks online and waits for (decision 14).
fn first_run_note(doctor: &Doctor) -> &'static str {
    if doctor.cx.platform == crate::service::Platform::MacOs {
        " (macOS checks a newly written program online on its first run, and a network problem can delay that: run doctor again)"
    } else {
        ""
    }
}

/// Whether other users can change what `cli` runs: its program, or the
/// directory it is in, writable by them. Doctor does not run such a CLI.
fn others_can_change(doctor: &Doctor, cli: &Cli) -> Option<PathBuf> {
    let program = cli.program.as_path();
    [program, program.parent().unwrap_or(program)]
        .into_iter()
        .find(|p| writable_by_others(p, doctor.cx.uid))
        .map(Path::to_path_buf)
}

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
/// `initialize` within 20 s with the version its set pins (on macOS, a
/// silence only warns: decision 15).
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
            // On macOS a silence may be the first run of a program the
            // adapter starts, waiting for the machine's online check: a
            // warning there, never a failure (decision 15).
            (Started::Failed(why), _)
                if why.starts_with(spawn::NO_ANSWER) && doctor.cx.platform == crate::service::Platform::MacOs =>
            {
                verdict.warn(
                    format!("{name} does not start: {why}{}", first_run_note(doctor)),
                    "run `hennery doctor` again; if it still does not answer, run `hennery host adapters update`; the host's log has the adapter's own output",
                )
            }
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
            "run `claude` in a terminal and sign in (`/login`); doctor never logs in, and does not read a login given by a variable in the service's environment file or plist",
        )),
        "codex" => Some((
            &["login", "status"],
            "run `codex login` in a terminal; doctor never logs in, and does not read a login given by a variable in the service's environment file or plist",
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
        if let Some(writable) = others_can_change(doctor, cli) {
            verdict.warn(
                format!(
                    "{name}'s CLI is not run: other users can write to {}",
                    writable.display()
                ),
                format!("run `chmod go-w {}`", writable.display()),
            );
            continue;
        }
        match cli.run(args, &env, false) {
            Some(ran) if ran.ok => verdict.ok(format!("{name} is logged in")),
            Some(_) => verdict.warn(format!("{name} is not logged in"), fix),
            None => {
                let keychain = if doctor.cx.platform == crate::service::Platform::MacOs {
                    " (a keychain prompt may be waiting on the Mac's screen)"
                } else {
                    ""
                };
                verdict.warn(
                    format!(
                        "{name}'s CLI did not say whether it is logged in{keychain}{}",
                        first_run_note(doctor)
                    ),
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
