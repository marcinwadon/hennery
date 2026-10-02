//! Extraction of package and Node archives (distribution spec §3.2), with
//! nothing taken on trust from the archive: only regular files and
//! directories, only plain relative paths, never through a link, modes
//! normalised, sizes bounded.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Entries one package may hold.
pub const MAX_ENTRIES: usize = 100_000;

/// Slack over a package's `unpacked_size` before extraction stops.
pub const SIZE_ALLOWANCE: u64 = 1 << 20;

/// Extract an npm package tarball (gzip) into `dest`, which must not
/// exist yet, stripping each entry's first component (npm's `package/`).
/// At most `max_bytes` of file contents are written.
pub fn package(tgz: &Path, dest: &Path, max_bytes: u64) -> Result<()> {
    let file = std::fs::File::open(tgz).with_context(|| format!("open {}", tgz.display()))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(file)));
    if std::fs::symlink_metadata(dest).is_ok() {
        bail!("{} exists already: two packages at one path", dest.display());
    }
    create_dirs(dest)?;
    let mut written = 0u64;
    let mut dirs = vec![dest.to_path_buf()];
    for (count, entry) in archive.entries()?.enumerate() {
        if count >= MAX_ENTRIES {
            bail!("more than {MAX_ENTRIES} entries");
        }
        let mut entry = entry.context("read the archive")?;
        let kind = entry.header().entry_type();
        // A pax global header carries only metadata; per-file pax and GNU
        // long-name records are applied by the reader to the entry after.
        if kind.is_pax_global_extensions() {
            continue;
        }
        let raw = entry.path_bytes().into_owned();
        let name = std::str::from_utf8(&raw).map_err(|_| anyhow::anyhow!("an entry name is not UTF-8"))?;
        let Some(relative) = strip_first(name)? else {
            // The root directory (`package/`) itself.
            continue;
        };
        let target = dest.join(relative);
        if kind.is_dir() {
            create_dirs_below(dest, &target)?;
            dirs.push(target);
        } else if kind.is_file() {
            let parent = target.parent().expect("a joined path has a parent");
            create_dirs_below(dest, parent)?;
            let executable = entry.header().mode()? & 0o111 != 0;
            let budget = max_bytes.saturating_sub(written);
            written +=
                write_file(&mut entry, &target, executable, budget).with_context(|| format!("extract {relative}"))?;
        } else {
            bail!("{name:?} is not a regular file or a directory ({kind:?}); links and special files are refused");
        }
    }
    for dir in dirs {
        sync_dir(&dir)?;
    }
    Ok(())
}

/// Extract only `node-v<version>-<platform>/bin/node` from a Node archive
/// (gzip) to `dest`, a file that must not exist yet: a regular file of
/// exactly `size` bytes, mode 0755.
pub fn node_binary(tgz: &Path, version: &str, platform: &str, dest: &Path, size: u64) -> Result<()> {
    let wanted = format!("node-v{version}-{platform}/bin/node");
    let file = std::fs::File::open(tgz).with_context(|| format!("open {}", tgz.display()))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(file)));
    for entry in archive.entries()? {
        let mut entry = entry.context("read the Node archive")?;
        if entry.path_bytes().as_ref() != wanted.as_bytes() {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            bail!("{wanted} is not a regular file");
        }
        if entry.size() != size {
            bail!("{wanted} is {} bytes, the manifest says {size}", entry.size());
        }
        create_dirs(dest.parent().expect("bin/node has a parent"))?;
        let written = write_file(&mut entry, dest, true, size)?;
        if written != size {
            bail!("{wanted}: {written} bytes written, {size} expected");
        }
        return Ok(());
    }
    bail!("the Node archive has no {wanted}")
}

/// The path below the archive's root, or `None` for the root itself. Every
/// component must be plain: no `/` at the start, no `.`, `..` or empty one.
fn strip_first(name: &str) -> Result<Option<&str>> {
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    let (first, rest) = match trimmed.split_once('/') {
        Some((first, rest)) => (first, Some(rest)),
        None => (trimmed, None),
    };
    if first.is_empty() || first == "." || first == ".." {
        bail!("{name:?} is not a plain relative path");
    }
    match rest {
        None => Ok(None),
        Some(rest) => {
            super::manifest::check_relative_path(rest).with_context(|| format!("entry {name:?}"))?;
            Ok(Some(rest))
        }
    }
}

/// Write one file, refusing to follow or replace anything already there.
fn write_file(entry: &mut impl Read, target: &Path, executable: bool, budget: u64) -> Result<u64> {
    let mode = if executable { 0o755 } else { 0o644 };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(mode)
        .open(target)
        .with_context(|| format!("create {}", target.display()))?;
    // One byte past the budget is enough to know it was overrun.
    let copied = std::io::copy(&mut entry.take(budget.saturating_add(1)), &mut file)?;
    if copied > budget {
        bail!("the package holds more bytes than the manifest says");
    }
    // The umask must not decide: exactly 0644 or 0755.
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    file.flush()?;
    sync(&file)?;
    Ok(copied)
}

