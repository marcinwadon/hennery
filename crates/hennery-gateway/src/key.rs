//! The master key (gateway spec §6, kernel spec §10): 32 random bytes that
//! seal every credential the gateway holds. It comes from, in this order:
//! - the environment, `HENNERY_MASTER_KEY`, as 64 hexadecimal digits;
//! - a systemd credential, `$CREDENTIALS_DIRECTORY/hennery-master-key`,
//!   as 32 raw bytes or 64 hexadecimal digits (`LoadCredential=` or
//!   `SetCredential=`);
//! - `<data>/master.key`, 32 raw bytes, created at the first start.
//!
//! Supplying both of the first two is refused: which one sealed the stored
//! credentials would be a guess. A file is never followed if it is a
//! symlink, and `master.key` is refused if anyone but its user can read or
//! write it. Without a key, credentials are unrecoverable, so a
//! `master.key` that is missing while credentials are stored is an error,
//! never a new key (plan 8a decision 6).
//!
//! The key is held in zeroizing memory and never shown: its `Debug` names
//! only its version.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// The key file, in the collector's data directory.
pub const KEY_FILE: &str = "master.key";

/// The environment variable that supplies the key, as 64 hexadecimal digits.
/// The host strips it from what its children inherit
/// (`hennery_host::adapter::HOST_SECRET_VARS`).
pub const KEY_ENV: &str = "HENNERY_MASTER_KEY";

/// The systemd credential's name, in `$CREDENTIALS_DIRECTORY`.
pub const CREDENTIAL_NAME: &str = "hennery-master-key";

/// The version every credential is sealed under until `rotate-key`
/// (plan 8g) adds others.
pub const KEY_VERSION: u32 = 1;

const KEY_LEN: usize = 32;

/// The way out when the key is gone for good (plan 8a decision 8): the
/// collector does not start until the key is back, or the credentials only
/// it opens are given up. Each is then set again.
pub const GIVE_UP: &str = "To give the stored gateway credentials up instead, stop the collector and run \
     `sqlite3 <data>/hennery.db 'DELETE FROM gw_credentials; DELETE FROM gw_oauth_clients'`, then set each \
     connection's credential (and OAuth client) again.";

/// The master key, wiped from memory when dropped.
pub struct MasterKey {
    version: u32,
    bytes: Zeroizing<[u8; KEY_LEN]>,
}

impl MasterKey {
    /// A key of `KEY_VERSION` from its bytes.
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self {
            version: KEY_VERSION,
            bytes: Zeroizing::new(bytes),
        }
    }

    /// The version every blob this key seals carries.
    pub fn version(&self) -> u32 {
        self.version
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKey")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// Where a key may come from (see the module's documentation).
pub struct KeySource {
    /// The collector's data directory, which holds `master.key`.
    pub data_dir: PathBuf,
    /// `HENNERY_MASTER_KEY`, if set.
    pub env: Option<Zeroizing<String>>,
    /// `$CREDENTIALS_DIRECTORY`, if set.
    pub credentials_dir: Option<PathBuf>,
}

impl KeySource {
    /// The sources the collector's own environment names.
    pub fn from_env(data_dir: &Path) -> Result<Self> {
        Self::from_vars(
            data_dir,
            std::env::var_os(KEY_ENV),
            std::env::var_os("CREDENTIALS_DIRECTORY"),
        )
    }

    /// The sources `HENNERY_MASTER_KEY` and `$CREDENTIALS_DIRECTORY` name,
    /// as the environment holds them. A key that is set but not text is an
    /// error, not an unset variable: the operator chose it (the review's
    /// R3).
    pub fn from_vars(
        data_dir: &Path,
        env: Option<std::ffi::OsString>,
        credentials_dir: Option<std::ffi::OsString>,
    ) -> Result<Self> {
        let env = match env.map(std::ffi::OsString::into_string) {
            None => None,
            Some(Ok(text)) => Some(Zeroizing::new(text)),
            Some(Err(raw)) => {
                drop(Zeroizing::new(raw.into_encoded_bytes()));
                bail!("{KEY_ENV}: the master key must be 64 hexadecimal digits");
            }
        };
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            env,
            credentials_dir: credentials_dir.map(PathBuf::from),
        })
    }
}

impl std::fmt::Debug for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeySource")
            .field("data_dir", &self.data_dir)
            .field("env", &self.env.as_ref().map(|_| "<set>"))
            .field("credentials_dir", &self.credentials_dir)
            .finish()
    }
}

/// Where the key in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrigin {
    /// Made now, as `<data>/master.key`.
    Created,
    /// `<data>/master.key`, as an earlier start made it.
    File,
    Environment,
    Credential,
}

