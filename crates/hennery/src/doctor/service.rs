//! The service's check (distribution spec §5.2, §6): the service itself
//! (check 10). The service manager is only asked (`print`, `is-active`,
//! `is-enabled`, `show`, `show-user`), and only its parsed answers are
//! kept: `launchctl print` lists the job's environment.

use super::{Doctor, Finding, Verdict, process};
use crate::service::{self, Platform, Report, unit::Role};
use std::path::{Path, PathBuf};

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
