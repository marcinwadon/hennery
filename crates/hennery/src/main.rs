//! The `hennery` binary (distribution spec §1): collector, host (join and
//! run) and an all-in-one mode. Hosts authenticate with the key they paired
//! with; the operator with the session the setup link or a login opened.

mod admin;
mod config;
mod data_dir;
mod doctor;
mod healthcheck;
mod inherit;
mod lock;
mod log;
mod revoked;
mod runtime;
mod service;
mod supervisor;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use hennery_host::identity::Paired;
use hennery_host::pairing::Joined;
use hennery_host::session::IDLE_TIMEOUT;
use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::{Operator, PublicUrl, SetupLink};
use hennery_sessions::{AppState, store::Store};
use std::net::SocketAddr;
use std::os::fd::AsRawFd;
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
    Collector(CollectorCli),
    /// Host commands.
    Host {
        #[command(subcommand)]
        command: HostCommand,
    },
    /// Run a collector and a host together (two processes).
    Up(UpArgs),
    /// Recovery commands for a running collector, over its admin socket.
    Admin(admin::AdminArgs),
    /// Run hennery as a per-user service (launchd, systemd).
    Service(service::ServiceArgs),
    /// Diagnose this machine's hennery: each check ok, warn or fail, with a
    /// fix. Reads only.
    Doctor(doctor::DoctorArgs),
}

#[derive(Subcommand)]
enum HostCommand {
    /// Pair this machine with a collector (kernel spec §4.1).
    Join(JoinArgs),
    /// Run a host.
    Run(HostArgs),
    /// The managed adapter set (distribution spec §3.2).
    Adapters {
        #[command(subcommand)]
        command: runtime::AdaptersCommand,
    },
}

#[derive(Args)]
struct JoinArgs {
    /// The collector's public URL, e.g. https://hennery.example.
    url: String,
    /// The pairing code shown by the collector (`XXXX-XXXX`). Leave it out
    /// to type it, or pipe it, on standard input instead: that keeps it out
    /// of the process list and the shell history.
    code: Option<String>,
    /// How the collector lists this host; defaults to the host name.
    #[arg(long)]
    name: Option<String>,
    /// The host's data directory. Else the one the installed host service
    /// runs, else the platform's (distribution spec §8).
    #[arg(long, env = "HENNERY_HOST_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Run this agent with your own CLI instead of the bundled one
    /// (`claude=/path/to/claude`, `codex=/path/to/codex`). Recorded in
    /// `host.toml`; repeatable. Advanced: the agent loses the pin's guarantee.
    #[arg(long = "use-cli", value_parser = runtime::parse_use_cli)]
    use_cli: Vec<hennery_host::runtime::agents::UseCli>,
    /// Pair only: install no adapter runtime (a host that runs only
    /// `--agent` commands, e.g. Nix-provided adapters).
    #[arg(long, conflicts_with = "use_cli")]
    no_runtime: bool,
    #[command(flatten)]
    mirrors: runtime::MirrorArgs,
}

/// `collector` runs the collector; `collector healthcheck` checks one.
#[derive(Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
struct CollectorCli {
    #[command(subcommand)]
    command: Option<CollectorCommand>,
    #[command(flatten)]
    run: CollectorArgs,
}

#[derive(Subcommand)]
enum CollectorCommand {
    /// Exit 0 if the collector answers `/healthz`, else 1 (for containers).
    Healthcheck(HealthcheckArgs),
}

#[derive(Args)]
struct HealthcheckArgs {
    /// The collector's listen address, as for `collector`: the first one
    /// given is checked, a wildcard one over loopback. Else `listen` in
    /// `config.toml`, else 127.0.0.1:7117.
    #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
    listen: Vec<String>,
    /// Where `config.toml` is read from, when no address is given.
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: Option<PathBuf>,
}

#[derive(Args, Clone)]
struct CollectorArgs {
    /// An address to listen on, e.g. `127.0.0.1:7117`. Repeatable, or
    /// comma-separated in `HENNERY_LISTEN`; the collector serves the same
    /// routes on each (kernel spec §7). Else `listen` in `config.toml`, else
    /// 127.0.0.1:7117.
    #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
    listen: Vec<String>,
    /// The collector's data directory; else the platform's (distribution
    /// spec §8).
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Where browsers reach the collector, for the setup link until setup
    /// stores its own (kernel spec §3.1). Else `public_url` in
    /// `config.toml`.
    #[arg(long, env = "HENNERY_PUBLIC_URL")]
    public_url: Option<String>,
    /// Presume a host's sessions parked once it has been offline this long.
    #[arg(long, default_value_t = hennery_sessions::offline::OFFLINE_THRESHOLD.as_secs())]
    host_offline_secs: u64,
    /// `hennery up` only: once listening, write one pairing code to this
    /// inherited descriptor (kernel spec §4.2).
    #[arg(long, hide = true, value_parser = clap::value_parser!(i32).range(3..))]
    pairing_code_fd: Option<i32>,
    /// `hennery up` only: serve on this inherited listening socket, which
    /// `up` bound, in place of binding `--listen`. Repeatable.
    #[arg(long = "listen-fd", hide = true, conflicts_with = "listen", value_parser = clap::value_parser!(i32).range(3..))]
    listen_fd: Vec<i32>,
    /// `hennery up` only: the reading end of a pipe only `up` writes to.
    /// At its end-of-file `up` is gone, and the collector stops.
    #[arg(long, hide = true, value_parser = clap::value_parser!(i32).range(3..))]
    parent_fd: Option<i32>,
}

#[derive(Args, Clone)]
struct HostArgs {
    /// Holds the pairing `hennery host join` stored (`host.key`, `host.toml`).
    /// Else the one the installed host service runs, else the platform's
    /// (distribution spec §8).
    #[arg(long, env = "HENNERY_HOST_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Agent adapter, as `name=command args…`. Repeatable. With none,
    /// `claude` and `codex` come from the installed adapter set. An agent
    /// never inherits CLAUDE_CODE_EXECUTABLE, CODEX_PATH, CODEX_CONFIG,
    /// DISABLE_MCP_CONFIG_FILTERING or APP_SERVER_LOGS: a command that needs
    /// one sets it itself, e.g. `name=/usr/bin/env CODEX_PATH=… <cmd>`.
    #[arg(long = "agent", value_parser = parse_agent)]
    agents: Vec<(String, AgentCommand)>,
    /// Park sessions idle for this many seconds; 0 turns the reaper off.
    #[arg(long, default_value_t = IDLE_TIMEOUT.as_secs())]
    idle_timeout_secs: u64,
    /// `hennery up` only: join this collector first if the host is not
    /// paired, with the code read from `--join-code-fd`.
    #[arg(long, hide = true, requires = "join_code_fd")]
    join_url: Option<String>,
    #[arg(long, hide = true, requires = "join_url", value_parser = clap::value_parser!(i32).range(3..))]
    join_code_fd: Option<i32>,
    /// `hennery up` only: the collector's host WebSocket as it listens now,
    /// in place of the stored one (its port may have changed).
    #[arg(long, hide = true)]
    collector_url: Option<String>,
    /// `hennery up` only: as for `collector`; at its end-of-file the host
    /// stops, as on SIGTERM.
    #[arg(long, hide = true, value_parser = clap::value_parser!(i32).range(3..))]
    parent_fd: Option<i32>,
    /// A directory to find projects in and to allow browsing under (ACP
    /// core §7): absolute, or `~/…`. Repeatable; when given, replaces
    /// `workspace_roots` in `host.toml`.
    #[arg(long = "workspace-root")]
    workspace_roots: Vec<String>,
    /// With no `--agent`: where the pinned adapter set comes from, if it is
    /// installed at start.
    #[command(flatten)]
    mirrors: runtime::MirrorArgs,
}

