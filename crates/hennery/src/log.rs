//! hennery's own log (distribution spec §6.2, §8). Run by a service, `up`,
//! the collector and `host run` each write a size-capped rotating file of
//! their own in the logs directory, and the service manager's capture of
//! standard output and error gets crash output only. Anywhere else (a
//! terminal, a container) they log to standard output, as every other
//! command always does.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

/// Names the directory to log to, in place of the service default. Only the
/// directory itself is checked (the user's, writable by nobody else): one
/// inside a directory others can write to could be swapped between
/// rotations, so give a private path.
pub const LOG_DIR_VAR: &str = "HENNERY_LOG_DIR";

/// Set by the launchd agent and the systemd unit `hennery service install`
/// writes (`launchd`, `systemd`): a process run by a service logs to its
/// file.
pub const SERVICE_VAR: &str = "HENNERY_SERVICE";

/// Distribution spec §8: "Logs rotate at 10 MiB × 5 files": the file
/// being written and four rotated ones.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const FILES: usize = 5;

/// Where a long-running process logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Stdout,
    Dir(PathBuf),
    /// Run by a service, but with no directory to log to (no absolute
    /// HOME): standard error, and why.
    Unavailable(String),
}

/// Where to log, from the environment (`var`) on a macOS host or not:
/// `HENNERY_LOG_DIR` if set and not empty; else, under a service
/// (`HENNERY_SERVICE`), §8's directory for the platform; else standard
/// output. A `HENNERY_LOG_DIR` that is not absolute is a configuration
/// error, refused.
pub fn target(var: impl Fn(&str) -> Option<OsString>, macos: bool) -> Result<Target, String> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    let absolute = |name: &str| set(name).map(PathBuf::from).filter(|p| p.is_absolute());
    if let Some(dir) = set(LOG_DIR_VAR) {
        let dir = PathBuf::from(dir);
        if !dir.is_absolute() {
            return Err(format!("{LOG_DIR_VAR} is not an absolute path: {}", dir.display()));
        }
        return Ok(Target::Dir(dir));
    }
    if set(SERVICE_VAR).is_none() {
        return Ok(Target::Stdout);
    }
    let Some(home) = absolute("HOME") else {
        return Ok(Target::Unavailable("HOME is not an absolute path".into()));
    };
    Ok(Target::Dir(if macos {
        home.join("Library/Logs/hennery")
    } else {
        absolute("XDG_STATE_HOME")
            .unwrap_or_else(|| home.join(".local/state"))
            .join("hennery/log")
    }))
}

/// Set up logging for `process` (`up`, `collector`, `host`), or for a
/// command that always logs to standard output (`None`). `RUST_LOG`
/// filters, `info` by default; the targets that trace whole messages stay
/// capped at their own level whatever it says, since they would show a
/// session's gateway token (`hennery_host::logging::capped`, plan 8c).
///
/// A file that cannot be used (the directory not creatable, not private,
/// the disk full) falls back to standard error, saying why there, at `warn`
/// unless `RUST_LOG` says otherwise: under a service, that is the crash log,
/// which must not grow with every line. Only a configuration error
/// (`HENNERY_LOG_DIR` relative) is returned, to stop the start.
pub fn init(process: Option<&str>) -> Result<(), String> {
    let filter = |default: &str| {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default))
    };
    let why = match process.map(|_| target(|name| std::env::var_os(name), cfg!(target_os = "macos"))) {
        None | Some(Ok(Target::Stdout)) => {
            install(tracing_subscriber::fmt().with_env_filter(filter("info")).finish());
            return Ok(());
        }
        Some(Err(why)) => return Err(why),
        Some(Ok(Target::Unavailable(why))) => why,
        Some(Ok(Target::Dir(dir))) => {
            let name = format!("hennery-{}.log", process.unwrap_or_default());
            match Rotating::open(&dir, &name, MAX_BYTES, FILES) {
                Ok(log) => {
                    record_panics(log.clone());
                    // One line where the service manager keeps output, so
                    // its log points at this one.
                    say(&format!("logging to {}", dir.join(&name).display()));
                    install(
                        tracing_subscriber::fmt()
                            .with_env_filter(filter("info"))
                            .with_ansi(false)
                            // A failed write is said once by the writer, not
                            // once per event on standard error.
                            .log_internal_errors(false)
                            .with_writer(log)
                            .finish(),
                    );
                    return Ok(());
                }
                Err(err) => format!("cannot write the log in {}: {err}", dir.display()),
            }
        }
    };
    say(&format!("{why}; logging warnings and errors to standard error"));
    // Colour only on a terminal: under a service this is the crash log.
    install(
        tracing_subscriber::fmt()
            .with_env_filter(filter("warn"))
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
            .with_writer(std::io::stderr)
            .finish(),
    );
    Ok(())
}

