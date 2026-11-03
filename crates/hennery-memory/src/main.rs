//! Explicit standalone service: no default data path and no core installer.
use clap::{Parser, Subcommand};
use hennery_memory::{
    consumer::{self, FeedConfig},
    keys,
    store::Store,
    ui,
};
use std::io::Read;
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
};
#[derive(Parser)]
#[command(
    name = "hennery-memory",
    version,
    about = "Optional memory MCP service; no model required"
)]
struct Args {
    #[arg(long)]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Preview an uncompressed, regular-file-only vault snapshot; no entries are written.
    ImportPreview {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long)]
        vault: String,
        #[arg(long)]
        mapping: PathBuf,
    },
    /// Apply exactly a previously reviewed, signed preview and content/mapping digest.
    ImportApply {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        preview: PathBuf,
        #[arg(long)]
        digest: String,
    },
    /// Bootstrap this plug-in's independent browser authentication. Password from stdin.
    UiInit {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        public_url: String,
    },
    /// Read a C1 grant from bounded stdin, never command arguments.
    Attach {
        #[arg(long)]
        namespace: String,
    },
    /// Explicit filesystem-owner erasure, including after an offline revoke.
    Purge {
        #[arg(long)]
        namespace: String,
    },
    Suppress {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        session: String,
    },
    Writes {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        enabled: bool,
    },
    Status {
        #[arg(long)]
        namespace: String,
    },
    /// Mint one isolated namespace's tool and lifecycle credentials, shown once.
    Create {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        hat: String,
    },
    /// Run stateless HTTP MCP. Put TLS/authenticated routing in front for remote use.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7332", value_parser = loopback_listener)]
        listen: std::net::SocketAddr,
        /// Exact TCP peer (repeatable, at most eight) whose Hennery-Client-IP
        /// header keys sign-in budgets. None: the header is ignored.
        #[arg(long = "proxy-client-ip-peer", value_name = "IP")]
        proxy_client_ip_peers: Vec<std::net::IpAddr>,
    },
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("memory operation failed; check private directory, configuration and namespace state");
        std::process::exit(1);
    }
}
async fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    if !args.data_dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&args.data_dir)?;
    }
    let meta = args.data_dir.symlink_metadata()?;
    anyhow::ensure!(
        meta.is_dir() && !meta.file_type().is_symlink() && meta.permissions().mode() & 0o077 == 0,
        "private real directory required"
    );
    let store = Arc::new(Store::open(&args.data_dir.join("memory.db"))?);
    match args.command {
        Command::ImportPreview {
            owner,
            snapshot,
            vault,
            mapping,
        } => {
            let manager = store.filesystem_manager(&owner)?;
            let bundle = hennery_memory::import::Bundle::from_snapshot(&snapshot, &vault)?;
            let mapping = hennery_memory::import::read_local_json(&mapping, 2 * 1024 * 1024)?;
            let preview = store.preview_import(&manager, &bundle, &mapping)?;
            println!("{}", serde_json::to_string_pretty(&preview)?);
        }
        Command::ImportApply { owner, preview, digest } => {
            let manager = store.filesystem_manager(&owner)?;
            let preview = hennery_memory::import::read_local_json(&preview, 128 * 1024 * 1024)?;
            let results = store.apply_import(&manager, &preview, &digest)?;
            println!("{}", serde_json::to_string_pretty(&results)?);
            anyhow::ensure!(results.iter().all(|r| r.status == "committed"), "some hats untouched");
        }
        Command::UiInit { owner, public_url } => {
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            std::io::stdin().take(1026).read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len() <= 1025, "password too large");
            let password = std::str::from_utf8(&bytes)?
                .strip_suffix('\n')
                .unwrap_or(std::str::from_utf8(&bytes)?);
            ui::bootstrap(&store, &args.data_dir, &owner, &public_url, password)?;
            println!("independent UI authentication bootstrapped");
        }
        Command::Attach { namespace } => {
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            std::io::stdin().take(16385).read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() <= 16384 && bytes.first() == Some(&b'{'),
                "bounded object required"
            );
            let config: FeedConfig = serde_json::from_slice(&bytes)?;
            if args.data_dir.join("memory-auth.db").try_exists()? {
                let operator = hennery_kernel::operator::Operator::open(&args.data_dir.join("memory-auth.db"))?;
                let public = operator
                    .public_url()
                    .ok_or_else(|| anyhow::anyhow!("UI not configured"))?;
                ui::check_collector_host(public.origin(), &config.collector_url)?;
            }
            let key = keys::load(&args.data_dir, store.has_feed_credentials()?)?;
            store.attach(&namespace, config, &key)?;
            println!("grant attached; namespace requires a fresh lease");
        }
        Command::Purge { namespace } => println!("{}", serde_json::to_string(&store.owner_purge(&namespace)?)?),
        Command::Suppress { namespace, session } => store.suppress(&namespace, &session)?,
        Command::Writes { namespace, enabled } => store.set_writes(&namespace, enabled)?,
        Command::Status { namespace } => println!("{}", store.status(&namespace)?),
        Command::Create { owner, hat } => {
            let c = store.create(&owner, &hat)?;
            println!(
                "{}",
                serde_json::json!({"namespace":c.namespace,"tool_credential":c.tool,"lifecycle_credential":c.lifecycle,"mode":"standalone; no collector lease or automatic deletion guarantee"})
            );
        }
        Command::Serve {
            listen,
            proxy_client_ip_peers,
        } => {
            let policy = hennery_kernel::client_address::ProxyPolicy::new(proxy_client_ip_peers)?;
            let operator = if args.data_dir.join("memory-auth.db").try_exists()? {
                let operator = Arc::new(hennery_kernel::operator::Operator::open(
                    &args.data_dir.join("memory-auth.db"),
                )?);
                let public = operator
                    .public_url()
                    .ok_or_else(|| anyhow::anyhow!("UI not configured"))?;
                ui::validate_configuration(&store, &args.data_dir, public.origin())?;
                Some(operator)
            } else {
                None
            };
            let listener = tokio::net::TcpListener::bind(listen).await?;
            println!("memory listening on {}", listener.local_addr()?);
            let cleanup = tokio::spawn(hennery_memory::management::cleanup_worker(store.as_ref().clone()));
            let worker = if store.has_feed_credentials()? {
                let key = Arc::new(keys::load(&args.data_dir, true)?);
                Some(tokio::spawn(consumer::worker(store.clone(), key)))
            } else {
                None
            };
            let router = hennery_memory::serve::app(store.clone(), operator, policy)?;
            let shutdown = tokio_util::sync::CancellationToken::new();
            let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
            let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let signal_shutdown = shutdown.clone();
            let signals = tokio::spawn(async move {
                tokio::select! {
                    _ = interrupt.recv() => {},
                    _ = terminate.recv() => {},
                }
                signal_shutdown.cancel();
            });
            let served = hennery_kernel::server::serve_all(vec![listener], router, shutdown, Default::default()).await;
            signals.abort();
            if let Some(worker) = worker {
                worker.abort();
            }
            cleanup.abort();
            served?;
        }
    }
    Ok(())
}

fn loopback_listener(value: &str) -> Result<std::net::SocketAddr, String> {
    let address: std::net::SocketAddr = value.parse().map_err(|_| "expected a loopback socket address")?;
    if !address.ip().to_canonical().is_loopback() {
        return Err("memory serves plaintext HTTP; use a loopback listener behind a TLS reverse proxy".into());
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serve_refuses_plaintext_non_loopback_listeners_during_argument_parsing() {
        for address in ["0.0.0.0:7332", "[::]:7332", "192.0.2.1:7332", "[::ffff:192.0.2.1]:7332"] {
            assert!(Args::try_parse_from(["memory", "--data-dir", "/unused", "serve", "--listen", address]).is_err());
        }
        for address in ["127.0.0.1:0", "127.0.0.2:7332", "[::1]:7332", "[::ffff:127.0.0.1]:7332"] {
            assert!(Args::try_parse_from(["memory", "--data-dir", "/unused", "serve", "--listen", address]).is_ok());
        }
    }
}
