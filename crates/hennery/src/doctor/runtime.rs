//! The managed runtime's checks (distribution spec §3.2, §7): the binary and
//! the set it runs (check 1), the set against this binary's pin and a
//! rollback's hold (check 12), and the CLIs `--use-cli` put in place of the
//! bundled ones (check 17). Read from the host's data directory alone:
//! nothing here installs, collects or holds a set.

use super::dirs::NO_HOST;
use super::{Doctor, Finding, Verdict};
use hennery_host::runtime::agents::{self, CLI_VARS};
use hennery_host::runtime::install::{InstalledSet, Layout, Selection};
use hennery_host::runtime::manifest::{self, Manifest, Platform};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Why the runtime checks did not run on a platform without one.
pub const NO_RUNTIME: &str = "no managed runtime on this platform";

/// Whether a user other than `uid` (and root) can write to `path`: its
/// group or others may, or someone else owns it. Agents run what is there.
pub fn writable_by_others(path: &Path, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).is_ok_and(|m| m.mode() & 0o022 != 0 || (m.uid() != uid && m.uid() != 0))
}

/// The selection this binary pins for the host in `dir`, less the CLIs its
/// `host.toml` overrides: what `hennery host adapters update` would install.
fn pinned(dir: &Path) -> anyhow::Result<Selection> {
    let overrides = agents::cli_overrides(dir)?;
    Selection::pinned(&agents::skipped(&overrides))
}

/// Check 1: this binary, the manifest it embeds, and whether the current
/// set's files are there. Their digests are not checked: an install records
/// none (decision 4), so "present" is all that can be said.
pub fn binary_and_set(doctor: &Doctor) -> Finding {
    let mut verdict = Verdict::default();
    verdict.ok(format!(
        "hennery {}, adapter manifest {}",
        env!("CARGO_PKG_VERSION"),
        &Manifest::hash_of(manifest::EMBEDDED)[..12]
    ));
    if let Some(host) = &doctor.dirs.host {
        match Layout::new(host).and_then(|layout| layout.current()) {
            Ok(None) if doctor.agents_given() => verdict.ok("the service gives its agents with --agent"),
            Ok(None) => verdict.ok("no adapter set is installed (see check 12)"),
            Ok(Some(set)) => {
                set_present(&set, pinned(host).ok().as_ref(), &mut verdict);
                // What agents run: the set, every set, the runtime, its
                // `bin/` and Node itself.
                let bin = set.node.parent().unwrap_or(&set.node);
                let runtime = bin.parent().unwrap_or(bin);
                let sets = set.path.parent().unwrap_or(&set.path);
                for dir in [&set.path, sets, runtime, bin, &set.node] {
                    if writable_by_others(dir, doctor.cx.uid) {
                        verdict.warn(
                            format!(
                                "{} can be written by other users, who could change what agents run",
                                dir.display()
                            ),
                            format!("run `chmod go-w {}`", dir.display()),
                        );
                    }
                }
            }
            Err(err) => verdict.fail(
                format!("the current adapter set cannot be read: {err:#}"),
                "run `hennery host adapters update`",
            ),
        }
    }
    Finding::Checked(verdict.check(1, "binary and adapter set"))
}

/// Whether `set`'s Node, its adapters' entry files and, when it is the
/// pinned set, every package the manifest lists for it are in place.
fn set_present(set: &InstalledSet, pinned: Option<&Selection>, verdict: &mut Verdict) {
    let executable = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if !executable(&set.node) {
        verdict.fail(
            format!("adapter set {}: its Node ({}) is missing", set.id, set.node.display()),
            "run `hennery host adapters update`",
        );
        return;
    }
    let mut missing: Vec<String> = set
        .record
        .adapters
        .iter()
        .map(|(name, adapter)| set.path.join(name).join(&adapter.entry))
        .filter(|entry| !entry.is_file())
        .map(|entry| entry.display().to_string())
        .collect();
    let described = pinned.filter(|selection| selection.set_id() == set.id);
    if let Some(selection) = described {
        for adapter in &selection.adapters {
            for file in &adapter.files {
                let dir = set.path.join(&adapter.name).join(&file.path);
                if !dir.is_dir() {
                    missing.push(dir.display().to_string());
                }
            }
        }
    }
    if missing.is_empty() {
        let what = if described.is_some() {
            "Node and every pinned package are present"
        } else {
            "Node and each adapter's entry are present"
        };
        verdict.ok(format!(
            "adapter set {}: {what} (their contents are not verified)",
            set.id
        ));
    } else {
        verdict.fail(
            format!("adapter set {} is missing {}", set.id, missing.join(", ")),
            format!(
                "stop the host, remove {}, then run `hennery host adapters update`",
                set.path.display()
            ),
        );
    }
}

