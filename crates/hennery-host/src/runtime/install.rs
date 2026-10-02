//! Installing, switching, rolling back and collecting adapter sets
//! (distribution spec §3.2). In the host's data directory:
//!
//! ```text
//! runtimes/node-<version>-<platform>/bin/node
//! adapters/sets/<set id>/          one complete set (hennery-set.json, .in-use)
//! adapters/sets/<set id>/<agent>/  one adapter's tree: node_modules/…
//! adapters/current -> sets/<id>    what the next host start launches
//! adapters/previous -> sets/<id>   what `rollback` returns to
//! adapters/hold                    a rollback holds the host on its set
//! adapters/downloads/              resumable downloads of an install under way
//! adapters/.install.lock           one install, rollback or collection at a time
//! ```
//!
//! A set directory exists only once it is complete (it is staged, then
//! renamed), and `current` and `previous` are swapped by rename, so a crash
//! leaves either the old state or the new one.

use super::download::{self, Expected, Sources};
use super::extract;
use super::manifest::{self, File, Manifest, NodeArchive, Platform};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The installer's own layout and extraction rules. Bumped whenever they
/// change, so a set laid out by older code is never taken for a new one.
pub const LAYOUT: u32 = 1;

/// The record each set keeps of itself.
pub const RECORD: &str = "hennery-set.json";

/// The file a host holds a shared lock on while it launches from a set.
pub const IN_USE: &str = ".in-use";

/// Free space kept over what an install needs.
pub const SPACE_MARGIN: u64 = 64 << 20;

/// What one platform needs of a manifest: the Node archive and each
/// adapter's files, less the CLIs `--use-cli` replaces.
#[derive(Debug, Clone)]
pub struct Selection {
    pub platform: Platform,
    pub manifest_hash: String,
    pub node_version: String,
    pub node: NodeArchive,
    pub adapters: Vec<SelectedAdapter>,
}

#[derive(Debug, Clone)]
pub struct SelectedAdapter {
    pub name: String,
    pub version: String,
    pub entry: String,
    /// Its bundled CLI is left out (`--use-cli`).
    pub cli_skipped: bool,
    pub files: Vec<File>,
}

impl Selection {
    /// What `platform` installs of `manifest` (whose text hashes to
    /// `manifest_hash`), without the CLIs of the adapters in `skip_cli`.
    pub fn new(
        manifest: &Manifest,
        manifest_hash: &str,
        platform: Platform,
        skip_cli: &BTreeSet<String>,
    ) -> Result<Self> {
        if let Some(name) = skip_cli.iter().find(|name| !manifest.adapters.contains_key(*name)) {
            bail!("no pinned adapter is called {name:?}");
        }
        let node = manifest
            .node
            .platforms
            .get(platform.key())
            .context("no Node for this platform")?;
        let mut adapters = Vec::new();
        for (name, adapter) in &manifest.adapters {
            let cli_skipped = skip_cli.contains(name);
            let files: Vec<File> = adapter
                .platforms
                .get(platform.key())
                .with_context(|| format!("{name} has no files for {}", platform.key()))?
                .iter()
                .filter(|f| !(cli_skipped && f.cli))
                .cloned()
                .collect();
            if let Some(file) = files.iter().find(|f| f.install_script) {
                bail!(
                    "{name}: {} needs an install script, which hennery never runs",
                    file.path
                );
            }
            adapters.push(SelectedAdapter {
                name: name.clone(),
                version: adapter.version.clone(),
                entry: adapter.entry.clone(),
                cli_skipped,
                files,
            });
        }
        Ok(Self {
            platform,
            manifest_hash: manifest_hash.to_string(),
            node_version: manifest.node.version.clone(),
            node: node.clone(),
            adapters,
        })
    }

    /// The pinned selection for this machine: the embedded manifest.
    pub fn pinned(skip_cli: &BTreeSet<String>) -> Result<Self> {
        let platform = Platform::current()
            .context("the managed runtime supports macOS on Apple silicon and Linux on x86-64 or arm64 only")?;
        Self::new(
            &Manifest::embedded(),
            &Manifest::hash_of(manifest::EMBEDDED),
            platform,
            skip_cli,
        )
    }

