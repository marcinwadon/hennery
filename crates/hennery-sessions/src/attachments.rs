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

/// The temporary file `write` writes `sha256` to before its rename:
/// `.<sha256>.<16 hex digits>.tmp`, hidden, and unique per write.
fn temp_name(sha256: &str, random: [u8; 8]) -> String {
    format!(".{sha256}.{}.tmp", hex::encode(random))
}

/// `name` is a temporary file as `write` names one, which a crash may
/// leave (plan 9b): the only other name the sweep removes.
pub fn is_temp(name: &str) -> bool {
    let is_hex = |s: &str| s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    name.strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .and_then(|rest| rest.split_once('.'))
        .is_some_and(|(sha256, random)| is_sha256(sha256) && random.len() == 16 && is_hex(random))
}

/// Refresh the mtime of the file stored as `sha256` in `dir`, if there is
/// one (plan 9b): whether there was. The sweep removes a file no row names
/// only once its mtime is older than its grace.
///
/// Only a regular file is refreshed: anything else under that name is
/// not one, and opening it could follow a link out of the directory or
/// block on a FIFO.
pub fn refresh(dir: &Path, sha256: &str) -> std::io::Result<bool> {
    let path = path(dir, sha256)?;
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Ok(false),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    }
    match std::fs::File::open(path) {
        Ok(file) => file.set_modified(std::time::SystemTime::now()).map(|()| true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
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
    let temp = dir.join(temp_name(sha256, hennery_kernel::secret::random_bytes::<8>()));
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

/// Remove the file stored as `sha256` in `dir`; one already gone is fine.
pub fn remove(dir: &Path, sha256: &str) -> std::io::Result<()> {
    match std::fs::remove_file(path(dir, sha256)?) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
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

    /// Plan 9b: the sweep removes a leftover temporary file by its name,
    /// so the name `write` makes is the only one it matches.
    #[test]
    fn only_the_names_write_makes_are_temporary_files() {
        let random = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let name = temp_name(SHA, random);
        assert_eq!(name, format!(".{SHA}.0123456789abcdef.tmp"));
        assert!(is_temp(&name));
        for other in [
            SHA.to_string(),
            name[1..].to_string(),
            format!("{name}.bak"),
            format!(".{SHA}.tmp"),
            format!(".{SHA}.0123456789abcde.tmp"),
            format!(".{SHA}.0123456789abcdef0.tmp"),
            format!(".{SHA}.0123456789ABCDEF.tmp"),
            format!(".{}.0123456789abcdef.tmp", SHA.to_uppercase()),
            format!(".{}.0123456789abcdef.tmp", &SHA[1..]),
            format!(".{SHA}.0123456789abcdef.tmq"),
            format!(".{SHA}/0123456789abcdef.tmp"),
        ] {
            assert!(!is_temp(&other), "{other}");
        }
    }

    /// Plan 9b: `refresh` touches a regular file only. It never follows a
    /// link out of the directory, and never opens a FIFO, which would
    /// block it until a writer came.
    #[test]
    fn only_a_regular_file_is_refreshed() {
        let data = tempfile::tempdir().unwrap();
        let dir = data.path().join(DIR);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        assert!(!refresh(&dir, SHA).unwrap(), "a missing file was refreshed");
        write(&dir, SHA, b"test").unwrap();
        std::fs::File::open(dir.join(SHA)).unwrap().set_modified(old).unwrap();
        assert!(refresh(&dir, SHA).unwrap());
        assert!(std::fs::metadata(dir.join(SHA)).unwrap().modified().unwrap() > old);

        let outside = data.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        std::fs::File::open(&outside).unwrap().set_modified(old).unwrap();
        let link = "0".repeat(64);
        std::os::unix::fs::symlink(&outside, dir.join(&link)).unwrap();
        assert!(!refresh(&dir, &link).unwrap(), "a symlink was refreshed");
        assert_eq!(std::fs::metadata(&outside).unwrap().modified().unwrap(), old);

        let directory = "1".repeat(64);
        std::fs::create_dir(dir.join(&directory)).unwrap();
        assert!(!refresh(&dir, &directory).unwrap(), "a directory was refreshed");

        let fifo = "2".repeat(64);
        let made = std::process::Command::new("mkfifo")
            .arg(dir.join(&fifo))
            .status()
            .unwrap();
        assert!(made.success());
        let (sent, received) = std::sync::mpsc::channel();
        let (in_dir, name) = (dir.clone(), fifo.clone());
        std::thread::spawn(move || sent.send(refresh(&in_dir, &name).map_err(|e| e.to_string())));
        let refreshed = received.recv_timeout(std::time::Duration::from_secs(20));
        if refreshed.is_err() {
            // Release the thread blocked in `open`.
            let _ = std::fs::OpenOptions::new().write(true).open(dir.join(&fifo));
        }
        assert_eq!(refreshed.expect("refresh blocked on a FIFO"), Ok(false));
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
