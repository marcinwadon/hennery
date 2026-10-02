//! PATH capture for the service environment (distribution spec §6.1).
//! Services do not inherit the login shell's PATH, but agents run `sh`,
//! `git`, toolchains and MCP servers through it (D-3: without it every tool
//! call failed, and looked like the agent ignoring its tools).

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the login shell may take to start.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// The most environment a login shell may print.
const MAX_ENVIRONMENT: u64 = 1 << 20;

/// The variable naming the file the shell writes its environment to.
const OUT_VAR: &str = "HENNERY_ENV_OUT";

/// What a service's PATH always ends with: the base system.
fn system_dirs(macos: bool) -> &'static [&'static str] {
    if macos {
        &["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
    } else {
        &["/usr/bin", "/bin"]
    }
}

/// The variables the login shell starts with, from `outer` (this process's
/// environment): enough to find the user's configuration, and nothing of
/// the shell `hennery service install` runs in — a project shell (`nix
/// develop`, direnv, a virtualenv) must not lend the service its own
/// short-lived PATH, and the agent nesting variables are left behind too.
pub fn shell_environment(outer: &BTreeMap<String, String>, shell: &Path, macos: bool) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = [
        "HOME",
        "USER",
        "LOGNAME",
        "LANG",
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
    ]
    .into_iter()
    .filter_map(|name| outer.get(name).map(|value| (name.to_string(), value.clone())))
    .collect();
    env.push(("SHELL".into(), shell.display().to_string()));
    env.push(("TERM".into(), "dumb".into()));
    env.push(("PATH".into(), system_dirs(macos).join(":")));
    env
}

