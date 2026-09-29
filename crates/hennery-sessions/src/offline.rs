//! Host offline → presumed park (ACP core §5.3).
//!
//! A host whose connection has been gone for `AppState::offline_threshold`
//! has its `active` sessions marked `parked` with `presumed_parked`, so the
//! operator is not offered a session that cannot answer. It is a
//! presumption, not a fact: the host may still be running them. Its open
//! turns stay open and nothing is revoked; the next handshake's
//! reconciliation turns each back into `active` (`reattached`) or, if the
//! host no longer has it, into a host restart.

use crate::AppState;

/// The default threshold (ACP core §11).
pub const OFFLINE_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Called when a host connection ends: presume its sessions parked if the
/// host has not connected again within the threshold.
pub fn after_disconnect(state: &AppState, host_id: String, conn_id: u64) {
    watch(state.clone(), host_id, Some(conn_id));
}

/// Called once when the collector starts: sessions it believes `active` on
/// a host that never connects get the same threshold (ACP core §5.4).
pub fn after_startup(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let hosts = match state.store.hosts_with_active_sessions() {
            Ok(hosts) => hosts,
            Err(err) => {
                tracing::error!(error = %err, "could not list hosts with active sessions");
                return;
            }
        };
        for host_id in hosts {
            watch(state.clone(), host_id, None);
        }
    });
}

/// After the threshold, presume `host_id`'s sessions parked unless it has
/// connected since `since` (the connection that dropped; `None` for "since
/// this collector started").
fn watch(state: AppState, host_id: String, since: Option<u64>) {
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(state.offline_threshold) => {}
            _ = state.shutdown.cancelled() => return,
        }
        let presumed = state
            .hub
            .if_offline_since(&host_id, since, || state.store.presume_parked(&host_id));
        match presumed {
            None => {}
            Some(Ok(events)) => {
                if !events.is_empty() {
                    tracing::info!(%host_id, sessions = events.len(), "host offline; sessions presumed parked");
                }
                for event in events {
                    state.hub.publish(event);
                }
            }
            Some(Err(err)) => tracing::error!(%host_id, error = %err, "could not presume sessions parked"),
        }
    });
}
