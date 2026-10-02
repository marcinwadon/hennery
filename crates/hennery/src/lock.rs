//! Single-instance locks (distribution spec §8): `host.lock` lets one
//! `hennery host run` serve a host data directory, and `up.lock` one
//! `hennery up` its data root. Two hosts on one directory would share one
//! key, one outbox and one host id; two `up`s would run two collectors on
//! one database.

use anyhow::{Context, Result, bail};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// `hennery host run`'s lock, in the host data directory.
pub const HOST_LOCK: &str = "host.lock";

/// `hennery up`'s lock, in its data root.
pub const UP_LOCK: &str = "up.lock";

/// Held for as long as the process runs; dropping it (or the process
/// ending, however it ends) releases the lock.
#[derive(Debug)]
pub struct Lock {
    _file: std::fs::File,
}

/// Take `dir/name`, which must not be held, and write this process's pid
/// into it; `command` names who holds it in the refusal. `dir` must exist.
/// The file is opened close-on-exec (as `std` opens every file), so no child
/// inherits it: an adapter orphaned by a host that was killed cannot hold
/// the lock against the host started again after it.
pub fn acquire(dir: &Path, name: &str, command: &str) -> Result<Lock> {
    let path = dir.join(name);
    // Never truncated on open: the pid in it is the holder's until the lock
    // is ours.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    // SAFETY: flock(2) on a descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            let mut holder = String::new();
            let _ = file.read_to_string(&mut holder);
            let holder = match holder.trim().parse::<u32>() {
                Ok(pid) => format!("pid {pid}"),
                Err(_) => "another process".to_string(),
            };
            bail!(
                "{} is held by {holder}: another `{command}` serves this data directory",
                path.display()
            );
        }
        return Err(err).with_context(|| format!("lock {}", path.display()));
    }
    file.set_len(0)
        .with_context(|| format!("truncate {}", path.display()))?;
    file.rewind()?;
    writeln!(file, "{}", std::process::id()).with_context(|| format!("write {}", path.display()))?;
    Ok(Lock { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two `flock`s on two opens of one file conflict, in one process too:
    /// the second is refused, names the holder's pid, and succeeds once the
    /// first is dropped.
    #[test]
    fn a_held_lock_refuses_a_second_holder_until_it_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path(), HOST_LOCK, "hennery host run").unwrap();
        let text = std::fs::read_to_string(dir.path().join(HOST_LOCK)).unwrap();
        assert_eq!(text.trim(), std::process::id().to_string());

        let err = acquire(dir.path(), HOST_LOCK, "hennery host run")
            .unwrap_err()
            .to_string();
        assert!(err.contains("host.lock"), "{err}");
        assert!(err.contains(&format!("pid {}", std::process::id())), "{err}");
        assert!(err.contains("another `hennery host run`"), "{err}");
        // The refused attempt did not wipe the holder's pid. (Both attempts
        // are this process's: this pins no truncation on open, not that the
        // pid is written only once the lock is held; the CLI's
        // `a_second_host_on_one_data_directory_refuses_to_start` pins that.)
        let text = std::fs::read_to_string(dir.path().join(HOST_LOCK)).unwrap();
        assert_eq!(text.trim(), std::process::id().to_string());

        // Free once dropped. A process another test forks meanwhile holds a
        // copy of the descriptor until it execs (close-on-exec closes it
        // then), so this may take a moment.
        drop(first);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while let Err(err) = acquire(dir.path(), HOST_LOCK, "hennery host run") {
            assert!(std::time::Instant::now() < deadline, "{err}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The lock's descriptor is close-on-exec, and its file private.
    #[test]
    fn the_lock_is_not_inherited_and_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let lock = acquire(dir.path(), UP_LOCK, "hennery up").unwrap();
        // SAFETY: fcntl(2) reads the flags of a descriptor `lock` owns.
        let flags = unsafe { libc::fcntl(lock._file.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0, "flags {flags}");
        let mode = std::fs::metadata(dir.path().join(UP_LOCK))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }
}
