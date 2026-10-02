//! Removing an agent's entries through directory descriptors (plan 9d B3,
//! R1, R2): every step is relative to a directory already open, opened
//! with `O_NOFOLLOW`, so no path component is looked up twice and no
//! symlink, however it is swapped in, is ever followed.
//!
//! - A directory is opened with `openat(O_RDONLY|O_DIRECTORY|O_NOFOLLOW|
//!   O_CLOEXEC|O_NONBLOCK)`; `ELOOP` or `ENOTDIR` means it is no directory,
//!   and it is unlinked as an entry (`unlinkat(…, 0)`), a symlink included:
//!   its target is never touched. The entry type `readdir` reports is
//!   never trusted: every entry is tried as a directory first.
//! - Entries are read through `fdopendir` on a `dup` of the directory's
//!   descriptor; a directory goes with `unlinkat(AT_REMOVEDIR)` after its
//!   contents.
//! - `EINTR` is retried; `ENOENT` means gone already.
//! - The walk is iterative, at most `MAX_DEPTH` directories deep, one
//!   descriptor per level; it stops at another file system (`st_dev`).

use std::ffi::{CStr, CString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The deepest a removal descends (R2): one open descriptor per level.
pub const MAX_DEPTH: usize = 32;

/// Why a removal stopped short (R2, B3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// A directory on another file system (a mount point).
    MountPoint,
    /// Deeper than `MAX_DEPTH`.
    TooDeep,
    /// A system call failed with this errno.
    Io(i32),
    /// The forget's deadline passed (B6; the review's item 3).
    Deadline,
    /// The named entry was a directory when it was looked at and is not
    /// one by the time it is opened (a symlink swapped in): left alone
    /// (the review's item 8).
    Swapped,
}

/// Seams for tests (the host crate's `test-hooks` feature): nothing in a
/// real build.
#[derive(Clone, Default)]
pub struct Hooks {
    /// Called after each directory of a removal has been listed, before any
    /// of its entries is acted on, with its path (for the test to find it;
    /// the walk itself never uses a path) and its entries.
    #[cfg(feature = "test-hooks")]
    #[allow(clippy::type_complexity)]
    pub listed: Option<std::sync::Arc<dyn Fn(&Path, &[std::ffi::OsString]) + Send + Sync>>,
    /// Called once a named entry has been found to be a directory, before
    /// it is opened, with its path.
    #[cfg(feature = "test-hooks")]
    pub stated: Option<PathHook>,
    /// Asked before a project directory is listed, with its path: `true`
    /// makes the listing fail (`EIO`).
    #[cfg(feature = "test-hooks")]
    pub fail_listing: Option<PathTest>,
}

/// A test hook given a path (`Hooks::stated`).
pub type PathHook = std::sync::Arc<dyn Fn(&Path) + Send + Sync>;

/// A test hook asked about a path (`Hooks::fail_listing`).
pub type PathTest = std::sync::Arc<dyn Fn(&Path) -> bool + Send + Sync>;

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hooks")
    }
}

impl Hooks {
    pub(crate) fn listed(&self, _path: &Path, _entries: &[CString]) {
        #[cfg(feature = "test-hooks")]
        if let Some(hook) = &self.listed {
            use std::os::unix::ffi::OsStringExt;
            let names: Vec<std::ffi::OsString> = _entries
                .iter()
                .map(|n| std::ffi::OsString::from_vec(n.as_bytes().to_vec()))
                .collect();
            hook(_path, &names);
        }
    }

    pub(crate) fn stated(&self, _path: &Path) {
        #[cfg(feature = "test-hooks")]
        if let Some(hook) = &self.stated {
            hook(_path);
        }
    }

    /// `list`, unless a test makes the listing of `_path` fail.
    pub fn list(&self, dir: RawFd, _path: &Path) -> Result<Vec<CString>, i32> {
        #[cfg(feature = "test-hooks")]
        if let Some(hook) = &self.fail_listing
            && hook(_path)
        {
            return Err(libc::EIO);
        }
        list(dir)
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO)
}