/// Install `subscriber` as the process's, `log` records bridged in, with
/// the targets that trace whole messages capped (plan 8c): the one place
/// this file installs one (`every_subscriber_is_installed_capped`).
fn install<S>(subscriber: S)
where
    S: tracing::Subscriber + Send + Sync + 'static,
{
    use tracing_subscriber::util::SubscriberInitExt;
    hennery_host::logging::capped(subscriber).init();
}

/// `line` on standard error, prefixed, in one `write`: `up` and its
/// children share it, and `eprintln!` writes a line in pieces, between
/// which another process's line can land (seen under load).
fn say(line: &str) {
    let _ = std::io::stderr().write_all(format!("hennery: {line}\n").as_bytes());
}

/// Write each panic to `log` too, then to standard error as before. Never
/// waits for the file: a panic while it is being written (on this thread
/// or another) leaves it out rather than deadlock.
fn record_panics(log: Rotating) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record_panic(&log, &info.to_string());
        previous(info);
    }));
}

/// `text` as one line of `log`, if it is free now.
fn record_panic(log: &Rotating, text: &str) {
    let inner = match log.0.try_lock() {
        Ok(inner) => Some(inner),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    };
    if let Some(mut inner) = inner {
        let _ = inner.write_all(format!("PANIC {text}\n").as_bytes());
    }
}

/// A log file that never grows past `max` bytes (but for one longer line):
/// before a write would take it past, it becomes `<name>.1`, `.1` becomes
/// `.2`, and so on; `<name>.<files - 1>` is dropped. One process writes it:
/// tracing formats each event whole and writes it with one `write_all`, so
/// a rotation falls between lines. A clone writes the same file.
#[derive(Clone)]
pub struct Rotating(Arc<Mutex<Inner>>);

struct Inner {
    path: PathBuf,
    file: std::fs::File,
    /// The file's length, counted from its length at open.
    len: u64,
    max: u64,
    files: usize,
    /// The first failure, of a write or a rotation, was said on standard
    /// error; no later one is, of either kind: under a service that is the
    /// crash log, which must not grow with them.
    complained: bool,
}

impl Rotating {
    /// Open `dir/name` to append to, making `dir` (0700) if it is missing.
    /// `dir` must be this user's and writable by nobody else; the file must
    /// be a regular file of this user's, with no other link to it.
    pub fn open(dir: &Path, name: &str, max: u64, files: usize) -> std::io::Result<Self> {
        assert!(files >= 1 && max > 0);
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let meta = std::fs::metadata(dir)?;
        if meta.uid() != euid() || meta.mode() & 0o022 != 0 {
            return Err(std::io::Error::other(format!(
                "{} is not this user's own directory, or others can write to it",
                dir.display()
            )));
        }
        let path = dir.join(name);
        let file = open_file(&path)?;
        let len = file.metadata()?.len();
        Ok(Self(Arc::new(Mutex::new(Inner {
            path,
            file,
            len,
            max,
            files,
            complained: false,
        }))))
    }
}

fn euid() -> u32 {
    // SAFETY: geteuid(2) cannot fail.
    unsafe { libc::geteuid() }
}