/// `create_dir_all`, for a directory this code owns entirely.
fn create_dirs(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))
}

/// Create `dir` and every missing directory between `root` and it, never
/// through a link: each component that exists must be a real directory.
fn create_dirs_below(root: &Path, dir: &Path) -> Result<()> {
    let relative = dir
        .strip_prefix(root)
        .context("a directory outside the extraction root")?;
    let mut current = PathBuf::from(root);
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => bail!("{} exists and is not a directory", current.display()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current).with_context(|| format!("create {}", current.display()))?;
                std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o755))?;
            }
            Err(err) => return Err(err).with_context(|| format!("inspect {}", current.display())),
        }
    }
    Ok(())
}

/// Fsync `root` and every directory below it (files are synced as they are
/// written).
pub fn sync_tree(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        }
    }
    sync_dir(root)
}

/// Fsync a directory, so the entries just made in it survive a crash.
pub fn sync_dir(dir: &Path) -> Result<()> {
    let file = std::fs::File::open(dir).with_context(|| format!("open {}", dir.display()))?;
    sync(&file).with_context(|| format!("fsync {}", dir.display()))
}

/// fsync(2). Not `File::sync_all`, which is `F_FULLFSYNC` on macOS: a
/// cache flush per file would make an install of a few thousand files take
/// minutes. On macOS plain fsync orders nothing against a later rename, so
/// every rename that publishes a set or a runtime is preceded by one
/// `barrier`.
pub fn sync(file: &std::fs::File) -> Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: fsync(2) on a descriptor the caller holds open.
    if unsafe { libc::fsync(file.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error()).context("fsync");
    }
    Ok(())
}

