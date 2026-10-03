//! `host run`'s agents and what it reports of them (plan 4d-B1-i): the
//! managed set's or the `--agent` commands, the static view `hello` gives,
//! and doctor's knowledge as `probe_agents` uses it: which CLI each agent
//! runs and how to ask it whether it is logged in (check 4,
//! `doctor::status_of`). The host runs the question itself, in its own
//! guarded groups (`hennery_host::adapter::exit_status`): nothing here
//! spawns anything, so doctor's process groups and signal handler are never
//! involved.

use crate::doctor::{bundled_cli, status_of, writable_by_others};
use crate::runtime::MirrorArgs;
use hennery_host::availability::{AgentChecks, LoginCheck};
use hennery_host::runtime::agents::{self, CLI_VARS};
use hennery_host::runtime::install::InstalledSet;
use hennery_host::{AgentCommand, HostConfig};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where `host run`'s agents come from.
pub enum AgentSetup {
    /// No `--agent`: the installed set, as `prepare` left it.
    Managed(Box<agents::Prepared>),
    /// `--agent` commands, as parsed: nothing managed, no CLI known.
    Given(HashMap<String, AgentCommand>),
}

impl AgentSetup {
    /// `given` if there are any, else the pinned set, installed first if it
    /// is not current (distribution spec §3.2).
    pub async fn new(data_dir: &Path, given: Vec<(String, AgentCommand)>, mirrors: &MirrorArgs) -> Self {
        if given.is_empty() {
            Self::Managed(Box::new(crate::runtime::default_agents(data_dir, mirrors).await))
        } else {
            Self::Given(given.into_iter().collect())
        }
    }

    /// Give `cfg` its agents, what `hello` reports of them, and the checks
    /// `probe_agents` runs. The file returned holds the managed set in use
    /// for as long as it is kept.
    pub fn configure(self, cfg: &mut HostConfig) -> Option<std::fs::File> {
        match self {
            Self::Managed(prepared) => {
                let agents::Prepared {
                    agents,
                    in_use,
                    set,
                    runtime,
                } = *prepared;
                cfg.checks = Arc::new(DoctorChecks::new(set, &agents.agents));
                cfg.agents = agents.agents;
                cfg.profiles = agents.profiles;
                cfg.codex_app_server = agents.codex_app_server;
                cfg.agent_infos = agents.infos;
                cfg.runtime = runtime;
                in_use
            }
            Self::Given(given) => {
                cfg.checks = Arc::new(DoctorChecks::new(None, &given));
                // A given command is a generic agent (ACP core §6): no
                // profile, and no app-server, so a Codex forget takes the
                // fallback.
                cfg.agents = given;
                cfg.profiles = HashMap::new();
                cfg.codex_app_server = None;
                cfg.agent_infos = Vec::new();
                cfg.runtime = Some(hennery_host::availability::given_runtime());
                None
            }
        }
    }
}

/// The CLIs of a host's agents, as the host was started.
#[derive(Debug, Clone)]
pub struct DoctorChecks {
    /// The set the managed agents launch from; `None` for `--agent`.
    set: Option<InstalledSet>,
    /// Each overridden agent's CLI, as its command passes it to the
    /// adapter (`CLAUDE_CODE_EXECUTABLE`, `CODEX_PATH`).
    overrides: BTreeMap<String, PathBuf>,
    /// The host's user: a CLI others can change is not run (doctor's O1).
    uid: u32,
}

impl DoctorChecks {
    /// For a managed host: its set, and the overrides its agents' commands
    /// carry. For a host given `--agent` commands: `set` is `None` and no
    /// CLI is known, as doctor knows none for them.
    pub fn new(set: Option<InstalledSet>, agents: &HashMap<String, AgentCommand>) -> Self {
        let overrides = match &set {
            Some(_) => agents
                .iter()
                .filter_map(|(name, command)| {
                    let var = CLI_VARS.iter().find(|(agent, _)| agent == name)?.1;
                    let (_, path) = command.env.iter().find(|(k, _)| k == var)?;
                    Some((name.clone(), PathBuf::from(path)))
                })
                .collect(),
            None => BTreeMap::new(),
        };
        Self {
            set,
            overrides,
            // SAFETY: geteuid(2) has no failure mode.
            uid: unsafe { libc::geteuid() },
        }
    }

    /// `agent`'s CLI, its program and the arguments before its own: its
    /// override, else the set's bundled one. The `Cli` doctor returns is
    /// held only to move its fields out: its `run` starts it in doctor's own
    /// groups (hazard (a)), and a test forbids any process API here.
    fn cli(&self, agent: &str) -> Option<(PathBuf, Vec<String>)> {
        let set = self.set.as_ref()?;
        match self.overrides.get(agent) {
            Some(path) => Some((path.clone(), Vec::new())),
            None => bundled_cli(set, agent).map(|cli| (cli.program, cli.args)),
        }
    }
}

