//! Attachment files (ACP core §7, plan 6a): a prompt's images, stored once
//! each, as `<data>/attachments/<sha256>`.
//!
//! - **Content-addressed:** a file is named by the SHA-256 of its bytes, so
//!   the same image sent twice is one file, and a name says what it holds.
//! - **Written whole or not at all:** into a temporary file in the same
//!   directory, synced, then renamed over its name, and the directory
//!   synced. A crash leaves at most a `.tmp` file, never a short image
//!   under a real name.
//! - **Private:** the directory is 0700 and every file 0600, whatever the
//!   umask, like the database beside them.
//!
//! Which owner may read a file is the store's to say (`attachments` rows,
//! one per owner and image); this module only reads and writes bytes, and
//! only under names that are a SHA-256 in lowercase hex.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The directory's name in the data directory (kernel spec §1).
pub const DIR: &str = "attachments";

/// `name` is a SHA-256 in lowercase hex: the only names this module reads
/// or writes, so a name from a URL can never leave the directory.
pub fn is_sha256(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn path(dir: &Path, sha256: &str) -> std::io::Result<PathBuf> {
    if !is_sha256(sha256) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "an attachment is named by its SHA-256",
        ));
    }
    Ok(dir.join(sha256))
}

/// Store `bytes` as `sha256` in `dir`, creating `dir` (0700) if it does
/// not exist yet. A file of that name is already those bytes, so it is
/// kept as it is.
pub fn write(dir: &Path, sha256: &str, bytes: &[u8]) -> std::io::Result<()> {
    let target = path(dir, sha256)?;
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    if target.exists() {
        return Ok(());
    }
    let temp = dir.join(format!(
        ".{sha256}.{}.tmp",
        hex::encode(hennery_kernel::secret::random_bytes::<8>())
    ));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, &target)?;
        // The rename is durable only once the directory is (the review's
        // O3): else a power loss could leave a recorded image without its
        // file.
        std::fs::File::open(dir)?.sync_all()
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// The bytes stored as `sha256` in `dir`, if there are any.
pub fn read(dir: &Path, sha256: &str) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path(dir, sha256)?) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const SHA: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn only_lowercase_sha256_names_are_attachments() {
        assert!(is_sha256(SHA));
        for name in [
            "",
            &SHA[1..],
            &SHA.to_uppercase(),
            "../hennery.db",
            &format!("{}/x", &SHA[..62]),
            &format!("{}..", &SHA[..62]),
        ] {
            assert!(!is_sha256(name), "{name}");
            assert!(read(Path::new("/"), name).is_err(), "{name}");
            assert!(write(Path::new("/nonexistent"), name, b"x").is_err(), "{name}");
        }
    }

    #[test]
    fn a_file_is_written_whole_private_and_once() {
        let data = tempfile::tempdir().unwrap();
        let dir = data.path().join(DIR);
        write(&dir, SHA, b"test").unwrap();
        assert_eq!(read(&dir, SHA).unwrap().as_deref(), Some(&b"test"[..]));
        assert_eq!((mode(&dir), mode(&dir.join(SHA))), (0o700, 0o600));
        // Written again: the file is kept, and no temporary file is left.
        let before = std::fs::metadata(dir.join(SHA)).unwrap().modified().unwrap();
        write(&dir, SHA, b"test").unwrap();
        assert_eq!(std::fs::metadata(dir.join(SHA)).unwrap().modified().unwrap(), before);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [SHA]);
        assert_eq!(read(&dir, &"0".repeat(64)).unwrap(), None);
    }
}