#[derive(Args)]
struct UpArgs {
    /// An address to listen on, as for `collector`; repeatable, else
    /// `listen` in the collector's `config.toml` (`<data-dir>/collector`).
    /// The host child connects over the first one that loopback reaches.
    #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
    listen: Vec<String>,
    /// As for `collector`, handed on to the collector child.
    #[arg(long, env = "HENNERY_PUBLIC_URL")]
    public_url: Option<String>,
    /// The data root, holding `collector/` and `host/`; else the platform's
    /// (distribution spec §8).
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Agent adapter for the host child, as for `host run --agent`. With
    /// none, `claude` and `codex` come from the host's installed adapter set
    /// (`<data-dir>/host`; the mirrors from `HENNERY_NPM_REGISTRY` and
    /// `HENNERY_NODE_MIRROR`). No agent inherits CLAUDE_CODE_EXECUTABLE,
    /// CODEX_PATH, CODEX_CONFIG, DISABLE_MCP_CONFIG_FILTERING or
    /// APP_SERVER_LOGS.
    #[arg(long = "agent", value_parser = parse_agent)]
    agents: Vec<(String, AgentCommand)>,
    /// Park sessions idle for this many seconds; 0 turns the reaper off.
    #[arg(long, default_value_t = IDLE_TIMEOUT.as_secs())]
    idle_timeout_secs: u64,
    /// As for `host run`, handed on to the host child: replaces
    /// `workspace_roots` in its `host.toml` (`<data-dir>/host`).
    #[arg(long = "workspace-root")]
    workspace_roots: Vec<String>,
}

fn parse_agent(s: &str) -> Result<(String, AgentCommand), String> {
    let (name, command) = s.split_once('=').ok_or("expected name=command")?;
    let command = AgentCommand::parse(command).ok_or("empty command")?;
    Ok((name.to_string(), command))
}

/// Where the collector listens when nothing says otherwise (kernel spec §7).
const DEFAULT_LISTEN: &str = "127.0.0.1:7117";

/// The most addresses one collector listens on: `up` hands each to its
/// collector child as a descriptor of its own.
pub(crate) const MAX_LISTENERS: usize = 8;

/// The addresses to listen on: those given, else `DEFAULT_LISTEN`. An empty
/// one, or more than `MAX_LISTENERS`, is refused.
fn listen_addresses(given: &[String]) -> Result<Vec<String>> {
    let addresses: Vec<String> = if given.is_empty() {
        vec![DEFAULT_LISTEN.to_string()]
    } else {
        given.iter().map(|a| a.trim().to_string()).collect()
    };
    if addresses.iter().any(String::is_empty) {
        bail!("an empty listen address: give each as host:port");
    }
    if addresses.len() > MAX_LISTENERS {
        bail!(
            "{} listen addresses; at most {MAX_LISTENERS} are supported",
            addresses.len()
        );
    }
    Ok(addresses)
}

/// Bind every address, in order (kernel spec §7): one that cannot be bound
/// fails the start, before anything else is done.
fn bind_all(addresses: &[String]) -> Result<Vec<std::net::TcpListener>> {
    addresses
        .iter()
        .map(|address| {
            let listener = std::net::TcpListener::bind(address).with_context(|| format!("bind {address}"))?;
            listener.set_nonblocking(true)?;
            Ok(listener)
        })
        .collect()
}

