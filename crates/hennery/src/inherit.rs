//! Handing the all-in-one host its pairing code (kernel spec §4.2): one pipe
//! from the collector child to the host child, each end inherited as a
//! file descriptor, so the code is never on a command line or in the
//! environment.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

/// The descriptor number each child finds its end of the pipe at.
pub const CHILD_FD: RawFd = 3;

/// Make `fd` (one of this process's descriptors) the child's `CHILD_FD`,
/// open across `exec`. The pipe's descriptors are close-on-exec, so the
/// child inherits nothing else of the pipe.
pub fn pass_to_child(cmd: &mut tokio::process::Command, fd: &impl AsRawFd) {
    let fd = fd.as_raw_fd();
    // SAFETY: the closure runs in the forked child before `exec` and calls
    // only async-signal-safe functions (`dup2`, `fcntl`).
    unsafe {
        cmd.pre_exec(move || {
            if fd == CHILD_FD {
                // `dup2` onto itself would keep close-on-exec set.
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(fd, CHILD_FD) < 0 {
                return Err(std::io::Error::last_os_error());
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
