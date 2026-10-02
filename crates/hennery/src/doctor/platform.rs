//! Check 2 (distribution spec §1.1): whether this machine can run a host's
//! managed runtime. What its C library is, and what stops the runtime, are
//! the host's own (`hennery_host::runtime::glibc`): the host refuses to
//! install a set where this check fails it. What is read is gathered apart
//! from how it is judged, so the judging is tested on fixtures.

use super::{Check, Doctor, Finding, Verdict};
use hennery_host::runtime::glibc::{self, Said};
pub use hennery_host::runtime::glibc::{Libc, major_minor};
// The parsers' fixture tests live with the doctor's.
#[cfg(test)]
pub use hennery_host::runtime::glibc::{parse_getconf, parse_loader};
use hennery_host::runtime::install;
use hennery_host::runtime::manifest::Platform;

/// The oldest Linux kernel Node 24 supports.
pub const MIN_KERNEL: (u32, u32) = (4, 18);

/// What check 2 reads of the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// `<os>-<arch>`, as Rust names them.
    pub name: String,
    /// The managed runtime's platform, if it supports this one.
    pub platform: Option<Platform>,
    /// glibc's loader, and whether it is there (Linux only).
    pub loader: Option<(&'static str, bool)>,
    /// `/etc/NIXOS` is there.
    pub nixos: bool,
    pub libc: Libc,
    /// The kernel's release, as `uname` gives it (Linux only).
    pub kernel: Option<String>,
}

/// Read the facts of the machine: its files under `cx.root`, its programs
/// through `doctor.run`.
pub fn gather(doctor: &Doctor) -> Facts {
    let root = &doctor.cx.root;
    let platform = Platform::current();
    let nixos = root.join("etc/NIXOS").exists();
    let loader = platform
        .and_then(install::glibc_loader)
        .map(|path| (path, root.join(path.trim_start_matches('/')).exists()));
    let libc = match loader {
        Some((path, true)) => libc_behind(doctor, path, nixos),
        _ => Libc::Unknown,
    };
    Facts {
        name: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        platform,
        loader,
        nixos,
        libc,
        kernel: if cfg!(target_os = "linux") {
            kernel_release()
        } else {
            None
        },
    }
}

/// What the loader at `path` says it is, else what `/usr/bin/getconf`
/// says (not on NixOS; decision 5).
pub fn libc_behind(doctor: &Doctor, path: &str, nixos: bool) -> Libc {
    let root = &doctor.cx.root;
    let run = |program: &std::path::Path, args: &[&str]| {
        (doctor.run)(program, args).map(|ran| Said {
            ok: ran.ok,
            stdout: ran.stdout,
            stderr: ran.stderr,
        })
    };
    glibc::libc_behind(
        &root.join(path.trim_start_matches('/')),
        &root.join("usr/bin/getconf"),
        nixos,
        &run,
    )
}

/// `uname(2)`'s release.
fn kernel_release() -> Option<String> {
    // SAFETY: uname(2) into a zeroed local struct of the right type.
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut name) } != 0 {
        return None;
    }
    // SAFETY: on success `release` is a NUL-terminated string in `name`.
    let release = unsafe { std::ffi::CStr::from_ptr(name.release.as_ptr()) };
    Some(release.to_string_lossy().into_owned())
}

/// Check 2.
pub fn platform(doctor: &Doctor) -> Finding {
    let strict = doctor.dirs.host.is_some() && !doctor.agents_given();
    Finding::Checked(judge(&gather(doctor), strict))
}

/// Check 2's verdict on `facts`. What stops a managed runtime fails only
/// where a host needs one (`strict`): elsewhere, with no host here or with
/// agents given by `--agent`, it is a warning about this machine.
pub fn judge(facts: &Facts, strict: bool) -> Check {
    let mut verdict = Verdict::default();
    let bad = |verdict: &mut Verdict, summary: String, fix: &str| {
        if strict {
            verdict.fail(summary, fix);
        } else {
            verdict.warn(format!("{summary}: no managed runtime can run here"), fix);
        }
    };
    match facts.platform {
        None => bad(
            &mut verdict,
            format!("hennery's managed runtime does not support {}", facts.name),
            "run hosts on macOS on Apple silicon or on Linux (x86-64, arm64) with glibc; this machine can run the collector",
        ),
        Some(Platform::DarwinArm64) => verdict.ok("macOS on Apple silicon"),
        Some(platform) => {
            match (glibc::refusal(facts.loader, facts.nixos, facts.libc), facts.libc) {
                (Some((summary, fix)), _) => bad(&mut verdict, summary, fix),
                (None, Libc::Glibc(major, minor)) => {
                    verdict.ok(format!("{} with glibc {major}.{minor}", platform.key()))
                }
                (None, _) => verdict.warn(
                    format!("{}: glibc's version is unknown", platform.key()),
                    "check that glibc is 2.28 or later (`ldd --version`)",
                ),
            }
            match facts.kernel.as_deref().map(|k| (k, major_minor(k))) {
                Some((release, Some(version))) if version < MIN_KERNEL => bad(
                    &mut verdict,
                    format!("Linux {release} is older than 4.18"),
                    "upgrade the kernel to 4.18 or later",
                ),
                Some((release, Some(_))) => verdict.ok(format!("Linux {release}")),
                _ => verdict.warn(
                    "the kernel's release is unknown",
                    "check that `uname -r` says 4.18 or later",
                ),
            }
        }
    }
    verdict.check(2, "platform")
}
