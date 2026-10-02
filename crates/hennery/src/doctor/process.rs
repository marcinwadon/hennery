//! What doctor reads of other processes: a process's parent, whether it
//! still runs the binary on disk, and who holds an `flock`. On Linux all of
//! it comes from `/proc` under the context's root, so the tests give it a
//! made-up `/proc`; on macOS from `proc_pidinfo` and `proc_pidpath`, about
//! real processes only.

use crate::service::{Context, Platform};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// What macOS's `proc_pidinfo` says of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct BsdInfo {
    ppid: u32,
    /// Its start, as seconds and nanoseconds since the Unix epoch.
    start: (i64, i64),
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: u32) -> Option<BsdInfo> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: all-zero bytes are a valid `proc_bsdinfo`; proc_pidinfo(3)
    // writes at most the size it is given into it.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (n == size).then(|| BsdInfo {
        ppid: info.pbi_ppid,
        start: (info.pbi_start_tvsec as i64, info.pbi_start_tvusec as i64 * 1000),
    })
}

#[cfg(not(target_os = "macos"))]
fn bsd_info(_pid: u32) -> Option<BsdInfo> {
    None
}

#[cfg(target_os = "macos")]
fn bsd_path(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: proc_pidpath(3) writes at most `buf.len()` bytes into `buf`.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    let n = usize::try_from(n).ok().filter(|&n| n > 0)?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..n])))
}

#[cfg(not(target_os = "macos"))]
fn bsd_path(_pid: u32) -> Option<PathBuf> {
    None
}

/// The fields after the command name of `/proc/<pid>/stat`: the name may
/// hold spaces and parentheses, so the split is at the last `)`.
fn proc_stat(cx: &Context, pid: u32) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(cx.root.join(format!("proc/{pid}/stat"))).ok()?;
    let (_, rest) = text.rsplit_once(')')?;
    Some(rest.split_whitespace().map(str::to_string).collect())
}

/// The parent of process `pid`.
pub fn parent(cx: &Context, pid: u32) -> Option<u32> {
    match cx.platform {
        // The fields after the name: state, then ppid.
        Platform::Linux => proc_stat(cx, pid)?.get(1)?.parse().ok(),
        Platform::MacOs => bsd_info(pid).map(|info| info.ppid),
    }
}

/// Whether process `pid` still runs the file `exe` is now (decision 7):
/// `Some(false)` once it was replaced or removed, `None` when it cannot be
/// told. On Linux `/proc/<pid>/exe` names the very file the process runs,
/// so its device and inode decide. On macOS the process's path must be
/// `exe`'s, and `exe` must not have changed (its ctime) since the process
/// started.
pub fn runs(cx: &Context, pid: u32, exe: &Path) -> Option<bool> {
    let on_disk = std::fs::metadata(exe).ok()?;
    match cx.platform {
        Platform::Linux => {
            let link = cx.root.join(format!("proc/{pid}/exe"));
            if std::fs::read_link(&link)
                .ok()?
                .to_string_lossy()
                .ends_with(" (deleted)")
            {
                return Some(false);
            }
            let running = std::fs::metadata(&link).ok()?;
            Some(running.dev() == on_disk.dev() && running.ino() == on_disk.ino())
        }
        Platform::MacOs => {
            let path = bsd_path(pid)?;
            if path.canonicalize().ok()? != exe.canonicalize().ok()? {
                return Some(false);
            }
            let start = bsd_info(pid)?.start;
            Some((on_disk.ctime(), on_disk.ctime_nsec()) <= start)
        }
    }
}

/// Linux's device number as `/proc/locks` prints it: `major:minor`, hex.
pub fn linux_device(dev: u64) -> String {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    format!("{major:02x}:{minor:02x}")
}

/// Who holds an `flock` on `file`, from Linux's `/proc/locks`, which takes
/// no lock to read. Only a match is an answer: `None` when it cannot be
/// read (not Linux, or no `/proc`) and when no line names `file`, since
/// `/proc/locks` prints the superblock's device, which is not `stat`'s on
/// btrfs subvolumes or overlayfs (the review's N1).
pub fn flock_holder(cx: &Context, file: &Path) -> Option<u32> {
    if cx.platform != Platform::Linux {
        return None;
    }
    let meta = std::fs::metadata(file).ok()?;
    let locks = std::fs::read_to_string(cx.root.join("proc/locks")).ok()?;
    let id = format!("{}:{}", linux_device(meta.dev()), meta.ino());
    // `1: FLOCK  ADVISORY  WRITE 4321 08:01:1234567 0 EOF`; a waiter's line
    // has `->` after the number.
    locks.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        match fields.as_slice() {
            [_, "FLOCK", _, _, pid, file_id, ..] if *file_id == id => pid.parse().ok(),
            _ => None,
        }
    })
}