/// The key from the first source that has one, or a new `master.key` when
/// none has and `ciphertext_exists` is false.
pub fn load_or_create(source: &KeySource, ciphertext_exists: bool) -> Result<(MasterKey, KeyOrigin)> {
    // Only a credential that is not there is absent: one that cannot be
    // looked at is an error, never a reason to make a key file instead.
    let credential = match source.credentials_dir.as_ref().map(|dir| dir.join(CREDENTIAL_NAME)) {
        None => None,
        Some(path) => match path.symlink_metadata() {
            Ok(_) => Some(path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err).with_context(|| format!("look for the credential {}", path.display())),
        },
    };
    let file = source.data_dir.join(KEY_FILE);
    let supplied = match (&source.env, &credential) {
        (Some(_), Some(_)) => {
            bail!("both {KEY_ENV} and the systemd credential {CREDENTIAL_NAME} supply a master key: keep one")
        }
        (Some(hex), None) => Some((from_hex(hex).context(KEY_ENV)?, KeyOrigin::Environment)),
        (None, Some(path)) => Some((read_credential(path)?, KeyOrigin::Credential)),
        (None, None) => None,
    };
    if let Some((key, origin)) = supplied {
        if file.symlink_metadata().is_ok() {
            tracing::warn!(file = %file.display(), "a master key is supplied, so this key file is not used");
        }
        return Ok((key, origin));
    }
    match file.symlink_metadata() {
        Ok(_) => Ok((read_key_file(&file)?, KeyOrigin::File)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if ciphertext_exists {
                bail!(
                    "{} is missing, but gateway credentials are stored that only it can open: restore it from a \
                     backup, or supply it through {KEY_ENV} or the systemd credential {CREDENTIAL_NAME}. {GIVE_UP}",
                    file.display()
                );
            }
            if source.credentials_dir.is_some() {
                tracing::warn!(
                    credential = CREDENTIAL_NAME,
                    "$CREDENTIALS_DIRECTORY is set but holds no master key: making a key file instead"
                );
            }
            Ok((create_key_file(&file)?, KeyOrigin::Created))
        }
        Err(err) => Err(err).with_context(|| format!("read {}", file.display())),
    }
}

/// 64 hexadecimal digits, in any case. The message never quotes the input.
fn from_hex(text: &str) -> Result<MasterKey> {
    let mut bytes = Zeroizing::new([0u8; KEY_LEN]);
    if hex::decode_to_slice(text.trim_end_matches('\n'), bytes.as_mut_slice()).is_err() {
        bail!("the master key must be 64 hexadecimal digits");
    }
    Ok(MasterKey {
        version: KEY_VERSION,
        bytes,
    })
}

/// The file at `path`, never through a symlink, with its metadata from
/// the descriptor itself.
fn open_no_follow(path: &Path) -> Result<(std::fs::File, std::fs::Metadata)> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("open {} (a symlink is refused)", path.display()))?;
    let meta = file.metadata().with_context(|| format!("stat {}", path.display()))?;
    if !meta.file_type().is_file() {
        bail!("{} is not a regular file", path.display());
    }
    Ok((file, meta))
}

/// At most `max` bytes of `file`, in zeroizing memory.
fn read_at_most(file: std::fs::File, path: &Path, max: usize) -> Result<Zeroizing<Vec<u8>>> {
    let mut content = Zeroizing::new(Vec::with_capacity(max + 1));
    file.take(max as u64 + 1)
        .read_to_end(&mut content)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(content)
}

fn read_key_file(path: &Path) -> Result<MasterKey> {
    let (file, meta) = open_no_follow(path)?;
    if meta.mode() & 0o077 != 0 {
        bail!(
            "{} can be read or changed by other users: `chmod 600` it (and consider whether the key leaked)",
            path.display()
        );
    }
    // A second name for the file is a second way to read or replace it
    // that its mode does not show.
    if meta.nlink() != 1 {
        bail!("{} has other hard links: keep one name for the key", path.display());
    }
    let content = read_at_most(file, path, KEY_LEN)?;
    from_raw(&content)
        .ok_or_else(|| anyhow::anyhow!("{} must hold exactly 32 bytes, not {}", path.display(), content.len()))
}

/// Exactly 32 bytes, copied straight into zeroizing memory.
fn from_raw(content: &[u8]) -> Option<MasterKey> {
    if content.len() != KEY_LEN {
        return None;
    }
    let mut bytes = Zeroizing::new([0u8; KEY_LEN]);
    bytes.copy_from_slice(content);
    Some(MasterKey {
        version: KEY_VERSION,
        bytes,
    })
}

/// A systemd credential: systemd owns the file (often root, with an ACL
/// for the service's user), so only access by every other user is refused.
fn read_credential(path: &Path) -> Result<MasterKey> {
    let (file, meta) = open_no_follow(path)?;
    if meta.mode() & 0o007 != 0 {
        bail!("the credential {} can be read by other users", path.display());
    }
    let content = read_at_most(file, path, 2 * KEY_LEN + 1)?;
    if let Some(key) = from_raw(&content) {
        return Ok(key);
    }
    match std::str::from_utf8(&content) {
        Ok(text) if text.trim_end_matches('\n').len() == 2 * KEY_LEN => {
            from_hex(text).with_context(|| path.display().to_string())
        }
        _ => bail!(
            "the credential {} must hold 32 bytes, or 64 hexadecimal digits",
            path.display()
        ),
    }
}

/// A new key at `path`: created exclusively, never through a symlink, and
/// private (0600) whatever the umask. A write cut short leaves a file of
/// the wrong length, which the next start refuses rather than replaces.
fn create_key_file(path: &Path) -> Result<MasterKey> {
    let key = MasterKey {
        version: KEY_VERSION,
        bytes: Zeroizing::new(hennery_kernel::secret::random_bytes::<KEY_LEN>()),
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(key.bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("write {}", path.display()))?;
    // The directory's entry too: without it a crash can lose the file while
    // the database keeps credentials sealed under the key.
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)
            .and_then(|dir| dir.sync_all())
            .with_context(|| format!("sync {}", dir.display()))?;
    }
    tracing::info!(file = %path.display(), "created the gateway's master key: back it up with hennery.db");
    Ok(key)
}