impl Command {
    /// The name of the log this command writes when run by a service
    /// (`log::init`): only the long-running ones have one.
    fn log_name(&self) -> Option<&'static str> {
        match self {
            Self::Up(_) => Some("up"),
            Self::Collector(CollectorCli { command: None, .. }) => Some("collector"),
            Self::Host {
                command: HostCommand::Run(_),
            } => Some("host"),
            _ => None,
        }
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    if let Err(why) = log::init(cli.command.log_name()) {
        eprintln!("Error: {why}");
        return std::process::ExitCode::FAILURE;
    }
    // Every arm returns a `Result<ExitCode>` (not just `Result<()>`), and
    // nothing on this path ever calls `std::process::exit`: that call tears
    // down the whole process immediately, without unwinding this `async fn`
    // or dropping the `#[tokio::main]` runtime — so any other task still
    // running on it (a session actor mid-launch of a slow adapter, say)
    // never gets to run its own `Drop` and never kills its adapter's process
    // group. Returning an `ExitCode` here instead lets the runtime finish
    // and drop normally first, exactly like a plain `Ok(())` return always
    // did; only then does the process actually exit with that code.
    let result = match cli.command {
        Command::Collector(CollectorCli {
            command: Some(CollectorCommand::Healthcheck(args)),
            ..
        }) => Ok(collector_healthcheck(args)),
        Command::Collector(CollectorCli {
            run: args,
            command: None,
        }) => run_collector(args).await.map(|()| std::process::ExitCode::SUCCESS),
        Command::Host {
            command: HostCommand::Join(args),
        } => join_host(args).await.map(|()| std::process::ExitCode::SUCCESS),
        Command::Host {
            command: HostCommand::Run(args),
        } => run_host(args).await,
        Command::Host {
            command: HostCommand::Adapters { command },
        } => runtime::run(command).await.map(|()| std::process::ExitCode::SUCCESS),
        Command::Up(args) => run_up(args).await,
        Command::Admin(args) => admin::run(args).await.map(|()| std::process::ExitCode::SUCCESS),
        Command::Service(args) => service::run(args),
        Command::Doctor(args) => doctor::run(args),
    };
    match result {
        Ok(code) => code,
        // `{err:?}`, not `{err:#}`: this replaces the default `Result<(),
        // E>` `Termination`, which prints via `Debug` — keep the same output
        // other tests (and operators) already read the exit-1 path by.
        Err(err) => {
            eprintln!("Error: {err:?}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// `collector healthcheck`: 0 if healthy, else 1 and why, on standard
/// error (Docker keeps it in the container's health log).
fn collector_healthcheck(args: HealthcheckArgs) -> std::process::ExitCode {
    let checked = (|| {
        let file = match &args.data_dir {
            Some(dir) => config::FileConfig::load(dir)?,
            None => config::FileConfig::default(),
        };
        let addresses = listen_addresses(&file.listen(&args.listen)?)?;
        healthcheck::check(healthcheck::probe_address(&addresses[0])?, healthcheck::TIMEOUT)
    })();
    match checked {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("unhealthy: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run_collector(args: CollectorArgs) -> Result<()> {
    let data_dir = data_dir::or_default(args.data_dir.clone(), service::unit::Role::Collector)?;
    // First: a SIGINT or SIGTERM from here on shuts down cleanly, removing
    // the admin socket, instead of killing the collector by the default
    // action while it starts.
    let mut signals = Signals::new()?;
    // Before anything is created: the pipe `up` hands over, or a clear
    // refusal.
    if let Some(fd) = args.pairing_code_fd {
        inherit::check_pipe("--pairing-code-fd", fd)?;
    }
    let parent = args.parent_fd.map(inherit::watch_parent).transpose()?;
    warn_if_dev_token();
    let file = config::FileConfig::load(&data_dir)?;
    // Named by its source: an operator cannot otherwise tell whether a flag,
    // `HENNERY_PUBLIC_URL` or the file gave the bad value.
    let public_url_source = if args.public_url.is_some() {
        "--public-url or HENNERY_PUBLIC_URL".to_string()
    } else {
        data_dir.join(config::CONFIG_FILE).display().to_string()
    };
    let public_url = file
        .public_url(args.public_url.as_deref())
        .map(|url| {
            PublicUrl::parse(&url)
                .map_err(|why| anyhow::anyhow!("{why}"))
                .with_context(|| public_url_source.clone())
        })
        .transpose()?;
    // Before anything is created: a descriptor that is not a listening TCP
    // socket, or an address that is taken, must fail here, and clearly.
    let listeners = if args.listen_fd.is_empty() {
        bind_all(&listen_addresses(&file.listen(&args.listen)?)?)?
    } else {
        if args.listen_fd.len() > MAX_LISTENERS {
            bail!("more than {MAX_LISTENERS} --listen-fd");
        }
        // One socket adopted twice would be two owners of one descriptor.
        for (i, fd) in args.listen_fd.iter().enumerate() {
            if args.listen_fd[..i].contains(fd) {
                bail!("--listen-fd {fd} is given twice");
            }
        }
        args.listen_fd
            .iter()
            .map(|&fd| inherited_listener(fd))
            .collect::<Result<_>>()?
    };
    private_data_dir(&data_dir)?;
    // Before the database and the setup link: a second collector on this
    // data directory stops here, while the first still answers on it.
    let admin_socket = hennery_kernel::admin::bind(&data_dir)?;
    let db = data_dir.join("hennery.db");
    let store = Store::open(&db)?;
    let hosts = Hosts::open(&db)?;
    let operator = Operator::open(&db)?;
    let mut state = AppState::new(store, hosts, operator);
    // Before anything serves: every push subscription is bound to this key
    // (kernel spec §6).
    state.vapid = std::sync::Arc::new(hennery_kernel::push::VapidKey::load_or_create(&data_dir)?);
    // The collector's one outbound HTTP policy (kernel spec §7.1), built
    // once: Web Push takes it here, and the gateway shares this same one
    // (agreed with plan 8), never a second. Delivery starts before anything
    // serves, so a notice queued meanwhile reaches it, not the queue nobody
    // reads (plan 10b-ii).
    let egress = hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT)?;
    start_push(&mut state, &egress);
    // Before serving: the gateway's store, and the master key that opens
    // what it holds (plan 8a), from `HENNERY_MASTER_KEY`, a systemd
    // credential or `<data>/master.key`. A key that is missing while
    // credentials are stored, or that does not open them, stops the start
    // (plan 8a decision 8; `KeyUnavailable` tells that case apart).
    let keys = hennery_gateway::key::KeySource::from_env(&data_dir)?;
    let gateway = hennery_gateway::open(&db, &keys, state.operator.clone())?;
    // The gateway's proxy (plan 8d), `/mcp/<slug>`: bearer tokens, beside
    // the operator's routes and outside them (lane L8), sending only
    // through the kernel's egress policy, the collector's one `Egress`
    // above, shared with Web Push.
    let proxy = hennery_gateway::proxy::ProxyState::full(
        std::sync::Arc::new(hennery_gateway::scope::ProxyStore::open(&db)?),
        gateway.store.clone(),
        gateway.key.clone(),
        egress.clone(),
        hennery_gateway::proxy::Limits::default(),
    );
    state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
    hennery_sessions::offline::after_startup(&state);
    hennery_sessions::sweep::after_startup(&state);
    let listeners = listeners
        .into_iter()
        .map(tokio::net::TcpListener::from_std)
        .collect::<std::io::Result<Vec<_>>>()
        .context("serve the listening sockets")?;
    // One line per listener, in order: the first names the setup link's port.
    let mut addresses = Vec::new();
    for listener in &listeners {
        let address = listener.local_addr()?;
        tracing::info!(%address, "collector listening");
        addresses.push(address);
    }
    // Only once listening: the link names the first listener's port, unless
    // there is a `public_url` to name (kernel spec §3.1).
    let base_url = match &public_url {
        Some(url) => url.origin().to_string(),
        None => format!("http://localhost:{}", addresses[0].port()),
    };
    warn_if_public_url_differs(state.operator.public_url().as_ref(), public_url.as_ref());
    if let Some(link) = state
        .operator
        .announce_setup(&data_dir, &base_url, hennery_kernel::secret::unix_now())?
    {
        announce_setup(&link);
    }
    if let Some(fd) = args.pairing_code_fd {
        // Only once migrated and listening: the host enrolls right away.
        let code = state
            .hosts
            .mint_local_pairing_code(hennery_kernel::secret::unix_now())?;
        // The host child may have died already (e.g. it could not bind, or
        // was killed) — closing its end of the pipe before this write. That
        // is the host's problem, not the collector's: it must keep serving
        // every other route regardless. Logged without the code itself.
        if let Err(err) = inherit::write_code(fd, &code.code) {
            tracing::warn!(error = %err, "could not hand the pairing code to the host child");
        }
    }
    let shutdown = state.shutdown.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = signals.recv() => {}
            () = inherit::parent_gone(parent) => {}
        }
        shutdown.cancel();
    });
    // Served on the router's own operator and registry (kernel spec §4.2).
    let admin = admin_socket.map(|socket| {
        let admin = hennery_kernel::admin::Admin {
            operator: state.operator.clone(),
            hosts: state.hosts.clone(),
            dir: data_dir.clone(),
            base_url,
        };
        tokio::spawn(hennery_kernel::admin::serve(
            socket,
            admin,
            state.shutdown.clone().cancelled_owned(),
        ))
    });
    let served = hennery_sessions::serve_all(
        listeners,
        hennery_sessions::router(state.clone())
            .merge(hennery_gateway::api::router(gateway))
            .merge(hennery_gateway::proxy::router(proxy)),
        state.shutdown.clone(),
    )
    .await;
    // Also when serving failed: the admin socket is removed once it stops.
    state.shutdown.cancel();
    if let Some(admin) = admin {
        let _ = admin.await;
    }
    served?;
    Ok(())
}

/// Adopt `--listen-fd`: `fd` must be an open, listening TCP socket. Anything
/// else is refused, not adopted: a closed descriptor would abort the process
/// on first use, a UDP or unconnected socket would hang it, and a Unix
/// socket would be served as if it were TCP. The socket is made
/// close-on-exec and non-blocking.
fn inherited_listener(fd: i32) -> Result<std::net::TcpListener> {
    let option = |name: libc::c_int| -> std::io::Result<libc::c_int> {
        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: getsockopt(2) into a local int of the size it is told.
        let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, name, (&raw mut value).cast(), &mut len) };
        if rc < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(value)
        }
    };
    // SAFETY: fcntl(2) on a descriptor number; it only reads its flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        bail!(
            "--listen-fd {fd} is not an open descriptor: {}",
            std::io::Error::last_os_error()
        );
    }
    let kind = option(libc::SO_TYPE).with_context(|| format!("--listen-fd {fd} is not a socket"))?;
    if kind != libc::SOCK_STREAM {
        bail!("--listen-fd {fd} is not a stream (TCP) socket");
    }
    let family = socket_family(fd).with_context(|| format!("--listen-fd {fd}: getsockname"))?;
    if family != libc::AF_INET && family != libc::AF_INET6 {
        bail!("--listen-fd {fd} is not a TCP socket (address family {family}, not IPv4 or IPv6)");
    }
    let listening = match option(libc::SO_ACCEPTCONN) {
        Ok(value) => value == 1,
        // macOS has no `SO_ACCEPTCONN` to read; its TCP state tells.
        Err(err) if err.raw_os_error() == Some(libc::ENOPROTOOPT) => {
            listens_by_tcp_state(fd).with_context(|| format!("--listen-fd {fd}: TCP_CONNECTION_INFO"))?
        }
        Err(err) => return Err(err).with_context(|| format!("--listen-fd {fd}: SO_ACCEPTCONN")),
    };
    if !listening {
        bail!("--listen-fd {fd} is not a listening socket");
    }
    // SAFETY: as above; the flags just read, plus close-on-exec.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error()).context("make --listen-fd close-on-exec");
    }
    // SAFETY: `fd` is an open listening socket, inherited for exactly this;
    // nothing else in this process owns it.
    let listener = unsafe { <std::net::TcpListener as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// The address family of socket `fd`, from getsockname(2).
fn socket_family(fd: i32) -> std::io::Result<libc::c_int> {
    // SAFETY: getsockname(2) into local storage of the size it is told;
    // all-zero bytes are a valid `sockaddr_storage`.
    unsafe {
        let mut addr: libc::sockaddr_storage = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if libc::getsockname(fd, (&raw mut addr).cast(), &mut len) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(libc::c_int::from(addr.ss_family))
    }
}

/// Whether TCP socket `fd` listens, from its TCP state (`TCPS_LISTEN`):
/// macOS's stand-in for `SO_ACCEPTCONN`. A socket bound and never listened
/// on is refused, as is one never bound and a connected one.
#[cfg(target_vendor = "apple")]
fn listens_by_tcp_state(fd: i32) -> std::io::Result<bool> {
    /// `TCPS_LISTEN` in `<netinet/tcp_fsm.h>`.
    const TCPS_LISTEN: u8 = 1;
    // SAFETY: all-zero bytes are a valid `tcp_connection_info` (integers).
    let mut info: libc::tcp_connection_info = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::tcp_connection_info>() as libc::socklen_t;
    // SAFETY: getsockopt(2) into a local struct of the size it is told.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_CONNECTION_INFO,
            (&raw mut info).cast(),
            &mut len,
        )
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(info.tcpi_state == TCPS_LISTEN)
}

