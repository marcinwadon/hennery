//! The host's identity (kernel spec §4.1, ACP core §3.5): the Ed25519 key it
//! generated when it paired, and the collector and host id it paired with.
//! Both live in the host's data directory (distribution spec §8):
//! `host.key` (the key's 32-byte seed in hex, mode 0600) and `host.toml`,
//! which also holds the operator's workspace roots (ACP core §7).

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const KEY_FILE: &str = "host.key";
pub const CONFIG_FILE: &str = "host.toml";

/// The host's private key.
#[derive(Clone)]
pub struct HostKey(SigningKey);

impl std::fmt::Debug for HostKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HostKey({})", self.public_key_hex())
    }
}

impl HostKey {
    /// A new key from the operating system's CSPRNG.
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).expect("the OS random number generator is available");
        Self::from_seed(seed)
    }

    /// The key a 32-byte seed determines (tests pin keys this way).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&seed))
    }

    /// The public half, as enrollment sends it: 64 hex characters.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.0.verifying_key().as_bytes())
    }

    /// `hello.proof`: the signature over the collector's nonce, the host id
    /// and the protocol version (`hennery_proto::hello_proof_message`), hex.
    pub fn sign_hello(&self, nonce: &[u8], host_id: &str, protocol_version: &str) -> String {
        let message = hennery_proto::hello_proof_message(nonce, host_id, protocol_version);
        hex::encode(self.0.sign(&message).to_bytes())
    }

    /// Read a key file written by `save`. A key other users can read is
    /// still used, with a warning: it may have leaked already.
    pub fn load(path: &Path) -> Result<Self> {
        if !is_private(path)? {
            tracing::warn!(
                path = %path.display(),
                "the host key is readable by other users; restrict it with `chmod 600`, or pair again if it may have leaked"
            );
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let seed: [u8; 32] = hex::decode(text.trim())
            .ok()
            .and_then(|b| b.try_into().ok())
            .with_context(|| format!("{} does not hold a host key", path.display()))?;
        Ok(Self::from_seed(seed))
    }

    /// Write the key to `path` with mode 0600, atomically (a temporary file
    /// in the same directory, then a rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        write_private(path, format!("{}\n", hex::encode(self.0.to_bytes())).as_bytes())
    }
}

/// `host.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct HostToml {
    /// The collector's host WebSocket, e.g. `wss://c.example/api/hosts/ws`.
    collector: String,
    host_id: String,
    /// The public key enrolled under `host_id`, hex. Written by `join`, so
    /// that a staged pairing is rolled forward only onto its own key; absent
    /// from pairings stored before it, and then not checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_key: Option<String>,
    /// Where projects are (ACP core §7): absolute paths, or `~/…`, as the
    /// operator wrote them (`projects::workspace_roots` expands them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workspace_roots: Vec<String>,
}

/// A paired host: what `hennery host run` needs to connect.
#[derive(Debug, Clone)]
pub struct Paired {
    pub collector_url: String,
    pub host_id: String,
    pub key: HostKey,
    /// `host.toml`'s `workspace_roots`, as written there.
    pub workspace_roots: Vec<String>,
}

impl Paired {
    /// The pairing stored in `data_dir`: `None` if the host was never
    /// paired there, an error if only half of it is there.
    pub fn load(data_dir: &Path) -> Result<Option<Self>> {
        finish_interrupted_pairing(data_dir)?;
        let key_path = data_dir.join(KEY_FILE);
        let config_path = data_dir.join(CONFIG_FILE);
        match (key_path.exists(), config_path.exists()) {
            (false, false) => return Ok(None),
            (true, true) => {}
            (true, false) => bail!(
                "{} holds a host key but no {CONFIG_FILE}: remove {} to pair this host again",
                data_dir.display(),
                key_path.display()
            ),
            (false, true) => bail!(
                "{} holds a {CONFIG_FILE} but no host key: remove {} to pair this host again",
                data_dir.display(),
                config_path.display()
            ),
        }
        let config = read_config(&config_path)?;
        let key = HostKey::load(&key_path)?;
        if config.public_key.as_ref().is_some_and(|k| *k != key.public_key_hex()) {
            bail!(
                "{} is not the key {} was paired with: remove both to pair this host again",
                key_path.display(),
                config_path.display()
            );
        }
        Ok(Some(Self {
            collector_url: config.collector,
            host_id: config.host_id,
            key,
            workspace_roots: config.workspace_roots,
        }))
    }