    /// The set's id: the first 32 hex digits of SHA-256 over exactly what is
    /// installed (decision 5). Mirror-independent: no URL is in it.
    pub fn set_id(&self) -> String {
        #[derive(Serialize)]
        struct Preimage<'a> {
            layout: u32,
            schema: u32,
            platform: &'a str,
            node_version: &'a str,
            node_sha256: &'a str,
            adapters: Vec<AdapterPreimage<'a>>,
        }
        #[derive(Serialize)]
        struct AdapterPreimage<'a> {
            name: &'a str,
            version: &'a str,
            entry: &'a str,
            cli_skipped: bool,
            files: Vec<(&'a str, &'a str)>,
        }
        let preimage = Preimage {
            layout: LAYOUT,
            schema: manifest::SCHEMA,
            platform: self.platform.key(),
            node_version: &self.node_version,
            node_sha256: &self.node.sha256,
            adapters: self
                .adapters
                .iter()
                .map(|a| AdapterPreimage {
                    name: &a.name,
                    version: &a.version,
                    entry: &a.entry,
                    cli_skipped: a.cli_skipped,
                    files: {
                        let mut files: Vec<(&str, &str)> = a
                            .files
                            .iter()
                            .map(|f| (f.path.as_str(), f.integrity.as_str()))
                            .collect();
                        files.sort();
                        files
                    },
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&preimage).expect("the preimage serialises");
        hex::encode(Sha256::digest(&bytes))[..32].to_string()
    }

    /// `runtimes/<this>/bin/node`.
    pub fn runtime_name(&self) -> String {
        format!("node-{}-{}", self.node_version, self.platform.key())
    }

    /// Bytes an install may need at its peak: every archive (downloads are
    /// kept until the set is complete) and everything extracted, and the
    /// Node archive and binary unless that runtime is there already.
    pub fn space_needed(&self, with_node: bool) -> u64 {
        let files: u64 = self
            .adapters
            .iter()
            .flat_map(|a| &a.files)
            .map(|f| f.archive_size + f.unpacked_size)
            .sum();
        let node = if with_node {
            self.node.archive_size + self.node.node_size
        } else {
            0
        };
        files + node + SPACE_MARGIN
    }
}

/// `hennery-set.json`: what a set holds, written last into its staging
/// directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetRecord {
    pub layout: u32,
    pub id: String,
    pub manifest_hash: String,
    pub platform: String,
    /// `runtimes/<runtime>/bin/node` runs every adapter of the set.
    pub runtime: String,
    pub adapters: BTreeMap<String, RecordAdapter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordAdapter {
    pub version: String,
    /// Relative to the adapter's directory, `<set>/<agent>/`.
    pub entry: String,
    pub cli_skipped: bool,
}

/// An installed, complete set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSet {
    pub id: String,
    /// Absolute: an adapter is started from here, never through `current`.
    pub path: PathBuf,
    pub record: SetRecord,
    /// The runtime's `bin/node`, absolute.
    pub node: PathBuf,
}

