//! Check 18: the collector's secret files, `vapid.key` (kernel spec §6)
//! and `master.key` (gateway spec), as the collector would take them: a
//! regular file, not a link, with no group or other bits, owned by the
//! collector directory's owner and readable by them, `master.key` with one
//! name only, and 32 bytes, in the order the collector looks. Judged by
//! their metadata, never opened: doctor reads no secret. What the collector
//! refuses fails here, before a start does.

use super::{Doctor, Finding, Verdict};
use hennery_gateway::key::{CREDENTIAL_NAME, GIVE_UP, KEY_ENV};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// The size of either key: a raw P-256 secret, or the gateway's 32-byte
/// master key.
pub const KEY_BYTES: u64 = 32;

/// What a secret file is, from its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Secret {
    /// Not there.
    Absent,
    /// It could not be looked at (a directory doctor may not search).
    Unknown(String),
    /// A symbolic link: the collector refuses to follow it.
    Link,
    /// Not a regular file (a directory, a FIFO).
    NotAFile,
    /// Group or others can read or change it.
    Shared,
    /// Owned by another user than the collector directory's.
    OtherOwner(u32),
    /// Its owner cannot read it, so a collector not run as root cannot.
    Unreadable,
    /// More than one name for it.
    HardLinked,
    /// Not `KEY_BYTES` long.
    WrongSize(u64),
    Fine,
}

/// `path` judged by its own metadata (a link is not followed), for a
/// collector whose directory `owner` owns. Only `hard_links_matter` files
/// are refused for a second name.
pub fn judge(path: &Path, owner: u32, hard_links_matter: bool) -> Secret {
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Secret::Absent,
        Err(err) => return Secret::Unknown(err.to_string()),
    };
    if meta.file_type().is_symlink() {
        Secret::Link
    } else if !meta.file_type().is_file() {
        Secret::NotAFile
    } else if meta.mode() & 0o077 != 0 {
        Secret::Shared
    } else if meta.uid() != owner {
        Secret::OtherOwner(meta.uid())
    } else if meta.mode() & 0o400 == 0 {
        Secret::Unreadable
    } else if hard_links_matter && meta.nlink() != 1 {
        Secret::HardLinked
    } else if meta.len() != KEY_BYTES {
        Secret::WrongSize(meta.len())
    } else {
        Secret::Fine
    }
}

/// Check 18.
pub fn secret_files(doctor: &Doctor) -> Finding {
    let Some(collector) = &doctor.dirs.collector else {
        return Finding::NotRun {
            number: 18,
            why: "no collector data directory",
        };
    };
    let mut verdict = Verdict::default();
    let owner = match collector.metadata() {
        Ok(meta) => meta.uid(),
        Err(err) => {
            verdict.warn(
                format!("{} cannot be looked at: {err}", collector.display()),
                "run doctor as the collector's user",
            );
            return Finding::Checked(verdict.check(18, "secret files"));
        }
    };
    // A collector that has run keeps its database: a key missing then is
    // one that was lost, not one not made yet.
    let has_run = collector.join("hennery.db").symlink_metadata().is_ok();

    let vapid = collector.join(hennery_kernel::push::VAPID_KEY_FILE);
    match judge(&vapid, owner, false) {
        Secret::Absent if has_run => verdict.warn(
            format!(
                "{} is missing, though the collector has run: its next start makes a new one, and every push subscription made with the old one stops working",
                vapid.display()
            ),
            "restore vapid.key from a backup before the collector starts again; without one, every device must subscribe again",
        ),
        Secret::Absent => verdict.ok("no vapid.key yet: the collector makes it on its first start"),
        Secret::Fine => verdict.ok("vapid.key is private"),
        bad => refused(
            &mut verdict,
            &vapid,
            bad,
            "every push subscription depends on it: restore it from a backup, or remove it and subscribe every device again",
            "remove it and subscribe every device again",
        ),
    }

    let master = collector.join(hennery_gateway::key::KEY_FILE);
    match judge(&master, owner, true) {
        Secret::Absent if has_run => verdict.warn(
            format!(
                "{} is missing: if the gateway has stored credentials, the collector refuses to start until it is restored, or supplied by {KEY_ENV} or the systemd credential {CREDENTIAL_NAME}",
                master.display()
            ),
            format!("restore master.key from a backup, or supply the key through {KEY_ENV}. {GIVE_UP}"),
        ),
        Secret::Absent => verdict.ok(format!(
            "no master.key: the gateway makes one on its first start, unless {KEY_ENV} or the systemd credential {CREDENTIAL_NAME} supplies it"
        )),
        Secret::Fine => verdict.ok("master.key is private"),
        Secret::WrongSize(64) => refused(
            &mut verdict,
            &master,
            Secret::WrongSize(64),
            &format!(
                "64 bytes may be the key written as hex: supply it through {KEY_ENV} instead, or restore the file from a backup. {GIVE_UP}"
            ),
            GIVE_UP,
        ),
        bad => refused(
            &mut verdict,
            &master,
            bad,
            &format!(
                "the gateway's stored credentials open only with it: restore it from a backup (unless {KEY_ENV} or the credential supplies the key: then this file is unused). {GIVE_UP}"
            ),
            GIVE_UP,
        ),
    }
    Finding::Checked(verdict.check(18, "secret files"))
}

/// A failure for `path`, which the collector refuses as `bad` is; `lost`
/// says what restoring it means, `leaked` what to do if it may have leaked.
fn refused(verdict: &mut Verdict, path: &Path, bad: Secret, lost: &str, leaked: &str) {
    let path = path.display();
    let (summary, fix) = match bad {
        Secret::Unknown(err) => {
            verdict.warn(
                format!("{path} cannot be looked at: {err}"),
                "run doctor as the collector's user",
            );
            return;
        }
        Secret::Link => (
            format!("{path} is a symbolic link, which the collector refuses"),
            format!("put the file itself at {path}, mode 600"),
        ),
        Secret::NotAFile => (
            format!("{path} is not a regular file"),
            format!("put the key file itself at {path}; {lost}"),
        ),
        Secret::Shared => (
            format!("{path} can be read or changed by other users, so the collector will not start"),
            format!("run `chmod 600 {path}`. If it may have leaked: {leaked}"),
        ),
        Secret::OtherOwner(uid) => (
            format!("{path} belongs to uid {uid}, not to the collector directory's owner"),
            "make the collector's user its owner (`chown`), mode 600".to_string(),
        ),
        Secret::Unreadable => (
            format!("{path} cannot be read by its owner, so the collector cannot open it (unless it runs as root)"),
            format!("run `chmod 600 {path}`"),
        ),
        Secret::HardLinked => (
            format!(
                "{path} has other hard links, a second way to read or replace it (and no backup: they are the same file)"
            ),
            format!("find them with `find <its filesystem> -xdev -samefile {path}`, and remove all but this one"),
        ),
        Secret::WrongSize(bytes) => (
            format!("{path} holds {bytes} bytes, not {KEY_BYTES}: it is not a key the collector takes"),
            lost.to_string(),
        ),
        Secret::Absent | Secret::Fine => return,
    };
    verdict.fail(summary, fix);
}