/// Elsewhere `SO_ACCEPTCONN` answers, so this is never reached.
#[cfg(not(target_vendor = "apple"))]
fn listens_by_tcp_state(_fd: i32) -> std::io::Result<bool> {
    Err(std::io::Error::from_raw_os_error(libc::ENOPROTOOPT))
}

/// The development bearer's variable, from before 3b-i. Nothing reads it
/// now; it is still stripped from every child (`HOST_SECRET_VARS`).
const DEV_TOKEN_VAR: &str = "HENNERY_DEV_TOKEN";

/// Warn, once at start, that `HENNERY_DEV_TOKEN` does nothing any more: a
/// service definition that still sets it would otherwise look like it
/// protects something. Its value is never logged.
fn warn_if_dev_token() {
    if std::env::var_os(DEV_TOKEN_VAR).is_some() {
        tracing::warn!(
            "{DEV_TOKEN_VAR} is set but no longer used: since operator auth (3b-i), operators sign in through \
             the setup link and a password; remove it from the environment"
        );
    }
}

/// Once set up, the stored `public_url` is the one in effect (kernel spec
/// §2): a configured one that differs is named in a warning, not used.
fn warn_if_public_url_differs(stored: Option<&PublicUrl>, configured: Option<&PublicUrl>) {
    if let (Some(stored), Some(configured)) = (stored, configured)
        && stored != configured
    {
        tracing::warn!(
            stored = stored.origin(),
            configured = configured.origin(),
            "the configured public_url is not the one setup stored, which stays in effect; \
             run `hennery admin reset-public-url` to move it"
        );
    }
}

/// Tell the operator where the setup link is (kernel spec §3.1): the link
/// itself only to a terminal, so the token never lands in a log collector;
/// otherwise only the path of the file that holds it.
fn announce_setup(link: &SetupLink) {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        println!(
            "hennery is not set up yet. Open this link within the hour to set it up:\n  {}",
            link.url
        );
    } else {
        tracing::info!(
            file = %link.file.display(),
            "hennery is not set up yet; the one-time setup link (valid for an hour) is in this file"
        );
    }
}

