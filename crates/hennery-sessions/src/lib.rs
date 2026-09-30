//! Collector-side session module (ACP core spec §4, §8, §9).

pub mod api;
pub mod hosts;
pub mod hub;
pub mod offline;
pub mod store;
pub mod ws;

use axum::Router;
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::ratelimit::{Limiter, Policy};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<store::Store>,
    /// The kernel's host registry (kernel spec §4).
    pub hosts: Arc<Hosts>,
    /// The owner, their sessions and setup (kernel spec §3).
    pub operator: Arc<Operator>,
    /// Wrong pairing codes per client address (kernel spec §4.1).
    pub enroll_limiter: Arc<Limiter>,
    pub hub: Arc<hub::Hub>,
    pub token: DevToken,
    /// Cancelled on shutdown; long-lived handlers (host sockets, SSE) end
    /// when it fires so graceful shutdown completes.
    pub shutdown: CancellationToken,
    /// A host gone this long has its sessions presumed parked (ACP core §5.3).
    pub offline_threshold: Duration,
}

impl AppState {
    pub fn new(store: store::Store, hosts: Hosts, operator: Operator, token: DevToken) -> Self {
        Self {
            store: Arc::new(store),
            hosts: Arc::new(hosts),
            operator: Arc::new(operator),
            enroll_limiter: Arc::new(Limiter::new(Policy::ENROLL)),
            hub: Arc::new(hub::Hub::new()),
            token,
            shutdown: CancellationToken::new(),
            offline_threshold: offline::OFFLINE_THRESHOLD,
        }
    }
}

/// The session module's part of a host revoke (kernel spec §4.3, §5.5).
/// The revoke endpoint calls it once the host's connection is gone.
impl hennery_kernel::lifecycle::LifecycleHooks for AppState {
    fn on_host_revoked(&self, host_id: &str) -> anyhow::Result<()> {
        for event in self.store.revoke_host(host_id)? {
            self.hub.publish(event);
        }
        Ok(())
    }
}

/// Serve until `state.shutdown` is cancelled.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    offline::after_startup(&state);
    let shutdown = state.shutdown.clone();
    // The peer address is what enrollment rate-limits on (kernel spec §4.1).
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.cancelled_owned())
    .await
}

/// Every session, host and operator route plus the host WebSocket. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`: enrollment reads
/// the client's address.
pub fn router(state: AppState) -> Router {
    api::router(state.clone())
        .merge(hosts::router(state.clone()))
        .merge(hennery_kernel::auth_api::router(state.operator.clone()))
        .merge(ws::router(state))
}