/// Clear `errno`, so a `NULL` from `readdir` can be told from an error.
fn clear_errno() {
    // SAFETY: the calling thread's own errno slot.
    unsafe {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            *libc::__error() = 0;
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            *libc::__errno_location() = 0;
        }
    }
}

/// `name` as a C string: no name read from a directory or built from a
/// checked id holds a NUL.
pub fn c_name(name: &[u8]) -> CString {
    CString::new(name).expect("a file name holds no NUL")
}

const DIR_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;

/// Open `path` as a directory, never through a symlink at its last
/// component: the agent's root, which the caller has checked is canonical.
pub fn open_root(path: &Path) -> Result<OwnedFd, i32> {
    let path = c_name(path.as_os_str().as_bytes());
    loop {
        // SAFETY: open(2) on a NUL-terminated path.
        let fd = unsafe { libc::open(path.as_ptr(), DIR_FLAGS) };
        if fd >= 0 {
            // SAFETY: a descriptor this call just opened, owned from here.
            return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        match errno() {
            libc::EINTR => continue,
            e => return Err(e),
        }
    }
}

/// Open the directory `name` in `dir`, never following a symlink (R1):
/// `ELOOP` or `ENOTDIR` if it is no directory.
pub fn open_dir_at(dir: RawFd, name: &CStr) -> Result<OwnedFd, i32> {
    loop {
        // SAFETY: openat(2) relative to an open directory descriptor.
        let fd = unsafe { libc::openat(dir, name.as_ptr(), DIR_FLAGS) };
        if fd >= 0 {
            // SAFETY: as in `open_root`.
            return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        match errno() {
            libc::EINTR => continue,
            e => return Err(e),
        }
    }
}

/// `fstat` of an open descriptor.
pub fn stat_fd(fd: RawFd) -> Result<libc::stat, i32> {
    // SAFETY: an all-zero `stat` is a valid value for the call to fill.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat(2) into a local struct.
    if unsafe { libc::fstat(fd, &mut st) } == 0 {
        Ok(st)
    } else {
        Err(errno())
    }
}

/// `fstatat(AT_SYMLINK_NOFOLLOW)` of `name` in `dir`; `None` if it is not
/// there.
pub fn stat_at(dir: RawFd, name: &CStr) -> Result<Option<libc::stat>, i32> {
    loop {
        // SAFETY: as in `stat_fd`.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: fstatat(2) relative to an open directory, into a local.
        if unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0 {
            return Ok(Some(st));
        }
        match errno() {
            libc::EINTR => continue,
            libc::ENOENT => return Ok(None),
            e => return Err(e),
        }
    }
}

pub fn is_link(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFLNK
}

pub fn is_dir(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFDIR
}

/// Unlink `name` in `dir`; gone already is fine.
fn unlink_at(dir: RawFd, name: &CStr, flags: libc::c_int) -> Result<(), i32> {
    loop {
        // SAFETY: unlinkat(2) relative to an open directory descriptor.
        if unsafe { libc::unlinkat(dir, name.as_ptr(), flags) } == 0 {
            return Ok(());
        }
        match errno() {
            libc::EINTR => continue,
            libc::ENOENT => return Ok(()),
            e => return Err(e),
        }
    }
}

/// Unlink the entry `name` in `dir`, never a directory (`unlinkat(…, 0)`):
/// a regular file the caller found with `stat_at`, or, if one was swapped
/// in since, the symlink itself, never its target. Gone already is fine.
pub fn unlink_file_at(dir: RawFd, name: &CStr) -> Result<(), i32> {
    unlink_at(dir, name, 0)
}

pub fn is_file(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFREG
}

/// The entries of the open directory `dir`, `.` and `..` left out, read
/// through `fdopendir` on a `dup` of it (R1), which is closed after.
pub fn list(dir: RawFd) -> Result<Vec<CString>, i32> {
    // SAFETY: dup(2) of an open descriptor; fdopendir(3) takes the copy,
    // and closedir(3) closes it.
    let copy = unsafe { libc::fcntl(dir, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        return Err(errno());
    }
    // SAFETY: as above.
    let stream = unsafe { libc::fdopendir(copy) };
    if stream.is_null() {
        let e = errno();
        // SAFETY: the copy is still ours when fdopendir failed.
        unsafe { libc::close(copy) };
        return Err(e);
    }
    // The stream reads from the copy's offset, which it shares with `dir`:
    // start from the beginning whatever was read before.
    // SAFETY: rewinddir(3) on the stream just opened.
    unsafe { libc::rewinddir(stream) };
    let mut out = Vec::new();
    let result = loop {
        clear_errno();
        // SAFETY: readdir(3) on a live stream; the entry is copied out
        // before the next call.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            match errno() {
                0 => break Ok(()),
                libc::EINTR => continue,
                e => break Err(e),
            }
        }
        // SAFETY: `d_name` is NUL-terminated within the entry (its length
        // differs by platform: 256 bytes on Linux, 1024 on macOS).
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let bytes = name.to_bytes();
        if bytes != b"." && bytes != b".." {
            out.push(name.to_owned());
        }
    };
    // SAFETY: closes the stream and the copy it owns.
    unsafe { libc::closedir(stream) };
    result.map(|()| out)
}

/// One directory being removed.
struct Frame {
    fd: OwnedFd,
    /// Its name in its parent.
    name: CString,
    entries: Vec<CString>,
    next: usize,
    /// For the hooks only.
    path: PathBuf,
}

/// Remove the directory `name` in `parent` and everything in it (R1, R2),
/// on the file system `dev`, before `until`. Below the top, an entry that
/// turns out not to be a directory (a symlink swapped in included) is
/// unlinked as an entry; the top itself is left (`Stop::Swapped`).
pub fn remove_tree(
    parent: RawFd,
    name: &CStr,
    dev: libc::dev_t,
    path: &Path,
    hooks: &Hooks,
    until: std::time::Instant,
) -> Result<(), Stop> {
    if std::time::Instant::now() >= until {
        return Err(Stop::Deadline);
    }
    let mut stack: Vec<Frame> = Vec::new();
    match descend(parent, name, dev, 1, path, hooks)? {
        Some(frame) => stack.push(frame),
        None => return Ok(()),
    }
    while let Some(top) = stack.last_mut() {
        if std::time::Instant::now() >= until {
            return Err(Stop::Deadline);
        }
        if top.next < top.entries.len() {
            let child = top.entries[top.next].clone();
            top.next += 1;
            let child_path = top.path.join(std::ffi::OsStr::from_bytes(child.as_bytes()));
            let fd = top.fd.as_raw_fd();
            let depth = stack.len() + 1;
            if let Some(frame) = descend(fd, &child, dev, depth, &child_path, hooks)? {
                stack.push(frame);
            }
        } else {
            let done = stack.pop().expect("a frame on the stack");
            let parent_fd = stack.last().map_or(parent, |f| f.fd.as_raw_fd());
            drop(done.fd);
            unlink_at(parent_fd, &done.name, libc::AT_REMOVEDIR).map_err(Stop::Io)?;
        }
    }
    Ok(())
}

/// Open `name` in `dir`, `depth` levels down (1: the named entry itself),
/// to remove what it holds: its frame, listed, if it is a directory on
/// `dev`; `None` once it is unlinked (no directory) or gone. Its depth is
/// judged on the descriptor it opened, so a directory swapped in after
/// its parent was listed counts too (the review's item 2).
fn descend(
    dir: RawFd,
    name: &CStr,
    dev: libc::dev_t,
    depth: usize,
    path: &Path,
    hooks: &Hooks,
) -> Result<Option<Frame>, Stop> {
    let fd = match open_dir_at(dir, name) {
        Ok(fd) => fd,
        // The named entry is no directory any more: never unlinked here.
        Err(libc::ELOOP | libc::ENOTDIR) if depth == 1 => return Err(Stop::Swapped),
        Err(libc::ELOOP | libc::ENOTDIR) => {
            unlink_at(dir, name, 0).map_err(Stop::Io)?;
            return Ok(None);
        }
        Err(libc::ENOENT) => return Ok(None),
        Err(e) => return Err(Stop::Io(e)),
    };
    if depth > MAX_DEPTH {
        return Err(Stop::TooDeep);
    }
    let st = stat_fd(fd.as_raw_fd()).map_err(Stop::Io)?;
    if st.st_dev != dev {
        return Err(Stop::MountPoint);
    }
    let entries = list(fd.as_raw_fd()).map_err(Stop::Io)?;
    hooks.listed(path, &entries);
    Ok(Some(Frame {
        fd,
        name: name.to_owned(),
        entries,
        next: 0,
        path: path.to_path_buf(),
    }))
}

/// What became of one named entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// It was not there.
    Absent,
    Removed,
    /// A symlink: reported, never followed or removed (decision 8).
    Symlink,
    Stopped(Stop),
}