/// The directories of one host's managed runtime.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// The layout under a host data directory, made absolute.
    pub fn new(data_dir: &Path) -> Result<Self> {
        Ok(Self {
            root: std::path::absolute(data_dir).with_context(|| format!("resolve {}", data_dir.display()))?,
        })
    }

    pub fn adapters(&self) -> PathBuf {
        self.root.join("adapters")
    }
    pub fn sets(&self) -> PathBuf {
        self.adapters().join("sets")
    }
    pub fn downloads(&self) -> PathBuf {
        self.adapters().join("downloads")
    }
    pub fn runtimes(&self) -> PathBuf {
        self.root.join("runtimes")
    }
    pub fn current_link(&self) -> PathBuf {
        self.adapters().join("current")
    }
    pub fn previous_link(&self) -> PathBuf {
        self.adapters().join("previous")
    }
    pub fn hold_file(&self) -> PathBuf {
        self.adapters().join("hold")
    }

    pub fn install_lock(&self) -> PathBuf {
        self.adapters().join(".install.lock")
    }

    /// Whether a rollback holds this host on its set: a host start does not
    /// install the pinned set until `hennery host adapters update`. The hold
    /// names the set rolled back to, and holds only while that set is
    /// current (a crash inside a rollback leaves no hold on another set).
    pub fn held(&self) -> bool {
        let held = std::fs::read_to_string(self.hold_file()).unwrap_or_default();
        !held.trim().is_empty() && self.current_id().as_deref() == Some(held.trim())
    }

    /// The id `current` points at, if it points at a set id at all: read
    /// from the link alone, so a set this binary cannot read (another
    /// layout, a removed directory) never stops an install or a rollback.
    pub fn current_id(&self) -> Option<String> {
        link_id(&self.current_link())
    }

    /// The id `previous` points at, as `current_id`.
    pub fn previous_id(&self) -> Option<String> {
        link_id(&self.previous_link())
    }

    /// The set `current` points at, if any.
    pub fn current(&self) -> Result<Option<InstalledSet>> {
        self.linked(&self.current_link())
    }

    /// The set `previous` points at, if any.
    pub fn previous(&self) -> Result<Option<InstalledSet>> {
        self.linked(&self.previous_link())
    }

    fn linked(&self, link: &Path) -> Result<Option<InstalledSet>> {
        let target = match std::fs::read_link(link) {
            Ok(target) => target,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).with_context(|| format!("read {}", link.display())),
        };
        let id = target
            .strip_prefix("sets")
            .ok()
            .and_then(|rest| rest.to_str())
            .filter(|id| is_set_id(id))
            .with_context(|| format!("{} points at {}, not at a set", link.display(), target.display()))?;
        self.set(id).map(Some)
    }

    /// The complete set `id`.
    pub fn set(&self, id: &str) -> Result<InstalledSet> {
        if !is_set_id(id) {
            bail!("{id:?} is not a set id");
        }
        let path = self.sets().join(id);
        let text = std::fs::read_to_string(path.join(RECORD))
            .with_context(|| format!("read {}", path.join(RECORD).display()))?;
        let record: SetRecord =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.join(RECORD).display()))?;
        if record.id != id || record.layout != LAYOUT {
            bail!(
                "{} does not describe set {id} of layout {LAYOUT}",
                path.join(RECORD).display()
            );
        }
        manifest::check_relative_path(&record.runtime)?;
        // Each agent's tree stays inside the set (the review's condition on
        // the per-agent layout).
        for (name, adapter) in &record.adapters {
            if !manifest::is_agent_name(name) {
                bail!("{}: adapter name {name:?}", path.join(RECORD).display());
            }
            manifest::check_relative_path(&adapter.entry)?;
        }
        let node = self.runtimes().join(&record.runtime).join("bin/node");
        Ok(InstalledSet {
            id: id.to_string(),
            path,
            record,
            node,
        })
    }
}

fn link_id(link: &Path) -> Option<String> {
    let target = std::fs::read_link(link).ok()?;
    let id = target.strip_prefix("sets").ok()?.to_str()?;
    is_set_id(id).then(|| id.to_string())
}

fn is_set_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// What `install` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// The set was current already.
    AlreadyCurrent(InstalledSet),
    /// `current` now points at it; `previous` at what was current before.
    Switched {
        set: InstalledSet,
        previous: Option<String>,
    },
}

impl Installed {
    pub fn set(&self) -> &InstalledSet {
        match self {
            Installed::AlreadyCurrent(set) | Installed::Switched { set, .. } => set,
        }
    }
}

