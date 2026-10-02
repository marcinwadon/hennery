//! The adapter manifest's generator (distribution spec §3.1, plan 7b). The
//! pins (`adapters/pins.toml`) and one npm lockfile per adapter are checked
//! in; the registry's version documents and Node's `SHASUMS256.txt` supply
//! the rest. Everything here is pure: `main.rs` does the fetching, so the
//! rules are tested on fixtures and never touch the network.

use anyhow::{Context, Result, bail};
use hennery_host::runtime::manifest::{
    self, Adapter, CodexAppServer, File, Manifest, NPM_REGISTRY, Node, NodeArchive, Platform, SCHEMA,
};
use serde::Deserialize;
use std::collections::BTreeMap;

/// `adapters/pins.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pins {
    /// npm resolves the lockfiles as of this instant (`npm --before`), so
    /// `hennery-pins lock` gives the same lockfile until the pins change.
    pub npm_before: String,
    pub node: NodePin,
    pub adapters: BTreeMap<String, AdapterPin>,
    /// `thread/delete`'s call shape for the bundled Codex (plan 9d decision
    /// 9), carried into the manifest as it is.
    #[serde(default)]
    pub codex_app_server: Option<CodexAppServer>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodePin {
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterPin {
    pub package: String,
    /// An exact version: no range.
    pub version: String,
    /// The adapter's script, relative to its package.
    pub entry: String,
    /// Lockfile path prefixes of the agent CLI's packages (skipped by
    /// `--use-cli`).
    pub cli: Vec<String>,
}

impl Pins {
    pub fn parse(text: &str) -> Result<Self> {
        let pins: Pins = toml::from_str(text).context("parse adapters/pins.toml")?;
        for (name, adapter) in &pins.adapters {
            if !manifest::is_agent_name(name) {
                bail!("adapter name {name:?} is not lowercase letters, digits and dashes");
            }
            if adapter.version.is_empty()
                || !adapter
                    .version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
            {
                bail!("{name}: version {:?} is not an exact version", adapter.version);
            }
            manifest::check_relative_path(&adapter.entry).with_context(|| format!("{name}: entry"))?;
            if adapter.cli.is_empty() {
                bail!("{name}: no CLI package prefix");
            }
        }
        if let Some(app_server) = &pins.codex_app_server {
            app_server.validate()?;
        }
        Ok(pins)
    }

    /// The `package.json` `hennery-pins lock` writes for one adapter.
    pub fn package_json(&self, name: &str) -> Result<String> {
        let adapter = self.adapters.get(name).with_context(|| format!("no adapter {name}"))?;
        let value = serde_json::json!({
            "name": format!("hennery-adapter-{name}"),
            "private": true,
            "dependencies": { adapter.package.clone(): adapter.version.clone() },
        });
        let mut text = serde_json::to_string_pretty(&value)?;
        text.push('\n');
        Ok(text)
    }
}

/// A `package-lock.json`, version 3: only what the manifest needs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lock {
    pub lockfile_version: u32,
    pub packages: BTreeMap<String, LockEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockEntry {
    pub name: Option<String>,
    pub version: Option<String>,
    pub resolved: Option<String>,
    pub integrity: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dev: bool,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub dev_optional: bool,
    #[serde(default)]
    pub link: bool,
    #[serde(default)]
    pub has_install_script: bool,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default)]
    pub cpu: Vec<String>,
    #[serde(default)]
    pub libc: Vec<String>,
}

/// A registry version document (`GET <registry>/<name>/<version>`): only
/// what the manifest needs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDoc {
    pub name: String,
    pub version: String,
    #[serde(default, deserialize_with = "string_or_list")]
    pub os: Vec<String>,
    #[serde(default, deserialize_with = "string_or_list")]
    pub cpu: Vec<String>,
    #[serde(default, deserialize_with = "string_or_list")]
    pub libc: Vec<String>,
    #[serde(default)]
    pub scripts: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub gypfile: bool,
    #[serde(default)]
    pub bin: Option<serde_json::Value>,
    pub dist: Dist,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dist {
    pub tarball: String,
    pub integrity: Option<String>,
    pub unpacked_size: Option<u64>,
}