/// Create `dir`, and any parent it lacks, 0700 whatever the umask: the
/// collector's database holds what only its user may read (decision 5). A
/// directory that already exists is the operator's and is left as it is,
/// but named in a warning if other users can reach into it.
fn private_data_dir(dir: &std::path::Path) -> Result<()> {
    hennery_host::identity::create_private_dir(dir)?;
    if !hennery_host::identity::is_private(dir)? {
        tracing::warn!(
            dir = %dir.display(),
            "the data directory is readable by other users; `chmod 700` it"
        );
    }
    Ok(())
}

async fn join_host(args: JoinArgs) -> Result<()> {
    let data_dir = data_dir::host_or_installed(args.data_dir.clone(), data_dir::HostUse::Join)?;
    // Before the code is spent: a bad mirror stops the join here.
    if !args.no_runtime {
        args.mirrors.sources()?;
    }
    let name = args.name.unwrap_or_else(hennery_host::pairing::default_name);
    let code = match args.code {
        Some(code) => code,
        None => read_code_from_stdin()?,
    };
    let host_id = match hennery_host::pairing::join(&args.url, &code, &data_dir, &name).await? {
        Joined::Paired { host_id } => {
            println!("paired as {host_id}");
            host_id
        }
        Joined::AlreadyPaired { host_id } => {
            println!("already paired as {host_id}");
            host_id
        }
    };
    if args.no_runtime {
        return Ok(());
    }
    // Then the runtime (distribution spec §3.2); the pairing stands either way.
    runtime::after_join(&data_dir, &host_id, &args.use_cli, &args.mirrors).await
}

/// The most bytes of standard input `host join` takes for one code: a code
/// is 9 characters, so this leaves room for whitespace and nothing else.
const MAX_CODE_LINE: usize = 256;

/// One line of standard input, prompted for on a terminal. Read only up to
/// `MAX_CODE_LINE` bytes: a longer line is refused once that many have come,
/// not waited on to its end, and never echoed.
fn read_code_from_stdin() -> Result<String> {
    use std::io::{BufRead, IsTerminal, Read, Write};
    if std::io::stdin().is_terminal() {
        eprint!("Pairing code: ");
        std::io::stderr().flush()?;
    }
    // One byte over the bound: `MAX_CODE_LINE` bytes and then the newline
    // is still a line within it.
    let mut line = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_CODE_LINE as u64 + 1)
        .read_until(b'\n', &mut line)
        .context("read the pairing code from standard input")?;
    if line.len() > MAX_CODE_LINE && line.last() != Some(&b'\n') {
        bail!("the pairing code on standard input is longer than {MAX_CODE_LINE} bytes");
    }
    let Ok(line) = String::from_utf8(line) else {
        bail!("the pairing code on standard input is not UTF-8");
    };
    let code = line.trim().to_string();
    if code.is_empty() {
        bail!("no pairing code: give it after the URL, or on standard input");
    }
    Ok(code)
}

async fn run_host(args: HostArgs) -> Result<std::process::ExitCode> {
    let data_dir = data_dir::host_or_installed(args.data_dir.clone(), data_dir::HostUse::Run)?;
    // Before anything else, the paired branch's `close` included: the pipe
    // `up` hands over, or a clear refusal.
    if let Some(fd) = args.join_code_fd {
        inherit::check_pipe("--join-code-fd", fd)?;
    }
    let parent = args.parent_fd.map(inherit::watch_parent).transpose()?;
    let under_up = args.parent_fd.is_some();
    // Before the first spawn, the installer's included: the pipe whose
    // end-of-file kills every adapter's group when this host dies.
    hennery_host::adapter::prepare_death_pipe().context("the adapters' death pipe")?;
    warn_if_dev_token();
    let home = hennery_host::projects::home_dir();
    // The flags first, before anything is paired: a bad one must not spend
    // a pairing code (decision 6). The file's roots are checked below.
    if !args.workspace_roots.is_empty() {
        hennery_host::projects::workspace_roots(&args.workspace_roots, &[], home.as_deref())?;
    }
    // Held until this returns (distribution spec §8): a second host on this
    // data directory stops here, before anything reads its pairing.
    // `Paired::load` is not only a read: it rolls an interrupted pairing
    // forward (renames, and moves the outbox aside). A directory that is not
    // there yet is made, 0700, only by `up`'s first start, which pairs in it.
    if !data_dir.exists() && args.join_url.is_some() {
        hennery_host::identity::create_private_dir(&data_dir)?;
    }
    if !data_dir.is_dir() {
        bail!(
            "{} holds no pairing; run `hennery host join <url> <code>` first",
            data_dir.display()
        );
    }
    let _lock = lock::acquire(&data_dir, lock::HOST_LOCK, "hennery host run")?;
    let paired = match Paired::load(&data_dir)? {
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
                    data_dir.display()
                );
            };
            let code = inherit::read_code(fd).await?;
            let name = hennery_host::pairing::default_name();
            hennery_host::pairing::join(url, &code, &data_dir, &name).await?;
            Paired::load(&data_dir)?.context("the pairing just stored")?
        }
    };
    // Checked before connecting: a bad root fails the start (decision 6).
    let workspace_roots =
        hennery_host::projects::workspace_roots(&args.workspace_roots, &paired.workspace_roots, home.as_deref())?;
    // No `--agent`: `claude` and `codex` from the installed set, the pinned
    // one installed first if it is not current (distribution spec §3.2).
    // The set stays held in use while the host runs.
    // A `--agent` command is a generic agent (ACP core §6): no profile.
    // With `--agent`, no app-server either: a Codex forget takes the
    // fallback.
    let (agents, profiles, codex_app_server, _set_in_use) = if args.agents.is_empty() {
        runtime::default_agents(&data_dir, &args.mirrors).await
    } else {
        (args.agents.into_iter().collect(), Default::default(), None, None)
    };
    // The pairing's collector, as `host join` takes it again: never a
    // `--collector-url` override (development only).
    let paired_collector = paired.collector_url.clone();
    let collector_url = args.collector_url.unwrap_or(paired.collector_url);
    // What a revoke records, beside the pairing it names.
    let record_dir = data_dir.clone();
    let revoke = revoked::Revoked {
        host_id: paired.host_id.clone(),
        collector_url: paired_collector,
        at: 0,
    };
    let mut cfg = HostConfig::new(collector_url, paired.host_id, paired.key, data_dir);
    cfg.workspace_roots = workspace_roots;
    cfg.home = home;
    cfg.agents = agents;
    cfg.profiles = profiles;
    cfg.codex_app_server = codex_app_server;
    cfg.idle_timeout = std::time::Duration::from_secs(args.idle_timeout_secs);
    // On SIGINT/SIGTERM, and on a revoke, the host stops its connection and
    // waits (bounded) for every session actor to SIGTERM its adapter's group
    // and SIGKILL it after the grace. `run_until` returning — however it
    // returns — is what lets `main` return in turn, which is what lets the
    // `#[tokio::main]` runtime drop normally: only that drop reaps any
    // actor `run_until`'s own bound gave up waiting on (one still starting a
    // slow adapter, say), by dropping its still-running task and, with it,
    // the `Adapter` whose `Drop` kills the whole process group. A bare
    // `std::process::exit` here would skip all of that.
    // `up` gone (its pipe's end-of-file) stops the host as a signal does.
    let shutdown = async {
        tokio::select! {
            () = terminated() => {}
            () = inherit::parent_gone(parent) => {}
        }
    };
    match hennery_host::run_until(cfg, shutdown).await {
        // Its own exit code, so `hennery up` can tell a revoke apart; and a
        // record of it, still under `host.lock`, for `service status` and
        // doctor (launchd's host exits 0, so the code does not say it).
        Err(err) if hennery_host::connection::revoked(&err) => {
            eprintln!("Error: {err:#}");
            let revoke = revoked::Revoked {
                at: hennery_kernel::secret::unix_now(),
                ..revoke
            };
            if let Err(why) = revoked::record(&record_dir, &revoke) {
                eprintln!("warning: the revoke could not be recorded: {why:#}");
            }
            let code = revoked::exit_code(std::env::var("HENNERY_SERVICE").ok().as_deref(), under_up);
            if code == 0 {
                // SAFETY: getuid(2) cannot fail.
                let uid = unsafe { libc::getuid() };
                let start = format!("launchctl kickstart -k gui/{uid}/{}", service::unit::Role::Host.label());
                let (_, fix) = revoke.said(&start);
                eprintln!(
                    "this host stays down: launchd does not start it again after exit 0. To bring it back, {fix}"
                );
            }
            Ok(std::process::ExitCode::from(code))
        }
        Err(err) => Err(err),
        Ok(()) => Ok(std::process::ExitCode::SUCCESS),
    }
}

