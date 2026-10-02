//! Check 9 (distribution spec §7, §8): the host's disk. Room for the next
//! adapter set beside the current one, the outbox's size, and the outboxes
//! of earlier identities that a re-pair moved aside. "Recent transcript
//! gaps" waits for `transcript_gap` (ACP core §5.5, not built yet).

use super::dirs::NO_HOST;
use super::{Doctor, Finding, Verdict};
use hennery_host::outbox::FILE as OUTBOX;
use hennery_host::runtime::install::{self, Layout, SPACE_MARGIN};
use std::path::Path;

/// An outbox larger than this is named: the collector has not taken what
/// the host sent for a long while.
pub const LARGE_OUTBOX: u64 = 1 << 30;

/// Check 9.
pub fn disk(doctor: &Doctor) -> Finding {
    let Some(host) = &doctor.dirs.host else {
        return Finding::NotRun {
            number: 9,
            why: NO_HOST,
        };
    };
    let mut verdict = Verdict::default();
    // A host given its agents by `--agent` installs no set.
    let needed = if doctor.agents_given() { None } else { next_set(host) };
    space(&mut verdict, host, install::free_space(host), needed);
    let outbox = size(&host.join(OUTBOX)) + size(&host.join(format!("{OUTBOX}-wal")));
    if outbox > LARGE_OUTBOX {
        verdict.warn(
            format!("the outbox holds {} MB the collector has not taken", outbox >> 20),
            "check that this host reaches its collector (`hennery service status`, the host's log)",
        );
    } else {
        verdict.ok(format!("outbox {} KB", outbox >> 10));
    }
    let orphaned = orphaned_outboxes(host);
    if !orphaned.is_empty() {
        verdict.warn(
            format!(
                "outboxes of earlier identities, never read again: {}",
                orphaned.join(", ")
            ),
            format!("remove them from {} once inspected", host.display()),
        );
    }
    Finding::Checked(verdict.check(9, "disk"))
}

/// The free space on `host`'s filesystem (`free`) against what the outbox
/// and the next set need (`needed`, when this platform has a pinned set).
pub fn space(verdict: &mut Verdict, host: &Path, free: anyhow::Result<u64>, needed: Option<u64>) {
    let room = format!("free space on the filesystem holding {}", host.display());
    match free {
        Err(err) => verdict.warn(
            format!("free space unknown: {err:#}"),
            "check the filesystem's free space by hand",
        ),
        Ok(free) if free < SPACE_MARGIN => {
            verdict.fail(format!("{} MB free: the outbox cannot grow", free >> 20), room)
        }
        Ok(free) => match needed {
            Some(needed) if free < needed => verdict.warn(
                format!(
                    "{} MB free; the next adapter set needs {} MB beside the current one",
                    free >> 20,
                    needed >> 20
                ),
                format!("{room} before `hennery host adapters update` or an upgrade"),
            ),
            _ => verdict.ok(format!("{} MB free", free >> 20)),
        },
    }
}

/// What installing this binary's pinned set would need now, the runtime
/// included if it is not there: the set beside the current one. Nothing
/// when it is current already: an update installs nothing then.
pub fn next_set(host: &Path) -> Option<u64> {
    let overrides = hennery_host::runtime::agents::cli_overrides(host).ok()?;
    let selection = install::Selection::pinned(&hennery_host::runtime::agents::skipped(&overrides)).ok()?;
    let layout = Layout::new(host).ok()?;
    if layout.current_id() == Some(selection.set_id()) {
        return None;
    }
    let with_node = !layout
        .runtimes()
        .join(selection.runtime_name())
        .join("bin/node")
        .exists();
    Some(selection.space_needed(with_node))
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// `outbox.db.orphaned-*` in `host`, by name, without their `-wal` and
/// `-shm` companions.
fn orphaned_outboxes(host: &Path) -> Vec<String> {
    let prefix = format!("{OUTBOX}.orphaned-");
    let mut names: Vec<String> = std::fs::read_dir(host)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix) && !name.ends_with("-wal") && !name.ends_with("-shm"))
        .collect();
    names.sort();
    names
}