impl AgentChecks for DoctorChecks {
    /// Check 4's question: `claude auth status` / `codex login status`,
    /// with the auto-updater off, as doctor asks it.
    fn login(&self, agent: &str) -> LoginCheck {
        let (Some((program, before)), Some((args, _))) = (self.cli(agent), status_of(agent)) else {
            return LoginCheck::NotAsked(None);
        };
        let program = program.as_path();
        if let Some(writable) = [program, program.parent().unwrap_or(program)]
            .into_iter()
            .find(|p| writable_by_others(p, self.uid))
        {
            return LoginCheck::NotAsked(Some(format!(
                "its CLI is not run: other users can write to {}",
                writable.display()
            )));
        }
        LoginCheck::Ask(AgentCommand {
            program: program.to_string_lossy().into_owned(),
            args: before.into_iter().chain(args.iter().map(|a| a.to_string())).collect(),
            env: vec![("DISABLE_AUTOUPDATER".to_string(), "1".to_string())],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_host::identity::HostKey;
    use hennery_host::runtime::install::{LAYOUT, RecordAdapter, SetRecord};
    use std::os::unix::fs::PermissionsExt;

    const PLATFORM: &str = "test-platform";

    /// A set under `dir` with both agents; `bundled` names the agents whose
    /// bundled CLI is there.
    fn set(dir: &Path, bundled: &[&str]) -> InstalledSet {
        let path = dir.join("sets/s1");
        let node = dir.join("runtimes/r/bin/node");
        let mut adapters = std::collections::BTreeMap::new();
        for agent in ["claude", "codex"] {
            adapters.insert(
                agent.to_string(),
                RecordAdapter {
                    version: "1.0.0".into(),
                    entry: "index.js".into(),
                    cli_skipped: false,
                },
            );
        }
        let tree = |agent: &str| path.join(agent).join("node_modules");
        for agent in bundled {
            let file = match *agent {
                "claude" => tree("claude").join(format!("@anthropic-ai/claude-agent-sdk-{PLATFORM}/claude")),
                _ => tree("codex").join("@openai/codex/bin/codex.js"),
            };
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "").unwrap();
        }
        InstalledSet {
            id: "s1".into(),
            path: path.clone(),
            record: SetRecord {
                layout: LAYOUT,
                id: "s1".into(),
                manifest_hash: "0".repeat(64),
                platform: PLATFORM.into(),
                runtime: "r".into(),
                adapters,
            },
            node,
        }
    }

    fn command(env: &[(&str, &str)]) -> AgentCommand {
        AgentCommand {
            program: "/node".into(),
            args: vec!["/entry".into()],
            env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn asked(check: LoginCheck) -> AgentCommand {
        match check {
            LoginCheck::Ask(command) => command,
            other => panic!("expected a question, got {other:?}"),
        }
    }

    fn autoupdater_off() -> Vec<(String, String)> {
        vec![("DISABLE_AUTOUPDATER".to_string(), "1".to_string())]
    }

    #[test]
    fn a_bundled_cli_is_asked_its_status_with_the_auto_updater_off() {
        let dir = tempfile::tempdir().unwrap();
        let set = set(dir.path(), &["claude", "codex"]);
        let agents = HashMap::from([
            ("claude".to_string(), command(&[])),
            ("codex".to_string(), command(&[])),
        ]);
        let checks = DoctorChecks::new(Some(set.clone()), &agents);
        let claude = asked(checks.login("claude"));
        assert_eq!(
            Path::new(&claude.program),
            set.path.join(format!(
                "claude/node_modules/@anthropic-ai/claude-agent-sdk-{PLATFORM}/claude"
            ))
        );
        assert_eq!(claude.args, ["auth", "status"]);
        assert_eq!(claude.env, autoupdater_off());
        let codex = asked(checks.login("codex"));
        assert_eq!(Path::new(&codex.program), set.node);
        assert_eq!(
            codex.args,
            [
                set.path
                    .join("codex/node_modules/@openai/codex/bin/codex.js")
                    .to_string_lossy()
                    .into_owned(),
                "login".into(),
                "status".into()
            ]
        );
        assert_eq!(codex.env, autoupdater_off());
    }

    /// The CLI the agent's command passes its adapter, not the bundled one.
    #[test]
    fn an_overridden_cli_is_the_one_asked() {
        let dir = tempfile::tempdir().unwrap();
        let set = set(dir.path(), &["claude"]);
        let agents = HashMap::from([(
            "claude".to_string(),
            command(&[("CLAUDE_CODE_EXECUTABLE", "/opt/claude/bin/claude")]),
        )]);
        let claude = asked(DoctorChecks::new(Some(set), &agents).login("claude"));
        assert_eq!(claude.program, "/opt/claude/bin/claude");
        assert_eq!(claude.args, ["auth", "status"]);
    }

    /// No CLI known: an `--agent` command, an agent doctor does not know,
    /// or no bundled CLI and no override.
    #[test]
    fn with_no_known_cli_nothing_is_asked() {
        let dir = tempfile::tempdir().unwrap();
        let agents = HashMap::from([
            ("claude".to_string(), command(&[])),
            ("gemini".to_string(), command(&[])),
        ]);
        let given = DoctorChecks::new(None, &agents);
        assert!(matches!(given.login("claude"), LoginCheck::NotAsked(None)));
        let managed = DoctorChecks::new(Some(set(dir.path(), &[])), &agents);
        assert!(matches!(managed.login("claude"), LoginCheck::NotAsked(None)));
        assert!(matches!(managed.login("codex"), LoginCheck::NotAsked(None)));
        assert!(matches!(managed.login("gemini"), LoginCheck::NotAsked(None)));
    }

    /// Doctor's O1: a CLI other users can change is not run, and the note
    /// names where.
    #[test]
    fn a_cli_others_can_write_to_is_not_run() {
        let dir = tempfile::tempdir().unwrap();
        let open = dir.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        let cli = open.join("claude");
        std::fs::write(&cli, "").unwrap();
        let agents = HashMap::from([(
            "claude".to_string(),
            command(&[("CLAUDE_CODE_EXECUTABLE", cli.to_str().unwrap())]),
        )]);
        let checks = DoctorChecks::new(Some(set(dir.path(), &[])), &agents);
        match checks.login("claude") {
            LoginCheck::NotAsked(Some(note)) => {
                assert!(
                    note.contains("other users can write to") && note.contains("open"),
                    "{note}"
                )
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn host_config() -> HostConfig {
        HostConfig::new(
            "ws://127.0.0.1:1/api/hosts/ws",
            "host-1",
            HostKey::from_seed([1; 32]),
            PathBuf::from("/nonexistent"),
        )
    }

    /// Hazard (a), the review's A2: this module only says which program to
    /// run; the host runs it, in its own guarded group. Nothing here may
    /// reach doctor's `Cli::run` (doctor's own groups) or start a process.
    #[test]
    fn nothing_here_reaches_a_process_api() {
        let source = include_str!("host_agents.rs");
        let code = &source[..source.find("#[cfg(test)]").expect("a test module")];
        // The scan covers the module's code: a test module placed earlier
        // would shrink it silently (the second re-confirmation's note).
        assert!(code.contains("fn configure"), "the scan stops before the code");
        // `run(` catches `Cli::run(&cli, …)` and doctor's `run`, as a method
        // or a function; `run_bounded` and `spawn(` doctor's other spawners
        // (the re-confirmation's R1).
        for banned in [
            "run(",
            "run_bounded",
            "spawn(",
            "kill_all",
            "spawn::",
            "Command::new",
            "std::process",
            "tokio::process",
        ] {
            assert!(!code.contains(banned), "host_agents.rs uses {banned}");
        }
    }

    /// `--agent` commands: reported as given, nothing managed, no CLI asked.
    #[test]
    fn given_agents_are_configured_as_given() {
        let mut cfg = host_config();
        let given = HashMap::from([("claude".to_string(), command(&[]))]);
        let in_use = AgentSetup::Given(given).configure(&mut cfg);
        assert!(in_use.is_none());
        assert_eq!(cfg.agents.keys().collect::<Vec<_>>(), ["claude"]);
        assert!(cfg.agent_infos.is_empty());
        // As the wire says it: `given`, and no managed field.
        assert_eq!(
            serde_json::to_value(cfg.runtime.as_ref().unwrap()).unwrap(),
            serde_json::json!({"source": "given"})
        );
        assert!(matches!(cfg.checks.login("claude"), LoginCheck::NotAsked(None)));
    }

    /// The managed set: its agents, their infos, its runtime and its CLIs.
    #[test]
    fn a_managed_set_is_configured_with_what_it_reports() {
        let dir = tempfile::tempdir().unwrap();
        let set = set(dir.path(), &["claude"]);
        let mut cfg = host_config();
        let file = std::fs::File::open(dir.path()).unwrap();
        let mut prepared = agents::Prepared {
            set: Some(set.clone()),
            in_use: Some(file),
            ..Default::default()
        };
        prepared.agents.agents.insert("claude".into(), command(&[]));
        prepared.agents.infos = hennery_host::availability::static_agents(&prepared.agents.agents, &[]);
        let infos = prepared.agents.infos.clone();
        // Any runtime: it is passed on as it is.
        prepared.runtime = Some(hennery_host::availability::given_runtime());
        let runtime = prepared.runtime.clone();
        let in_use = AgentSetup::Managed(Box::new(prepared)).configure(&mut cfg);
        assert!(in_use.is_some());
        assert_eq!(cfg.agents.keys().collect::<Vec<_>>(), ["claude"]);
        assert_eq!(cfg.agent_infos, infos);
        assert_eq!(cfg.runtime, runtime);
        assert!(matches!(cfg.checks.login("claude"), LoginCheck::Ask(_)));
    }
}