/// Resolves on SIGINT or SIGTERM.
async fn terminated() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

/// SIGINT and SIGTERM for `up` and the collector, caught from the moment it
/// is made: unlike `terminated`, whose handlers exist only once it is first
/// polled.
struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

impl Signals {
    fn new() -> Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt()).context("SIGINT handler")?,
            terminate: signal(SignalKind::terminate()).context("SIGTERM handler")?,
        })
    }

    /// Resolves on the next SIGINT or SIGTERM, also one sent before it was
    /// called.
    async fn recv(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
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

/// `up`'s host child, but for the pairing descriptor (`UpChildren::host`
/// adds it).
fn host_command(
    exe: &std::path::Path,
    host_dir: &std::path::Path,
    collector_ws_url: &str,
    args: &UpArgs,
) -> tokio::process::Command {
    let mut host_cmd = tokio::process::Command::new(exe);
    host_cmd
        .args(["host", "run"])
        .arg("--data-dir")
        .arg(host_dir)
        .arg("--collector-url")
        .arg(collector_ws_url)
        .arg("--idle-timeout-secs")
        .arg(args.idle_timeout_secs.to_string())
        .kill_on_drop(true)
        .process_group(0);
    // `up` may still have the old development token in its environment (a
    // shell set up before it was removed); the host needs none, and its
    // agents must never see it. The adapter spawn strips the same list
    // again, for a host started by hand.
    for var in hennery_host::adapter::HOST_SECRET_VARS {
        host_cmd.env_remove(var);
    }
    for root in &args.workspace_roots {
        host_cmd.arg("--workspace-root").arg(root);
    }
    for (name, command) in &args.agents {
        let mut spec = format!("{name}={}", command.program);
        for a in &command.args {
            spec.push(' ');
            spec.push_str(a);
        }
        host_cmd.arg("--agent").arg(spec);
    }
    host_cmd
}

/// What a host data directory holds of a pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pairing {
    None,
    /// `host.key` and `host.toml`.
    Whole,
    /// Staged by a join and not yet put in place (`host.toml.pending`).
    Interrupted,
}

/// What `dir` holds of a host's pairing, from the files alone. Not
/// `Paired::load`, which rolls an interrupted pairing forward (renames, the
/// outbox moved aside): only the host may do that, under `host.lock`
/// (decision 8 of plan 7c).
fn pairing_in(dir: &std::path::Path) -> Pairing {
    use hennery_host::identity::{CONFIG_FILE, KEY_FILE};
    if dir.join(format!("{CONFIG_FILE}.pending")).exists() {
        Pairing::Interrupted
    } else if dir.join(KEY_FILE).is_file() && dir.join(CONFIG_FILE).is_file() {
        Pairing::Whole
    } else {
        Pairing::None
    }
}

/// What `up` starts its children from, again after a crash.
struct UpChildren<'a> {
    exe: PathBuf,
    args: &'a UpArgs,
    /// `up`'s data root.
    data_dir: PathBuf,
    host_dir: PathBuf,
    collector_url: String,
    collector_ws_url: String,
    /// Bound once, by `up`, and kept for as long as it runs: a collector
    /// started again serves the same ports, which the host's URL names.
    /// While none runs, connections wait in the backlog.
    listeners: Vec<std::net::TcpListener>,
    /// Made before the first child and held until `up` ends: every child
    /// gets the reading end (`--parent-fd`), and only `up` holds the
    /// writing end, so however `up` dies, each child reads end-of-file and
    /// stops (plan 7c-iii).
    parent: (std::io::PipeReader, std::io::PipeWriter),
}

