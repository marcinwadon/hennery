//! Check 2 (distribution spec §1.1): whether this machine can run a host's
//! managed runtime. The binary is musl-static on Linux, so it cannot ask
//! glibc its version: it runs glibc's own loader with `--version`, and only
//! a GNU C library's banner counts; `/usr/bin/getconf` is asked only when
//! the loader says nothing, and never on NixOS, whose `getconf` is Nix's own
//! glibc's (decision 5). What is read is gathered apart from how it is
//! judged, so the judging is tested on fixtures.

use super::{Check, Doctor, Finding, Verdict};
use hennery_host::runtime::install;
use hennery_host::runtime::manifest::Platform;

/// The oldest glibc the managed Node and the Claude CLI run on.
pub const MIN_GLIBC: (u32, u32) = (2, 28);

/// The oldest Linux kernel Node 24 supports.
pub const MIN_KERNEL: (u32, u32) = (4, 18);

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
/// says (not on NixOS).
pub fn libc_behind(doctor: &Doctor, path: &str, nixos: bool) -> Libc {
    let root = &doctor.cx.root;
    if let Some(ran) = (doctor.run)(&root.join(path.trim_start_matches('/')), &["--version"]) {
        let said = format!("{}\n{}", ran.stdout, ran.stderr);
        match parse_loader(&said) {
            Libc::Unknown => {}
            known => return known,
        }
    }
    if nixos {
        return Libc::Unknown;
    }
    (doctor.run)(&root.join("usr/bin/getconf"), &["GNU_LIBC_VERSION"])
        .filter(|ran| ran.ok)
        .and_then(|ran| parse_getconf(&ran.stdout))
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
    if lower.contains("musl") {
        return Libc::Musl;
    }
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
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
    let nix_ld =
        "enable programs.nix-ld, or use the Nix-provided adapters (`hennery host join --no-runtime`, then `--agent`)";
    let musl = "run only the collector here, or the host on a Linux with glibc 2.28 or later";
    match facts.platform {
        None => bad(
            &mut verdict,
            format!("hennery's managed runtime does not support {}", facts.name),
            "run hosts on macOS on Apple silicon or on Linux (x86-64, arm64) with glibc; this machine can run the collector",
        ),
        Some(Platform::DarwinArm64) => verdict.ok("macOS on Apple silicon"),
        Some(platform) => {
            match (facts.loader, facts.libc) {
                (Some((path, false)), _) if facts.nixos => bad(
                    &mut verdict,
                    format!("NixOS without nix-ld: glibc's loader {path} is missing"),
                    nix_ld,
                ),
                (Some((path, false)), _) => bad(
                    &mut verdict,
                    format!("no glibc loader ({path}): a musl system, or one without glibc"),
                    musl,
                ),
                (_, Libc::NixStub) => bad(
                    &mut verdict,
                    "NixOS without nix-ld: glibc's loader is NixOS's stub".to_string(),
                    nix_ld,
                ),
                (_, Libc::Musl) => bad(&mut verdict, "glibc's loader is musl's".to_string(), musl),
                (_, Libc::Glibc(major, minor)) if (major, minor) < MIN_GLIBC => bad(
                    &mut verdict,
                    format!("glibc {major}.{minor} is older than 2.28"),
                    "upgrade to a distribution with glibc 2.28 or later",
                ),
                (_, Libc::Glibc(major, minor)) => verdict.ok(format!("{} with glibc {major}.{minor}", platform.key())),
                (_, Libc::Unknown) => verdict.warn(
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
