//! The adapter manifest (distribution spec §3.1): the pinned Node runtime
//! and adapter packages, per platform, with their digests. One manifest is
//! compiled into each binary; `hennery-pins` generates it from the lockfiles
//! under `adapters/`, and the Nix flake (7e) reads the same file.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The manifest this binary pins.
pub const EMBEDDED: &str = include_str!("../../../../adapters/manifest.json");

/// The only schema this binary reads.
pub const SCHEMA: u32 = 1;

/// Where every pinned package comes from; a mirror replaces this prefix
/// (decision 7), the digests stay the manifest's.
pub const NPM_REGISTRY: &str = "https://registry.npmjs.org/";

/// Where every pinned Node archive comes from; a mirror replaces this prefix.
pub const NODE_DIST: &str = "https://nodejs.org/dist/";

/// A host platform the managed runtime supports (distribution §1, §1.1).
/// Linux means glibc: the Node builds and the Claude CLI are glibc builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Platform {
    DarwinArm64,
    LinuxX64,
    LinuxArm64,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Platform::DarwinArm64, Platform::LinuxArm64, Platform::LinuxX64];

    /// The manifest's key, and Node's own name for the platform.
    pub fn key(self) -> &'static str {
        match self {
            Platform::DarwinArm64 => "darwin-arm64",
            Platform::LinuxX64 => "linux-x64",
            Platform::LinuxArm64 => "linux-arm64",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    /// npm's `os`, `cpu` and `libc` for the platform (`libc` only on Linux).
    pub fn npm(self) -> (&'static str, &'static str, Option<&'static str>) {
        match self {
            Platform::DarwinArm64 => ("darwin", "arm64", None),
            Platform::LinuxX64 => ("linux", "x64", Some("glibc")),
            Platform::LinuxArm64 => ("linux", "arm64", Some("glibc")),
        }
    }

    /// The platform this process runs on, if the runtime supports it.
    pub fn current() -> Option<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => Some(Platform::DarwinArm64),
            ("linux", "x86_64") => Some(Platform::LinuxX64),
            ("linux", "aarch64") => Some(Platform::LinuxArm64),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub node: Node,
    pub adapters: BTreeMap<String, Adapter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Without the `v`, e.g. `24.21.0`.
    pub version: String,
    pub platforms: BTreeMap<String, NodeArchive>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeArchive {
    pub url: String,
    /// Hex, from Node's signed `SHASUMS256.txt`.
    pub sha256: String,
    /// Bytes of the archive: a download is cut off past it.
    pub archive_size: u64,
    /// Bytes of `bin/node`, the only file taken from the archive.
    pub node_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    pub package: String,
    pub version: String,
    /// The script Node runs, relative to the set, e.g.
    /// `node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js`.
    pub entry: String,
    pub platforms: BTreeMap<String, Vec<File>>,
}

/// One package tarball, extracted at `path` with its first component
/// stripped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    /// Its lockfile path, e.g. `node_modules/zod`.
    pub path: String,
    pub url: String,
    /// The registry's SRI digest, `sha512-…`.
    pub integrity: String,
    /// Bytes of the tarball: a download is cut off past it.
    pub archive_size: u64,
    /// Bytes of its files once extracted (the registry's `unpackedSize`).
    pub unpacked_size: u64,
    /// Part of the agent's bundled CLI: skipped when `--use-cli` names one.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cli: bool,
    /// The package needs an install script. The generator never writes
    /// one (it fails instead); the installer refuses an entry that has it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub install_script: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Manifest {
    /// Parse and validate a manifest.
    pub fn parse(text: &str) -> Result<Self> {
        let manifest: Manifest = serde_json::from_str(text).context("parse the adapter manifest")?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// The manifest compiled into this binary.
    pub fn embedded() -> Self {
        Self::parse(EMBEDDED).expect("the embedded adapter manifest is valid (tested)")
    }

    /// The manifest as the generator writes it: two-space indented JSON and
    /// a final newline.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a manifest serialises");
        text.push('\n');
        text
    }

    /// Every rule a manifest must keep, whoever wrote it.
    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA {
            bail!("adapter manifest schema {} (this binary reads {SCHEMA})", self.schema);
        }
        if !is_version(&self.node.version) {
            bail!("Node version {:?} is not x.y.z", self.node.version);
        }
        let keys: BTreeSet<&str> = Platform::ALL.iter().map(|p| p.key()).collect();
        let node_keys: BTreeSet<&str> = self.node.platforms.keys().map(String::as_str).collect();
        if node_keys != keys {
            bail!("Node lists platforms {node_keys:?}, not {keys:?}");
        }
        for (platform, archive) in &self.node.platforms {
            let expected = format!("{NODE_DIST}v{v}/node-v{v}-{platform}.tar.gz", v = self.node.version);
            if archive.url != expected {
                bail!("Node for {platform}: url {:?}, expected {expected:?}", archive.url);
            }
            if archive.sha256.len() != 64 || !archive.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("Node for {platform}: sha256 is not 64 hex digits");
            }
            if archive.archive_size == 0 || archive.node_size == 0 {
                bail!("Node for {platform}: a size is zero");
            }
        }
        if self.adapters.is_empty() {
            bail!("the manifest pins no adapter");
        }
        for (name, adapter) in &self.adapters {
            if !is_agent_name(name) {
                bail!("adapter name {name:?} is not lowercase letters, digits and dashes");
            }
            let package_path = format!("node_modules/{}", adapter.package);
            check_relative_path(&adapter.entry).with_context(|| format!("{name}: entry"))?;
            if !adapter.entry.starts_with(&format!("{package_path}/")) {
                bail!("{name}: entry {:?} is not inside {package_path}", adapter.entry);
            }
            let adapter_keys: BTreeSet<&str> = adapter.platforms.keys().map(String::as_str).collect();
            if adapter_keys != keys {
                bail!("{name} lists platforms {adapter_keys:?}, not {keys:?}");
            }
            for (platform, files) in &adapter.platforms {
                let mut paths = BTreeSet::new();
                for file in files {
                    check_file(file).with_context(|| format!("{name} on {platform}: {}", file.path))?;
                    if !paths.insert(file.path.as_str()) {
                        bail!("{name} on {platform}: {} is listed twice", file.path);
                    }
                }
                if !paths.contains(package_path.as_str()) {
                    bail!("{name} on {platform}: the adapter package itself is missing");
                }
                if !files.iter().any(|f| f.cli) {
                    bail!("{name} on {platform}: no file is marked as the agent CLI");
                }
                // A path inside another is a package nested in its parent's
                // `node_modules`; anything else nested would overlap it.
                for path in &paths {
                    for other in &paths {
                        if path != other
                            && other.starts_with(&format!("{path}/"))
                            && !other[path.len() + 1..].starts_with("node_modules/")
                        {
                            bail!("{name} on {platform}: {other} overlaps {path}");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// SHA-256 of the manifest's text, hex: what `doctor` reports.
    pub fn hash_of(text: &str) -> String {
        hex::encode(Sha256::digest(text.as_bytes()))
    }
}

fn check_file(file: &File) -> Result<()> {
    check_relative_path(&file.path)?;
    if !file.path.starts_with("node_modules/") {
        bail!("path is not under node_modules/");
    }
    if !file.url.starts_with(NPM_REGISTRY) || file.url.contains('?') || file.url.contains('#') {
        bail!("url {:?} is not a plain {NPM_REGISTRY} URL", file.url);
    }
    sri_sha512(&file.integrity)?;
    if file.archive_size == 0 {
        bail!("archive_size is zero");
    }
    Ok(())
}

/// The 64 bytes of an SRI `sha512-<base64>` digest.
pub fn sri_sha512(integrity: &str) -> Result<[u8; 64]> {
    use base64::Engine;
    let encoded = integrity
        .strip_prefix("sha512-")
        .with_context(|| format!("integrity {integrity:?} is not sha512"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .with_context(|| format!("integrity {integrity:?} is not base64"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("integrity {integrity:?} is not 64 bytes"))
}

/// A relative path of normal components only, `/`-separated: no root, no
/// `.` or `..`, no empty component, no backslash.
pub fn check_relative_path(path: &str) -> Result<()> {
    let plain = !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && path.split('/').all(|c| !c.is_empty() && c != "." && c != "..");
    if !plain {
        bail!("{path:?} is not a plain relative path");
    }
    Ok(())
}

fn is_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// An agent name as `hello.agents` and `--use-cli` spell it.
pub fn is_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_manifest_is_valid_and_pins_both_agents_everywhere() {
        let manifest = Manifest::embedded();
        assert_eq!(manifest.schema, 1);
        assert_eq!(
            manifest.adapters.keys().map(String::as_str).collect::<Vec<_>>(),
            ["claude", "codex"]
        );
        assert_eq!(
            manifest.adapters["claude"].package,
            "@agentclientprotocol/claude-agent-acp"
        );
        assert_eq!(manifest.adapters["codex"].package, "@agentclientprotocol/codex-acp");
        for platform in Platform::ALL {
            for adapter in manifest.adapters.values() {
                let files = &adapter.platforms[platform.key()];
                assert!(files.iter().all(|f| f.unpacked_size > 0 && !f.install_script));
            }
        }
        // As the generator writes it: re-serialising changes nothing.
        assert_eq!(manifest.to_json(), EMBEDDED);
    }

    #[test]
    fn the_embedded_claude_cli_is_the_glibc_build_only() {
        let manifest = Manifest::embedded();
        let claude = &manifest.adapters["claude"];
        for platform in Platform::ALL {
            let cli: Vec<&str> = claude.platforms[platform.key()]
                .iter()
                .filter(|f| f.cli)
                .map(|f| f.path.as_str())
                .collect();
            assert_eq!(
                cli,
                [format!(
                    "node_modules/@anthropic-ai/claude-agent-sdk-{}",
                    platform.key()
                )],
                "{platform:?}"
            );
        }
    }

    #[test]
    fn paths_must_be_plain_and_relative() {
        for good in ["node_modules/zod", "node_modules/@a/b/node_modules/c"] {
            check_relative_path(good).unwrap();
        }
        for bad in ["", "/abs", "a/../b", "./a", "a//b", "a/", "..", "a\\b", "a/./b"] {
            assert!(check_relative_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_manifest_off_its_registry_or_with_a_short_digest_is_refused() {
        let good = Manifest::embedded();
        let mut off = good.clone();
        off.adapters
            .get_mut("codex")
            .unwrap()
            .platforms
            .get_mut("linux-x64")
            .unwrap()[0]
            .url = "https://registry.example/zod.tgz".into();
        assert!(format!("{:#}", off.validate().unwrap_err()).contains("not a plain"));
        let mut short = good.clone();
        short
            .adapters
            .get_mut("codex")
            .unwrap()
            .platforms
            .get_mut("linux-x64")
            .unwrap()[0]
            .integrity = "sha512-AAAA".into();
        assert!(format!("{:#}", short.validate().unwrap_err()).contains("64 bytes"));
        let mut node = good.clone();
        node.node.platforms.get_mut("darwin-arm64").unwrap().url = "http://nodejs.org/dist/x.tar.gz".into();
        assert!(node.validate().is_err());
        let mut schema = good;
        schema.schema = 2;
        assert!(schema.validate().is_err());
    }

    #[test]
    fn unknown_fields_are_refused() {
        let text = EMBEDDED.replacen("\"schema\": 1,", "\"schema\": 1,\n  \"extra\": true,", 1);
        assert!(Manifest::parse(&text).is_err());
    }
}