impl UpChildren<'_> {
    /// The collector child, handed the listening sockets and, on the first
    /// start of a host that is not paired, the pipe's writing end.
    fn collector(&self, pairing: Option<&std::io::PipeWriter>) -> tokio::process::Command {
        // Both children are kept out of the terminal's foreground process
        // group: a Ctrl-C there delivers SIGINT to every process in that
        // group at once, which would race each child's own signal handler
        // against the ordered shutdown. With their own group, only this
        // supervisor is signalled and it alone decides the order (host, then
        // collector).
        let mut cmd = tokio::process::Command::new(&self.exe);
        cmd.arg("collector");
        let mut fds = Vec::new();
        for (listener, to) in self.listeners.iter().zip(inherit::LISTENER_FD..) {
            cmd.arg("--listen-fd").arg(to.to_string());
            fds.push((listener.as_raw_fd(), to));
        }
        cmd.arg("--data-dir")
            .arg(self.data_dir.join("collector"))
            // `up` has warned about it already; the collector has no use for it.
            .env_remove(DEV_TOKEN_VAR)
            // `up` bound these addresses already; the child takes the sockets.
            .env_remove("HENNERY_LISTEN")
            .kill_on_drop(true)
            .process_group(0);
        if let Some(public_url) = &self.args.public_url {
            cmd.arg("--public-url").arg(public_url);
        }
        if let Some(writer) = pairing {
            fds.push((writer.as_raw_fd(), inherit::CHILD_FD));
            cmd.arg("--pairing-code-fd").arg(inherit::CHILD_FD.to_string());
        }
        fds.push((self.parent.0.as_raw_fd(), inherit::PARENT_FD));
        cmd.arg("--parent-fd").arg(inherit::PARENT_FD.to_string());
        inherit::pass_to_child(&mut cmd, &fds);
        cmd
    }

    /// The host child, handed the pipe's reading end on its first start if
    /// it is not paired.
    fn host(&self, pairing: Option<&std::io::PipeReader>) -> tokio::process::Command {
        let mut cmd = host_command(&self.exe, &self.host_dir, &self.collector_ws_url, self.args);
        let mut fds = vec![(self.parent.0.as_raw_fd(), inherit::PARENT_FD)];
        cmd.arg("--parent-fd").arg(inherit::PARENT_FD.to_string());
        if let Some(reader) = pairing {
            fds.push((reader.as_raw_fd(), inherit::CHILD_FD));
            cmd.arg("--join-url")
                .arg(&self.collector_url)
                .arg("--join-code-fd")
                .arg(inherit::CHILD_FD.to_string());
        }
        inherit::pass_to_child(&mut cmd, &fds);
        cmd
    }
}

impl supervisor::Children for UpChildren<'_> {
    type Child = tokio::process::Child;

    fn spawn(&mut self, which: supervisor::Which) -> Result<tokio::process::Child> {
        let mut cmd = match which {
            supervisor::Which::Collector => self.collector(None),
            supervisor::Which::Host => self.host(None),
        };
        Ok(cmd.spawn()?)
    }

    fn host_paired(&self) -> bool {
        pairing_in(&self.host_dir) == Pairing::Whole
    }

    fn revoked(&self) {
        tracing::warn!(
            "the all-in-one host was revoked; the collector keeps serving. To pair it again, stop `hennery up`, remove {} and {}, and start it again",
            self.host_dir.join(hennery_host::identity::KEY_FILE).display(),
            self.host_dir.join(hennery_host::identity::CONFIG_FILE).display()
        );
    }
}

/// Two child processes of this binary, exchanging the same frames as a
/// remote host (architecture spec §3.3), started again when they crash
/// (distribution spec §5.2; see `supervisor::supervise`). `up` exits 0 when
/// a signal stopped it, and 1 when it could not go on.
async fn run_up(args: UpArgs) -> Result<std::process::ExitCode> {
    let data_dir = data_dir::or_default(args.data_dir.clone(), service::unit::Role::Up)?;
    // First, before any child exists: from here on a SIGINT or SIGTERM is
    // caught and waits for `supervisor::supervise`, which stops both
    // children. Caught
    // only once the loop first ran, one sent just after the collector's
    // spawn killed `up` by the default action and left that collector
    // running with nobody to stop it.
    let mut signals = Signals::new()?;
    warn_if_dev_token();
    let exe = std::env::current_exe()?;
    let host_dir = data_dir.join("host");
    let file = config::FileConfig::load(&data_dir.join("collector"))?;
    let addresses = listen_addresses(&file.listen(&args.listen)?)?;
    // Validated before any child starts: with no address that loopback
    // reaches (or only schemes it cannot make sense of) `up` fails here,
    // not after the collector is already up and serving.
    let Some(host_listener) = addresses
        .iter()
        .position(|address| hennery_host::pairing::collector_ws_url(&loopback_url(address)).is_ok())
    else {
        let err = hennery_host::pairing::collector_ws_url(&loopback_url(&addresses[0]))
            .expect_err("no address passed the check");
        return Err(err.context("the all-in-one host reaches its collector over loopback"));
    };
    // Bound here and handed to the collector child, so the host's URL names
    // the port the collector serves on, also for `--listen` port 0. Before
    // the data root is touched: a busy port leaves nothing behind.
    let listeners = bind_all(&addresses)?;
    let collector_url = loopback_url(&listeners[host_listener].local_addr()?.to_string());
    let collector_ws_url = hennery_host::pairing::collector_ws_url(&collector_url)?;
    // Before either child creates its own directory in it.
    private_data_dir(&data_dir)?;
    // Held until `up` returns, before either child starts: a second `up` on
    // this data root would run a second collector on its database, also
    // where the collector's admin socket cannot tell (its path too long).
    let _lock = lock::acquire(&data_dir, lock::UP_LOCK, "hennery up")?;
    // The host pairs itself on first start only; a pairing that was revoked
    // is not replaced (kernel spec §4.2). One left half-done needs no code:
    // the host child finishes it, under its lock.
    let pairing = match pairing_in(&host_dir) {
        Pairing::Whole | Pairing::Interrupted => None,
        Pairing::None => Some(std::io::pipe()?),
    };
    let mut children = UpChildren {
        exe,
        args: &args,
        data_dir: data_dir.clone(),
        host_dir,
        collector_url,
        collector_ws_url,
        listeners,
        // Made here, before any child: on macOS `std::io::pipe` marks its
        // ends close-on-exec only after making them, so a spawn on another
        // thread meanwhile could inherit the writing end and keep it open.
        // Nothing else spawns while `up` starts, and it is never made again.
        parent: std::io::pipe()?,
    };
    // A low `ulimit -n` can make `pass_to_child`'s `F_DUPFD_CLOEXEC` at
    // `MOVE_FLOOR` fail with a bare "Invalid argument (os error 22)": named
    // here so that is not left a mystery.
    let collector = children
        .collector(pairing.as_ref().map(|(_, writer)| writer))
        .spawn()
        .context("pass the listening sockets to the collector")?;
    let host = children.host(pairing.as_ref().map(|(reader, _)| reader)).spawn()?;
    // Both children hold their ends now; with the supervisor's copies
    // closed, the host sees end-of-file if the collector dies first.
    drop(pairing);
    let report = |collector: &supervisor::ChildReport, host: &supervisor::ChildReport| {
        let state = supervisor::State {
            pid: std::process::id(),
            updated_at: hennery_kernel::secret::unix_now().try_into().unwrap_or(0),
            collector: collector.clone(),
            host: host.clone(),
        };
        if let Err(err) = supervisor::write_state(&data_dir, &state) {
            tracing::warn!(error = %format!("{err:#}"), "could not write {}", supervisor::STATE_FILE);
        }
    };
    let outcome = supervisor::supervise(
        &mut children,
        collector,
        host,
        &supervisor::POLICY,
        signals.recv(),
        report,
    )
    .await;
    match outcome {
        supervisor::Outcome::Stopped => Ok(std::process::ExitCode::SUCCESS),
        supervisor::Outcome::Failed(why) => {
            eprintln!("Error: {why}");
            Ok(std::process::ExitCode::FAILURE)
        }
    }
}

