//! `hennery-pins`: lock the adapter pins, and generate or check
//! `adapters/manifest.json` (distribution spec §3.1). The only code in the
//! workspace, with the pin-bump CI job, that reaches the npm registry and
//! nodejs.org.
//!
//! - `lock`: write `adapters/<agent>/package.json` from `pins.toml` and let
//!   npm resolve `package-lock.json` (no scripts, isolated configuration).
//! - `generate`: the lockfiles + the registry + Node's `SHASUMS256.txt` →
//!   `adapters/manifest.json`.
//! - `check`: generate and compare; never re-resolves.
//! - `extract`: download every pinned package once, check its size and
//!   digest, and extract it as a host would, into a temporary directory
//!   removed at once: no link, nothing past its pinned sizes.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use hennery_host::runtime::manifest::{self, Manifest, NODE_DIST, NPM_REGISTRY, NodeArchive, Platform};
use hennery_pins::{Lock, Pins, Registry, VersionDoc};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "hennery-pins",
    about = "Lock the adapter pins and generate the adapter manifest"
)]
struct Cli {
    /// The repository root (holds `adapters/`).
    #[arg(long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))]
    root: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write each adapter's package.json and resolve its package-lock.json.
    Lock,
    /// Write adapters/manifest.json.
    Generate(NodeArgs),
    /// Fail unless adapters/manifest.json is what `generate` would write.
    Check(NodeArgs),
    /// Download, verify and extract every pinned package, as a host would,
    /// into a temporary directory removed at once.
    Extract,
}

#[derive(clap::Args)]
struct NodeArgs {
    /// A `SHASUMS256.txt` already verified against Node's release keys (the
    /// pin-bump job's); else it is fetched.
    #[arg(long)]
    node_shasums: Option<PathBuf>,
}

/// One package as the registry describes it: name, version, tarball URL,
/// version document, tarball bytes.
type Fetched = (String, String, String, VersionDoc, u64);

/// Requests to the registry at once.
const CONCURRENCY: usize = 8;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let adapters = cli.root.join("adapters");
    let pins = Pins::parse(&read(&adapters.join("pins.toml"))?)?;
    refuse_npmrc(&adapters)?;
    match cli.command {
        Command::Lock => lock(&adapters, &pins),
        Command::Generate(args) => {
            let manifest = generate(&adapters, &pins, args.node_shasums.as_deref()).await?;
            std::fs::write(adapters.join("manifest.json"), manifest.to_json())?;
            eprintln!("wrote {}", adapters.join("manifest.json").display());
            Ok(())
        }
        Command::Check(args) => {
            let manifest = generate(&adapters, &pins, args.node_shasums.as_deref()).await?;
            let path = adapters.join("manifest.json");
            if read(&path)? != manifest.to_json() {
                bail!(
                    "{} is not what the pins and lockfiles give: run `cargo run -p hennery-pins -- generate`",
                    path.display()
                );
            }
            eprintln!("{} matches the pins and lockfiles", path.display());
            Ok(())
        }
        Command::Extract => extract_all(&Manifest::parse(&read(&adapters.join("manifest.json"))?)?).await,
    }
}