/// Remove the entry `name` in `dir`, which the forget names exactly (B9),
/// before `until`: a symlink is left and reported, one swapped in for a
/// directory too; a directory goes with its contents; any other entry is
/// unlinked.
pub fn remove_entry(
    dir: RawFd,
    name: &CStr,
    dev: libc::dev_t,
    path: &Path,
    hooks: &Hooks,
    until: std::time::Instant,
) -> Removal {
    match stat_at(dir, name) {
        Ok(None) => Removal::Absent,
        Ok(Some(st)) if is_link(&st) => Removal::Symlink,
        Ok(Some(st)) if is_dir(&st) => {
            hooks.stated(path);
            match remove_tree(dir, name, dev, path, hooks, until) {
                Ok(()) => Removal::Removed,
                Err(Stop::Swapped) => match stat_at(dir, name) {
                    Ok(Some(st)) if is_link(&st) => Removal::Symlink,
                    Ok(None) => Removal::Absent,
                    _ => Removal::Stopped(Stop::Io(libc::EAGAIN)),
                },
                Err(stop) => Removal::Stopped(stop),
            }
        }
        Ok(Some(_)) => match unlink_at(dir, name, 0) {
            Ok(()) => Removal::Removed,
            Err(e) => Removal::Stopped(Stop::Io(e)),
        },
        Err(e) => Removal::Stopped(Stop::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (tempfile::TempDir, OwnedFd, libc::dev_t) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("t/a/b")).unwrap();
        std::fs::write(dir.path().join("t/a/b/f"), "x").unwrap();
        let fd = open_root(dir.path()).unwrap();
        let dev = stat_fd(fd.as_raw_fd()).unwrap().st_dev;
        (dir, fd, dev)
    }

    fn later() -> std::time::Instant {
        std::time::Instant::now() + std::time::Duration::from_secs(60)
    }

    /// R2, the review's item 7: a directory on another device stops the
    /// removal, and nothing of it goes.
    #[test]
    fn another_file_system_stops_the_removal() {
        let (dir, fd, dev) = tree();
        let t = c_name(b"t");
        assert_eq!(
            remove_tree(fd.as_raw_fd(), &t, dev ^ 1, Path::new("t"), &Hooks::default(), later()),
            Err(Stop::MountPoint)
        );
        assert!(dir.path().join("t/a/b/f").exists());
        assert_eq!(
            remove_tree(fd.as_raw_fd(), &t, dev, Path::new("t"), &Hooks::default(), later()),
            Ok(())
        );
        assert!(!dir.path().join("t").exists());
    }

    /// B6, the review's item 3: past its deadline, a removal stops.
    #[test]
    fn a_removal_past_its_deadline_stops() {
        let (dir, fd, dev) = tree();
        let past = std::time::Instant::now();
        assert_eq!(
            remove_tree(
                fd.as_raw_fd(),
                &c_name(b"t"),
                dev,
                Path::new("t"),
                &Hooks::default(),
                past
            ),
            Err(Stop::Deadline)
        );
        assert!(dir.path().join("t/a/b/f").exists());
    }
}
