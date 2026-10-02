//! The checks on this process's own environment: the agent nesting
//! variables (check 6) and which `hennery` PATH finds first (check 11).

use super::{Doctor, Finding, Verdict};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Check 6 (distribution spec §6.1): a variable an agent session sets, in
/// the environment doctor runs in. The host strips them at every adapter
/// spawn, but a shell that has them is an agent's, and a service installed
/// from it is not. Only the names are printed, never the values. Also run
/// as root, doctor would examine root's services and data, not the user's.
pub fn environment(doctor: &Doctor) -> Finding {
    let set: Vec<&str> = hennery_host::adapter::NESTING_VARS
        .iter()
        .copied()
        .filter(|var| doctor.cx.env.contains_key(*var))
        .collect();
    let mut verdict = Verdict::default();
    if set.is_empty() {
        verdict.ok("no agent nesting variable is set");
    } else {
        verdict.warn(
            format!("{} set: this shell runs inside an agent session", set.join(", ")),
            "unset them, or run hennery from a shell outside any agent session",
        );
    }
    if doctor.cx.uid == 0 {
        verdict.warn(
            "run as root: this examines root's services and data",
            "run hennery doctor as the user who runs hennery",
        );
    }
    Finding::Checked(verdict.check(6, "environment"))
}

/// Check 11 (distribution spec §11): the `hennery` PATH finds first is not
/// this one, e.g. a Homebrew install and a curl install side by side.
pub fn hennery_on_path(doctor: &Doctor) -> Finding {
    let this = canonical(&doctor.cx.exe);
    let found: Vec<PathBuf> = doctor
        .cx
        .env
        .get("PATH")
        .map(|path| {
            std::env::split_paths(path)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join("hennery"))
                .filter(|candidate| executable(candidate))
                .collect()
        })
        .unwrap_or_default();
    let mut verdict = Verdict::default();
    match found.first() {
        None => verdict.warn(
            format!("no hennery is on PATH; this one is {}", doctor.cx.exe.display()),
            "add its directory to PATH, so `hennery` names this binary",
        ),
        Some(first) if canonical(first) != this => verdict.warn(
            format!(
                "`hennery` on PATH is {}, another install, not this one ({})",
                first.display(),
                doctor.cx.exe.display()
            ),
            "remove the install you do not use, or put this one's directory first on PATH",
        ),
        Some(first) => {
            let others: Vec<String> = found[1..]
                .iter()
                .filter(|p| canonical(p) != this)
                .map(|p| p.display().to_string())
                .collect();
            if others.is_empty() {
                verdict.ok(format!("{} is this binary", first.display()));
            } else {
                verdict.ok(format!(
                    "{} is this binary; later on PATH: {}",
                    first.display(),
                    others.join(", ")
                ));
            }
        }
    }
    Finding::Checked(verdict.check(11, "hennery on PATH"))
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