/// The environment `shell` has as a login shell: it runs `shell -l -i -c`
/// with standard input from `/dev/null`, its output discarded, in a session
/// of its own (no terminal to take), with only `env`, and kills it, with its
/// whole process group, after `timeout`. It writes `/usr/bin/env -0` to a
/// private file, so nothing its startup files print can be mistaken for a
/// variable, and a value holding a newline stays one value.
pub fn login_environment(
    shell: &Path,
    env: &[(String, String)],
    timeout: Duration,
) -> Result<BTreeMap<String, String>> {
    // Unique within this process too: two captures may start in one tick.
    static CAPTURES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let out = std::env::temp_dir().join(format!(
        "hennery-env-{}-{}-{}",
        std::process::id(),
        CAPTURES.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&out)
        .with_context(|| format!("create {}", out.display()))?;
    // It holds the whole login environment for a moment, whatever tokens
    // the startup files export among it: private, and removed below
    // however the capture ends.
    let result = run_shell(shell, env, &out, timeout).and_then(|()| {
        // Bounded as it is read: a process the startup files detached may
        // still be writing.
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(&out)?
            .take(MAX_ENVIRONMENT + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_ENVIRONMENT {
            bail!("the login shell's environment is over {MAX_ENVIRONMENT} bytes");
        }
        Ok(parse_env0(&bytes))
    });
    let _ = std::fs::remove_file(&out);
    let captured = result?;
    if captured.is_empty() {
        bail!("{} -l -i printed no environment", shell.display());
    }
    Ok(captured)
}

fn run_shell(shell: &Path, env: &[(String, String)], out: &Path, timeout: Duration) -> Result<()> {
    let mut cmd = std::process::Command::new(shell);
    cmd.args(["-l", "-i", "-c", &format!("/usr/bin/env -0 > \"${OUT_VAR}\"")])
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .env(OUT_VAR, out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: setsid(2) in the forked child, before exec; async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("start the login shell {}", shell.display()))?;
    let group = child.id() as libc::pid_t;
    let deadline = Instant::now() + timeout;
    // Watched without being reaped: until it is, its pid (the group's id)
    // cannot be another process's, so the kill below reaches only what the
    // shell left behind.
    let finished = loop {
        match exited(group) {
            Ok(true) => break true,
            Ok(false) if Instant::now() >= deadline => break false,
            Ok(false) => std::thread::sleep(Duration::from_millis(20)),
            // Not left running: killed, with its group, and reaped.
            Err(err) => {
                // SAFETY: kill(2) on the process group the shell leads.
                unsafe {
                    libc::kill(-group, libc::SIGKILL);
                }
                let _ = child.wait();
                return Err(err).context("watch the login shell");
            }
        }
    };
    // Whatever its startup files left running in its session goes too.
    // SAFETY: kill(2) on the process group the shell leads.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    let status = child.wait()?;
    if !finished {
        bail!(
            "{} -l -i did not finish within {} s",
            shell.display(),
            timeout.as_secs()
        );
    }
    if !status.success() {
        bail!("{} -l -i -c env failed ({status})", shell.display());
    }
    Ok(())
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

/// `env -0`'s output as variables; a record without `=` is skipped.
pub fn parse_env0(bytes: &[u8]) -> BTreeMap<String, String> {
    bytes
        .split(|&b| b == 0)
        .filter_map(|record| {
            let record = String::from_utf8_lossy(record);
            let (name, value) = record.split_once('=')?;
            (!name.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

/// The service's PATH, and what was changed on the way, to tell the user.
#[derive(Debug, PartialEq, Eq)]
pub struct ServicePath {
    pub path: String,
    pub notes: Vec<String>,
}

/// Turn a login shell's environment into the service's PATH:
/// - empty and relative entries are dropped (a service's working directory
///   is not the user's), and so is any with a control character;
/// - a repeated entry is kept once, first;
/// - an fnm per-shell entry (`…/fnm_multishells/<id>/bin`), gone with its
///   shell, becomes fnm's default alias, or is dropped if there is none;
/// - nvm's current version becomes the version nvm's `default` alias
///   names, when that resolves to an installed one;
/// - the base system's directories are appended unless present.
///
/// Entries other users can write to, and Nix store paths (removed by a
/// garbage collection), are kept but named in `notes`.
pub fn service_path(captured: &BTreeMap<String, String>, home: &Path, macos: bool) -> Result<ServicePath> {
    let Some(path) = captured.get("PATH") else {
        bail!("the login shell's environment has no PATH");
    };
    let mut notes = Vec::new();
    let mut entries: Vec<String> = Vec::new();
    for entry in path.split(':') {
        if entry.is_empty() {
            continue;
        }
        if entry.chars().any(char::is_control) {
            notes.push(format!("dropped {entry:?}: it holds a control character"));
            continue;
        }
        // `parse_env0` decodes lossily: this was not valid UTF-8.
        if entry.contains('\u{fffd}') {
            notes.push(format!("dropped {entry:?}: it is not valid UTF-8"));
            continue;
        }
        if !entry.starts_with('/') {
            notes.push(format!("dropped {entry}: not an absolute path"));
            continue;
        }
        let entry = match stable(entry, captured, home) {
            Stable::Keep => entry.to_string(),
            Stable::Replace(with) => {
                notes.push(format!("replaced {entry} with {with}"));
                with
            }
            Stable::Drop(why) => {
                notes.push(format!("dropped {entry}: {why}"));
                continue;
            }
        };
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    for dir in system_dirs(macos) {
        if !entries.iter().any(|e| e == dir) {
            entries.push((*dir).to_string());
        }
    }
    // SAFETY: getuid(2) cannot fail.
    let uid = unsafe { libc::getuid() };
    for entry in &entries {
        if entry.starts_with("/nix/store/") {
            notes.push(format!(
                "{entry} is in the Nix store: a garbage collection may remove it; prefer a profile's bin directory"
            ));
        }
        if let Ok(meta) = std::fs::metadata(entry)
            && (meta.mode() & 0o022 != 0 || (meta.uid() != uid && meta.uid() != 0))
        {
            notes.push(format!(
                "{entry} can be written by another user, who could put a program there that agents would run"
            ));
        }
    }
    Ok(ServicePath {
        path: entries.join(":"),
        notes,
    })
}

enum Stable {
    Keep,
    Replace(String),
    Drop(&'static str),
}

/// The stable stand-in for a version manager's short-lived entry.
fn stable(entry: &str, captured: &BTreeMap<String, String>, home: &Path) -> Stable {
    if entry.contains("/fnm_multishells/") {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(dir) = captured.get("FNM_DIR") {
            candidates.push(dir.into());
        }
        if let Some(data) = captured.get("XDG_DATA_HOME") {
            candidates.push(Path::new(data).join("fnm"));
        }
        candidates.extend([
            home.join(".local/share/fnm"),
            home.join("Library/Application Support/fnm"),
            home.join(".fnm"),
        ]);
        return match candidates
            .iter()
            .map(|dir| dir.join("aliases/default/bin"))
            .find(|bin| bin.is_dir())
        {
            Some(bin) => Stable::Replace(bin.display().to_string()),
            None => Stable::Drop("fnm's per-shell directory, and fnm has no default alias"),
        };
    }
    let nvm = captured
        .get("NVM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".nvm"));
    let versions = nvm.join("versions/node");
    if let Ok(rest) = Path::new(entry).strip_prefix(&versions)
        && rest.components().count() == 2
        && rest.ends_with("bin")
        && let Some(default) = nvm_default(&nvm)
    {
        let bin = versions.join(default).join("bin");
        if bin != Path::new(entry) {
            return Stable::Replace(bin.display().to_string());
        }
    }
    Stable::Keep
}

/// The installed version nvm's `default` alias names: `vX.Y.Z` exactly, the
/// highest installed `X` or `X.Y`, or the highest of all for `node` and
/// `stable`. `None` for anything else (an `lts/…` alias, say).
fn nvm_default(nvm: &Path) -> Option<String> {
    let alias = std::fs::read_to_string(nvm.join("alias/default")).ok()?;
    let alias = alias.trim();
    let installed: Vec<(Vec<u64>, String)> = std::fs::read_dir(nvm.join("versions/node"))
        .ok()?
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let parts = name
                .strip_prefix('v')?
                .split('.')
                .map(|p| p.parse().ok())
                .collect::<Option<Vec<u64>>>()?;
            Some((parts, name))
        })
        .collect();
    let wanted: Option<Vec<u64>> = match alias {
        "node" | "stable" => Some(Vec::new()),
        _ => alias
            .strip_prefix('v')
            .unwrap_or(alias)
            .split('.')
            .map(|p| p.parse().ok())
            .collect(),
    };
    let wanted = wanted?;
    installed
        .into_iter()
        .filter(|(parts, _)| parts.starts_with(&wanted))
        .max()
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured(path: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("PATH".to_string(), path.to_string())])
    }

    #[test]
    fn env0_output_keeps_newlines_inside_values() {
        let parsed = parse_env0(b"A=1\0B=two\nPATH=/evil\0PATH=/usr/bin\0junk\0");
        assert_eq!(parsed["A"], "1");
        assert_eq!(parsed["B"], "two\nPATH=/evil");
        assert_eq!(parsed["PATH"], "/usr/bin");
        assert!(!parsed.contains_key("junk"));
    }

    /// Relative, empty and repeated entries go; the base system's
    /// directories are appended unless present (`/usr/sbin:/sbin` on macOS
    /// only).
    #[test]
    fn relative_and_repeated_entries_go_and_the_system_dirs_are_appended() {
        let home = tempfile::tempdir().unwrap();
        let path = "/opt/a/bin::.:bin:/opt/a/bin:/bin:./x";
        let linux = service_path(&captured(path), home.path(), false).unwrap();
        assert_eq!(linux.path, "/opt/a/bin:/bin:/usr/bin");
        assert_eq!(
            linux
                .notes
                .iter()
                .filter(|n| n.contains("not an absolute path"))
                .count(),
            3
        );
        let macos = service_path(&captured(path), home.path(), true).unwrap();
        assert_eq!(macos.path, "/opt/a/bin:/bin:/usr/bin:/usr/sbin:/sbin");
        assert!(service_path(&BTreeMap::new(), home.path(), false).is_err());
        let control = service_path(&captured("/opt/a\nb/bin:/opt/c"), home.path(), false).unwrap();
        assert_eq!(control.path, "/opt/c:/usr/bin:/bin");
        let lossy = parse_env0(b"PATH=/opt/\xff/bin:/opt/c\0");
        let lossy = service_path(&lossy, home.path(), false).unwrap();
        assert_eq!(lossy.path, "/opt/c:/usr/bin:/bin");
        assert!(
            lossy.notes.iter().any(|n| n.contains("not valid UTF-8")),
            "{:?}",
            lossy.notes
        );
    }

    /// fnm's per-shell directory is replaced by its default alias, found
    /// through `FNM_DIR` or fnm's own default directories, and dropped when
    /// there is none.
    #[test]
    fn fnms_per_shell_directory_becomes_its_default_alias() {
        let home = tempfile::tempdir().unwrap();
        let fnm = home.path().join("fnm-data");
        std::fs::create_dir_all(fnm.join("aliases/default/bin")).unwrap();
        let entry = "/run/user/1000/fnm_multishells/1234_5678/bin";
        let mut env = captured(&format!("{entry}:/usr/bin"));
        env.insert("FNM_DIR".into(), fnm.display().to_string());
        let path = service_path(&env, home.path(), false).unwrap();
        let alias = fnm.join("aliases/default/bin").display().to_string();
        assert_eq!(path.path, format!("{alias}:/usr/bin:/bin"));
        assert!(path.notes.iter().any(|n| n.starts_with("replaced")), "{:?}", path.notes);

        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(elsewhere.path().join(".local/share/fnm/aliases/default/bin")).unwrap();
        let path = service_path(&captured(entry), elsewhere.path(), false).unwrap();
        assert!(path.path.starts_with(&format!(
            "{}/.local/share/fnm/aliases/default/bin:",
            elsewhere.path().display()
        )));

        let none = tempfile::tempdir().unwrap();
        let path = service_path(&captured(entry), none.path(), false).unwrap();
        assert_eq!(path.path, "/usr/bin:/bin");
        assert!(
            path.notes.iter().any(|n| n.contains("no default alias")),
            "{:?}",
            path.notes
        );
    }

    /// nvm's current version becomes the one its `default` alias resolves
    /// to; an alias that does not resolve leaves the entry as it is.
    #[test]
    fn nvms_current_version_becomes_its_default() {
        let home = tempfile::tempdir().unwrap();
        let nvm = home.path().join(".nvm");
        for v in ["v20.11.1", "v22.1.0", "v22.9.0", "v24.0.0"] {
            std::fs::create_dir_all(nvm.join("versions/node").join(v).join("bin")).unwrap();
        }
        let current = nvm.join("versions/node/v24.0.0/bin").display().to_string();
        let expect = |alias: &str, version: &str| {
            std::fs::write(nvm.join("alias/default"), format!("{alias}\n")).unwrap();
            let path = service_path(&captured(&current), home.path(), false).unwrap();
            let bin = nvm.join("versions/node").join(version).join("bin");
            assert_eq!(path.path, format!("{}:/usr/bin:/bin", bin.display()), "alias {alias}");
        };
        std::fs::create_dir_all(nvm.join("alias")).unwrap();
        expect("22", "v22.9.0");
        expect("v20.11.1", "v20.11.1");
        expect("22.1", "v22.1.0");
        expect("node", "v24.0.0");
        expect("lts/*", "v24.0.0");
        expect("18", "v24.0.0");
    }

    /// Other users' writable directories and the Nix store are named.
    #[test]
    fn writable_and_nix_store_entries_are_named() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let open = home.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = format!("{}:/nix/store/abc-git/bin", open.display());
        let path = service_path(&captured(&path), home.path(), false).unwrap();
        assert!(
            path.notes.iter().any(|n| n.contains("another user")),
            "{:?}",
            path.notes
        );
        assert!(path.notes.iter().any(|n| n.contains("Nix store")), "{:?}", path.notes);
    }

    /// `name` on this process's PATH, if it is there; else `None`, which
    /// fails the test where `HENNERY_REQUIRE_SHELLS` names it (CI's ubuntu
    /// job), so the capture tests run there rather than skip.
    fn shell(name: &str) -> Option<PathBuf> {
        let found = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
        });
        let required = std::env::var("HENNERY_REQUIRE_SHELLS").unwrap_or_default();
        if found.is_none() {
            assert!(
                !required.split(',').any(|r| r.trim() == name),
                "{name} is required (HENNERY_REQUIRE_SHELLS) but not on PATH"
            );
            eprintln!("skipped: {name} is not on PATH");
        }
        found
    }

    /// Capture `name`'s login PATH with `files` (relative to a fresh HOME)
    /// as its startup files.
    fn capture(name: &str, files: &[(&str, &str)]) -> Option<(tempfile::TempDir, ServicePath)> {
        let shell = shell(name)?;
        let home = tempfile::tempdir().unwrap();
        for (file, text) in files {
            let path = home.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let mut outer = BTreeMap::new();
        outer.insert("HOME".to_string(), home.path().display().to_string());
        outer.insert("USER".to_string(), "hennery-test".to_string());
        outer.insert("CLAUDECODE".to_string(), "1".to_string());
        let env = shell_environment(&outer, &shell, cfg!(target_os = "macos"));
        assert!(env.iter().all(|(k, _)| k != "CLAUDECODE"));
        let captured = login_environment(&shell, &env, CAPTURE_TIMEOUT).unwrap();
        assert!(!captured.contains_key("CLAUDECODE"), "{captured:?}");
        let path = service_path(&captured, home.path(), cfg!(target_os = "macos")).unwrap();
        Some((home, path))
    }

    /// The marker directory a login shell's startup files put on PATH comes
    /// first in the service's PATH, despite what those files print; this
    /// process's own PATH (the test runner's, perhaps a dev shell's) does
    /// not leak in.
    fn assert_marker(path: &ServicePath, marker: &str) {
        assert!(path.path.starts_with(&format!("{marker}:")), "{}", path.path);
        let ours = std::env::var("PATH").unwrap_or_default();
        for entry in ours.split(':').filter(|e| e.contains("/nix/store/")) {
            assert!(
                !path.path.split(':').any(|e| e == entry),
                "{entry} leaked into {}",
                path.path
            );
        }
    }

    #[test]
    fn bash_login_path_is_captured() {
        let profile = "echo welcome to bash\nexport PATH=\"/opt/hennery-marker-bash/bin:$PATH\"\nsleep 30 &\n";
        if let Some((_home, path)) = capture("bash", &[(".bash_profile", profile)]) {
            assert_marker(&path, "/opt/hennery-marker-bash/bin");
        }
    }

    #[test]
    fn zsh_login_path_is_captured() {
        let rc = "print hello from zsh\npath=(/opt/hennery-marker-zsh/bin $path)\n";
        if let Some((_home, path)) = capture("zsh", &[(".zshrc", rc)]) {
            assert_marker(&path, "/opt/hennery-marker-zsh/bin");
        }
    }

    #[test]
    fn fish_login_path_is_captured() {
        let config = "echo hello from fish\nset -gx PATH /opt/hennery-marker-fish/bin $PATH\n";
        if let Some((_home, path)) = capture("fish", &[(".config/fish/config.fish", config)]) {
            assert_marker(&path, "/opt/hennery-marker-fish/bin");
        }
    }

    /// A login shell that never finishes is killed, with what it started,
    /// after the timeout.
    #[test]
    fn a_shell_that_hangs_is_killed_after_the_timeout() {
        let Some(bash) = shell("bash") else {
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let pidfile = home.path().join("sleep.pid");
        std::fs::write(
            home.path().join(".bash_profile"),
            format!("sleep 60 &\necho $! > {}\nwait\n", pidfile.display()),
        )
        .unwrap();
        let outer = BTreeMap::from([("HOME".to_string(), home.path().display().to_string())]);
        let env = shell_environment(&outer, &bash, false);
        let started = Instant::now();
        let err = login_environment(&bash, &env, Duration::from_secs(2)).unwrap_err();
        assert!(err.to_string().contains("did not finish within 2 s"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        // SAFETY: kill(2) with signal 0 only checks that the pid exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(Instant::now() < deadline, "the shell's sleep {pid} outlived it");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
