//! Descriptors `hennery up` hands its children (kernel spec §4.2): the
//! pairing code travels over one pipe from the collector child to the host
//! child, each end inherited as a file descriptor, so the code is never on a
//! command line or in the environment; the collector child inherits the
//! listening sockets `up` bound for it; and each child inherits the reading
//! end of a pipe whose end-of-file says `up` is gone.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};

/// The descriptor number each child finds its end of the pairing pipe at.
pub const CHILD_FD: RawFd = 3;

/// The descriptor number the collector child finds its first listening
/// socket at; the others follow it, in order.
pub const LISTENER_FD: RawFd = 4;

/// The descriptor number each child finds the reading end of `up`'s parent
/// pipe at: past the pairing pipe's and every listening socket's.
pub const PARENT_FD: RawFd = LISTENER_FD + crate::MAX_LISTENERS as RawFd;

/// The most descriptors one child is handed: the pairing pipe's end, a
/// listening socket for each of up to `MAX_LISTENERS` addresses, and the
/// parent pipe's reading end.
pub const MAX_PASSED: usize = 2 + crate::MAX_LISTENERS;

const _: () = assert!(PARENT_FD < MOVE_FLOOR);

/// How long a child may take to stop once `up` is gone, before it exits
/// outright: past the host's bound on stopping its adapters (5 s and one
/// more), within systemd's `TimeoutStopSec` (30 s). With `up` gone nothing
/// else would end a shutdown that hangs.
pub const PARENT_GONE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

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

/// Check the descriptor `flag` names before it is taken: it must be open
/// and a pipe (or FIFO). Anything else is refused, not taken: a closed
/// descriptor would abort the process when its `File` is dropped, a socket
/// would hang the host's read, and a file would be written to or read from
/// in place of the supervisor's pipe. Only inspected, never wrapped (so
/// never closed) here.
pub fn check_pipe(flag: &str, fd: RawFd) -> Result<()> {
    // SAFETY: fcntl(2) on a descriptor number; it only reads its flags.
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        bail!(
            "{flag} {fd} is not an open descriptor: {}",
            std::io::Error::last_os_error()
        );
    }
    // SAFETY: fstat(2) into local storage; all-zero bytes are a valid `stat`.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } < 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("{flag} {fd}: fstat"));
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFIFO {
        bail!("{flag} {fd} is not a pipe");
    }
    Ok(())
}

/// The collector's side: write the code and close the descriptor.
pub fn write_code(fd: RawFd, code: &str) -> Result<()> {
    // SAFETY: `fd` was inherited for exactly this and nothing else owns it.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    writeln!(file, "{code}").context("write the pairing code to the supervisor's pipe")?;
    Ok(())
}

/// The most `read_code` reads: a code and its newline need far less.
const MAX_CODE_BYTES: usize = 256;

/// The host's side: read the code the collector wrote. Blocks until the
/// collector has written it, or has exited without doing so. More than
/// `MAX_CODE_BYTES` is refused, not buffered.
pub async fn read_code(fd: RawFd) -> Result<String> {
    // SAFETY: `fd` was inherited for exactly this and nothing else owns it.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let text = tokio::task::spawn_blocking(move || {
        let mut text = String::new();
        file.take(MAX_CODE_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map(|_| text)
    })
    .await?
    .context("read the pairing code from the supervisor's pipe")?;
    if text.len() > MAX_CODE_BYTES {
        bail!("the pairing code on the supervisor's pipe is longer than {MAX_CODE_BYTES} bytes");
    }
    let code = text.trim().to_string();
    if code.is_empty() {
        bail!("the collector exited without handing over a pairing code");
    }
    Ok(code)
}

/// Watch `--parent-fd`, the reading end of a pipe whose only writing end
/// `hennery up` holds: the receiver resolves once `up` is gone, however it
/// died, as the kernel then closes that end. The descriptor is checked as
/// the other inherited pipes are, and made close-on-exec at once, so no
/// agent inherits it.
///
/// A thread of its own blocks in `read`, detached, so it never holds the
/// process's exit back: not `spawn_blocking`, whose tasks the runtime waits
/// for as it shuts down, and not a non-blocking read, whose `O_NONBLOCK`
/// would be set on the pipe both children share. Once `up` is gone, the same
/// thread ends the process outright if it is still there after
/// `PARENT_GONE_DEADLINE`. That covers the runtime's drop too: should it
/// block that long, `_exit` skips `Adapter`'s `Drop`, knowingly trading an
/// orphaned adapter for a process that ends (A-1). A host still pairing
/// when `up` dies stops by the pairing pipe's end-of-file, or by this
/// deadline, without the "gone" line.
pub fn watch_parent(fd: RawFd) -> Result<tokio::sync::oneshot::Receiver<()>> {
    check_pipe("--parent-fd", fd)?;
    // SAFETY: fcntl(2) on the descriptor just checked; it only sets its
    // flags.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(std::io::Error::last_os_error()).context("make --parent-fd close-on-exec");
        }
    }
    // SAFETY: `fd` was inherited for exactly this and nothing else owns it.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let (gone, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("parent-watch".into())
        .spawn(move || {
            let mut buf = [0u8; 64];
            // `up` never writes; anything read is ignored. End-of-file, or
            // any error but an interruption, means it is gone.
            loop {
                match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = gone.send(());
            std::thread::sleep(PARENT_GONE_DEADLINE);
            // One `write`, as `log::say` does: the other child shares the
            // descriptor.
            let _ = std::io::stderr().write_all(
                format!(
                    "hennery: still running {} s after `hennery up` is gone; exiting\n",
                    PARENT_GONE_DEADLINE.as_secs()
                )
                .as_bytes(),
            );
            // SAFETY: _exit(2) ends the process at once, running nothing
            // else: no other thread's state is touched.
            unsafe { libc::_exit(1) }
        })
        .context("start the thread that watches --parent-fd")?;
    Ok(receiver)
}

/// Resolves once `up` is gone (`watch_parent`), and says so; never without
/// a parent pipe (a child started by hand).
pub async fn parent_gone(watch: Option<tokio::sync::oneshot::Receiver<()>>) {
    match watch {
        // A watcher that stopped without a word is as good as `up` gone.
        Some(watch) => {
            let _ = watch.await;
            tracing::warn!("hennery up is gone; stopping");
        }
        None => std::future::pending().await,
    }
}

/// Close an inherited descriptor that is not needed after all.
pub fn close(fd: RawFd) {
    // SAFETY: as above; dropping the `File` closes it.
    drop(unsafe { std::fs::File::from_raw_fd(fd) });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::IntoRawFd;

    /// The host reads at most `MAX_CODE_BYTES` from the pipe: a writer
    /// that sends more is refused, not buffered without end.
    #[tokio::test]
    async fn a_code_longer_than_the_bound_is_refused() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let feed = std::thread::spawn(move || {
            // Stops at EPIPE once the reader is done.
            let _ = writer.write_all(&vec![b'A'; 1 << 20]);
        });
        let err = read_code(reader.into_raw_fd()).await.unwrap_err().to_string();
        assert!(err.contains("longer than"), "{err}");
        feed.join().unwrap();

        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"ABCD-EFGH\n").unwrap();
        drop(writer);
        assert_eq!(read_code(reader.into_raw_fd()).await.unwrap(), "ABCD-EFGH");
    }
}
