//! The service's checks (distribution spec §6, §8): its PATH (check 5), the
//! service itself (check 10) and the host data directory it serves, with
//! `host.lock` (check 14). The service manager is only asked (`print`,
//! `is-active`, `is-enabled`, `show`, `show-user`), and only its parsed
//! answers are kept: `launchctl print` lists the job's environment.

use super::dirs::NO_HOST;
use super::{Doctor, Finding, Verdict, process};
use crate::service::{self, Platform, Report, path, unit, unit::Role};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// Why check 5 did not run.
const NO_SERVICE: &str = "no service installed";

/// Why check 5 did not run with several roles (check 10 fails them).
const SEVERAL: &str = "more than one service installed";

/// How `role`'s service is restarted.
fn restart(doctor: &Doctor, role: Role) -> String {
    match doctor.cx.platform {
        Platform::MacOs => format!("launchctl kickstart -k gui/{}/{}", doctor.cx.uid, role.label()),
        Platform::Linux => format!("systemctl --user restart {}", role.unit()),
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Check 10 (distribution spec §5.2, §6): installed, running, running this
/// binary as it is on disk, linger on Linux, one role, and `up`'s children
/// judged as `hennery service status` judges them.
pub fn service(doctor: &Doctor) -> Finding {
    let installed = doctor.cx.installed();
    let mut verdict = Verdict::default();
    match installed.as_slice() {
        [] => verdict.warn(
            "no service is installed: hennery runs only while started by hand",
            "run `hennery service install` (with `--role host` on a machine that only hosts)",
        ),
        [_] => {}
        several => {
            let names: Vec<&str> = several.iter().map(|r| r.name()).collect();
            verdict.fail(
                format!("{} roles are installed: {}", several.len(), names.join(", ")),
                "keep one: `hennery service uninstall --role <role>` for the others",
            );
        }
    }
    for role in installed {
        one_service(doctor, role, &mut verdict);
    }
    Finding::Checked(verdict.check(10, "service"))
}

fn one_service(doctor: &Doctor, role: Role, verdict: &mut Verdict) {
    let cx = doctor.cx;
    let reinstall = format!("run `hennery service install --role {role}` again");
    let Some(argv) = service::read_command_line(cx, role).filter(|argv| !argv.is_empty()) else {
        verdict.fail(
            format!(
                "the {role} service's file cannot be read ({})",
                cx.service_file(role).display()
            ),
            reinstall,
        );
        return;
    };
    let exe = PathBuf::from(&argv[0]);
    if !exe.exists() {
        verdict.fail(
            format!("the {role} service runs {}, which is missing", exe.display()),
            reinstall,
        );
        return;
    }
    if canonical(&exe) != canonical(&cx.exe) {
        verdict.warn(
            format!(
                "the {role} service runs {}, not this binary ({})",
                exe.display(),
                cx.exe.display()
            ),
            format!("run `hennery service install --role {role}` with the binary to use"),
        );
    }
    let managed = match service::managed(cx, role) {
        Ok(Some(managed)) => managed,
        Ok(None) => {
            verdict.warn(
                format!("`systemctl --user` cannot be run, so whether the {role} service runs is unknown"),
                "run doctor in the login session the service runs in (not under `su` or `sudo -u`)",
            );
            return;
        }
        Err(err) => {
            verdict.warn(
                format!("the service manager cannot be asked about the {role} service: {err:#}"),
                "run doctor in the login session the service runs in",
            );
            return;
        }
    };
    if cx.platform == Platform::Linux && !service::linger_on(cx) {
        verdict.warn(
            "linger is off: the service stops when you log out and does not start at boot",
            format!("run `loginctl enable-linger {}`", cx.user),
        );
    }
    if !managed.running {
        verdict.fail(
            format!("the {role} service is not running ({})", managed.said),
            format!("see `hennery service status` and the service's log, then {reinstall}"),
        );
        return;
    }
    verdict.ok(format!("the {role} service runs ({})", managed.said));
    if let Some(pid) = managed.pid {
        match process::runs(cx, pid, &exe) {
            Some(true) => {}
            Some(false) => verdict.warn(
                format!(
                    "{} changed after the service started: pid {pid} still runs the old binary",
                    exe.display()
                ),
                format!("restart it: `{}`", restart(doctor, role)),
            ),
            None => verdict.ok(format!(
                "whether pid {pid} runs {} as it is now is unknown",
                exe.display()
            )),
        }
    }
    if role == Role::Up
        && let Some(data) = service::data_dir_of(&argv)
    {
        children(doctor, &data, managed.pid, verdict);
    }
}

/// `up`'s report of its children (`supervisor.json`), judged as `service
/// status` judges it: one given up on, or revoked, fails.
fn children(doctor: &Doctor, data: &Path, pid: Option<u32>, verdict: &mut Verdict) {
    let state = match service::report(data, pid) {
        Report::None | Report::Stale(_) => return,
        Report::Unreadable(err) => {
            verdict.fail(
                format!("up's report of its children cannot be read: {err}"),
                format!("restart the service: `{}`", restart(doctor, Role::Up)),
            );
            return;
        }
        Report::Current(state) => state,
    };
    for (name, child) in [("collector", &state.collector), ("host", &state.host)] {
        let (what, healthy) = service::child_state(child);
        if healthy {
            continue;
        }
        let fix = match child.state {
            crate::supervisor::ChildState::Revoked => {
                let host = data.join("host");
                format!(
                    "stop the service, remove {} and {}, and start it: up pairs its host again",
                    host.join(hennery_host::identity::KEY_FILE).display(),
                    host.join(hennery_host::identity::CONFIG_FILE).display()
                )
            }
            _ => format!(
                "fix the cause (the service's log), then restart the service: `{}`",
                restart(doctor, Role::Up)
            ),
        };
        verdict.fail(format!("up's {name}: {what}"), fix);
    }
}

/// The installed service's PATH, read back from its plist or environment
/// file: that one value, nothing else of the file (decision 9).
fn installed_path(doctor: &Doctor, role: Role) -> Option<String> {
    let cx = doctor.cx;
    match cx.platform {
        Platform::MacOs => unit::plist_path(&std::fs::read_to_string(cx.service_file(role)).ok()?),
        Platform::Linux => unit::env_file_path(&std::fs::read_to_string(cx.env_file()).ok()?),
    }
}

/// The PATH the one installed service gives the host, if there is one.
pub fn service_path_of(doctor: &Doctor) -> Option<String> {
    match doctor.cx.installed()[..] {
        [role] => installed_path(doctor, role),
        _ => None,
    }
}

/// Whether an executable `name` is in one of `entries`.
fn on_path(entries: &[&str], name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    entries.iter().any(|dir| {
        std::fs::metadata(Path::new(dir).join(name)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// Check 5 (distribution spec §6.1): the service's PATH has `sh` and `git`
/// (`rg` is optional), no entry that is gone or short-lived, and has not
/// drifted from the login shell's. The login shell is run again, exactly
/// as `service install` runs it, and only the PATH is kept of what it gives.
pub fn service_path(doctor: &Doctor) -> Finding {
    let cx = doctor.cx;
    let role = match cx.installed()[..] {
        [] => {
            return Finding::NotRun {
                number: 5,
                why: NO_SERVICE,
            };
        }
        [role] => role,
        _ => {
            return Finding::NotRun {
                number: 5,
                why: SEVERAL,
            };
        }
    };
    let mut verdict = Verdict::default();
    let reinstall = format!("run `hennery service install --role {role}` again");
    let Some(installed) = installed_path(doctor, role) else {
        verdict.fail(format!("the {role} service's PATH cannot be read"), reinstall);
        return Finding::Checked(verdict.check(5, "service PATH"));
    };
    let entries: Vec<&str> = installed.split(':').filter(|e| !e.is_empty()).collect();
    let mut missing = false;
    // Agents run `sh` and `git` (distribution §6.1); a collector runs none.
    let needed: &[&str] = if role == Role::Collector { &[] } else { &["sh", "git"] };
    for &tool in needed {
        if !on_path(&entries, tool) {
            missing = true;
            verdict.fail(
                format!("{tool} is not on the service's PATH"),
                format!("install {tool}, or put its directory on your login shell's PATH, then {reinstall}"),
            );
        }
    }
    if on_path(&entries, "rg") {
        verdict.ok(if missing {
            "rg is on the service's PATH"
        } else {
            "sh, git and rg are on the service's PATH"
        });
    } else {
        verdict.ok("rg is not on the service's PATH (optional)");
    }
    let shaky = "fix that in your login shell's PATH, then run `hennery service install` again";
    for entry in &entries {
        if !Path::new(entry).is_dir() {
            verdict.warn(format!("{entry} on the service's PATH does not exist"), shaky);
        }
    }
    let macos = cx.platform == Platform::MacOs;
    let given = std::collections::BTreeMap::from([("PATH".to_string(), installed.clone())]);
    if let Ok(read) = path::service_path(&given, &cx.home, macos) {
        for note in read.notes {
            verdict.warn(format!("the service's PATH: {note}"), shaky);
        }
    }
    let shell = cx.shell.display();
    let env = path::shell_environment(&cx.env, &cx.shell, macos);
    // Only the PATH is kept: the rest of the login environment is dropped
    // here, never printed or logged.
    let now = path::login_environment(&cx.shell, &env, path::CAPTURE_TIMEOUT)
        .and_then(|captured| path::service_path(&captured, &cx.home, macos));
    match now {
        Ok(now) if now.path != installed => verdict.warn(
            format!("your login shell ({shell}) now gives another PATH than the service has"),
            format!("{reinstall} (with `--shell` if it was installed with another shell)"),
        ),
        Ok(_) => verdict.ok(format!("your login shell ({shell}) gives the same PATH")),
        Err(err) => verdict.warn(
            format!("your login shell's ({shell}) PATH could not be captured: {err:#}"),
            format!("check that `{shell} -l -i -c env` runs, or {reinstall} with `--shell`"),
        ),
    }
    Finding::Checked(verdict.check(5, "service PATH"))
}

/// Who holds `host.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    Nobody,
    Pid(u32),
    Unknown(String),
}

/// The most of `host.lock` read: a pid and a newline.
const MAX_LOCK_TEXT: u64 = 32;

/// Who holds `lock`, never by taking it (decision 8): a line of Linux's
/// `/proc/locks` naming it says so without a lock. Else a shared `flock` is
/// tried only while the pid written in the file is a live process, and
/// released at once.
pub fn holder(doctor: &Doctor, lock: &Path) -> Holder {
    match std::fs::symlink_metadata(lock) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Holder::Nobody,
        Err(err) => return Holder::Unknown(format!("{}: {err}", lock.display())),
        Ok(meta) if !meta.is_file() => return Holder::Unknown(format!("{} is not a regular file", lock.display())),
        Ok(_) => {}
    }
    if let Some(pid) = process::flock_holder(doctor.cx, lock) {
        return Holder::Pid(pid);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(lock);
    let mut file = match file {
        Ok(file) => file,
        Err(err) => return Holder::Unknown(format!("{}: {err}", lock.display())),
    };
    let mut text = String::new();
    let _ = (&mut file).take(MAX_LOCK_TEXT).read_to_string(&mut text);
    // A pid of a process that is gone: its lock went with it. No pid at all
    // is a host between taking the lock and writing its pid (or none ever
    // ran here): the probe tells.
    let pid = text.trim().parse::<u32>().ok();
    if pid.is_some_and(|pid| !service::alive(pid)) {
        return Holder::Nobody;
    }
    use std::os::fd::AsRawFd;
    // SAFETY: flock(2) on a descriptor this function owns; dropping the
    // file releases the shared lock if it was taken.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
        return Holder::Nobody;
    }
    match std::io::Error::last_os_error() {
        err if err.raw_os_error() == Some(libc::EWOULDBLOCK) => match pid {
            Some(pid) => Holder::Pid(pid),
            None => Holder::Unknown(format!(
                "{} is held by a process that has not written its pid",
                lock.display()
            )),
        },
        err => Holder::Unknown(format!("{}: {err}", lock.display())),
    }
}

/// The installed service whose host directory `host` is, and its process
/// as the service manager (or, for `up`, its current report) names it:
/// `Ok(None)` when no service serves `host`, `Err` when which one does
/// cannot be told (check 10 says why).
fn service_of(doctor: &Doctor, host: &Path) -> Result<Option<(Role, Option<u32>)>, &'static str> {
    let cx = doctor.cx;
    let role = match cx.installed()[..] {
        [] => return Ok(None),
        [role] => role,
        _ => return Err("more than one service is installed"),
    };
    let Some(data) = service::read_command_line(cx, role).and_then(|argv| service::data_dir_of(&argv)) else {
        return Err("the service's command line cannot be read");
    };
    let served = match role {
        Role::Up => data.join("host"),
        Role::Host => data.clone(),
        Role::Collector => return Ok(None),
    };
    if canonical(&served) != canonical(host) {
        return Ok(None);
    }
    let mut pid = service::managed(cx, role).ok().flatten().and_then(|m| m.pid);
    if role == Role::Up
        && pid.is_none()
        && let Report::Current(state) = service::report(&data, None)
    {
        pid = Some(state.pid);
    }
    Ok(Some((role, pid)))
}

/// Check 14 (distribution spec §8): one host on the directory, and that
/// one the service's; and the directory and its key private (by their
/// modes; the key is never read).
pub fn host_directory(doctor: &Doctor) -> Finding {
    let Some(host) = &doctor.dirs.host else {
        return Finding::NotRun {
            number: 14,
            why: NO_HOST,
        };
    };
    let mut verdict = Verdict::default();
    if std::fs::metadata(host).is_ok_and(|m| m.mode() & 0o077 != 0) {
        verdict.warn(
            format!("{} can be read by other users", host.display()),
            format!("run `chmod 700 {}`", host.display()),
        );
    }
    let key = host.join(hennery_host::identity::KEY_FILE);
    // The key's own mode: a link to it is followed.
    if std::fs::metadata(&key).is_ok_and(|m| m.mode() & 0o077 != 0) {
        verdict.warn(
            format!("{} can be read by other users", key.display()),
            format!(
                "run `chmod 600 {}`, and pair again if it may have leaked",
                key.display()
            ),
        );
    }
    let lock = host.join(crate::lock::HOST_LOCK);
    match holder(doctor, &lock) {
        Holder::Nobody => verdict.ok("no host runs on it"),
        Holder::Unknown(why) => verdict.warn(
            format!("whether a host runs on it is unknown: {why}"),
            "see `hennery service status`, and the processes named hennery",
        ),
        Holder::Pid(pid) => match service_of(doctor, host) {
            Err(why) => verdict.warn(
                format!("pid {pid} serves it, and which service should is unknown: {why}"),
                "see check 10, and keep one service with a command line hennery wrote",
            ),
            Ok(None) => verdict.ok(format!("pid {pid} serves it: a host started by hand")),
            Ok(Some((Role::Host, Some(service)))) if service == pid => {
                verdict.ok(format!("the host service (pid {pid}) serves it"))
            }
            Ok(Some((Role::Up, Some(up)))) if process::parent(doctor.cx, pid) == Some(up) => {
                verdict.ok(format!("up's host (pid {pid}) serves it"))
            }
            // Which process the service runs, or whose child the holder is,
            // cannot be read (no user bus, `/proc` mounted `hidepid`): never
            // tell the operator to stop what may be the service's own host.
            Ok(Some((role, None))) => verdict.warn(
                format!("pid {pid} serves it; whether it is the {role} service's host is unknown"),
                "see check 10: the service manager could not say which process it runs",
            ),
            Ok(Some((Role::Up, Some(_)))) if process::parent(doctor.cx, pid).is_none() => verdict.warn(
                format!("pid {pid} serves it; whether it is up's host is unknown: its parent cannot be read"),
                "check by hand that its parent is the up service's process",
            ),
            Ok(Some((role, _))) => verdict.warn(
                format!("pid {pid}, not the {role} service's host, serves it: the service's host cannot start"),
                format!("stop pid {pid} (`kill {pid}`), and let the service run its host"),
            ),
        },
    }
    Finding::Checked(verdict.check(14, "host data directory"))
}
