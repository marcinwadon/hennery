//! What C library a Linux machine's glibc loader path leads to, and whether
//! the managed runtime can run on it (distribution spec §1.1). Shared by
//! the host, which refuses to install a set on a machine that cannot run
//! it, and by `hennery doctor`'s check 2, which says why.
//!
//! The binary is musl-static on Linux, so it cannot ask glibc its version:
//! it runs glibc's own loader with `--version`, and only a GNU C library's
//! banner counts; `/usr/bin/getconf` is asked only when the loader says
//! nothing, and never on NixOS, whose `getconf` is Nix's own glibc's. What
//! is read is gathered apart from how it is judged, so the judging is
//! tested on fixtures.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

/// The oldest glibc the managed Node and the Claude CLI run on.
pub const MIN_GLIBC: (u32, u32) = (2, 28);

/// The fix for NixOS without nix-ld.
pub const NIX_LD_FIX: &str =
    "enable programs.nix-ld, or use the Nix-provided adapters (`hennery host join --no-runtime`, then `--agent`)";

/// The fix for a machine without glibc.
pub const MUSL_FIX: &str = "run only the collector here, or the host on a Linux with glibc 2.28 or later";

/// The fix for a glibc older than `MIN_GLIBC`.
pub const UPGRADE_FIX: &str = "upgrade to a distribution with glibc 2.28 or later";

/// What answers as the C library behind glibc's loader path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Libc {
    Glibc(u32, u32),
    /// musl's loader (or a compatibility link to it).
    Musl,
    /// NixOS's stub loader: nix-ld is not enabled.
    NixStub,
    /// Nothing that could be read.
    Unknown,
}

/// What a program run to find the C library printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// What the loader at `loader` says it is, else what `getconf` says (not on
/// NixOS). `run` runs a program, bounded: `None` when it could not be run or
/// did not finish in time.
pub fn libc_behind(loader: &Path, getconf: &Path, nixos: bool, run: &dyn Fn(&Path, &[&str]) -> Option<Said>) -> Libc {
    if let Some(said) = run(loader, &["--version"]) {
        match parse_loader(&format!("{}\n{}", said.stdout, said.stderr)) {
            Libc::Unknown => {}
            known => return known,
        }
    }
    if nixos {
        return Libc::Unknown;
    }
    run(getconf, &["GNU_LIBC_VERSION"])
        .filter(|said| said.ok)
        .and_then(|said| parse_getconf(&said.stdout))
        .map_or(Libc::Unknown, |(major, minor)| Libc::Glibc(major, minor))
}

/// What a loader printed for `--version`: glibc's banner (`ld.so (Ubuntu
/// GLIBC 2.35-0ubuntu3) stable release version 2.35.`, `ld.so (GNU libc)
/// stable release version 2.39.`), musl's (`musl libc (x86_64)`), or
/// NixOS's stub (`… https://nix.dev/permalink/stub-ld`).
pub fn parse_loader(text: &str) -> Libc {
    let lower = text.to_ascii_lowercase();
    if lower.contains("stub-ld") || lower.contains("nixos cannot run") {
        return Libc::NixStub;
    }
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
    if first.trim_start().starts_with("musl libc") {
        return Libc::Musl;
    }
    let glibc = first.contains("GLIBC") || first.contains("GNU libc") || first.contains("GNU C Library");
    match first
        .rsplit_once("version ")
        .and_then(|(_, version)| major_minor(version))
    {
        Some((major, minor)) if glibc => Libc::Glibc(major, minor),
        _ => Libc::Unknown,
    }
}

/// `getconf GNU_LIBC_VERSION`'s answer: `glibc 2.35`.
pub fn parse_getconf(text: &str) -> Option<(u32, u32)> {
    major_minor(text.trim().strip_prefix("glibc ")?)
}

