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
use std::time::Duration;

/// How old a file must be before a sweep removes it: an image is saved
/// before its turn records it (`save_images`, then `open_prompt`), and a
/// `.tmp` file may be a write still running.
pub const GRACE: Duration = Duration::from_secs(60 * 60);

/// How often the collector sweeps, after its sweep at startup: the
/// default of `AppState::sweep_interval`.
pub const INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Called once when the collector starts: sweep now, then every
/// `state.sweep_interval`, until `state.shutdown`. A sweep that fails is
/// logged, and the next one runs as planned.
pub fn after_startup(state: &AppState) -> tokio::task::JoinHandle<()> {
    let state = state.clone();
    tokio::spawn(async move {
        loop {
            let store = state.store.clone();
            let swept =
                tokio::task::spawn_blocking(move || store.sweep_attachments(std::time::SystemTime::now())).await;
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
