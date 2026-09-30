//! The `hennery` binary (distribution spec §1): collector, host (join and
//! run) and an all-in-one mode. Hosts authenticate with the key they paired
//! with; the REST API still takes a development bearer token until operator
//! auth lands.

mod inherit;

use anyhow::{Context, Result, bail};
use axum::response::Html;
use axum::routing::get;
use clap::{Args, Parser, Subcommand};
use hennery_host::identity::Paired;
use hennery_host::pairing::Joined;
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
    /// Pair this machine with a collector (kernel spec §4.1).
    Join(JoinArgs),
    /// Run a host.
    Run(HostArgs),
}

#[derive(Args)]
struct JoinArgs {
    /// The collector's public URL, e.g. https://hennery.example.
    url: String,
    /// The pairing code shown by the collector (`XXXX-XXXX`).
    code: String,
    /// How the collector lists this host; defaults to the host name.
    #[arg(long)]
    name: Option<String>,
    #[arg(long, env = "HENNERY_HOST_DATA_DIR")]
    data_dir: PathBuf,
}

#[derive(Args, Clone)]
struct CollectorArgs {
    #[arg(long, default_value = "127.0.0.1:7117")]
    listen: String,
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: PathBuf,
    /// Development bearer token for REST clients, until operator auth.
    /// Hosts authenticate with their paired key instead.
    #[arg(long, env = "HENNERY_DEV_TOKEN", hide_env_values = true)]
    dev_token: String,
    /// Presume a host's sessions parked once it has been offline this long.
    #[arg(long, default_value_t = hennery_sessions::offline::OFFLINE_THRESHOLD.as_secs())]
    host_offline_secs: u64,
    /// `hennery up` only: once listening, write one pairing code to this
    /// inherited descriptor (kernel spec §4.2).
    #[arg(long, hide = true)]
    pairing_code_fd: Option<i32>,
}

#[derive(Args, Clone)]
struct HostArgs {
    /// Holds the pairing `hennery host join` stored (`host.key`, `host.toml`).
    #[arg(long, env = "HENNERY_HOST_DATA_DIR")]
    data_dir: PathBuf,
    /// Agent adapter, as `name=command args…`. Repeatable.
    #[arg(long = "agent", value_parser = parse_agent)]
    agents: Vec<(String, AgentCommand)>,
    /// Park sessions idle for this many seconds; 0 turns the reaper off.
    #[arg(long, default_value_t = IDLE_TIMEOUT.as_secs())]
    idle_timeout_secs: u64,
    /// `hennery up` only: join this collector first if the host is not
    /// paired, with the code read from `--join-code-fd`.
    #[arg(long, hide = true, requires = "join_code_fd")]
    join_url: Option<String>,
    #[arg(long, hide = true, requires = "join_url")]
    join_code_fd: Option<i32>,
    /// `hennery up` only: the collector's host WebSocket as it listens now,
    /// in place of the stored one (its port may have changed).
    #[arg(long, hide = true)]
    collector_url: Option<String>,
}

#[derive(Args)]
struct UpArgs {
    #[arg(long, default_value = "127.0.0.1:7117")]
    listen: String,
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: PathBuf,
    /// The collector's development bearer token for REST clients.
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
            command: HostCommand::Join(args),
        } => join_host(args).await,
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
    if let Some(fd) = args.pairing_code_fd {
        // Only once migrated and listening: the host enrolls right away.
        let code = state.hosts.mint_pairing_code(hennery_kernel::secret::unix_now())?;
        inherit::write_code(fd, &code.code)?;
    }
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

async fn join_host(args: JoinArgs) -> Result<()> {
    let name = args.name.unwrap_or_else(hennery_host::pairing::default_name);
    match hennery_host::pairing::join(&args.url, &args.code, &args.data_dir, &name).await? {
        Joined::Paired { host_id } => println!("paired as {host_id}"),
        Joined::AlreadyPaired { host_id } => println!("already paired as {host_id}; nothing to do"),
    }
    Ok(())
}

