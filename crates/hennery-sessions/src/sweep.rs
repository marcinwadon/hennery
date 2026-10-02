//! The orphan sweep (plan 9b decision 9, A14).
//!
//! Images outlive what showed them: a turn abandoned or never delivered,
//! a prompt that lost its race for the turn (plan 6a), and a session
//! delete that crashed between its commit and its files (plan 9a decision
//! 7). A crashed write leaves a `.tmp` file (plan 6a decision 7). When the
//! collector starts and then every `AppState::sweep_interval` (`INTERVAL`,
//! hourly), `Store::sweep_attachments` deletes the owner's rows nothing of
//! theirs shows, then the files no row of any owner names and the `.tmp`
//! files, each only once its mtime is older than `GRACE`.

use crate::AppState;
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

/// How old a file must be before a sweep removes it: an image is saved
/// before its turn records it (`save_images`, then `open_prompt`), and a
/// `.tmp` file may be a write still running.
pub const GRACE: Duration = Duration::from_secs(60 * 60);

/// How often the collector sweeps, after its sweep at startup: the
/// default of `AppState::sweep_interval`.
pub const INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Called once when the collector starts: sweep now, then every
/// `state.sweep_interval`, until `state.shutdown`, which also stops a
/// sweep that is running before its next batch. A sweep that fails is
/// logged, and the next one runs as planned.
pub fn after_startup(state: &AppState) -> tokio::task::JoinHandle<()> {
    let state = state.clone();
    tokio::spawn(async move {
        loop {
            let (store, cancel) = (state.store.clone(), state.shutdown.clone());
            let swept =
                tokio::task::spawn_blocking(move || store.sweep_attachments(std::time::SystemTime::now(), &cancel))
                    .await;
            match swept {
                Ok(Ok(report)) => tracing::info!(
                    rows = report.rows,
                    files = report.files,
                    temps = report.temps,
                    "attachments swept"
                ),
                Ok(Err(err)) => tracing::error!("the attachment sweep failed: {err:#}"),
                Err(err) => tracing::error!("the attachment sweep panicked: {err}"),
            }
            tokio::select! {
                _ = tokio::time::sleep(state.sweep_interval) => {}
                _ = state.shutdown.cancelled() => return,
            }
        }
    })
}

/// What `sweep_file` removed.
pub(crate) enum Swept {
    Image,
    Temp,
}

/// Remove `name` from `dir` if it is still a regular file older than the
/// grace at `now` and, for an image, no row of any owner names it (A6);
/// under the store's lock (`Store::sweep_attachments`).
pub(crate) fn sweep_file(
    conn: &Connection,
    dir: &Path,
    name: &str,
    now: std::time::SystemTime,
) -> Result<Option<Swept>> {
    let path = dir.join(name);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if !meta.file_type().is_file() {
        return Ok(None);
    }
    // An mtime ahead of `now` is young.
    let old = now.duration_since(meta.modified()?).is_ok_and(|age| age > GRACE);
    if !old {
        return Ok(None);
    }
    let swept = if crate::attachments::is_sha256(name) {
        if crate::shared_files::hash_named_by_any_owner(conn, name)? {
            return Ok(None);
        }
        Swept::Image
    } else {
        Swept::Temp
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Some(swept)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}
