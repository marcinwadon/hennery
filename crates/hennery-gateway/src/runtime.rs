//! What the gateway's routes, its proxy and its probe share, built once per
//! collector (plan 8f decision 3): the stores, the master key, the
//! collector's one egress policy, the `Notifier`, the OAuth flows in
//! flight, and one refresh lock per connection. One `Arc<Runtime>` goes to
//! both the API's state and the proxy's: two would be two locks, and no
//! refresh would be single-flight.

use crate::flows::Flows;
use crate::key::MasterKey;
use crate::model::StatusChange;
use crate::notify::{self, Notifier};
use crate::scope::ProxyStore;
use crate::store::GatewayStore;
use hennery_kernel::egress::{Allowance, Egress, EgressClient};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OwnedMutexGuard;

/// How long a started refresh may take at the vendor (gateway spec §4.5).
pub const REFRESH_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Runtime {
    pub store: Arc<GatewayStore>,
    /// Session tokens, connections in scope, and status writes.
    pub statuses: Arc<ProxyStore>,
    pub key: Arc<MasterKey>,
    /// The collector's one egress policy, shared with Web Push.
    pub egress: Egress,
    pub notifier: Arc<dyn Notifier>,
    pub flows: Flows,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub(crate) probes: crate::probe::Probes,
    pub(crate) refresh_timeout: Duration,
    pub(crate) probe_timeout: Duration,
}

impl Runtime {
    pub fn new(
        store: Arc<GatewayStore>,
        statuses: Arc<ProxyStore>,
        key: Arc<MasterKey>,
        egress: Egress,
        notifier: Arc<dyn Notifier>,
    ) -> Self {
        Self {
            store,
            statuses,
            key,
            egress,
            notifier,
            flows: Flows::default(),
            locks: Mutex::default(),
            probes: crate::probe::Probes::default(),
            refresh_timeout: REFRESH_TIMEOUT,
            probe_timeout: crate::probe::PROBE_TIMEOUT,
        }
    }

    /// The refresh's bound, shorter for a test.
    pub fn with_refresh_timeout(mut self, timeout: Duration) -> Self {
        self.refresh_timeout = timeout;
        self
    }

    /// The probe's bound, shorter for a test.
    pub fn with_probe_timeout(mut self, timeout: Duration) -> Self {
        self.probe_timeout = timeout;
        self
    }

    /// The connection's refresh lock (gateway spec §4.5): every refresh and
    /// every credential write of the connection holds it.
    pub async fn lock(&self, connection_id: &str) -> OwnedMutexGuard<()> {
        let lock = self
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(connection_id.to_string())
            .or_default()
            .clone();
        lock.lock_owned().await
    }

    /// The egress client for a connection's stored `internal_network` flag
    /// (plan 8d decision 9): the one place the choice is made.
    pub fn client(&self, internal_network: bool) -> EgressClient {
        self.egress.client(if internal_network {
            Allowance::InternalNetwork
        } else {
            Allowance::PublicOnly
        })
    }

    /// Tell the `Notifier` of a status write's transition, if it made one;
    /// a write that failed is logged (gateway spec §7).
    pub fn announce(&self, written: anyhow::Result<Option<StatusChange>>) {
        match written {
            Ok(Some(change)) => notify::announce(self.notifier.as_ref(), &change),
            Ok(None) => {}
            Err(err) => tracing::error!(error = %err, "gateway: a connection's status was not recorded"),
        }
    }

    /// A problem present at startup is announced once (gateway spec §7):
    /// every connection in `needs_auth` or `error` now.
    pub fn announce_startup(&self) -> anyhow::Result<()> {
        for change in self.statuses.problems()? {
            notify::announce(self.notifier.as_ref(), &change);
        }
        Ok(())
    }
}