/// Every pinned package, once: its bytes are exactly `archive_size` and
/// match `integrity`, and it extracts under the host's rules within
/// `unpacked_size` (decision 8). Nothing is kept.
async fn extract_all(manifest: &Manifest) -> Result<()> {
    let mut files: BTreeMap<String, manifest::File> = BTreeMap::new();
    for adapter in manifest.adapters.values() {
        for file in adapter.platforms.values().flatten() {
            files.entry(file.url.clone()).or_insert_with(|| file.clone());
        }
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("hennery-pins/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(900))
        .build()?;
    let scratch = tempfile::tempdir()?;
    let total = files.len();
    for (n, file) in files.values().enumerate() {
        eprintln!("[{}/{total}] {}", n + 1, file.url);
        let bytes = client.get(&file.url).send().await?.error_for_status()?.bytes().await?;
        if bytes.len() as u64 != file.archive_size {
            bail!(
                "{}: {} bytes, the manifest says {}",
                file.url,
                bytes.len(),
                file.archive_size
            );
        }
        if <[u8; 64]>::from(sha2::Sha512::digest(&bytes)) != manifest::sri_sha512(&file.integrity)? {
            bail!("{} does not match its integrity", file.url);
        }
        let tgz = scratch.path().join("package.tgz");
        std::fs::write(&tgz, &bytes)?;
        let dest = scratch.path().join("out");
        hennery_host::runtime::extract::package(
            &tgz,
            &dest,
            file.unpacked_size + hennery_host::runtime::extract::SIZE_ALLOWANCE,
        )
        .with_context(|| format!("extract {}", file.url))?;
        std::fs::remove_dir_all(&dest)?;
        std::fs::remove_file(&tgz)?;
    }
    eprintln!("{total} packages download, verify and extract as pinned");
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

/// A `.npmrc` under `adapters/` would steer npm (a scoped `@x:registry`
/// beats `--registry`): there must be none.
fn refuse_npmrc(adapters: &Path) -> Result<()> {
    for entry in std::fs::read_dir(adapters)? {
        let dir = entry?.path();
        if dir.join(".npmrc").exists() || dir.file_name() == Some(".npmrc".as_ref()) {
            bail!("{} holds an .npmrc; remove it", dir.display());
        }
    }
    Ok(())
}

fn lock(adapters: &Path, pins: &Pins) -> Result<()> {
    let scratch = tempfile::tempdir()?;
    // npm refuses one file as both: two empty ones.
    let (user, global) = (scratch.path().join("user-npmrc"), scratch.path().join("global-npmrc"));
    std::fs::write(&user, "")?;
    std::fs::write(&global, "")?;
    for name in pins.adapters.keys() {
        let dir = adapters.join(name);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("package.json"), pins.package_json(name)?)?;
        let _ = std::fs::remove_file(dir.join("package-lock.json"));
        // Only PATH from this environment: no `npm_config_*`, no HOME with
        // the operator's `.npmrc` or cache, no proxy settings.
        let status = std::process::Command::new("npm")
            .args([
                "install",
                "--package-lock-only",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                "--lockfile-version=3",
            ])
            .arg(format!("--registry={NPM_REGISTRY}"))
            .arg(format!("--before={}", pins.npm_before))
            .arg(format!("--userconfig={}", user.display()))
            .arg(format!("--globalconfig={}", global.display()))
            .current_dir(&dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", scratch.path())
            .status()
            .context("run npm (is the dev shell active?)")?;
        if !status.success() {
            bail!("npm failed for {name}: {status}");
        }
        eprintln!("locked {}", dir.join("package-lock.json").display());
    }
    Ok(())
}

async fn generate(adapters: &Path, pins: &Pins, node_shasums: Option<&Path>) -> Result<Manifest> {
    let mut locks = BTreeMap::new();
    for name in pins.adapters.keys() {
        let dir = adapters.join(name);
        if read(&dir.join("package.json"))? != pins.package_json(name)? {
            bail!(
                "{}/package.json is not what pins.toml gives: run `hennery-pins lock`",
                dir.display()
            );
        }
        let lock: Lock = serde_json::from_str(&read(&dir.join("package-lock.json"))?)
            .with_context(|| format!("parse {}/package-lock.json", dir.display()))?;
        locks.insert(name.clone(), lock);
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("hennery-pins/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(600))
        .build()?;
    let wanted = hennery_pins::wanted(&locks)?;
    for (_, _, url) in &wanted {
        if !url.starts_with(NPM_REGISTRY) {
            bail!("{url} is not on {NPM_REGISTRY}");
        }
    }
    eprintln!("asking the registry about {} packages", wanted.len());
    let fetched: Vec<Result<Fetched>> = futures::stream::iter(wanted)
        .map(|(name, version, url)| {
            let client = client.clone();
            async move {
                let doc_url = format!("{NPM_REGISTRY}{name}/{version}");
                let mut doc: VersionDoc = client
                    .get(&doc_url)
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await
                    .with_context(|| format!("parse {doc_url}"))?;
                let size = archive_size(&client, &url).await?;
                // Older packages were published without `unpackedSize`:
                // measured from the tarball itself, once its digest checks.
                if doc.dist.unpacked_size.is_none() {
                    let bytes = client.get(&url).send().await?.error_for_status()?.bytes().await?;
                    let integrity = doc
                        .dist
                        .integrity
                        .as_deref()
                        .with_context(|| format!("{doc_url}: no integrity"))?;
                    if manifest::sri_sha512(integrity)? != <[u8; 64]>::from(sha2::Sha512::digest(&bytes)) {
                        bail!("{url} does not match its integrity");
                    }
                    doc.dist.unpacked_size = Some(hennery_pins::unpacked_size(&bytes)?);
                }
                Ok((name, version, url, doc, size))
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let mut registry = Registry::default();
    for result in fetched {
        let (name, version, url, doc, size) = result?;
        registry.docs.insert((name, version), doc);
        registry.archive_sizes.insert(url, size);
    }
    let version = &pins.node.version;
    let shasums = match node_shasums {
        Some(path) => read(path)?,
        None => {
            client
                .get(format!("{NODE_DIST}v{version}/SHASUMS256.txt"))
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?
        }
    };
    let digests = hennery_pins::node_digests(&shasums, version)?;
    let mut node = BTreeMap::new();
    for platform in Platform::ALL {
        let url = format!("{NODE_DIST}v{version}/node-v{version}-{}.tar.gz", platform.key());
        eprintln!("downloading {url}");
        let archive = client.get(&url).send().await?.error_for_status()?.bytes().await?;
        if hex::encode(Sha256::digest(&archive)) != digests[&platform] {
            bail!("{url} does not match SHASUMS256.txt");
        }
        node.insert(
            platform,
            NodeArchive {
                url,
                sha256: digests[&platform].clone(),
                archive_size: archive.len() as u64,
                node_size: hennery_pins::node_binary_size(&archive, version, platform)?,
            },
        );
    }
    hennery_pins::build(pins, &locks, &registry, &node)
}

/// A tarball's size without downloading it: the total of a one-byte range.
async fn archive_size(client: &reqwest::Client, url: &str) -> Result<u64> {
    let response = client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await?
        .error_for_status()?;
    let range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .with_context(|| format!("{url}: no Content-Range"))?;
    range
        .rsplit_once('/')
        .and_then(|(_, total)| total.parse().ok())
        .with_context(|| format!("{url}: Content-Range {range:?}"))
}
