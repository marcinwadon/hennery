//! The managed runtime's commands (distribution spec §1, §3.2, §13
//! decision 5): `hennery host adapters update|rollback`, the install `host
//! join` ends with, and `host run`'s default agents.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use hennery_host::AgentCommand;
use hennery_host::runtime::agents::{self, UseCli};
use hennery_host::runtime::download::Sources;
use hennery_host::runtime::install::{self, Installed, InstalledSet, Layout, Selection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum AdaptersCommand {
    /// Install the adapter set this binary pins and make it current; the
    /// previous set is kept for `rollback`.
    Update(UpdateArgs),
    /// Make the previous adapter set current again, and keep the host on it
    /// until the next `update`.
    Rollback(RollbackArgs),
}

/// Where the pinned packages and Node are fetched from (decision 7).
#[derive(Args, Clone, Default)]
pub struct MirrorArgs {
    /// A mirror of the npm registry, in place of https://registry.npmjs.org/.
    /// The digests stay those this binary pins.
    #[arg(long, env = "HENNERY_NPM_REGISTRY")]
    pub npm_registry: Option<String>,
    /// A mirror of https://nodejs.org/dist/; the digests stay those pinned.
    #[arg(long, env = "HENNERY_NODE_MIRROR")]
    pub node_mirror: Option<String>,
}

impl MirrorArgs {
    pub fn sources(&self) -> Result<Sources> {
        Sources::new(self.npm_registry.as_deref(), self.node_mirror.as_deref())
    }
}

#[derive(Args)]
pub struct UpdateArgs {
    #[arg(long, env = "HENNERY_HOST_DATA_DIR")]
    pub data_dir: PathBuf,
    /// Run this agent with your own CLI instead of the bundled one
    /// (`claude=/path/to/claude`, `codex=/path/to/codex`), or go back to it
    /// (`claude=bundled`). Recorded in `host.toml`; repeatable. Advanced: the
    /// agent loses the pin's guarantee.
    #[arg(long = "use-cli", value_parser = parse_use_cli)]
    pub use_cli: Vec<UseCli>,
    #[command(flatten)]
    pub mirrors: MirrorArgs,
}

#[derive(Args)]
pub struct RollbackArgs {
    #[arg(long, env = "HENNERY_HOST_DATA_DIR")]
    pub data_dir: PathBuf,
}

pub fn parse_use_cli(arg: &str) -> Result<UseCli, String> {
    UseCli::parse(arg).map_err(|err| format!("{err:#}"))
}

pub async fn run(command: AdaptersCommand) -> Result<()> {
    match command {
        AdaptersCommand::Update(args) => {
            // Before anything is recorded: a bad mirror changes nothing.
            args.mirrors.sources()?;
            record_cli_choices(&args.data_dir, &args.use_cli)?;
            update(&args.data_dir, &args.mirrors).await?;
            println!(
                "A running host keeps starting agents from the set it started with until it restarts; \
                 running sessions keep theirs."
            );
            Ok(())
        }
        AdaptersCommand::Rollback(args) => {
            let layout = Layout::new(&args.data_dir)?;
            let back = install::rollback(&layout, &progress).await?;
            println!(
                "rolled back from adapter set {} to {} ({}); the host stays on it until `hennery host adapters update`",
                back.from,
                back.to.id,
                versions(&back.to)
            );
            println!(
                "A running host keeps starting agents from the set it started with until it restarts; \
                 restart it to use this one."
            );
            Ok(())
        }
    }
}

/// Record each `--use-cli` in `host.toml`, saying what it costs.
pub fn record_cli_choices(data_dir: &Path, choices: &[UseCli]) -> Result<()> {
    for choice in choices {
        agents::set_cli_override(data_dir, choice)?;
        match &choice.path {
            Some(path) => eprintln!(
                "{agent} will run {path} instead of its bundled CLI. It loses the pin's guarantee (this release \
                 was tested with the bundled one). On a host shared by several hats it is meant to get the MCP \
                 fallback unless you accept unverified isolation; this release does not enforce that yet. \
                 `--use-cli {agent}=bundled` undoes this.",
                agent = choice.agent,
                path = path.display()
            ),
            None => eprintln!("{} goes back to its bundled CLI", choice.agent),
        }
    }
    Ok(())
}

/// Install the pinned set (less the CLIs `host.toml` overrides) and say
/// what is current.
pub async fn update(data_dir: &Path, mirrors: &MirrorArgs) -> Result<()> {
    let sources = mirrors.sources()?;
    let overrides = agents::cli_overrides(data_dir)?;
    let selection = Selection::pinned(&agents::skipped(&overrides))?;
    let layout = Layout::new(data_dir)?;
    match install::install(&layout, &selection, &sources, &progress).await? {
        Installed::AlreadyCurrent(set) => println!("adapter set {} is current already ({})", set.id, versions(&set)),
        Installed::Switched { set, previous } => {
            println!("adapter set {} is current ({})", set.id, versions(&set));
            if let Some(previous) = previous {
                println!("the previous set, {previous}, is kept for `hennery host adapters rollback`");
            }
        }
    }
    Ok(())
}

/// `join`'s last step: the pairing stands whatever happens here.
pub async fn after_join(data_dir: &Path, host_id: &str, choices: &[UseCli], mirrors: &MirrorArgs) -> Result<()> {
    record_cli_choices(data_dir, choices)?;
    update(data_dir, mirrors).await.with_context(|| {
        format!(
            "paired as {host_id}, but the adapter runtime was not installed; \
             `hennery host adapters update` retries"
        )
    })
}

/// `host run`'s agents with no `--agent`: from the installed set, after
/// installing the pinned one if it is not current. The file returned holds
/// that set in use for as long as it is kept.
pub async fn default_agents(
    data_dir: &Path,
    mirrors: &MirrorArgs,
) -> (HashMap<String, AgentCommand>, Option<std::fs::File>) {
    // A bad mirror skips the install, and never falls back to the public
    // registry; it does not stop the host either.
    let sources = mirrors
        .sources()
        .inspect_err(|err| tracing::warn!("not installing the pinned adapter set: {err:#}"))
        .ok();
    let log = |line: &str| tracing::info!("{line}");
    let prepared = agents::prepare_pinned(data_dir, sources.as_ref(), &log).await;
    for note in &prepared.agents.notes {
        tracing::warn!("{note}");
    }
    if let Some(set) = &prepared.set {
        tracing::info!(set = %set.id, "agents from the adapter set: {}", versions(set));
    }
    (prepared.agents.agents, prepared.in_use)
}

fn versions(set: &InstalledSet) -> String {
    set.record
        .adapters
        .iter()
        .map(|(name, a)| {
            if a.cli_skipped {
                format!("{name} {}, own CLI", a.version)
            } else {
                format!("{name} {}", a.version)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn progress(line: &str) {
    eprintln!("{line}");
}
