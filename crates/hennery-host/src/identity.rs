//! The host's identity (kernel spec §4.1, ACP core §3.5): the Ed25519 key it
//! generated when it paired, and the collector and host id it paired with.
//! Both live in the host's data directory (distribution spec §8):
//! `host.key` (the key's 32-byte seed in hex, mode 0600) and `host.toml`.

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
}

/// A paired host: what `hennery host run` needs to connect.
#[derive(Debug, Clone)]
pub struct Paired {
    pub collector_url: String,
    pub host_id: String,
    pub key: HostKey,
}

impl Paired {
    /// The pairing stored in `data_dir`: `None` if the host was never
    /// paired there, an error if only half of it is there.
    pub fn load(data_dir: &Path) -> Result<Option<Self>> {
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
        let text = std::fs::read_to_string(&config_path).with_context(|| format!("read {}", config_path.display()))?;
        let config: HostToml = toml::from_str(&text).with_context(|| format!("parse {}", config_path.display()))?;
        Ok(Some(Self {
            collector_url: config.collector,
            host_id: config.host_id,
            key: HostKey::load(&key_path)?,
        }))
    }

    /// Store the pairing in `data_dir`: the key first, then `host.toml`, so
    /// a `host.toml` is never there without its key.
    pub fn save(&self, data_dir: &Path) -> Result<()> {
        create_private_dir(data_dir)?;
        self.key.save(&data_dir.join(KEY_FILE))?;
        let config = toml::to_string(&HostToml {
            collector: self.collector_url.clone(),
            host_id: self.host_id.clone(),
        })?;
        write_private(&data_dir.join(CONFIG_FILE), config.as_bytes())
    }
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
    Ok(())
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
        };
        paired.save(dir.path()).unwrap();
        let loaded = Paired::load(dir.path()).unwrap().unwrap();
        assert_eq!(
            (loaded.collector_url.as_str(), loaded.host_id.as_str()),
            ("ws://127.0.0.1:7117/api/hosts/ws", "host-1")
        );
        assert_eq!(loaded.key.public_key_hex(), paired.key.public_key_hex());
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
}