/// Web Push delivery (plan 10b-ii): the state's notices go to a task that
/// sends them through `egress`, public addresses only.
fn start_push(state: &mut AppState, egress: &hennery_kernel::egress::Egress) {
    state.push =
        hennery_kernel::delivery::spawn(state.hosts.clone(), state.operator.clone(), state.vapid.clone(), egress);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plan 10b-ii: the collector's notices reach delivery, and delivery is
    /// public only. A subscription at a loopback address gets a notice
    /// refused, recorded on it; nothing is sent.
    #[tokio::test]
    async fn the_collectors_notices_go_to_a_public_only_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let mut state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        let now = hennery_kernel::secret::unix_now();
        let token = state.operator.issue_setup_token(now).unwrap().unwrap();
        state
            .operator
            .set_up(&token, "correct horse battery", "https://hennery.example", now)
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute(
                "INSERT INTO push_subscriptions(id, owner_id, endpoint, p256dh, auth, device_label, auth_session, created_at)
                 VALUES ('push-a', ?1, ?2,
                     'BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY',
                     '_ordMnz7uTCmrpBTeUV4Bw', 'test', 's', ?3)",
                rusqlite::params![
                    state.hosts.owner_id(),
                    format!("http://{}/push", listener.local_addr().unwrap()),
                    now
                ],
            )
            .unwrap();
        start_push(
            &mut state,
            &hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap(),
        );
        state.push.notify(hennery_kernel::push::Notice {
            hat_id: state.hosts.default_hat_for_new_hosts().unwrap(),
            urgency: hennery_kernel::push::Urgency::Normal,
            title: "t".into(),
            generic_title: "g".into(),
            body: "finished".into(),
            detail: None,
            url: "/sessions/s1".into(),
            tag: "s1".into(),
        });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let error = loop {
            if let Some(error) = state.hosts.subscriptions().unwrap()[0].last_error.clone() {
                break error;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the notice never reached delivery"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert!(error.contains("public address"), "{error}");
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err(), "nothing connected");
    }

    /// Final review I1: `up` may itself have the old operator bearer in its
    /// environment (`HENNERY_DEV_TOKEN`, left in a shell from before 3b);
    /// its host child must not inherit it, so neither can any agent that
    /// host runs.
    #[test]
    fn ups_host_child_does_not_inherit_the_operator_token() {
        let args = UpArgs {
            listen: vec!["127.0.0.1:7117".into()],
            public_url: None,
            data_dir: Some("/nonexistent".into()),
            agents: Vec::new(),
            idle_timeout_secs: 0,
            workspace_roots: Vec::new(),
        };
        let cmd = host_command(
            std::path::Path::new("/bin/hennery"),
            std::path::Path::new("/nonexistent/host"),
            "ws://127.0.0.1:7117/api/hosts/ws",
            &args,
        );
        let removed = cmd
            .as_std()
            .get_envs()
            .any(|(key, value)| key == "HENNERY_DEV_TOKEN" && value.is_none());
        assert!(removed, "the host child inherits HENNERY_DEV_TOKEN");
    }

    /// Plan 8a decision 6: the master key may come from the environment
    /// (`HENNERY_MASTER_KEY`), which `up` passes to its collector; its host
    /// child, and so every agent, must not inherit it. The adapter spawn
    /// strips the same list (`HOST_SECRET_VARS`).
    #[test]
    fn ups_host_child_does_not_inherit_the_master_key() {
        assert!(hennery_host::adapter::HOST_SECRET_VARS.contains(&hennery_gateway::key::KEY_ENV));
        let args = UpArgs {
            listen: vec!["127.0.0.1:7117".into()],
            public_url: None,
            data_dir: Some("/nonexistent".into()),
            agents: Vec::new(),
            idle_timeout_secs: 0,
            workspace_roots: Vec::new(),
        };
        let cmd = host_command(
            std::path::Path::new("/bin/hennery"),
            std::path::Path::new("/nonexistent/host"),
            "ws://127.0.0.1:7117/api/hosts/ws",
            &args,
        );
        let removed = cmd
            .as_std()
            .get_envs()
            .any(|(key, value)| key == hennery_gateway::key::KEY_ENV && value.is_none());
        assert!(removed, "the host child inherits HENNERY_MASTER_KEY");
    }

    /// Decision 6: `up` hands its `--workspace-root` flags to its host
    /// child, in order.
    #[test]
    fn ups_host_child_gets_its_workspace_roots() {
        let args = UpArgs {
            listen: vec!["127.0.0.1:7117".into()],
            public_url: None,
            data_dir: Some("/nonexistent".into()),
            agents: Vec::new(),
            idle_timeout_secs: 0,
            workspace_roots: vec!["/srv/projects".into(), "~/src".into()],
        };
        let cmd = host_command(
            std::path::Path::new("/bin/hennery"),
            std::path::Path::new("/nonexistent/host"),
            "ws://127.0.0.1:7117/api/hosts/ws",
            &args,
        );
        let argv: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let roots: Vec<&str> = argv
            .windows(2)
            .filter(|w| w[0] == "--workspace-root")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(roots, ["/srv/projects", "~/src"]);
    }

    /// Plan 7c, decision 8: `up` judges its host's pairing from the files,
    /// never by `Paired::load`, which would put a staged pairing in place
    /// without the host's lock. A staged one is neither whole nor touched.
    #[test]
    fn the_pairing_is_judged_from_the_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(pairing_in(dir.path()), Pairing::None);
        std::fs::write(dir.path().join("host.key"), "k").unwrap();
        assert_eq!(pairing_in(dir.path()), Pairing::None);
        std::fs::write(dir.path().join("host.toml"), "t").unwrap();
        assert_eq!(pairing_in(dir.path()), Pairing::Whole);
        std::fs::write(dir.path().join("host.toml.pending"), "staged").unwrap();
        assert_eq!(pairing_in(dir.path()), Pairing::Interrupted);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("host.toml.pending")).unwrap(),
            "staged"
        );
        assert_eq!(pairing_in(&dir.path().join("missing")), Pairing::None);
    }
}
