//! Descriptors `hennery up` hands its children (kernel spec §4.2): the
//! pairing code travels over one pipe from the collector child to the host
//! child, each end inherited as a file descriptor, so the code is never on a
//! command line or in the environment; and the collector child inherits the
//! listening sockets `up` bound for it.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};

/// The descriptor number each child finds its end of the pairing pipe at.
pub const CHILD_FD: RawFd = 3;

/// The descriptor number the collector child finds its first listening
/// socket at; the others follow it, in order.
pub const LISTENER_FD: RawFd = 4;

/// The most descriptors one child is handed: the pairing pipe's end and a
/// listening socket for each of up to `MAX_LISTENERS` addresses.
pub const MAX_PASSED: usize = 1 + crate::MAX_LISTENERS;

/// Each source is first copied to a number at or above this one, clear of
/// every target.
const MOVE_FLOOR: RawFd = 64;

/// Make each `(fd, child_fd)`'s `fd` (one of this process's descriptors) the
/// child's `child_fd`, open across `exec`. The sources are close-on-exec, so
/// the child inherits nothing else of them.
pub fn pass_to_child(cmd: &mut tokio::process::Command, fds: &[(RawFd, RawFd)]) {
    // Checked here, not in the child: nothing may panic after the fork.
    assert!(fds.len() <= MAX_PASSED && fds.iter().all(|&(_, to)| (3..MOVE_FLOOR).contains(&to)));
    let fds = fds.to_vec();
    // SAFETY: the closure runs in the forked child before `exec` and calls
    // only async-signal-safe functions (`fcntl`, `dup2`, `close`), and
    // allocates nothing: `fds` was built before the fork.
    unsafe {
        cmd.pre_exec(move || {
            // A source may sit at another pair's target (the listener at 3,
            // say): moving every source above the targets first means no
            // `dup2` below closes a descriptor still to be passed. The copies
            // are close-on-exec, so none is left open in the program.
            let mut moved = [0; MAX_PASSED];
            for (slot, &(fd, _)) in moved.iter_mut().zip(&fds) {
                *slot = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, MOVE_FLOOR);
                if *slot < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            // `dup2` leaves the new descriptor open across `exec`.
            for (&from, &(_, to)) in moved.iter().zip(&fds) {
                if libc::dup2(from, to) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(from);
            }
            Ok(())
        });
    }
}

/// The collector's side: write the code and close the descriptor.
pub fn write_code(fd: RawFd, code: &str) -> Result<()> {
    // SAFETY: `fd` was inherited for exactly this and nothing else owns it.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    writeln!(file, "{code}").context("write the pairing code to the supervisor's pipe")?;
    Ok(())
}

/// The host's side: read the code the collector wrote. Blocks until the
/// collector has written it, or has exited without doing so.
pub async fn read_code(fd: RawFd) -> Result<String> {
    // SAFETY: `fd` was inherited for exactly this and nothing else owns it.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let text = tokio::task::spawn_blocking(move || {
        let mut text = String::new();
        file.read_to_string(&mut text).map(|_| text)
    })
    .await?
    .context("read the pairing code from the supervisor's pipe")?;
    let code = text.trim().to_string();
    if code.is_empty() {
        bail!("the collector exited without handing over a pairing code");
    }
    Ok(code)
}

/// Close an inherited descriptor that is not needed after all.
pub fn close(fd: RawFd) {
    // SAFETY: as above; dropping the `File` closes it.
    drop(unsafe { std::fs::File::from_raw_fd(fd) });
}