/// Make every write so far durable before what follows (the review's
/// amendment to decision 3): on macOS one `F_FULLFSYNC`, which flushes the
/// drive's cache and with it every earlier write; elsewhere fsync(2), which
/// flushes already.
pub fn barrier(path: &Path) -> Result<()> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    #[cfg(target_vendor = "apple")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: fcntl(2) F_FULLFSYNC on a descriptor this function holds.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| format!("F_FULLFSYNC {}", path.display()));
        }
        Ok(())
    }
    #[cfg(not(target_vendor = "apple"))]
    sync(&file).with_context(|| format!("fsync {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gzip tarball of raw entries: the name is written straight into the
    /// header, so names `tar::Builder` would refuse can be made.
    fn tarball(entries: &[(&str, tar::EntryType, u32, &[u8], &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, kind, mode, body, link) in entries {
            let mut header = tar::Header::new_gnu();
            header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_entry_type(*kind);
            header.set_mode(*mode);
            header.set_size(body.len() as u64);
            if !link.is_empty() {
                header.as_old_mut().linkname[..link.len()].copy_from_slice(link.as_bytes());
            }
            header.set_cksum();
            builder.append(&header, *body).unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    fn extract(entries: &[(&str, tar::EntryType, u32, &[u8], &str)], max: u64) -> (tempfile::TempDir, Result<()>) {
        let dir = tempfile::tempdir().unwrap();
        let tgz = dir.path().join("p.tgz");
        std::fs::write(&tgz, tarball(entries)).unwrap();
        let result = package(&tgz, &dir.path().join("out/node_modules/p"), max);
        (dir, result)
    }

    use tar::EntryType::{Block, Char, Directory, Fifo, Link, Regular, Symlink};

    #[test]
    fn a_package_lands_under_its_path_with_normalised_modes() {
        let (dir, result) = extract(
            &[
                ("package/", Directory, 0o777, b"", ""),
                ("package/package.json", Regular, 0o666, b"{}", ""),
                ("package/bin/tool", Regular, 0o4777, b"#!/bin/sh\n", ""),
                ("package/lib/a.js", Regular, 0o600, b"x", ""),
            ],
            1 << 20,
        );
        result.unwrap();
        let root = dir.path().join("out/node_modules/p");
        let mode = |p: &str| std::fs::metadata(root.join(p)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(std::fs::read(root.join("package.json")).unwrap(), b"{}");
        assert_eq!(mode("package.json"), 0o644);
        assert_eq!(mode("bin/tool"), 0o755, "the exec bit is kept, setuid is not");
        assert_eq!(mode("lib/a.js"), 0o644);
        assert_eq!(mode("lib"), 0o755);
    }

    #[test]
    fn traversal_and_absolute_names_are_refused() {
        for name in [
            "package/../../escape",
            "package/a/../../../escape",
            "/package/escape",
            "../escape",
            "package//escape",
            "package/./escape",
        ] {
            let (dir, result) = extract(&[(name, Regular, 0o644, b"x", "")], 1 << 20);
            assert!(result.is_err(), "{name:?} was extracted");
            assert!(!dir.path().join("escape").exists() && !dir.path().join("out/escape").exists());
        }
    }

    #[test]
    fn links_and_special_files_are_refused() {
        for (kind, link) in [
            (Symlink, "../../../etc"),
            (Symlink, "inside.js"),
            (Link, "package/a.js"),
            (Char, ""),
            (Block, ""),
            (Fifo, ""),
        ] {
            let (dir, result) = extract(
                &[
                    ("package/a.js", Regular, 0o644, b"x", ""),
                    ("package/l", kind, 0o777, b"", link),
                ],
                1 << 20,
            );
            let err = format!("{:#}", result.expect_err("a link or special file was extracted"));
            assert!(err.contains("refused"), "{kind:?}: {err}");
            assert!(std::fs::symlink_metadata(dir.path().join("out/node_modules/p/l")).is_err());
        }
    }

    #[test]
    fn a_file_twice_or_more_bytes_than_pinned_is_refused() {
        let (_dir, result) = extract(
            &[
                ("package/a", Regular, 0o644, b"x", ""),
                ("package/a", Regular, 0o644, b"y", ""),
            ],
            1 << 20,
        );
        assert!(result.is_err(), "a duplicate overwrote the first");
        let (_dir, result) = extract(&[("package/a", Regular, 0o644, &[0u8; 100], "")], 99);
        assert!(format!("{:#}", result.unwrap_err()).contains("more bytes"));
        let (_dir, result) = extract(&[("package/a", Regular, 0o644, &[0u8; 100], "")], 100);
        result.unwrap();
    }

    #[test]
    fn a_file_where_a_directory_is_needed_is_refused() {
        let (_dir, result) = extract(
            &[
                ("package/a", Regular, 0o644, b"x", ""),
                ("package/a/b", Regular, 0o644, b"y", ""),
            ],
            1 << 20,
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("not a directory"));
    }

    /// Decision 8's carve-out: a GNU long name and pax records (a global
    /// header, a per-file path) are metadata the reader applies, not
    /// entries to refuse.
    #[test]
    fn long_names_and_pax_records_are_read_not_refused() {
        let long = format!("package/{}/deep.js", "d".repeat(120));
        let mut builder = tar::Builder::new(Vec::new());
        let mut global = tar::Header::new_ustar();
        let record = b"18 comment=hello\n";
        global.set_entry_type(tar::EntryType::XGlobalHeader);
        global.set_size(record.len() as u64);
        global.set_path("pax_global_header").unwrap();
        global.set_cksum();
        builder.append(&global, &record[..]).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(4);
        header.set_entry_type(Regular);
        builder.append_data(&mut header, &long, &b"long"[..]).unwrap();
        builder
            .append_pax_extensions([("path", b"package/from-pax.js".as_slice())])
            .unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_path("package/short.js").unwrap();
        header.set_mode(0o644);
        header.set_size(3);
        header.set_entry_type(Regular);
        header.set_cksum();
        builder.append(&header, &b"pax"[..]).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&builder.into_inner().unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let tgz = dir.path().join("p.tgz");
        std::fs::write(&tgz, gz.finish().unwrap()).unwrap();
        let root = dir.path().join("out");
        package(&tgz, &root, 1 << 20).unwrap();
        assert_eq!(
            std::fs::read(root.join(long.strip_prefix("package/").unwrap())).unwrap(),
            b"long"
        );
        assert_eq!(std::fs::read(root.join("from-pax.js")).unwrap(), b"pax");
        assert!(!root.join("short.js").exists() && !root.join("pax_global_header").exists());
    }

    #[test]
    fn only_bin_node_is_taken_from_a_node_archive() {
        let dir = tempfile::tempdir().unwrap();
        let tgz = dir.path().join("node.tgz");
        let body = b"#!/bin/sh\necho v24.0.0\n";
        std::fs::write(
            &tgz,
            tarball(&[
                ("node-v24.0.0-linux-x64/README.md", Regular, 0o644, b"readme", ""),
                ("node-v24.0.0-linux-x64/bin/node", Regular, 0o755, body, ""),
                (
                    "node-v24.0.0-linux-x64/bin/npm",
                    Symlink,
                    0o777,
                    b"",
                    "../lib/npm-cli.js",
                ),
            ]),
        )
        .unwrap();
        let dest = dir.path().join("rt/bin/node");
        node_binary(&tgz, "24.0.0", "linux-x64", &dest, body.len() as u64).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_dir(dest.parent().unwrap()).unwrap().count(), 1);
        let other = dir.path().join("rt2/bin/node");
        let err = node_binary(&tgz, "24.0.0", "linux-x64", &other, 3)
            .unwrap_err()
            .to_string();
        assert!(err.contains("the manifest says 3"), "{err}");
        assert!(node_binary(&tgz, "24.0.0", "darwin-arm64", &other, 3).is_err());
    }
}
