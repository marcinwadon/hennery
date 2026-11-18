//! Bounded descriptor-relative file reads; no symlink traversal.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path};

pub const MAX_FILE: usize = 16 << 20;
pub fn forget(root: &Path, session: &str) -> Result<()> {
    match std::fs::symlink_metadata(root) {
        Ok(meta) => ensure!(meta.file_type().is_dir(), "invalid_cache_root"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    }
    let path = root.join(hex::encode(Sha256::digest(session.as_bytes())));
    // remove_dir_all does not follow symlinks, including the final component.
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\'))
}
fn open_at(parent: &File, name: &str, directory: bool) -> Result<File> {
    let name = CString::new(name)?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: owned NUL-terminated name, live parent descriptor.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    ensure!(fd >= 0, "file_unavailable");
    // SAFETY: openat returned a new, owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub fn read(cwd: &str, path: &str) -> Result<(String, Vec<u8>)> {
    ensure!(
        cwd.starts_with('/') && cwd.len() <= 4096 && path.len() <= 4096,
        "invalid_path"
    );
    let root = Path::new(cwd);
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).context("outside_session")?
    } else {
        path
    };
    let parts: Vec<_> = relative
        .components()
        .filter(|part| *part != Component::CurDir)
        .collect();
    ensure!(!parts.is_empty() && parts.len() <= 128, "invalid_path");
    let mut dir = File::open("/")?;
    for part in root.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(n) => dir = open_at(&dir, n.to_str().context("invalid_path")?, true)?,
            _ => anyhow::bail!("invalid_path"),
        }
    }
    for (i, part) in parts.iter().enumerate() {
        let Component::Normal(n) = part else {
            anyhow::bail!("invalid_path")
        };
        let name = n.to_str().context("invalid_path")?;
        ensure!(valid_name(name), "invalid_path");
        let mut next = open_at(&dir, name, i + 1 != parts.len())?;
        if i + 1 == parts.len() {
            let meta = next.metadata()?;
            ensure!(
                meta.is_file() && meta.len() <= MAX_FILE as u64,
                "file_too_large_or_not_regular"
            );
            let mut bytes = Vec::new();
            (&mut next).take(MAX_FILE as u64 + 1).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= MAX_FILE, "file_too_large");
            return Ok((name.to_owned(), bytes));
        }
        dir = next;
    }
    anyhow::bail!("invalid_path")
}
pub fn stage(root: &Path, session: &str, name: &str, data: &str) -> Result<String> {
    ensure!(
        valid_name(name) && data.len() <= (5_usize << 20).div_ceil(3) * 4,
        "invalid_file"
    );
    let bytes = STANDARD.decode(data)?;
    ensure!(bytes.len() <= 5 << 20, "file_too_large");
    let session = hex::encode(Sha256::digest(session.as_bytes()));
    let digest = hex::encode(Sha256::digest(&bytes));
    let dir = root.join(session).join(digest);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let path = dir.join(name);
    let temporary = dir.join(format!(".{}.tmp", uuid::Uuid::now_v7()));
    if !path.try_exists()? {
        let result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            File::open(&dir)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)?;
            ensure!(file.metadata()?.is_file(), "invalid_file");
            let mut old = Vec::new();
            file.take(bytes.len() as u64 + 1).read_to_end(&mut old)?;
            ensure!(old == bytes, "invalid_file");
        }
        Err(e) => return Err(e.into()),
    }
    Ok(path.to_str().context("invalid_path")?.to_owned())
}