/// Install `selection` (downloading only what is missing), make it current,
/// lift any rollback hold, and collect what is no longer needed. Progress
/// lines go to `progress`.
pub async fn install(
    layout: &Layout,
    selection: &Selection,
    sources: &Sources,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<Installed> {
    check_host(selection.platform)?;
    let _lock = lock_install(layout, progress).await?;
    install_locked(layout, selection, sources, progress).await
}

/// `install`, unless another install holds the lock: then `Ok(None)` at
/// once (a host start never waits on an `adapters update`).
pub async fn try_install(
    layout: &Layout,
    selection: &Selection,
    sources: &Sources,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<Option<Installed>> {
    check_host(selection.platform)?;
    let Some(_lock) = try_lock_install(layout)? else {
        return Ok(None);
    };
    install_locked(layout, selection, sources, progress).await.map(Some)
}

async fn install_locked(
    layout: &Layout,
    selection: &Selection,
    sources: &Sources,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<Installed> {
    for dir in [layout.adapters(), layout.sets(), layout.downloads(), layout.runtimes()] {
        crate::identity::create_private_dir(&dir)?;
    }
    let id = selection.set_id();
    let previous = layout.current_id();
    if previous.as_deref() == Some(id.as_str())
        && let Ok(set) = layout.set(&id)
        && set.node.is_file()
    {
        let _ = std::fs::remove_file(layout.hold_file());
        return Ok(Installed::AlreadyCurrent(set));
    }
    if layout.set(&id).is_err() {
        // A directory under this id that is not a complete set of this
        // layout (a torn one, an older binary's) is moved aside, not used.
        let path = layout.sets().join(&id);
        if std::fs::symlink_metadata(&path).is_ok() {
            let trash = layout.sets().join(format!(".trash-{id}"));
            let _ = std::fs::remove_dir_all(&trash);
            std::fs::rename(&path, &trash)?;
        }
        build(layout, selection, sources, progress).await?;
    }
    let set = layout.set(&id)?;
    // A set whose runtime went (removed by hand, or by a binary that could
    // not read this set's record) gets it back before it is current.
    if !set.node.is_file() {
        install_node(layout, selection, sources, progress).await?;
    }
    // The set's rename is durable before `current` names it.
    extract::barrier(&layout.sets())?;
    if let Some(old) = previous.as_ref().filter(|old| **old != id) {
        swap_link(layout, &layout.previous_link(), old)?;
    }
    swap_link(layout, &layout.current_link(), &id)?;
    let _ = std::fs::remove_file(layout.hold_file());
    extract::sync_dir(&layout.adapters())?;
    // The switch is done: tidying up afterwards cannot undo it, so its
    // failure is a warning, not the install's.
    // Every download belonged to this install, or to one that cannot be
    // resumed into anything now current.
    if let Err(err) = clear_dir(&layout.downloads()) {
        tracing::warn!("the adapter set is installed, but its downloads were not removed: {err:#}");
    }
    if let Err(err) = collect(layout) {
        tracing::warn!("the adapter set is installed, but collecting older ones failed: {err:#}");
    }
    Ok(Installed::Switched { set, previous })
}

/// What `rollback` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolledBack {
    pub from: String,
    pub to: InstalledSet,
}

/// Swap `current` and `previous`, and hold the host on the set rolled back
/// to until the next `install`.
pub async fn rollback(layout: &Layout, progress: &(dyn Fn(&str) + Sync)) -> Result<RolledBack> {
    let _lock = lock_install(layout, progress).await?;
    let to = layout
        .previous()?
        .context("there is no previous adapter set to roll back to")?;
    let from = layout.current_id().context("there is no current adapter set")?;
    if from == to.id {
        bail!("there is no previous adapter set to roll back to: previous is the current one");
    }
    if !to.node.is_file() {
        bail!(
            "the previous adapter set's Node ({}) is gone; `hennery host adapters update` installs the pinned set",
            to.node.display()
        );
    }
    // Durable before `current` names the set it holds the host on.
    write_synced(&layout.hold_file(), format!("{}\n", to.id).as_bytes())?;
    extract::barrier(&layout.hold_file())?;
    swap_link(layout, &layout.previous_link(), &from)?;
    swap_link(layout, &layout.current_link(), &to.id)?;
    extract::sync_dir(&layout.adapters())?;
    if let Err(err) = collect(layout) {
        tracing::warn!("rolled back, but cleaning up afterwards failed: {err:#}");
    }
    Ok(RolledBack { from, to })
}

/// Hold a shared lock on `set` for as long as the returned file lives: a
/// collection never removes a set a running host launches from. Fails if
/// the set was removed before the lock was taken.
pub fn hold_in_use(layout: &Layout, set: &InstalledSet) -> Result<std::fs::File> {
    let file = std::fs::File::open(set.path.join(IN_USE))
        .with_context(|| format!("open {}", set.path.join(IN_USE).display()))?;
    // A collection holds the exclusive lock only while it renames the set
    // away; wait it out briefly.
    for _ in 0..50 {
        if flock(&file, libc::LOCK_SH | libc::LOCK_NB)? {
            // Taken after the set was renamed away: it is gone.
            layout.set(&set.id)?;
            return Ok(file);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    bail!("set {} stays locked by a collection", set.id)
}

/// Refuse a host the managed runtime cannot run on: on Linux, one without
/// glibc's dynamic loader (musl, or NixOS without nix-ld; distribution §1.1).
pub fn check_host(platform: Platform) -> Result<()> {
    let loader = match platform {
        Platform::LinuxX64 => "/lib64/ld-linux-x86-64.so.2",
        Platform::LinuxArm64 => "/lib/ld-linux-aarch64.so.1",
        Platform::DarwinArm64 => return Ok(()),
    };
    if !Path::new(loader).exists() {
        bail!(
            "this host has no glibc loader ({loader}): the managed Node and the Claude CLI are glibc builds. \
             On musl, run only the collector here; on NixOS, enable programs.nix-ld or use the Nix-provided adapters"
        );
    }
    Ok(())
}

/// Bytes free to this user on the filesystem holding `dir`.
pub fn free_space(dir: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes())?;
    // SAFETY: statvfs(3) into a zeroed local struct of the right type.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("statvfs {}", dir.display()));
    }
    #[allow(clippy::unnecessary_cast)] // `fsblkcnt_t` is u32 on some targets.
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

async fn build(
    layout: &Layout,
    selection: &Selection,
    sources: &Sources,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<()> {
    let id = selection.set_id();
    let runtime = layout.runtimes().join(selection.runtime_name());
    let with_node = !runtime.join("bin/node").exists();
    let needed = selection.space_needed(with_node);
    let free = free_space(&layout.adapters())?;
    if free < needed {
        bail!(
            "installing the adapter set needs {} MB free in {}, and {} MB are",
            needed >> 20,
            layout.adapters().display(),
            free >> 20
        );
    }
    let download: u64 = selection
        .adapters
        .iter()
        .flat_map(|a| &a.files)
        .map(|f| f.archive_size)
        .sum::<u64>()
        + if with_node { selection.node.archive_size } else { 0 };
    progress(&format!(
        "downloading the adapter set: about {} MB, {} MB once installed",
        download >> 20,
        (needed - download - SPACE_MARGIN) >> 20
    ));
    if with_node {
        install_node(layout, selection, sources, progress).await?;
    }
    let staging = layout.sets().join(format!(".staging-{id}"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir(&staging)?;
    let total: usize = selection.adapters.iter().map(|a| a.files.len()).sum();
    let mut done = 0;
    for adapter in &selection.adapters {
        // Each adapter has a tree of its own (`<set>/<agent>/node_modules`):
        // the two lockfiles were resolved apart and may disagree on a
        // shared package. Shallower paths first, so a nested package goes
        // inside its parent.
        let mut files: Vec<&File> = adapter.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        for file in files {
            done += 1;
            let digest = manifest::sri_sha512(&file.integrity)?;
            let part = layout.downloads().join(format!("{}.part", &hex::encode(digest)[..32]));
            let url = sources.locate(&file.url)?;
            progress(&format!("[{done}/{total}] {} {}", adapter.name, file.path));
            download::fetch(&url, &part, file.archive_size, Expected::Sha512(digest)).await?;
            let dest = staging.join(&adapter.name).join(&file.path);
            let max = file.unpacked_size + extract::SIZE_ALLOWANCE;
            tokio::task::spawn_blocking(move || extract::package(&part, &dest, max))
                .await?
                .with_context(|| format!("extract {}", file.path))?;
        }
    }
    let record = SetRecord {
        layout: LAYOUT,
        id: id.clone(),
        manifest_hash: selection.manifest_hash.clone(),
        platform: selection.platform.key().to_string(),
        runtime: selection.runtime_name(),
        adapters: selection
            .adapters
            .iter()
            .map(|a| {
                (
                    a.name.clone(),
                    RecordAdapter {
                        version: a.version.clone(),
                        entry: a.entry.clone(),
                        cli_skipped: a.cli_skipped,
                    },
                )
            })
            .collect(),
    };
    for adapter in &selection.adapters {
        if !staging.join(&adapter.name).join(&adapter.entry).is_file() {
            bail!("{}: its entry {} is not in the set", adapter.name, adapter.entry);
        }
    }
    write_synced(&staging.join(IN_USE), b"")?;
    write_synced(
        &staging.join(RECORD),
        format!("{}\n", serde_json::to_string_pretty(&record)?).as_bytes(),
    )?;
    // Every directory too: a tarball's directories mostly come from its file
    // names, not from entries of their own.
    extract::sync_tree(&staging)?;
    // Every file of the set is durable before the rename publishes it.
    extract::barrier(&staging.join(RECORD))?;
    let path = layout.sets().join(&id);
    std::fs::rename(&staging, &path).with_context(|| format!("rename {} to {}", staging.display(), path.display()))?;
    extract::sync_dir(&layout.sets())?;
    Ok(())
}

async fn install_node(
    layout: &Layout,
    selection: &Selection,
    sources: &Sources,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<()> {
    let name = selection.runtime_name();
    let node = &selection.node;
    let mut digest = [0u8; 32];
    hex::decode_to_slice(&node.sha256, &mut digest).context("the pinned Node digest")?;
    let part = layout.downloads().join(format!("{}.part", &node.sha256[..32]));
    let url = sources.locate(&node.url)?;
    progress(&format!(
        "Node {} for {}",
        selection.node_version,
        selection.platform.key()
    ));
    download::fetch(&url, &part, node.archive_size, Expected::Sha256(digest)).await?;
    let staging = layout.runtimes().join(format!(".staging-{name}"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    let binary = staging.join("bin/node");
    let (version, platform, size) = (
        selection.node_version.clone(),
        selection.platform.key().to_string(),
        node.node_size,
    );
    let target = binary.clone();
    tokio::task::spawn_blocking(move || extract::node_binary(&part, &version, &platform, &target, size)).await??;
    // Before anything depends on it: a data directory mounted noexec, or a
    // Linux without the loader Node needs, fails here, not at a session.
    let mut attempts = 0;
    let output = loop {
        let run = tokio::process::Command::new(&binary)
            .arg("--version")
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(std::time::Duration::from_secs(20), run)
            .await
            .context("the downloaded Node did not answer --version within 20 s")?
        {
            Err(err) if err.raw_os_error() == Some(libc::ETXTBSY) && attempts < 10 => {
                attempts += 1;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            result => break result.with_context(|| format!("run {}", binary.display()))?,
        }
    };
    let answered = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || answered != format!("v{}", selection.node_version) {
        bail!(
            "the downloaded Node does not run here ({}): {answered:?}",
            output.status
        );
    }
    extract::sync_dir(&staging.join("bin"))?;
    extract::sync_dir(&staging)?;
    extract::barrier(&binary)?;
    let path = layout.runtimes().join(&name);
    std::fs::rename(&staging, &path)?;
    extract::sync_dir(&layout.runtimes())?;
    Ok(())
}

/// Point `link` at `sets/<id>`, atomically: a new link, renamed over it.
fn swap_link(layout: &Layout, link: &Path, id: &str) -> Result<()> {
    let name = link.file_name().and_then(|n| n.to_str()).context("a link name")?;
    let tmp = layout.adapters().join(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(Path::new("sets").join(id), &tmp)?;
    std::fs::rename(&tmp, link).with_context(|| format!("rename a link over {}", link.display()))?;
    Ok(())
}

/// Remove every set but `current` and `previous` that no host holds, every
/// leftover staging directory, and every runtime no remaining set names.
/// Runs under the install lock.
fn collect(layout: &Layout) -> Result<()> {
    let keep: BTreeSet<String> = [layout.current_id(), layout.previous_id()]
        .into_iter()
        .flatten()
        .collect();
    let mut runtimes = BTreeSet::new();
    for entry in std::fs::read_dir(layout.sets())? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if name.starts_with(".staging-") || name.starts_with(".trash-") {
            std::fs::remove_dir_all(&path)?;
            continue;
        }
        if !is_set_id(&name) {
            continue;
        }
        let runtime = runtime_named_by(&path);
        if keep.contains(&name) {
            runtimes.extend(runtime);
            continue;
        }
        // A host launching from it holds a shared lock: leave it.
        let lock = std::fs::File::open(path.join(IN_USE)).ok();
        let free = match &lock {
            Some(file) => flock(file, libc::LOCK_EX | libc::LOCK_NB)?,
            None => true,
        };
        if !free {
            runtimes.extend(runtime);
            continue;
        }
        let trash = layout.sets().join(format!(".trash-{name}"));
        let _ = std::fs::remove_dir_all(&trash);
        std::fs::rename(&path, &trash)?;
        drop(lock);
        std::fs::remove_dir_all(&trash)?;
    }
    for entry in std::fs::read_dir(layout.runtimes())? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if name.starts_with(".staging-") || (name.starts_with("node-") && !runtimes.contains(&name)) {
            std::fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
}

/// The runtime a set's record names, read leniently: a record of another
/// layout still keeps its Node from being collected.
fn runtime_named_by(set: &Path) -> Option<String> {
    let text = std::fs::read_to_string(set.join(RECORD)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let runtime = value.get("runtime")?.as_str()?;
    manifest::check_relative_path(runtime).ok()?;
    Some(runtime.to_string())
}

fn open_install_lock(layout: &Layout) -> Result<std::fs::File> {
    crate::identity::create_private_dir(&layout.adapters())?;
    let path = layout.install_lock();
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))
}

/// The install lock if it is free now, else `None`.
fn try_lock_install(layout: &Layout) -> Result<Option<std::fs::File>> {
    let file = open_install_lock(layout)?;
    Ok(flock(&file, libc::LOCK_EX | libc::LOCK_NB)?.then_some(file))
}

/// Take the install lock, saying so if another install holds it.
async fn lock_install(layout: &Layout, progress: &(dyn Fn(&str) + Sync)) -> Result<std::fs::File> {
    let file = open_install_lock(layout)?;
    if flock(&file, libc::LOCK_EX | libc::LOCK_NB)? {
        return Ok(file);
    }
    progress("another install of the adapter set is running; waiting for it");
    tokio::task::spawn_blocking(move || {
        flock(&file, libc::LOCK_EX)?;
        Ok(file)
    })
    .await?
}

/// flock(2): `Ok(false)` when a non-blocking request would block.
fn flock(file: &std::fs::File, operation: libc::c_int) -> Result<bool> {
    loop {
        // SAFETY: flock(2) on a descriptor this function borrows.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(true);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EWOULDBLOCK) => return Ok(false),
            Some(libc::EINTR) => continue,
            _ => return Err(err).context("flock"),
        }
    }
}

fn write_synced(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(contents)?;
    extract::sync(&file)?;
    Ok(())
}

fn clear_dir(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        std::fs::remove_file(entry?.path())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_id_follows_what_is_installed() {
        let pinned = Selection::pinned(&BTreeSet::new()).unwrap();
        let again = Selection::pinned(&BTreeSet::new()).unwrap();
        assert_eq!(pinned.set_id(), again.set_id());
        assert!(is_set_id(&pinned.set_id()));
        let without_claude = Selection::pinned(&BTreeSet::from(["claude".to_string()])).unwrap();
        assert_ne!(pinned.set_id(), without_claude.set_id());
        let claude = without_claude.adapters.iter().find(|a| a.name == "claude").unwrap();
        assert!(claude.cli_skipped && claude.files.iter().all(|f| !f.cli));
        assert!(without_claude.space_needed(false) < pinned.space_needed(false));
        assert!(Selection::pinned(&BTreeSet::from(["gemini".to_string()])).is_err());
    }

    /// A record whose agent or entry would leave the set is not a set.
    #[test]
    fn a_record_naming_paths_outside_its_set_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path()).unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let set = layout.sets().join(id);
        std::fs::create_dir_all(&set).unwrap();
        let record = |agent: &str, entry: &str| {
            let record = SetRecord {
                layout: LAYOUT,
                id: id.into(),
                manifest_hash: String::new(),
                platform: "linux-x64".into(),
                runtime: "node-24.0.0-linux-x64".into(),
                adapters: BTreeMap::from([(
                    agent.to_string(),
                    RecordAdapter {
                        version: "1".into(),
                        entry: entry.into(),
                        cli_skipped: false,
                    },
                )]),
            };
            std::fs::write(set.join(RECORD), serde_json::to_string(&record).unwrap()).unwrap();
        };
        record("claude", "node_modules/a/index.js");
        layout.set(id).unwrap();
        record("claude", "../../escape.js");
        assert!(layout.set(id).is_err());
        record("../claude", "index.js");
        assert!(layout.set(id).is_err());
    }

    #[test]
    fn this_host_can_run_the_runtime() {
        check_host(Platform::current().unwrap()).unwrap();
        assert!(free_space(Path::new("/")).unwrap() > 0);
    }
}