/// `path` for appending, private, never through a symbolic link, and only
/// if it is a regular file of this user's that nothing else links to: one
/// planted in a shared directory is refused, not written to. `O_NONBLOCK`
/// makes a FIFO planted there fail the open rather than block it; it does
/// nothing to a regular file.
fn open_file(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != euid() || meta.nlink() != 1 {
        return Err(std::io::Error::other(format!(
            "{} is not a regular file of this user's alone",
            path.display()
        )));
    }
    // One left by an older run may be broader: it is ours, so it is made
    // private again.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// `path` with `.n` appended.
fn numbered(path: &Path, n: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

/// `buf` with every line break but a final one escaped (`\n`, `\r`), so
/// text from an agent (an error's message, say) cannot forge a line. A
/// backslash is left as it is, so a message holding the two characters `\n`
/// reads the same as an escaped break: ambiguous, never a second line.
fn one_line(buf: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let body = buf.strip_suffix(b"\n").unwrap_or(buf);
    if !body.iter().any(|&b| b == b'\n' || b == b'\r') {
        return buf.into();
    }
    let mut out = Vec::with_capacity(buf.len() + 8);
    for &b in body {
        match b {
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b => out.push(b),
        }
    }
    if body.len() < buf.len() {
        out.push(b'\n');
    }
    out.into()
}

impl Inner {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let buf = one_line(buf);
        if self.len > 0 && self.len.saturating_add(buf.len() as u64) > self.max {
            self.rotate();
        }
        let written = self.file.write_all(&buf);
        // Counted also when it failed: part of it may be in the file.
        self.len = self.len.saturating_add(buf.len() as u64);
        if let Err(err) = &written {
            self.complain(&format!("cannot write {}: {err}", self.path.display()));
        }
        written
    }

    fn rotate(&mut self) {
        // Whether the file being written was renamed to `.1`: should the new
        // one then fail to open, it is moved back, so the log keeps its name
        // and the rotated files keep what they hold.
        let mut renamed = false;
        let opened = (|| -> std::io::Result<std::fs::File> {
            if self.files == 1 {
                std::fs::remove_file(&self.path)?;
            } else {
                for n in (1..self.files - 1).rev() {
                    match std::fs::rename(numbered(&self.path, n), numbered(&self.path, n + 1)) {
                        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
                        _ => {}
                    }
                }
                std::fs::rename(&self.path, numbered(&self.path, 1))?;
                renamed = true;
            }
            open_file(&self.path)
        })();
        match opened {
            Ok(file) => self.file = file,
            Err(err) => {
                if renamed {
                    let _ = std::fs::rename(numbered(&self.path, 1), &self.path);
                }
                // The cap holds whatever happens: the file being written
                // starts again empty.
                let _ = self.file.set_len(0);
                self.complain(&format!(
                    "cannot rotate {}: {err}; it is emptied instead",
                    self.path.display()
                ));
            }
        }
        self.len = 0;
    }

    /// Say `what` on standard error, the first time only: under a service
    /// that is the crash log.
    fn complain(&mut self, what: &str) {
        if !self.complained {
            self.complained = true;
            say(what);
        }
    }
}

/// The writer one event is written with: the file, locked.
pub struct Writer<'a>(MutexGuard<'a, Inner>);

