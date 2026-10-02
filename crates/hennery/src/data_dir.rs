//! Where a command's data directory is when none is given (distribution
//! spec §8): `--data-dir` and its variable win; else the commands that make
//! or run an install (`up`, `collector`) take the platform's default, and
//! those that act on one (`admin`, `host join`, `host run`, `host adapters`)
//! take the one installed service's directory, else the default, as doctor
//! looks. A default that holds another role's install is refused, saying
//! which directory to give, rather than written into.

use crate::doctor::dirs::{Dirs, Found};
use crate::service::Context;
use crate::service::unit::Role;
use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};

/// What a host command does with its directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostUse {
    /// `host join`: pairs it.
    Join,
    /// `host run`: runs its host.
    Run,
    /// `host adapters update` / `rollback`: changes its adapter set.
    Adapters,
}

/// What a directory holds, by the names in it (doctor's test; nothing is
/// created or opened).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holds {
    Nothing,
    /// A host's own files at its top.
    Host,
    /// A collector's own files at its top.
    Collector,
    /// `up`'s layout: `host/` or `collector/` in it.
    Up,
}

fn holds(dir: &Path) -> Holds {
    let dirs = Dirs::by_contents(dir.to_path_buf(), Found::Default);
    match (&dirs.host, &dirs.collector) {
        (None, None) => Holds::Nothing,
        (Some(host), None) if host == dir => Holds::Host,
        (None, Some(collector)) if collector == dir => Holds::Collector,
        _ => Holds::Up,
    }
}

/// `given`, else the platform's default, for `role` (`up` or `collector`):
/// refused when the default holds another role's install.
pub fn or_default(given: Option<PathBuf>, role: Role) -> Result<PathBuf> {
    match given {
        Some(dir) => Ok(dir),
        None => context(|cx| default_for(cx, role)),
    }
}

/// `given`, else the one installed collector's or `up`'s directory, else
/// the platform's default (`admin`).
pub fn collector_or_installed(given: Option<PathBuf>) -> Result<PathBuf> {
    match given {
        Some(dir) => Ok(dir),
        None => context(collector_dir),
    }
}

/// `given`, else the host directory the one installed service runs, else
/// the platform's default as a dedicated host's (`host …`).
pub fn host_or_installed(given: Option<PathBuf>, host_use: HostUse) -> Result<PathBuf> {
    match given {
        Some(dir) => Ok(dir),
        None => context(|cx| host_dir(cx, host_use)),
    }
}

fn context<T>(f: impl FnOnce(&Context) -> Result<T>) -> Result<T> {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        bail!("there is no default data directory on this platform: give one with --data-dir");
    }
    let system = crate::service::System;
    let cx = Context::from_process(&system).context("find the default data directory (or give one with --data-dir)")?;
    f(&cx)
}