async fn run_host(args: HostArgs) -> Result<()> {
    let paired = match Paired::load(&args.data_dir)? {
        Some(paired) => {
            // Paired already (kernel spec §4.2): the code is not needed.
            if let Some(fd) = args.join_code_fd {
                inherit::close(fd);
            }
            paired
        }
        None => {
            let (Some(url), Some(fd)) = (&args.join_url, args.join_code_fd) else {
                bail!(
                    "{} holds no pairing; run `hennery host join <url> <code>` first",
                    args.data_dir.display()
                );
            };
            let code = inherit::read_code(fd).await?;
            let name = hennery_host::pairing::default_name();
            hennery_host::pairing::join(url, &code, &args.data_dir, &name).await?;
            Paired::load(&args.data_dir)?.context("the pairing just stored")?
        }
    };
    let collector_url = args.collector_url.unwrap_or(paired.collector_url);
    let mut cfg = HostConfig::new(collector_url, paired.host_id, paired.key, args.data_dir);
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

/// The collector's URL as its own host child reaches it: over loopback,
/// also when it listens on every interface.
fn loopback_url(listen: &str) -> String {
    match listen.parse::<SocketAddr>() {
        Ok(addr) if addr.ip().is_unspecified() && addr.is_ipv6() => format!("http://[::1]:{}", addr.port()),
        Ok(addr) if addr.ip().is_unspecified() => format!("http://127.0.0.1:{}", addr.port()),
        _ => format!("http://{listen}"),
    }
}

/// Two child processes of this binary, exchanging the same frames as a
/// remote host (architecture spec §3.3). The supervisor exits when either
/// child exits; restart policy comes with the distribution work.
async fn run_up(args: UpArgs) -> Result<()> {
    // Checked here too, so a bad token stops `up` before any child starts.
    DevToken::new(args.dev_token.clone())?;
    let exe = std::env::current_exe()?;
    let host_dir = args.data_dir.join("host");
    // The host pairs itself on first start only; a pairing that was revoked
    // is not replaced (kernel spec §4.2).
    let pairing = match Paired::load(&host_dir)? {
        Some(_) => None,
        None => Some(std::io::pipe()?),
    };
    // Keep both children out of the terminal's foreground process group: a
    // Ctrl-C there delivers SIGINT to every process in that group at once,
    // which would race each child's own signal handler against the ordered
    // shutdown below. With their own group, only this supervisor is signalled
    // and it alone decides the order (host, then collector).
    let mut collector_cmd = tokio::process::Command::new(&exe);
    collector_cmd
        .args(["collector", "--listen", &args.listen])
        .arg("--data-dir")
        .arg(args.data_dir.join("collector"))
        .env("HENNERY_DEV_TOKEN", &args.dev_token)
        .kill_on_drop(true)
        .process_group(0);
    if let Some((_, writer)) = &pairing {
        inherit::pass_to_child(&mut collector_cmd, writer);
        collector_cmd
            .arg("--pairing-code-fd")
            .arg(inherit::CHILD_FD.to_string());
    }
    let mut collector = collector_cmd.spawn()?;
    let collector_url = loopback_url(&args.listen);
    let mut host_cmd = tokio::process::Command::new(&exe);
    host_cmd
        .args(["host", "run"])
        .arg("--data-dir")
        .arg(&host_dir)
        .arg("--collector-url")
        .arg(hennery_host::pairing::collector_ws_url(&collector_url)?)
        .arg("--idle-timeout-secs")
        .arg(args.idle_timeout_secs.to_string())
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
    if let Some((reader, _)) = &pairing {
        inherit::pass_to_child(&mut host_cmd, reader);
        host_cmd
            .arg("--join-url")
            .arg(&collector_url)
            .arg("--join-code-fd")
            .arg(inherit::CHILD_FD.to_string());
    }
    let mut host = host_cmd.spawn()?;
    // Both children hold their ends now; with the supervisor's copies
    // closed, the host sees end-of-file if the collector dies first.
    drop(pairing);
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