/// Check 12 (distribution spec §3.2): the installed set against the one this
/// binary pins (they differ after an upgrade, until the host's next start
/// or an update), and a rollback's hold.
pub fn adapter_set(doctor: &Doctor) -> Finding {
    let Some(host) = &doctor.dirs.host else {
        return Finding::NotRun {
            number: 12,
            why: NO_HOST,
        };
    };
    if Platform::current().is_none() {
        return Finding::NotRun {
            number: 12,
            why: NO_RUNTIME,
        };
    }
    let mut verdict = Verdict::default();
    let update = "run `hennery host adapters update`";
    let (layout, selection) = match Layout::new(host).and_then(|layout| Ok((layout, pinned(host)?))) {
        Ok(both) => both,
        Err(err) => {
            verdict.fail(format!("cannot tell: {err}"), "fix host.toml, or pair this host again");
            return Finding::Checked(verdict.check(12, "adapter set"));
        }
    };
    let pinned_id = selection.set_id();
    match layout.current_id() {
        None if doctor.agents_given() => {
            verdict.ok("the service gives its agents with --agent: no managed set is needed")
        }
        None => verdict.warn(
            "no adapter set is installed: a host started without `--agent` runs no agents",
            update,
        ),
        Some(id) if id != pinned_id => {
            verdict.warn(format!("set {id} is current; this binary pins {pinned_id}"), update)
        }
        Some(id) => verdict.ok(format!("the pinned set {id} is current")),
    }
    if layout.held() {
        verdict.warn(
            "a rollback holds this host on its set: a host start installs nothing",
            format!("{update} once the pinned set may be used again"),
        );
    }
    Finding::Checked(verdict.check(12, "adapter set"))
}

/// Check 17 (distribution spec §13, decision 5): each agent `host.toml`
/// runs with the operator's own CLI. Its version against the pin is 7d-ii's.
pub fn cli_overrides(doctor: &Doctor) -> Finding {
    let Some(host) = &doctor.dirs.host else {
        return Finding::NotRun {
            number: 17,
            why: NO_HOST,
        };
    };
    let mut verdict = Verdict::default();
    match agents::cli_overrides(host) {
        // `{err}`, not `{err:#}`: a parse error's detail quotes the file.
        Err(err) => verdict.fail(format!("{err}"), "fix host.toml's [cli] table"),
        Ok(overrides) if overrides.is_empty() => verdict.ok("every agent runs its bundled CLI"),
        Ok(overrides) => {
            for (agent, path) in overrides {
                if !CLI_VARS.iter().any(|(a, _)| *a == agent) {
                    verdict.warn(
                        format!("host.toml names a CLI for {agent}, which takes none; it is ignored"),
                        format!("remove {agent} from host.toml's [cli] table"),
                    );
                    continue;
                }
                match agents::check_cli(&path) {
                    Ok(path) => {
                        verdict.ok(format!(
                            "{agent} runs {} (your own CLI: the pin's guarantee does not cover it)",
                            path.display()
                        ));
                        super::agents::override_gap(doctor, host, &agent, &path, &mut verdict);
                        for p in [path.as_path(), path.parent().unwrap_or(&path)] {
                            if writable_by_others(p, doctor.cx.uid) {
                                verdict.warn(
                                    format!(
                                        "{} can be written by other users, who could change what {agent} runs",
                                        p.display()
                                    ),
                                    format!("run `chmod go-w {}`", p.display()),
                                );
                            }
                        }
                    }
                    Err(err) => verdict.fail(
                        format!("{agent} cannot start: its --use-cli CLI: {err:#}"),
                        format!(
                            "run `hennery host adapters update --use-cli {agent}=bundled`, or give a CLI that exists"
                        ),
                    ),
                }
            }
        }
    }
    Finding::Checked(verdict.check(17, "CLI overrides"))
}