/// The installed service's role and data directory, if one is; several
/// installed leave no one directory to take.
fn installed(cx: &Context) -> Result<Option<(Role, PathBuf)>> {
    match cx.installed()[..] {
        [] => Ok(None),
        [role] => Ok(crate::service::read_command_line(cx, role)
            .and_then(|argv| crate::service::data_dir_of(&argv))
            .map(|dir| (role, dir))),
        ref roles => bail!(
            "several hennery services are installed ({}), so which data directory to use is unclear: give one with \
             --data-dir, and keep one service (`hennery service uninstall --role …`)",
            roles.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// The message for a default directory `dir` that holds another role's
/// install than `wanted` (said as "the collector", "up", "a host").
fn taken(dir: &Path, holds: Holds, wanted: &str) -> anyhow::Error {
    let whose = match holds {
        Holds::Host => "a host's data directory",
        Holds::Collector => "a collector's data directory",
        Holds::Up => "hennery up's data directory",
        Holds::Nothing => "empty",
    };
    anyhow::anyhow!(
        "{} is {whose}: give {wanted} a directory of its own with --data-dir",
        dir.display()
    )
}

/// `up`'s or the collector's directory with none given.
pub fn default_for(cx: &Context, role: Role) -> Result<PathBuf> {
    let default = cx.default_data_dir();
    match (role, holds(&default)) {
        (_, Holds::Nothing) | (Role::Up, Holds::Up) | (Role::Collector, Holds::Collector) => Ok(default),
        (Role::Up, other) => Err(taken(&default, other, "up")),
        (_, other) => Err(taken(&default, other, "the collector")),
    }
}

/// `admin`'s directory with none given.
pub fn collector_dir(cx: &Context) -> Result<PathBuf> {
    Ok(match installed(cx)? {
        Some((Role::Up | Role::Collector, dir)) => dir,
        _ => cx.default_data_dir(),
    })
}

/// A host command's directory with none given.
pub fn host_dir(cx: &Context, host_use: HostUse) -> Result<PathBuf> {
    let up_runs_it = |dir: &Path| match host_use {
        HostUse::Join => bail!(
            "{} is hennery up's data directory: up pairs its own host. To pair this machine as another \
             host, give it a directory of its own with --data-dir",
            dir.display()
        ),
        HostUse::Run => bail!(
            "{} is hennery up's data directory: up runs its host. To run another host here, give it a \
             directory of its own with --data-dir",
            dir.display()
        ),
        HostUse::Adapters => Ok(dir.join("host")),
    };
    let default = cx.default_data_dir();
    match installed(cx)? {
        Some((Role::Host, dir)) => return Ok(dir),
        Some((Role::Up, dir)) => return up_runs_it(&dir),
        // A collector service on the default: its directory, even before
        // the collector has written anything in it.
        Some((Role::Collector, dir)) if dir == default => return Err(taken(&dir, Holds::Collector, "the host")),
        _ => {}
    }
    match holds(&default) {
        Holds::Nothing | Holds::Host => Ok(default),
        Holds::Up => up_runs_it(&default),
        Holds::Collector => Err(taken(&default, Holds::Collector, "the host")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{Manager, Platform, Ran};
    use std::collections::BTreeMap;

    struct NoManager;

    impl Manager for NoManager {
        fn run(&self, program: &str, _: &[&str]) -> Result<Ran> {
            bail!("{program} is not run in these tests")
        }
    }

    /// A machine of `platform` whose home and XDG directories are in `dir`.
    fn machine_on<'a>(dir: &Path, platform: Platform, manager: &'a NoManager) -> Context<'a> {
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        Context {
            platform,
            env: BTreeMap::from([
                ("HOME".to_string(), home.display().to_string()),
                ("XDG_DATA_HOME".to_string(), dir.join("data").display().to_string()),
                ("XDG_CONFIG_HOME".to_string(), dir.join("config").display().to_string()),
            ]),
            home,
            uid: 501,
            user: "hennery-test".into(),
            shell: PathBuf::from("/bin/sh"),
            exe: PathBuf::from("/usr/bin/hennery"),
            root: dir.join("root"),
            manager,
        }
    }

    fn machine<'a>(dir: &Path, manager: &'a NoManager) -> Context<'a> {
        machine_on(dir, Platform::Linux, manager)
    }

    /// `role`'s unit installed on `cx`, on `data`.
    fn install(cx: &Context, role: Role, data: &Path) {
        let argv = crate::service::unit::command_line(role, &cx.exe, data).unwrap();
        let file = cx.service_file(role);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            crate::service::unit::systemd_unit(role, &argv, "/tmp/service.env").unwrap(),
        )
        .unwrap();
    }

    /// `dir` made to hold `what` (by name, as doctor tells).
    fn make(dir: &Path, what: Holds) {
        std::fs::create_dir_all(dir).unwrap();
        match what {
            Holds::Nothing => {}
            Holds::Host => std::fs::write(dir.join(hennery_host::identity::CONFIG_FILE), "").unwrap(),
            Holds::Collector => std::fs::write(dir.join("hennery.db"), "").unwrap(),
            Holds::Up => {
                std::fs::create_dir_all(dir.join("host")).unwrap();
                std::fs::write(dir.join("host").join(hennery_host::identity::CONFIG_FILE), "").unwrap();
            }
        }
    }

    #[test]
    fn the_default_is_the_platforms() {
        let dir = tempfile::tempdir().unwrap();
        let manager = NoManager;
        let linux = machine(dir.path(), &manager);
        assert_eq!(linux.default_data_dir(), dir.path().join("data/hennery"));
        let mac = machine_on(dir.path(), Platform::MacOs, &manager);
        assert_eq!(
            mac.default_data_dir(),
            dir.path().join("home/Library/Application Support/hennery")
        );
    }

    #[test]
    fn up_and_the_collector_refuse_a_default_holding_another_roles_install() {
        let cases = [
            (Role::Up, Holds::Nothing, None),
            (Role::Up, Holds::Up, None),
            (Role::Up, Holds::Host, Some("a host's data directory: give up")),
            (
                Role::Up,
                Holds::Collector,
                Some("a collector's data directory: give up"),
            ),
            (Role::Collector, Holds::Nothing, None),
            (Role::Collector, Holds::Collector, None),
            (
                Role::Collector,
                Holds::Host,
                Some("a host's data directory: give the collector"),
            ),
            (
                Role::Collector,
                Holds::Up,
                Some("hennery up's data directory: give the collector"),
            ),
        ];
        for (role, what, refused) in cases {
            let dir = tempfile::tempdir().unwrap();
            let manager = NoManager;
            let cx = machine(dir.path(), &manager);
            let default = cx.default_data_dir();
            if what != Holds::Nothing {
                make(&default, what);
            }
            match (default_for(&cx, role), refused) {
                (Ok(got), None) => assert_eq!(got, default),
                (Err(err), Some(why)) => {
                    let err = err.to_string();
                    assert!(
                        err.contains(why) && err.contains("--data-dir"),
                        "{role} {what:?}: {err}"
                    );
                }
                (got, _) => panic!("{role} on {what:?}: {got:?}"),
            }
        }
    }

    #[test]
    fn a_host_command_takes_the_default_as_a_dedicated_hosts_directory() {
        let dir = tempfile::tempdir().unwrap();
        let manager = NoManager;
        let cx = machine(dir.path(), &manager);
        let default = cx.default_data_dir();
        // Nothing there yet, or a host's already.
        for host_use in [HostUse::Join, HostUse::Run, HostUse::Adapters] {
            assert_eq!(host_dir(&cx, host_use).unwrap(), default);
        }
        make(&default, Holds::Host);
        for host_use in [HostUse::Join, HostUse::Run, HostUse::Adapters] {
            assert_eq!(host_dir(&cx, host_use).unwrap(), default);
        }
        assert_eq!(collector_dir(&cx).unwrap(), default);
    }

    #[test]
    fn ups_directory_is_refused_for_join_and_run_and_its_host_used_for_adapters() {
        let dir = tempfile::tempdir().unwrap();
        let manager = NoManager;
        let cx = machine(dir.path(), &manager);
        let default = cx.default_data_dir();
        make(&default, Holds::Up);
        let err = host_dir(&cx, HostUse::Join).unwrap_err().to_string();
        assert!(
            err.contains("up pairs its own host") && err.contains("--data-dir"),
            "{err}"
        );
        let err = host_dir(&cx, HostUse::Run).unwrap_err().to_string();
        assert!(err.contains("up runs its host") && err.contains("--data-dir"), "{err}");
        assert_eq!(host_dir(&cx, HostUse::Adapters).unwrap(), default.join("host"));

        // The same for an installed `up` service on a directory of its own;
        // and admin reaches its root (whose `collector/` holds the socket).
        let up = dir.path().join("up-data");
        install(&cx, Role::Up, &up);
        assert!(host_dir(&cx, HostUse::Join).is_err());
        assert!(host_dir(&cx, HostUse::Run).is_err());
        assert_eq!(host_dir(&cx, HostUse::Adapters).unwrap(), up.join("host"));
        assert_eq!(collector_dir(&cx).unwrap(), up);
    }

    #[test]
    fn the_installed_hosts_directory_wins_over_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let manager = NoManager;
        let cx = machine(dir.path(), &manager);
        let host = dir.path().join("elsewhere");
        install(&cx, Role::Host, &host);
        for host_use in [HostUse::Join, HostUse::Run, HostUse::Adapters] {
            assert_eq!(host_dir(&cx, host_use).unwrap(), host);
        }
        // A host service is not where admin looks.
        assert_eq!(collector_dir(&cx).unwrap(), cx.default_data_dir());
    }

    #[test]
    fn a_collectors_directory_is_refused_for_a_host() {
        let dir = tempfile::tempdir().unwrap();
        let manager = NoManager;
        let cx = machine(dir.path(), &manager);
        let default = cx.default_data_dir();
        make(&default, Holds::Collector);
        for host_use in [HostUse::Join, HostUse::Run, HostUse::Adapters] {
            let err = host_dir(&cx, host_use).unwrap_err().to_string();
            assert!(err.contains("a collector's data directory"), "{err}");
        }
        let collector = dir.path().join("collector-data");
        install(&cx, Role::Collector, &collector);
        assert_eq!(collector_dir(&cx).unwrap(), collector);
    }

    /// A collector service installed on the default directory, before the
    /// collector has written anything there: still the collector's.
    #[test]
    fn an_installed_collector_on_the_default_is_refused_for_a_host_even_empty() {
        let dir = tempfile::tempdir().unwrap();
        let manager = NoManager;
        let cx = machine(dir.path(), &manager);
        install(&cx, Role::Collector, &cx.default_data_dir());
        for host_use in [HostUse::Join, HostUse::Run, HostUse::Adapters] {
            let err = host_dir(&cx, host_use).unwrap_err().to_string();
            assert!(err.contains("a collector's data directory"), "{err}");
        }
    }

    #[test]
    fn several_installed_services_leave_no_default_for_admin_or_a_host() {
        let dir = tempfile::tempdir().unwrap();
        let manager = NoManager;
        let cx = machine(dir.path(), &manager);
        install(&cx, Role::Host, &dir.path().join("h"));
        install(&cx, Role::Collector, &dir.path().join("c"));
        let err = collector_dir(&cx).unwrap_err().to_string();
        assert!(
            err.contains("several hennery services") && err.contains("--data-dir"),
            "{err}"
        );
        assert!(host_dir(&cx, HostUse::Join).is_err());
    }

    #[test]
    fn a_given_directory_is_taken_as_it_is() {
        let given = PathBuf::from("/given/dir");
        for role in [Role::Up, Role::Collector] {
            assert_eq!(or_default(Some(given.clone()), role).unwrap(), given);
        }
        assert_eq!(collector_or_installed(Some(given.clone())).unwrap(), given);
        for host_use in [HostUse::Join, HostUse::Run, HostUse::Adapters] {
            assert_eq!(host_or_installed(Some(given.clone()), host_use).unwrap(), given);
        }
    }
}
