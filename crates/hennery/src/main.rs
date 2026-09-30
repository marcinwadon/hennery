//! The `hennery` binary (distribution spec §1). Walking skeleton: collector,
//! host and an all-in-one mode, with a shared development token.

use anyhow::{Context, Result};
use axum::response::Html;
use axum::routing::get;
use clap::{Args, Parser, Subcommand};
use hennery_host::session::IDLE_TIMEOUT;
use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
use hennery_sessions::{AppState, store::Store};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "hennery", version, about = "Self-hosted cockpit for ACP coding agents")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the collector.
    Collector(CollectorArgs),
    /// Host commands.
    Host {
        #[command(subcommand)]
        command: HostCommand,
    },
    /// Run a collector and a host together (two processes).
    Up(UpArgs),
}

#[derive(Subcommand)]
enum HostCommand {
    /// Run a host.
    Run(HostArgs),
}

#[derive(Args, Clone)]
struct CollectorArgs {
    #[arg(long, default_value = "127.0.0.1:7117")]
    listen: String,
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: PathBuf,
    /// Development token for hosts and API clients (skeleton only).
    #[arg(long, env = "HENNERY_DEV_TOKEN", hide_env_values = true)]
    dev_token: String,
    /// Presume a host's sessions parked once it has been offline this long.
    #[arg(long, default_value_t = hennery_sessions::offline::OFFLINE_THRESHOLD.as_secs())]
    host_offline_secs: u64,
}

#[derive(Args, Clone)]
struct HostArgs {
    /// e.g. ws://127.0.0.1:7117/api/hosts/ws
    #[arg(long)]
    collector: String,
    #[arg(long, default_value = "local")]
    host_id: String,
    #[arg(long, env = "HENNERY_HOST_DATA_DIR")]
    data_dir: PathBuf,
    #[arg(long, env = "HENNERY_DEV_TOKEN", hide_env_values = true)]
    dev_token: String,
    /// Agent adapter, as `name=command args…`. Repeatable.
    #[arg(long = "agent", value_parser = parse_agent)]
    agents: Vec<(String, AgentCommand)>,
    /// Park sessions idle for this many seconds; 0 turns the reaper off.
    #[arg(long, default_value_t = IDLE_TIMEOUT.as_secs())]
    idle_timeout_secs: u64,
}

#[derive(Args)]
struct UpArgs {
    #[arg(long, default_value = "127.0.0.1:7117")]
    listen: String,
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: PathBuf,
    #[arg(long, env = "HENNERY_DEV_TOKEN", hide_env_values = true)]
    dev_token: String,
    #[arg(long = "agent", value_parser = parse_agent)]
    agents: Vec<(String, AgentCommand)>,
    /// Park sessions idle for this many seconds; 0 turns the reaper off.
    #[arg(long, default_value_t = IDLE_TIMEOUT.as_secs())]
    idle_timeout_secs: u64,
}

fn parse_agent(s: &str) -> Result<(String, AgentCommand), String> {
    let (name, command) = s.split_once('=').ok_or("expected name=command")?;
    let command = AgentCommand::parse(command).ok_or("empty command")?;
    Ok((name.to_string(), command))
}

const PLACEHOLDER: &str = "<!doctype html><meta charset=utf-8><title>hennery</title><h1>hennery</h1><p>Walking skeleton. The UI is not built yet.</p>";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    match Cli::parse().command {
        Command::Collector(args) => run_collector(args).await,
        Command::Host {
            command: HostCommand::Run(args),
        } => run_host(args).await,
        Command::Up(args) => run_up(args).await,
    }
}

async fn run_collector(args: CollectorArgs) -> Result<()> {
    // Checked before anything touches the data dir, like `run_up` does.
    let token = DevToken::new(args.dev_token)?;
    std::fs::create_dir_all(&args.data_dir)?;
    let db = args.data_dir.join("hennery.db");
    let store = Store::open(&db)?;
    let hosts = Hosts::open(&db)?;
    let mut state = AppState::new(store, hosts, token);
    state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
    hennery_sessions::offline::after_startup(&state);
    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .with_context(|| format!("bind {}", args.listen))?;
    tracing::info!(address = %listener.local_addr()?, "collector listening");
    let shutdown = state.shutdown.clone();
    tokio::spawn(async move {
        terminated().await;
        shutdown.cancel();
    });
    let app = hennery_sessions::router(state.clone()).route("/", get(|| async { Html(PLACEHOLDER) }));
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(state.shutdown.clone().cancelled_owned())
        .await?;
    Ok(())
}

async fn run_host(args: HostArgs) -> Result<()> {
    let mut cfg = HostConfig::new(args.collector, args.host_id, args.dev_token, args.data_dir);
    cfg.agents = args.agents.into_iter().collect();
    cfg.idle_timeout = std::time::Duration::from_secs(args.idle_timeout_secs);
    // On SIGINT/SIGTERM the host stops its connection and waits (bounded)
    // for every session actor to SIGTERM its adapter's group and SIGKILL it
    // after the grace; only then does returning drop the runtime.
    hennery_host::run_until(cfg, terminated()).await
}

/// Resolves on SIGINT or SIGTERM.
async fn terminated() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

/// Ask a child to shut down cleanly.
fn sigterm(child: &tokio::process::Child) {
    if let Some(pid) = child.id() {
        // SAFETY: plain kill(2) on a pid we spawned and still own.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
}

/// Two child processes of this binary, exchanging the same frames as a
/// remote host (architecture spec §3.3). The supervisor exits when either
/// child exits; restart policy comes with the distribution work.
async fn run_up(args: UpArgs) -> Result<()> {
    // Checked here too, so a bad token stops `up` before any child starts.
    DevToken::new(args.dev_token.clone())?;
    let exe = std::env::current_exe()?;
    // Keep both children out of the terminal's foreground process group: a
    // Ctrl-C there delivers SIGINT to every process in that group at once,
    // which would race each child's own signal handler against the ordered
    // shutdown below. With their own group, only this supervisor is signalled
    // and it alone decides the order (host, then collector).
    let mut collector = tokio::process::Command::new(&exe)
        .args(["collector", "--listen", &args.listen])
        .arg("--data-dir")
        .arg(args.data_dir.join("collector"))
        .env("HENNERY_DEV_TOKEN", &args.dev_token)
        .kill_on_drop(true)
        .process_group(0)
        .spawn()?;
    let mut host_cmd = tokio::process::Command::new(&exe);
    host_cmd
        .args([
            "host",
            "run",
            "--collector",
            &format!("ws://{}/api/hosts/ws", args.listen),
        ])
        .arg("--data-dir")
        .arg(args.data_dir.join("host"))
        .arg("--idle-timeout-secs")
        .arg(args.idle_timeout_secs.to_string())
        .env("HENNERY_DEV_TOKEN", &args.dev_token)
        .kill_on_drop(true)
        .process_group(0);
    for (name, command) in &args.agents {
        let mut spec = format!("{name}={}", command.program);
        for a in &command.args {
            spec.push(' ');
            spec.push_str(a);
        }
        host_cmd.arg("--agent").arg(spec);
    }
    let mut host = host_cmd.spawn()?;
    tokio::select! {
        status = collector.wait() => tracing::warn!(?status, "collector exited"),
        status = host.wait() => tracing::warn!(?status, "host exited"),
        _ = terminated() => {}
    }
    // Host first (it stops its adapters), then the collector.
    sigterm(&host);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), host.wait()).await;
    sigterm(&collector);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), collector.wait()).await;
    Ok(())
}