impl Write for Writer<'_> {
    /// Writes all of `buf` at once, so a rotation never splits it.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Rotating {
    type Writer = Writer<'a>;

    fn make_writer(&'a self) -> Writer<'a> {
        Writer(self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[cfg(test)]
mod tests {
    /// Plan 8c: every subscriber is installed through `install`, which
    /// caps the targets that trace whole messages. Read from the source:
    /// an installed subscriber cannot be inspected.
    #[test]
    fn every_subscriber_is_installed_capped() {
        let source = include_str!("log.rs");
        let init = concat!(".", "init()");
        assert_eq!(
            source.matches(init).count(),
            1,
            "a subscriber installed outside `install`"
        );
        assert!(source.contains(concat!("hennery_host::logging::capped(subscriber)", ".", "init();")));
    }

    use super::*;
    use std::collections::BTreeMap;
    use tracing_subscriber::fmt::MakeWriter;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: BTreeMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), v.into())).collect();
        move |name| map.get(name).cloned()
    }

    /// Distribution spec §8's directories, under a service only; the
    /// override first; an empty one is unset, a relative one refused.
    #[test]
    fn the_target_follows_the_service_and_the_override() {
        let dir = |p: &str| Ok(Target::Dir(PathBuf::from(p)));
        assert_eq!(target(env(&[("HOME", "/h")]), true), Ok(Target::Stdout));
        assert_eq!(
            target(env(&[("HOME", "/h"), (SERVICE_VAR, "")]), false),
            Ok(Target::Stdout)
        );
        let service = [("HOME", "/h"), (SERVICE_VAR, "launchd")];
        assert_eq!(target(env(&service), true), dir("/h/Library/Logs/hennery"));
        assert_eq!(target(env(&service), false), dir("/h/.local/state/hennery/log"));
        let state = [("HOME", "/h"), (SERVICE_VAR, "systemd"), ("XDG_STATE_HOME", "/s")];
        assert_eq!(target(env(&state), false), dir("/s/hennery/log"));
        let relative_state = [("HOME", "/h"), (SERVICE_VAR, "systemd"), ("XDG_STATE_HOME", "s")];
        assert_eq!(target(env(&relative_state), false), dir("/h/.local/state/hennery/log"));
        for no_home in [
            &[(SERVICE_VAR, "systemd"), ("HOME", "h")][..],
            &[(SERVICE_VAR, "launchd")],
        ] {
            assert!(matches!(target(env(no_home), true), Ok(Target::Unavailable(_))));
        }
        let custom = [("HOME", "/h"), (SERVICE_VAR, "systemd"), (LOG_DIR_VAR, "/l")];
        assert_eq!(target(env(&custom), false), dir("/l"));
        assert_eq!(target(env(&[(LOG_DIR_VAR, "/l")]), true), dir("/l"));
        assert_eq!(target(env(&[(LOG_DIR_VAR, "")]), true), Ok(Target::Stdout));
        assert!(target(env(&[(LOG_DIR_VAR, "l")]), true).is_err());
    }

    fn write(log: &Rotating, line: &str) {
        log.make_writer().write_all(line.as_bytes()).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// Lines move down `.1` … `.4` as the file fills; the oldest are dropped;
    /// no file passes the cap; every file is private.
    #[test]
    fn the_log_rotates_at_its_cap_and_keeps_five_files() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let log = Rotating::open(&logs, "x.log", 100, 5).unwrap();
        // 40 bytes each: two fit in a file, a third rotates it.
        let line = |n: usize| format!("{n:039}\n");
        for n in 0..12 {
            write(&log, &line(n));
        }
        let path = logs.join("x.log");
        assert_eq!(read(&path), line(10) + &line(11));
        assert_eq!(read(&numbered(&path, 1)), line(8) + &line(9));
        assert_eq!(read(&numbered(&path, 4)), line(2) + &line(3));
        let mut names: Vec<_> = std::fs::read_dir(&logs)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["x.log", "x.log.1", "x.log.2", "x.log.3", "x.log.4"]);
        for name in &names {
            let meta = std::fs::metadata(logs.join(name)).unwrap();
            assert!(meta.len() <= 100, "{name}");
            assert_eq!(meta.permissions().mode() & 0o777, 0o600, "{name}");
        }
        assert_eq!(std::fs::metadata(&logs).unwrap().permissions().mode() & 0o777, 0o700);
    }

    /// A restart counts what the file holds already, and a line longer than
    /// the cap is written whole, alone in its file.
    #[test]
    fn a_reopened_log_counts_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.log");
        std::fs::write(&path, "a".repeat(90)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let log = Rotating::open(dir.path(), "x.log", 100, 3).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        write(&log, &"b".repeat(20));
        assert_eq!(read(&numbered(&path, 1)), "a".repeat(90));
        assert_eq!(read(&path), "b".repeat(20));
        write(&log, &"c".repeat(150));
        assert_eq!(read(&path), "c".repeat(150));
        assert_eq!(read(&numbered(&path, 2)), "a".repeat(90));
    }

    /// Neither a symbolic link nor a hard link planted at the log's name is
    /// written through, and a directory others can write to is refused.
    #[test]
    fn planted_links_and_a_shared_directory_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, "kept").unwrap();
        let soft = dir.path().join("soft");
        std::fs::create_dir(&soft).unwrap();
        std::os::unix::fs::symlink(&elsewhere, soft.join("x.log")).unwrap();
        assert!(Rotating::open(&soft, "x.log", 100, 5).is_err());
        let hard = dir.path().join("hard");
        std::fs::create_dir(&hard).unwrap();
        std::fs::hard_link(&elsewhere, hard.join("x.log")).unwrap();
        assert!(Rotating::open(&hard, "x.log", 100, 5).is_err());
        assert_eq!(read(&elsewhere), "kept");
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Rotating::open(&shared, "x.log", 100, 5).is_err());
        assert!(!shared.join("x.log").exists());
    }

    /// A rotation that cannot rename (the directory made read-only) empties
    /// the file instead, so the cap still holds.
    #[test]
    fn a_failed_rotation_still_keeps_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let log = Rotating::open(&logs, "x.log", 100, 5).unwrap();
        write(&log, &"a".repeat(80));
        std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o500)).unwrap();
        write(&log, &"b".repeat(40));
        std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(read(&logs.join("x.log")), "b".repeat(40));
        assert!(!numbered(&logs.join("x.log"), 1).exists());
    }

    /// A line break inside an event cannot start a forged line; the final
    /// one stays.
    #[test]
    fn line_breaks_inside_an_event_are_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let log = Rotating::open(dir.path(), "x.log", 1000, 5).unwrap();
        write(&log, "WARN error=boom\nINFO forged\r\n");
        write(&log, "INFO plain\n");
        assert_eq!(
            read(&dir.path().join("x.log")),
            "WARN error=boom\\nINFO forged\\r\nINFO plain\n"
        );
    }

    /// A panic is written to the file when it is free, and left out, not
    /// waited for, when it is being written: by this very thread, say.
    #[test]
    fn a_panic_is_recorded_without_waiting_for_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = Rotating::open(dir.path(), "x.log", 1000, 5).unwrap();
        let held = log.make_writer();
        record_panic(&log, "while held");
        drop(held);
        record_panic(&log, "at line 7");
        assert_eq!(read(&dir.path().join("x.log")), "PANIC at line 7\n");
    }
}