/// `5.15.0-91-generic`, `4.18.0-553.el8_10.x86_64`, `2.35.`: the first two
/// numbers.
pub fn major_minor(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Why the managed runtime cannot run on a Linux machine, and the fix:
/// `loader` is glibc's loader path and whether it is there, `nixos` whether
/// `/etc/NIXOS` is, `libc` what the loader leads to. `None` unless
/// something shows it cannot: a glibc of unknown version may run it.
pub fn refusal(loader: Option<(&str, bool)>, nixos: bool, libc: Libc) -> Option<(String, &'static str)> {
    match (loader, libc) {
        (Some((path, false)), _) if nixos => Some((
            format!("NixOS without nix-ld: glibc's loader {path} is missing"),
            NIX_LD_FIX,
        )),
        (Some((path, false)), _) => Some((
            format!("no glibc loader ({path}): a musl system, or one without glibc"),
            MUSL_FIX,
        )),
        (_, Libc::NixStub) => Some((
            "NixOS without nix-ld: glibc's loader is NixOS's stub".to_string(),
            NIX_LD_FIX,
        )),
        (_, Libc::Musl) => Some(("glibc's loader is musl's".to_string(), MUSL_FIX)),
        (_, Libc::Glibc(major, minor)) if (major, minor) < MIN_GLIBC => {
            Some((format!("glibc {major}.{minor} is older than 2.28"), UPGRADE_FIX))
        }
        _ => None,
    }
}

/// How long the host waits for the loader or `getconf` to answer: a hung
/// one costs a host start this long, and then passes as unknown.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The most output read from a program run to find the C library.
const MAX_OUTPUT: u64 = 4096;

/// Run `program` to find the C library: no standard input, an environment
/// of the base system's PATH alone, at most `MAX_OUTPUT` bytes of each
/// output read. It leads a process group of its own, killed once it exits
/// or after `timeout`, so what it left running cannot hold the read; one
/// that fills a pipe is killed at the timeout and gives `None`. Only what
/// escapes the group (`setsid`) could still hold the read; glibc's loader
/// and `getconf` start nothing. Blocking: an async caller runs it off its
/// runtime's workers.
pub fn run_bounded(program: &Path, args: &[&str], timeout: Duration) -> Option<Said> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .ok()?;
    let group = child.id() as libc::pid_t;
    let deadline = Instant::now() + timeout;
    // Watched without being reaped: until it is, its pid (the group's id)
    // cannot be another process's, so the kill below reaches only its group.
    let finished = loop {
        match exited(group) {
            Ok(true) => break true,
            Ok(false) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => break false,
        }
    };
    // SAFETY: kill(2) on the process group the program leads.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    let status = child.wait().ok()?;
    if !finished {
        return None;
    }
    let mut stdout = Vec::new();
    child.stdout.take()?.take(MAX_OUTPUT).read_to_end(&mut stdout).ok()?;
    let mut stderr = Vec::new();
    child.stderr.take()?.take(MAX_OUTPUT).read_to_end(&mut stderr).ok()?;
    Some(Said {
        ok: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Whether child `pid` has exited, leaving it unreaped (`WNOWAIT`).
fn exited(pid: libc::pid_t) -> std::io::Result<bool> {
    // SAFETY: all-zero bytes are a valid `siginfo_t`; waitid(2) fills it.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let flags = libc::WEXITED | libc::WNOWAIT | libc::WNOHANG;
    // SAFETY: waitid(2) on our own child, into a local `siginfo_t`.
    if unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, flags) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // With `WNOHANG`, a child that has not changed state leaves `si_pid` 0.
    #[cfg(target_os = "linux")]
    // SAFETY: waitid(2) filled `info` for a `SIGCHLD`.
    let changed = unsafe { info.si_pid() };
    #[cfg(not(target_os = "linux"))]
    let changed = info.si_pid;
    Ok(changed != 0)
}

/// Why a host on the Linux machine under `root` cannot run the managed
/// runtime, if something shows it cannot: `loader` is glibc's loader path
/// there. The loader's presence is looked at before anything is run.
pub fn host_refusal(
    root: &Path,
    loader: &str,
    run: &dyn Fn(&Path, &[&str]) -> Option<Said>,
) -> Option<(String, &'static str)> {
    let nixos = root.join("etc/NIXOS").exists();
    let path = root.join(loader.trim_start_matches('/'));
    let present = path.exists();
    let libc = if present {
        libc_behind(&path, &root.join("usr/bin/getconf"), nixos, run)
    } else {
        Libc::Unknown
    };
    refusal(Some((loader, present)), nixos, libc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn said(stdout: &str, stderr: &str, ok: bool) -> Option<Said> {
        Some(Said {
            ok,
            stdout: stdout.into(),
            stderr: stderr.into(),
        })
    }

    #[test]
    fn a_nix_ld_loader_run_without_its_environment_is_unknown_not_the_stub() {
        // nix-ld run as a program, with NIX_LD cleared, as the probe runs it.
        let nix_ld = "cannot execute binary: NIX_LD or NIX_LD_x86_64_linux is not set\n";
        assert_eq!(parse_loader(nix_ld), Libc::Unknown);
    }

    /// A machine root with a loader at `loader` (or none) and `/etc/NIXOS`.
    fn root(loader: Option<&str>, nixos: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(loader) = loader {
            let path = dir.path().join(loader.trim_start_matches('/'));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
        }
        if nixos {
            std::fs::create_dir_all(dir.path().join("etc")).unwrap();
            std::fs::write(dir.path().join("etc/NIXOS"), "").unwrap();
        }
        dir
    }

    const LOADER: &str = "/lib64/ld-linux-x86-64.so.2";

    #[test]
    fn a_host_is_refused_only_on_what_shows_it_cannot_run_the_runtime() {
        let ubuntu = "ld.so (Ubuntu GLIBC 2.39-0ubuntu8.4) stable release version 2.39.\n";
        let el7 = "ld.so (GNU libc) stable release version 2.17, by Roland McGrath et al.\n";
        let stub = "NixOS cannot run dynamically linked executables intended for generic\n\
                    linux environments out of the box. For more information, see:\n\
                    https://nix.dev/permalink/stub-ld\n";
        let musl = "musl libc (x86_64)\nVersion 1.2.4\n";
        let refused = |loader: Option<&str>, nixos: bool, answer: Option<Said>| {
            let root = root(loader, nixos);
            host_refusal(root.path(), LOADER, &|_, _| answer.clone()).map(|(why, fix)| (why, fix.to_string()))
        };
        assert_eq!(refused(Some(LOADER), false, said(ubuntu, "", true)), None);
        let (why, fix) = refused(Some(LOADER), false, said(el7, "", true)).unwrap();
        assert!(
            why.contains("glibc 2.17 is older than 2.28") && fix == UPGRADE_FIX,
            "{why}"
        );
        let (why, fix) = refused(Some(LOADER), true, said("", stub, false)).unwrap();
        assert!(why.contains("NixOS's stub") && fix == NIX_LD_FIX, "{why}");
        let (why, fix) = refused(Some(LOADER), false, said("", musl, false)).unwrap();
        assert!(why.contains("musl's") && fix == MUSL_FIX, "{why}");
        let (why, fix) = refused(None, false, None).unwrap();
        assert!(why.contains("no glibc loader") && fix == MUSL_FIX, "{why}");
        let (why, fix) = refused(None, true, None).unwrap();
        assert!(why.contains("NixOS without nix-ld") && fix == NIX_LD_FIX, "{why}");
        // What says nothing, or cannot be run, does not stop a host.
        assert_eq!(refused(Some(LOADER), false, None), None);
        assert_eq!(refused(Some(LOADER), true, said("NIX_LD is not set", "", false)), None);
        assert_eq!(refused(Some(LOADER), false, said("", "garbage", false)), None);
    }

    #[test]
    fn nothing_is_run_where_there_is_no_loader() {
        let root = root(None, false);
        let ran = RefCell::new(0);
        assert!(
            host_refusal(root.path(), LOADER, &|_, _| {
                *ran.borrow_mut() += 1;
                None
            })
            .is_some()
        );
        assert_eq!(*ran.borrow(), 0);
    }

    #[test]
    fn getconf_answers_when_the_loader_does_not_but_never_on_nixos() {
        let asked = RefCell::new(Vec::new());
        let run = |program: &Path, _: &[&str]| {
            asked.borrow_mut().push(program.to_path_buf());
            if program.ends_with("getconf") {
                said("glibc 2.27\n", "", true)
            } else {
                said("", "", true)
            }
        };
        let loader = Path::new("/l");
        let getconf = Path::new("/usr/bin/getconf");
        assert_eq!(libc_behind(loader, getconf, false, &run), Libc::Glibc(2, 27));
        assert_eq!(asked.borrow().len(), 2);
        asked.borrow_mut().clear();
        assert_eq!(libc_behind(loader, getconf, true, &run), Libc::Unknown);
        assert_eq!(*asked.borrow(), vec![loader.to_path_buf()]);
    }

    /// A script standing in for a loader, run by `/bin/sh`: never the real
    /// loader, and never a file just written being exec'd (ETXTBSY on Linux
    /// while another test forks).
    fn script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("{body}\n")).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn a_probe_is_read_and_a_hung_one_gives_up_at_its_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let sh = Path::new("/bin/sh");
        let answering = script(
            dir.path(),
            "answering",
            "echo \"ld.so (GNU libc) stable release version 2.39.\"; echo err >&2",
        );
        let got = run_bounded(sh, &[&answering, "--version"], Duration::from_secs(30)).unwrap();
        assert!(
            got.ok && got.stdout.contains("2.39") && got.stderr == "err\n",
            "{got:?}"
        );
        // Shell builtins only: a NixOS machine has no `/bin/sleep`.
        let hung = script(dir.path(), "hung", "while :; do :; done");
        let started = Instant::now();
        assert_eq!(run_bounded(sh, &[&hung, "--version"], Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        assert_eq!(run_bounded(&dir.path().join("absent"), &[], PROBE_TIMEOUT), None);
    }

    /// What a probe leaves running is killed with it, and holds no read.
    #[test]
    fn what_a_probe_leaves_behind_does_not_hold_its_answer() {
        let dir = tempfile::tempdir().unwrap();
        let leaving = script(dir.path(), "leaving", "(while :; do :; done) &\necho answered");
        // On a thread of its own: a read held up fails the test, not hangs it.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_bounded(Path::new("/bin/sh"), &[&leaving], Duration::from_secs(30)));
        });
        let got = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the probe's read is held by what it left behind")
            .unwrap();
        assert_eq!(got.stdout, "answered\n");
    }

    /// A probe runs with the base PATH alone: nothing of the host's own
    /// environment (nix-ld's `NIX_LD`, for one) reaches it.
    #[test]
    fn a_probe_runs_in_an_empty_environment() {
        let dir = tempfile::tempdir().unwrap();
        let env = script(dir.path(), "env", "echo \"${HOME-unset} ${NIX_LD-unset} $PATH\"");
        let got = run_bounded(Path::new("/bin/sh"), &[&env], Duration::from_secs(30)).unwrap();
        assert_eq!(got.stdout, "unset unset /usr/bin:/bin\n");
    }

    #[test]
    fn musl_is_told_from_the_banners_first_line_only() {
        let glibc_warning = "ld.so (GNU libc) stable release version 2.39.\nwarning: /opt/musl-cross/lib ignored\n";
        assert_eq!(parse_loader(glibc_warning), Libc::Glibc(2, 39));
        assert_eq!(parse_loader("\nmusl libc (aarch64)\nVersion 1.2.5\n"), Libc::Musl);
    }
}