/// npm accepts `"os": "linux"` as well as `["linux"]`.
fn string_or_list<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Option::<OneOrMany>::deserialize(d)? {
        None => Vec::new(),
        Some(OneOrMany::One(s)) => vec![s],
        Some(OneOrMany::Many(v)) => v,
    })
}

/// What the registry said, keyed for `build`.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    /// By `(name, version)`.
    pub docs: BTreeMap<(String, String), VersionDoc>,
    /// Tarball bytes, by URL.
    pub archive_sizes: BTreeMap<String, u64>,
}

/// The package a lockfile entry names: its `name`, else its path's last
/// `node_modules/` segment.
pub fn package_name(path: &str, entry: &LockEntry) -> Result<String> {
    if let Some(name) = &entry.name {
        return Ok(name.clone());
    }
    let (_, name) = path
        .rsplit_once("node_modules/")
        .with_context(|| format!("lockfile path {path:?} is not under node_modules/"))?;
    Ok(name.to_string())
}

/// Every `(name, version, tarball URL)` the lockfiles name: what `main.rs`
/// fetches before `build`.
pub fn wanted(locks: &BTreeMap<String, Lock>) -> Result<Vec<(String, String, String)>> {
    let mut out = Vec::new();
    for lock in locks.values() {
        for (path, entry) in &lock.packages {
            if path.is_empty() {
                continue;
            }
            let name = package_name(path, entry)?;
            let version = entry.version.clone().with_context(|| format!("{path}: no version"))?;
            let resolved = entry
                .resolved
                .clone()
                .with_context(|| format!("{path}: no resolved URL"))?;
            out.push((name, version, resolved));
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// npm's rule for `os` and `cpu` (and `libc` on Linux): an empty list
/// allows every value; `!x` excludes `x`; a list with any positive entry
/// allows only those.
pub fn allows(list: &[String], value: &str) -> bool {
    if list.is_empty() {
        return true;
    }
    if list.iter().any(|v| v.strip_prefix('!') == Some(value)) {
        return false;
    }
    let positives: Vec<&String> = list.iter().filter(|v| !v.starts_with('!')).collect();
    positives.is_empty() || positives.iter().any(|v| v.as_str() == value)
}

/// Whether a package with these fields installs on `platform`.
pub fn installs_on(doc: &VersionDoc, platform: Platform) -> bool {
    let (os, cpu, libc) = platform.npm();
    allows(&doc.os, os) && allows(&doc.cpu, cpu) && libc.is_none_or(|libc| allows(&doc.libc, libc))
}

/// The manifest the pins, lockfiles, registry and Node archives make.
pub fn build(
    pins: &Pins,
    locks: &BTreeMap<String, Lock>,
    registry: &Registry,
    node: &BTreeMap<Platform, NodeArchive>,
) -> Result<Manifest> {
    let mut adapters = BTreeMap::new();
    for (name, pin) in &pins.adapters {
        let lock = locks.get(name).with_context(|| format!("no lockfile for {name}"))?;
        let adapter = build_adapter(pin, lock, registry).with_context(|| format!("adapter {name}"))?;
        adapters.insert(name.clone(), adapter);
    }
    if locks.keys().any(|name| !pins.adapters.contains_key(name)) {
        bail!("a lockfile for an adapter pins.toml does not name");
    }
    if let Some(pin) = &pins.codex_app_server {
        let lock = locks
            .get(hennery_host::agent_home::CODEX)
            .context("codex_app_server is pinned, but no codex adapter is")?;
        check_codex_app_server(pin, lock)?;
    }
    let manifest = Manifest {
        schema: SCHEMA,
        node: Node {
            version: pins.node.version.clone(),
            platforms: node.iter().map(|(p, a)| (p.key().to_string(), a.clone())).collect(),
        },
        adapters,
        codex_app_server: pins.codex_app_server.clone(),
    };
    manifest.validate()?;
    Ok(manifest)
}

/// The npm package of Codex's CLI.
const CODEX_PACKAGE: &str = "@openai/codex";

/// The app-server's call shape is read from one Codex version (plan 9d
/// decision 9): its launcher must be a file of a package the codex lockfile
/// installs, and that package must be at the pinned version.
fn check_codex_app_server(pin: &CodexAppServer, lock: &Lock) -> Result<()> {
    let (path, entry) = lock
        .packages
        .iter()
        .filter(|(path, _)| !path.is_empty() && pin.bin.starts_with(&format!("{path}/")))
        .max_by_key(|(path, _)| path.len())
        .with_context(|| {
            format!(
                "codex_app_server: bin {:?} is not in a package of the codex lockfile",
                pin.bin
            )
        })?;
    let name = package_name(path, entry)?;
    // Codex's own CLI, never another package that happens to hold the
    // launcher's path (the review's item 5).
    if name != CODEX_PACKAGE {
        bail!(
            "codex_app_server: bin {:?} is in {name}, which is not {CODEX_PACKAGE}",
            pin.bin
        );
    }
    let version = entry
        .version
        .as_deref()
        .with_context(|| format!("{path}: no version"))?;
    if version != pin.codex_version {
        bail!(
            "codex_app_server pins Codex {}, but the codex lockfile bundles {name} {version}: \
             read thread/delete's call shape from that version and update the pin",
            pin.codex_version
        );
    }
    Ok(())
}

fn build_adapter(pin: &AdapterPin, lock: &Lock, registry: &Registry) -> Result<Adapter> {
    if lock.lockfile_version != 3 {
        bail!("lockfile version {} (expected 3)", lock.lockfile_version);
    }
    let root = lock.packages.get("").context("the lockfile has no root package")?;
    let expected = BTreeMap::from([(pin.package.clone(), pin.version.clone())]);
    if root.dependencies != expected {
        bail!(
            "the lockfile's root depends on {:?}, pins.toml on {expected:?}: run `hennery-pins lock`",
            root.dependencies
        );
    }
    let package_path = format!("node_modules/{}", pin.package);
    let mut platforms: BTreeMap<String, Vec<File>> = Platform::ALL
        .iter()
        .map(|p| (p.key().to_string(), Vec::new()))
        .collect();
    for (path, entry) in &lock.packages {
        if path.is_empty() {
            continue;
        }
        manifest::check_relative_path(path).with_context(|| format!("lockfile path {path:?}"))?;
        if entry.link || entry.dev {
            bail!("{path}: a link or dev entry; the adapter lockfiles have neither");
        }
        let package = package_name(path, entry)?;
        let version = entry
            .version
            .as_deref()
            .with_context(|| format!("{path}: no version"))?;
        let resolved = entry
            .resolved
            .as_deref()
            .with_context(|| format!("{path}: no resolved URL"))?;
        if !resolved.starts_with(NPM_REGISTRY) {
            bail!("{path}: resolved from {resolved:?}, not {NPM_REGISTRY}; lock against the public registry");
        }
        let integrity = entry
            .integrity
            .as_deref()
            .with_context(|| format!("{path}: no integrity"))?;
        let doc = registry
            .docs
            .get(&(package.clone(), version.to_string()))
            .with_context(|| format!("{path}: no registry document for {package}@{version}"))?;
        if doc.name != package || doc.version != version {
            bail!("{path}: the registry answered {}@{}", doc.name, doc.version);
        }
        if doc.dist.tarball != resolved {
            bail!(
                "{path}: the registry's tarball is {:?}, the lockfile's {resolved:?}",
                doc.dist.tarball
            );
        }
        if doc.dist.integrity.as_deref() != Some(integrity) {
            bail!("{path}: the lockfile's integrity is not the registry's");
        }
        // The registry is the source of truth (distribution §3.1); a lockfile
        // that disagrees was not made from it.
        if (&entry.os, &entry.cpu) != (&doc.os, &doc.cpu) || (!entry.libc.is_empty() && entry.libc != doc.libc) {
            bail!("{path}: the lockfile's os/cpu/libc are not the registry's");
        }
        let install_phase = ["preinstall", "install", "postinstall"];
        if entry.has_install_script || doc.gypfile || install_phase.iter().any(|s| doc.scripts.contains_key(*s)) {
            bail!("{path}: {package}@{version} has an install script, which hennery never runs");
        }
        let unpacked_size = doc
            .dist
            .unpacked_size
            .with_context(|| format!("{path}: the registry gives no unpackedSize"))?;
        let archive_size = *registry
            .archive_sizes
            .get(resolved)
            .with_context(|| format!("{path}: no archive size for {resolved}"))?;
        if path == &package_path {
            if version != pin.version {
                bail!("{path} is {version}, pins.toml says {}", pin.version);
            }
            let bins: Vec<String> = match &doc.bin {
                Some(serde_json::Value::String(s)) => vec![s.clone()],
                Some(serde_json::Value::Object(map)) => {
                    map.values().filter_map(|v| v.as_str().map(str::to_string)).collect()
                }
                _ => Vec::new(),
            };
            let normalise = |s: &str| s.trim_start_matches("./").to_string();
            if !bins.iter().any(|b| normalise(b) == pin.entry) {
                bail!("{path}: entry {:?} is not one of its bins {bins:?}", pin.entry);
            }
        }
        let cli = pin.cli.iter().any(|prefix| path.starts_with(prefix.as_str()));
        for platform in Platform::ALL {
            if installs_on(doc, platform) {
                platforms.get_mut(platform.key()).expect("every platform").push(File {
                    path: path.clone(),
                    url: resolved.to_string(),
                    integrity: integrity.to_string(),
                    archive_size,
                    unpacked_size,
                    cli,
                    install_script: false,
                });
            } else if !(entry.optional || entry.dev_optional) {
                bail!("{path}: required, but it does not install on {}", platform.key());
            }
        }
        // A package for none of the three (Windows, Intel macOS, musl) is
        // simply not listed.
    }
    for files in platforms.values_mut() {
        files.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(Adapter {
        package: pin.package.clone(),
        version: pin.version.clone(),
        entry: format!("{package_path}/{}", pin.entry),
        platforms,
    })
}

/// The SHA-256 of each pinned Node archive, from `SHASUMS256.txt`.
pub fn node_digests(shasums: &str, version: &str) -> Result<BTreeMap<Platform, String>> {
    let mut out = BTreeMap::new();
    for platform in Platform::ALL {
        let file = format!("node-v{version}-{}.tar.gz", platform.key());
        let digest = shasums
            .lines()
            .find_map(|line| {
                let (digest, name) = line.split_once("  ")?;
                (name == file).then(|| digest.to_string())
            })
            .with_context(|| format!("SHASUMS256.txt lists no {file}"))?;
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("SHASUMS256.txt: {file} has a malformed digest");
        }
        out.insert(platform, digest.to_ascii_lowercase());
    }
    Ok(out)
}

/// The bytes of every file in a package tarball.
pub fn unpacked_size(tarball: &[u8]) -> Result<u64> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(tarball));
    let mut total = 0;
    for entry in tar.entries()? {
        let entry = entry?;
        if entry.header().entry_type().is_file() {
            total += entry.size();
        }
    }
    Ok(total)
}

/// The size of `node-v<version>-<platform>/bin/node` in a Node archive.
pub fn node_binary_size(archive: &[u8], version: &str, platform: Platform) -> Result<u64> {
    let wanted = format!("node-v{version}-{}/bin/node", platform.key());
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries()? {
        let entry = entry?;
        if entry.path()?.to_str() == Some(wanted.as_str()) {
            if !entry.header().entry_type().is_file() {
                bail!("{wanted} is not a regular file");
            }
            return Ok(entry.size());
        }
    }
    bail!("the archive has no {wanted}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINS: &str = r#"
npm_before = "2026-10-01T00:00:00Z"
[node]
version = "24.21.0"
[adapters.claude]
package = "@acp/claude"
version = "1.0.0"
entry = "dist/index.js"
cli = ["node_modules/@vendor/cli-"]
"#;

    fn entry(name: Option<&str>, version: &str, url: &str) -> LockEntry {
        LockEntry {
            name: name.map(str::to_string),
            version: Some(version.into()),
            resolved: Some(url.into()),
            integrity: Some(sri(url)),
            ..LockEntry::default()
        }
    }

    /// A distinct, well-formed digest per URL.
    fn sri(url: &str) -> String {
        use base64::Engine;
        use sha2::Digest;
        let digest = sha2::Sha512::digest(url.as_bytes());
        format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(digest))
    }

    struct Fixture {
        lock: Lock,
        registry: Registry,
    }

    impl Fixture {
        /// `@acp/claude` depending on `@vendor/cli` with glibc and musl
        /// variants for Linux, a darwin one and a win32 one.
        fn new() -> Self {
            let mut fixture = Fixture {
                lock: Lock {
                    lockfile_version: 3,
                    packages: BTreeMap::from([(
                        String::new(),
                        LockEntry {
                            dependencies: BTreeMap::from([("@acp/claude".into(), "1.0.0".into())]),
                            ..LockEntry::default()
                        },
                    )]),
                },
                registry: Registry::default(),
            };
            fixture.add("node_modules/@acp/claude", "@acp/claude", "1.0.0", &[], &[], &[], false);
            fixture.add("node_modules/@vendor/cli", "@vendor/cli", "2.0.0", &[], &[], &[], false);
            for (suffix, os, cpu, libc) in [
                ("linux-x64", "linux", "x64", "glibc"),
                ("linux-x64-musl", "linux", "x64", "musl"),
                ("linux-arm64", "linux", "arm64", "glibc"),
                ("linux-arm64-musl", "linux", "arm64", "musl"),
                ("darwin-arm64", "darwin", "arm64", ""),
                ("win32-x64", "win32", "x64", ""),
            ] {
                let libc: &[&str] = if libc.is_empty() { &[] } else { &[libc] };
                fixture.add(
                    &format!("node_modules/@vendor/cli-{suffix}"),
                    &format!("@vendor/cli-{suffix}"),
                    "2.0.0",
                    &[os],
                    &[cpu],
                    libc,
                    true,
                );
            }
            fixture
        }

        #[allow(clippy::too_many_arguments)]
        fn add(
            &mut self,
            path: &str,
            name: &str,
            version: &str,
            os: &[&str],
            cpu: &[&str],
            libc: &[&str],
            optional: bool,
        ) {
            let url = format!("{NPM_REGISTRY}{name}/-/x-{version}.tgz");
            let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            let mut lock = entry(None, version, &url);
            lock.os = strings(os);
            lock.cpu = strings(cpu);
            lock.libc = strings(libc);
            lock.optional = optional;
            self.lock.packages.insert(path.into(), lock);
            self.registry.docs.insert(
                (name.into(), version.into()),
                VersionDoc {
                    name: name.into(),
                    version: version.into(),
                    os: strings(os),
                    cpu: strings(cpu),
                    libc: strings(libc),
                    bin: Some(serde_json::json!({ "x": "dist/index.js" })),
                    dist: Dist {
                        tarball: url.clone(),
                        integrity: Some(sri(&url)),
                        unpacked_size: Some(1000),
                    },
                    ..VersionDoc::default()
                },
            );
            self.registry.archive_sizes.insert(url, 400);
        }

        fn build(&self) -> Result<Manifest> {
            let pins = Pins::parse(PINS).unwrap();
            let node = Platform::ALL
                .into_iter()
                .map(|p| {
                    (
                        p,
                        NodeArchive {
                            url: format!("https://nodejs.org/dist/v24.21.0/node-v24.21.0-{}.tar.gz", p.key()),
                            sha256: "a".repeat(64),
                            archive_size: 10,
                            node_size: 20,
                        },
                    )
                })
                .collect();
            build(
                &pins,
                &BTreeMap::from([("claude".into(), self.lock.clone())]),
                &self.registry,
                &node,
            )
        }
    }

    #[test]
    fn each_platform_gets_its_own_glibc_cli_and_nothing_for_musl_or_windows() {
        let manifest = Fixture::new().build().unwrap();
        let claude = &manifest.adapters["claude"];
        assert_eq!(claude.entry, "node_modules/@acp/claude/dist/index.js");
        for (platform, cli) in [
            ("darwin-arm64", "node_modules/@vendor/cli-darwin-arm64"),
            ("linux-x64", "node_modules/@vendor/cli-linux-x64"),
            ("linux-arm64", "node_modules/@vendor/cli-linux-arm64"),
        ] {
            let paths: Vec<&str> = claude.platforms[platform].iter().map(|f| f.path.as_str()).collect();
            assert_eq!(
                paths,
                ["node_modules/@acp/claude", "node_modules/@vendor/cli", cli],
                "{platform}"
            );
            let marked: Vec<&str> = claude.platforms[platform]
                .iter()
                .filter(|f| f.cli)
                .map(|f| f.path.as_str())
                .collect();
            assert_eq!(marked, [cli]);
        }
    }

    #[test]
    fn a_platform_missing_a_required_package_fails() {
        let mut fixture = Fixture::new();
        // Required (not optional) yet Linux-only: darwin would lack it.
        fixture.add(
            "node_modules/linux-only",
            "linux-only",
            "1.0.0",
            &["linux"],
            &[],
            &[],
            false,
        );
        let err = format!("{:#}", fixture.build().unwrap_err());
        assert!(err.contains("does not install on darwin-arm64"), "{err}");
    }

    #[test]
    fn a_platform_with_no_cli_fails() {
        let mut fixture = Fixture::new();
        fixture.lock.packages.remove("node_modules/@vendor/cli-linux-arm64");
        let err = format!("{:#}", fixture.build().unwrap_err());
        assert!(err.contains("linux-arm64: no file is marked as the agent CLI"), "{err}");
    }

    #[test]
    fn a_package_resolved_elsewhere_fails() {
        let mut fixture = Fixture::new();
        let entry = fixture.lock.packages.get_mut("node_modules/@vendor/cli").unwrap();
        entry.resolved = Some("https://npm.corp.example/@vendor/cli/-/x-2.0.0.tgz".into());
        let err = format!("{:#}", fixture.build().unwrap_err());
        assert!(err.contains("not https://registry.npmjs.org/"), "{err}");
    }

    #[test]
    fn an_install_script_fails_from_the_lockfile_or_the_registry() {
        let mut fixture = Fixture::new();
        fixture
            .lock
            .packages
            .get_mut("node_modules/@vendor/cli")
            .unwrap()
            .has_install_script = true;
        assert!(format!("{:#}", fixture.build().unwrap_err()).contains("install script"));
        let mut fixture = Fixture::new();
        let doc = fixture
            .registry
            .docs
            .get_mut(&("@vendor/cli".into(), "2.0.0".into()))
            .unwrap();
        doc.scripts.insert("postinstall".into(), "node x.js".into());
        assert!(format!("{:#}", fixture.build().unwrap_err()).contains("install script"));
    }

    #[test]
    fn a_lockfile_that_disagrees_with_the_registry_fails() {
        let mut fixture = Fixture::new();
        fixture
            .lock
            .packages
            .get_mut("node_modules/@vendor/cli-linux-x64-musl")
            .unwrap()
            .libc = vec!["glibc".into()];
        assert!(format!("{:#}", fixture.build().unwrap_err()).contains("os/cpu/libc"));
        let mut fixture = Fixture::new();
        fixture
            .lock
            .packages
            .get_mut("node_modules/@vendor/cli")
            .unwrap()
            .integrity = Some(sri("other"));
        assert!(format!("{:#}", fixture.build().unwrap_err()).contains("integrity"));
        let mut fixture = Fixture::new();
        fixture.lock.packages.get_mut("").unwrap().dependencies =
            BTreeMap::from([("@acp/claude".into(), "^1.0.0".into())]);
        assert!(format!("{:#}", fixture.build().unwrap_err()).contains("hennery-pins lock"));
    }

    #[test]
    fn a_lockfile_that_drops_libc_still_filters_by_the_registry() {
        let mut fixture = Fixture::new();
        for entry in fixture.lock.packages.values_mut() {
            entry.libc.clear();
        }
        let manifest = fixture.build().unwrap();
        let linux: Vec<&str> = manifest.adapters["claude"].platforms["linux-x64"]
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert!(!linux.contains(&"node_modules/@vendor/cli-linux-x64-musl"), "{linux:?}");
    }

    /// `PINS`, with a codex adapter bundling `@openai/codex` and the
    /// app-server's call shape pinned for its version (plan 9d decision 9).
    const CODEX_PINS: &str = r#"
[adapters.codex]
package = "@acp/codex"
version = "1.0.0"
entry = "dist/index.js"
cli = ["node_modules/@openai/codex-"]
[codex_app_server]
codex_version = "0.155.1"
bin = "node_modules/@openai/codex/bin/codex.js"
delete_method = "thread/delete"
params_shape = { threadId = "{thread_id}" }
[codex_app_server.initialize.clientInfo]
name = "hennery"
title = "hennery"
version = "1"
"#;

    /// The codex adapter's lockfile and registry for `CODEX_PINS`, its
    /// `@openai/codex` at `codex_version`.
    fn codex_fixture(codex_version: &str) -> (Pins, BTreeMap<String, Lock>, Registry) {
        let pins = Pins::parse(&format!("{PINS}{CODEX_PINS}")).unwrap();
        let claude = Fixture::new();
        let mut codex = Fixture {
            lock: Lock {
                lockfile_version: 3,
                packages: BTreeMap::from([(
                    String::new(),
                    LockEntry {
                        dependencies: BTreeMap::from([("@acp/codex".into(), "1.0.0".into())]),
                        ..LockEntry::default()
                    },
                )]),
            },
            registry: claude.registry.clone(),
        };
        codex.add("node_modules/@acp/codex", "@acp/codex", "1.0.0", &[], &[], &[], false);
        codex.add(
            "node_modules/@openai/codex",
            "@openai/codex",
            codex_version,
            &[],
            &[],
            &[],
            false,
        );
        for (suffix, os, cpu) in [
            ("linux-x64", "linux", "x64"),
            ("linux-arm64", "linux", "arm64"),
            ("darwin-arm64", "darwin", "arm64"),
        ] {
            codex.add(
                &format!("node_modules/@openai/codex-{suffix}"),
                &format!("@openai/codex-{suffix}"),
                codex_version,
                &[os],
                &[cpu],
                &[],
                true,
            );
        }
        let locks = BTreeMap::from([("claude".into(), claude.lock), ("codex".into(), codex.lock)]);
        (pins, locks, codex.registry)
    }

    fn node_archives() -> BTreeMap<Platform, NodeArchive> {
        Platform::ALL
            .into_iter()
            .map(|p| {
                (
                    p,
                    NodeArchive {
                        url: format!("https://nodejs.org/dist/v24.21.0/node-v24.21.0-{}.tar.gz", p.key()),
                        sha256: "a".repeat(64),
                        archive_size: 10,
                        node_size: 20,
                    },
                )
            })
            .collect()
    }

    /// Plan 9d decision 9: the app-server's call shape is carried into the
    /// manifest, and only for the Codex version the codex lockfile bundles.
    #[test]
    fn the_codex_app_server_pin_is_carried_only_for_the_bundled_codex_version() {
        let (pins, locks, registry) = codex_fixture("0.155.1");
        let manifest = build(&pins, &locks, &registry, &node_archives()).unwrap();
        let pin = manifest.codex_app_server.expect("carried");
        assert_eq!(pin.codex_version, "0.155.1");
        assert_eq!(pin.bin, "node_modules/@openai/codex/bin/codex.js");
        let (pins, locks, registry) = codex_fixture("0.156.0");
        let err = format!("{:#}", build(&pins, &locks, &registry, &node_archives()).unwrap_err());
        assert!(err.contains("bundles @openai/codex 0.156.0"), "{err}");
        // A launcher in another package, even at the pinned version (the
        // review's item 5).
        let (mut pins, locks, registry) = codex_fixture("0.155.1");
        let pin = pins.codex_app_server.as_mut().unwrap();
        pin.bin = "node_modules/@acp/codex/dist/index.js".into();
        pin.codex_version = "1.0.0".into();
        let err = format!("{:#}", build(&pins, &locks, &registry, &node_archives()).unwrap_err());
        assert!(err.contains("is not @openai/codex"), "{err}");
        // A launcher outside every package of the codex lockfile.
        let (mut pins, locks, registry) = codex_fixture("0.155.1");
        pins.codex_app_server.as_mut().unwrap().bin = "node_modules/@other/codex/bin/codex.js".into();
        let err = format!("{:#}", build(&pins, &locks, &registry, &node_archives()).unwrap_err());
        assert!(err.contains("not in a package of the codex lockfile"), "{err}");
        // A shape out of its rules is refused as pins.toml is read.
        let bad = format!("{PINS}{CODEX_PINS}").replace("\"thread/delete\"", "\"thread/archive\"");
        assert!(format!("{:#}", Pins::parse(&bad).unwrap_err()).contains("thread/delete"));
        // And none without a pin.
        let (mut pins, locks, registry) = codex_fixture("0.155.1");
        pins.codex_app_server = None;
        assert!(
            build(&pins, &locks, &registry, &node_archives())
                .unwrap()
                .codex_app_server
                .is_none()
        );
    }

    #[test]
    fn npms_platform_lists_allow_negation() {
        let list = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(allows(&[], "linux"));
        assert!(allows(&list(&["linux", "darwin"]), "darwin"));
        assert!(!allows(&list(&["linux"]), "darwin"));
        assert!(!allows(&list(&["!win32"]), "win32"));
        assert!(allows(&list(&["!win32"]), "linux"));
    }

    #[test]
    fn node_digests_come_from_the_tar_gz_lines() {
        let shasums = "\
aaaa  node-v24.21.0-darwin-arm64.tar.xz
1111111111111111111111111111111111111111111111111111111111111111  node-v24.21.0-darwin-arm64.tar.gz
2222222222222222222222222222222222222222222222222222222222222222  node-v24.21.0-linux-arm64.tar.gz
3333333333333333333333333333333333333333333333333333333333333333  node-v24.21.0-linux-x64.tar.gz
";
        let digests = node_digests(shasums, "24.21.0").unwrap();
        assert_eq!(digests[&Platform::LinuxX64], "3".repeat(64));
        assert!(node_digests(shasums, "24.20.0").is_err());
    }
}