    /// Store the pairing in `data_dir`: the key first, then `host.toml`, so
    /// a `host.toml` is never there without its key.
    pub fn save(&self, data_dir: &Path) -> Result<()> {
        create_private_dir(data_dir)?;
        self.key.save(&data_dir.join(KEY_FILE))?;
        write_config_with(&data_dir.join(CONFIG_FILE), |table| {
            set_pairing(table, &self.collector_url, &self.host_id, &self.key);
            if self.workspace_roots.is_empty() {
                table.remove("workspace_roots");
            } else {
                let roots = self.workspace_roots.iter().cloned().map(toml::Value::String).collect();
                table.insert("workspace_roots".into(), toml::Value::Array(roots));
            }
        })
    }
}

fn read_config(path: &Path) -> Result<HostToml> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// Write a `host.toml` at `path`, naming `key`'s public half:
/// `pairing::join` writes it as `pending_path(CONFIG_FILE)` first (see
/// `finish_interrupted_pairing`). Every other key of the `host.toml` in
/// place beside `path` is kept: the operator's `workspace_roots` survive a
/// re-pair after a revoke (plan 6c decision 6).
pub(crate) fn write_config_to(path: &Path, collector_url: &str, host_id: &str, key: &HostKey) -> Result<()> {
    write_config_with(path, |table| set_pairing(table, collector_url, host_id, key))
}

fn set_pairing(table: &mut toml::Table, collector_url: &str, host_id: &str, key: &HostKey) {
    table.insert("collector".into(), toml::Value::String(collector_url.to_string()));
    table.insert("host_id".into(), toml::Value::String(host_id.to_string()));
    table.insert("public_key".into(), toml::Value::String(key.public_key_hex()));
}

/// Write the table of the `host.toml` beside `path` (an empty one if there
/// is none yet), with `change` applied, to `path`: mode 0600, atomically. A
/// `host.toml` that does not parse is an error, never replaced.
fn write_config_with(path: &Path, change: impl FnOnce(&mut toml::Table)) -> Result<()> {
    let current = path.with_file_name(CONFIG_FILE);
    let mut table = match std::fs::read_to_string(&current) {
        Ok(text) => toml::from_str::<toml::Table>(&text).with_context(|| format!("parse {}", current.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(err) => return Err(err).with_context(|| format!("read {}", current.display())),
    };
    change(&mut table);
    write_private(path, toml::to_string(&table)?.as_bytes())
}

/// Where `join` stages `file` (`host.key` or `host.toml`) before renaming it
/// into place.
pub(crate) fn pending_path(data_dir: &Path, file: &str) -> PathBuf {
    data_dir.join(format!("{file}.pending"))
}

/// Finish a pairing that `join` committed but did not put in place (a crash
/// between its renames). `join` writes `host.toml.pending` only once the
/// collector has enrolled the key, and before it renames anything, so a
/// `host.toml.pending` means: the new key is `host.key.pending`, or already
/// `host.key`, and it belongs with that file, not with an older `host.toml`
/// (a revoked identity's, which the collector would refuse as `bad_proof`).
/// Roll both forward, in `join`'s order.
fn finish_interrupted_pairing(data_dir: &Path) -> Result<()> {
    let config_pending = pending_path(data_dir, CONFIG_FILE);
    if !config_pending.exists() {
        return Ok(());
    }
    let staged = read_config(&config_pending)?;
    let key_path = data_dir.join(KEY_FILE);
    let key_pending = pending_path(data_dir, KEY_FILE);
    let key_at = if key_pending.exists() { &key_pending } else { &key_path };
    // Rolled forward only onto the key it was enrolled with: never onto an
    // older identity's `host.key`.
    let matches = key_at.exists() && staged.public_key == Some(HostKey::load(key_at)?.public_key_hex());
    if !matches {
        bail!(
            "{} holds a {} without the key it was paired with: remove it to pair this host again",
            data_dir.display(),
            config_pending.display()
        );
    }
    if key_at == &key_pending {
        std::fs::rename(&key_pending, &key_path)
            .with_context(|| format!("rename {} to {}", key_pending.display(), key_path.display()))?;
        fsync_parent(&key_path)?;
    }
    crate::pairing::orphan_outbox(data_dir, "unpaired")?;
    let config_path = data_dir.join(CONFIG_FILE);
    std::fs::rename(&config_pending, &config_path)
        .with_context(|| format!("rename {} to {}", config_pending.display(), config_path.display()))?;
    fsync_parent(&config_path)?;
    tracing::warn!(dir = %data_dir.display(), host_id = %staged.host_id, "finished a pairing that was interrupted");
    Ok(())
}

/// Create `dir` (and its parents) with mode 0700: it holds the host key
/// and the outbox. An existing directory is left as it is.
pub fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("create {}", dir.display()))
}

/// Whether nobody but its owner can read or write `path`.
pub fn is_private(path: &Path) -> Result<bool> {
    let mode = std::fs::metadata(path)
        .with_context(|| format!("read {}", path.display()))?
        .permissions()
        .mode();
    Ok(mode & 0o077 == 0)
}

fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let tmp: PathBuf = {
        let mut name = path.file_name().context("a file path")?.to_os_string();
        name.push(".tmp");
        path.with_file_name(name)
    };
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("create {}", tmp.display()))?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename {} to {}", tmp.display(), path.display()))?;
    fsync_parent(path)
}

/// Fsync `path`'s parent directory: on its own, a rename is only guaranteed
/// durable once the directory entry that names it has been synced too — on
/// a crash right after a bare rename, some filesystems can still lose the
/// rename (or leave both names) even though the file's own `sync_all` ran.
pub(crate) fn fsync_parent(path: &Path) -> Result<()> {
    let dir = path.parent().context("a file path has a parent")?;
    let dir_file = std::fs::File::open(dir).with_context(|| format!("open {}", dir.display()))?;
    dir_file.sync_all().with_context(|| format!("fsync {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checked by the collector too (`hennery-kernel`'s `hosts` tests).
    const VECTOR_SIGNATURE: &str = "bd2b7388413c333e9ed69c330b4a8be8ffb6228609979b30607236fcdefab259\
cdf6b48fb39bfaa9b5a3cd01538280ec9e6d50c8831e9aae4d791f68112a6c04";

    #[test]
    fn the_host_signs_the_fixed_proof_vector() {
        let key = HostKey::from_seed([1; 32]);
        assert_eq!(key.sign_hello(&[2; 32], "host-1", "1.0"), VECTOR_SIGNATURE);
    }

    #[test]
    fn a_saved_pairing_loads_back_and_its_files_are_private() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Paired::load(dir.path()).unwrap().is_none());
        let paired = Paired {
            collector_url: "ws://127.0.0.1:7117/api/hosts/ws".into(),
            host_id: "host-1".into(),
            key: HostKey::generate(),
            workspace_roots: vec!["/srv/projects".into(), "~/src".into()],
        };
        paired.save(dir.path()).unwrap();
        let loaded = Paired::load(dir.path()).unwrap().unwrap();
        assert_eq!(
            (loaded.collector_url.as_str(), loaded.host_id.as_str()),
            ("ws://127.0.0.1:7117/api/hosts/ws", "host-1")
        );
        assert_eq!(loaded.key.public_key_hex(), paired.key.public_key_hex());
        assert_eq!(loaded.workspace_roots, ["/srv/projects", "~/src"]);
        for file in [KEY_FILE, CONFIG_FILE] {
            let mode = std::fs::metadata(dir.path().join(file)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }

    #[test]
    fn a_new_data_directory_is_private_and_a_readable_key_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("a").join("host");
        create_private_dir(&data).unwrap();
        let mode = std::fs::metadata(&data).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let key = data.join(KEY_FILE);
        HostKey::generate().save(&key).unwrap();
        assert!(is_private(&key).unwrap());
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_private(&key).unwrap());
        HostKey::load(&key).expect("still usable, with a warning");
    }

    /// 3a's M4: `join` crashed after enrolling a new key, at either point
    /// between its two renames. The older (revoked) `host.toml` must not be
    /// loaded beside the new key: the pairing is rolled forward.
    #[test]
    fn a_pairing_interrupted_between_its_renames_is_rolled_forward() {
        let new_key = HostKey::from_seed([2; 32]);
        for key_renamed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            Paired {
                collector_url: "ws://127.0.0.1:7117/api/hosts/ws".into(),
                host_id: "host-old".into(),
                key: HostKey::from_seed([1; 32]),
                workspace_roots: Vec::new(),
            }
            .save(dir.path())
            .unwrap();
            let key_at = if key_renamed {
                dir.path().join(KEY_FILE)
            } else {
                pending_path(dir.path(), KEY_FILE)
            };
            new_key.save(&key_at).unwrap();
            write_config_to(
                &pending_path(dir.path(), CONFIG_FILE),
                "ws://127.0.0.1:7117/api/hosts/ws",
                "host-new",
                &new_key,
            )
            .unwrap();
            let loaded = Paired::load(dir.path()).unwrap().unwrap();
            assert_eq!(loaded.host_id, "host-new", "key renamed: {key_renamed}");
            assert_eq!(loaded.key.public_key_hex(), new_key.public_key_hex());
            for file in [KEY_FILE, CONFIG_FILE] {
                assert!(
                    !pending_path(dir.path(), file).exists(),
                    "{file}, key renamed: {key_renamed}"
                );
            }
        }
    }

    /// A staged `host.toml` without the key it names is refused, never
    /// rolled forward: with no key at all, or beside an older identity's
    /// `host.key` (a `join` whose staged key was lost after enrolling).
    #[test]
    fn a_staged_pairing_without_its_own_key_is_refused() {
        let url = "ws://127.0.0.1:1/api/hosts/ws";
        let dir = tempfile::tempdir().unwrap();
        let staged = pending_path(dir.path(), CONFIG_FILE);
        write_config_to(&staged, url, "host-new", &HostKey::from_seed([2; 32])).unwrap();
        let err = Paired::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("host.toml.pending") && err.contains("remove"), "{err}");

        let dir = tempfile::tempdir().unwrap();
        let old = Paired {
            collector_url: url.into(),
            host_id: "host-old".into(),
            key: HostKey::from_seed([1; 32]),
            workspace_roots: Vec::new(),
        };
        old.save(dir.path()).unwrap();
        let staged = pending_path(dir.path(), CONFIG_FILE);
        write_config_to(&staged, url, "host-new", &HostKey::from_seed([2; 32])).unwrap();
        let err = Paired::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("host.toml.pending"), "{err}");
        assert!(staged.exists(), "left for the operator");
        // A pairing whose `host.toml` names another key is refused too.
        std::fs::remove_file(&staged).unwrap();
        write_config_to(
            &dir.path().join(CONFIG_FILE),
            url,
            "host-old",
            &HostKey::from_seed([3; 32]),
        )
        .unwrap();
        let err = Paired::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("is not the key"), "{err}");
    }

    /// A `host.toml` stored before it named its key still loads.
    #[test]
    fn a_pairing_stored_without_its_public_key_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        HostKey::from_seed([1; 32]).save(&dir.path().join(KEY_FILE)).unwrap();
        std::fs::write(
            dir.path().join(CONFIG_FILE),
            "collector = \"ws://127.0.0.1:1/api/hosts/ws\"\nhost_id = \"host-1\"\n",
        )
        .unwrap();
        assert_eq!(Paired::load(dir.path()).unwrap().unwrap().host_id, "host-1");
    }

    #[test]
    fn half_a_pairing_is_an_error_and_the_key_never_shows_in_debug() {
        let dir = tempfile::tempdir().unwrap();
        let key = HostKey::from_seed([1; 32]);
        key.save(&dir.path().join(KEY_FILE)).unwrap();
        let err = Paired::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("no host.toml") && err.contains("remove"), "{err}");
        assert!(err.contains(&dir.path().join(KEY_FILE).display().to_string()), "{err}");
        let seed_hex = hex::encode([1u8; 32]);
        assert!(!format!("{key:?}").contains(&seed_hex));
    }

    /// Plan 6c decision 6: a re-pair rewrites the pairing and keeps every
    /// other key, the operator's workspace roots among them.
    #[test]
    fn rewriting_the_pairing_keeps_the_operators_keys() {
        let dir = tempfile::tempdir().unwrap();
        let key = HostKey::generate();
        key.save(&dir.path().join(KEY_FILE)).unwrap();
        let config = dir.path().join(CONFIG_FILE);
        std::fs::write(
            &config,
            "collector = \"ws://127.0.0.1:1/api/hosts/ws\"\nhost_id = \"host-old\"\n\
             workspace_roots = [\"/srv/projects\"]\nfuture_key = 7\n",
        )
        .unwrap();
        write_config_to(&config, "ws://127.0.0.1:2/api/hosts/ws", "host-new", &key).unwrap();
        let loaded = Paired::load(dir.path()).unwrap().unwrap();
        assert_eq!(
            (loaded.collector_url.as_str(), loaded.host_id.as_str()),
            ("ws://127.0.0.1:2/api/hosts/ws", "host-new")
        );
        assert_eq!(loaded.workspace_roots, ["/srv/projects"]);
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("future_key = 7"), "{text}");
        assert_eq!(std::fs::metadata(&config).unwrap().permissions().mode() & 0o777, 0o600);
        // Staged beside it, as `join` does, it starts from the same file.
        write_config_to(
            &pending_path(dir.path(), CONFIG_FILE),
            "ws://127.0.0.1:3/api/hosts/ws",
            "host-3",
            &key,
        )
        .unwrap();
        let staged = std::fs::read_to_string(pending_path(dir.path(), CONFIG_FILE)).unwrap();
        assert!(
            staged.contains("/srv/projects") && staged.contains("host-3"),
            "{staged}"
        );
        std::fs::remove_file(pending_path(dir.path(), CONFIG_FILE)).unwrap();
        std::fs::write(&config, "not = [toml").unwrap();
        assert!(write_config_to(&config, "ws://127.0.0.1:2/api/hosts/ws", "host-new", &key).is_err());
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "not = [toml",
            "a host.toml that does not parse was replaced"
        );
    }
}
