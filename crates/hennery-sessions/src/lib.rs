//! Collector-side session module (ACP core spec §4, §8, §9).

pub mod api;
pub mod attachments;
pub mod content;
pub mod forget;
pub mod hats;
pub mod hosts;
pub mod hub;
pub mod notify;
pub mod offline;
pub mod projects;
pub mod push;
mod redact;
mod resolve;
mod shared_files;
pub mod store;
pub mod sweep;
pub mod ws;

use axum::Router;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::push::{Push, VapidKey};
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
    /// Cancelled on shutdown; long-lived handlers (host sockets, SSE) end
    /// when it fires so graceful shutdown completes.
    pub shutdown: CancellationToken,
    /// A host gone this long has its sessions presumed parked (ACP core §5.3).
    pub offline_threshold: Duration,
    /// Enumerations of each host's projects (ACP core §7).
    pub projects: Arc<projects::ProjectsCache>,
    /// How long a probe waits for its reply (ACP core §3.4).
    pub probe_timeout: Duration,
    /// The VAPID key pair (kernel spec §6). `new` makes one in memory; the
    /// collector replaces it with `<data>/vapid.key` before it serves.
    pub vapid: Arc<VapidKey>,
    /// Where push triggers send their notices (ACP core §10). `new` gives
    /// a queue nobody reads; the collector replaces it with one delivery
    /// drains.
    pub push: Push,
    /// How often the attachments are swept after the sweep at startup
    /// (plan 9b decision 9).
    pub sweep_interval: Duration,
    /// The host removals with an attempt in flight (plan 9d B7).
    pub forgets: Arc<forget::InFlight>,
}

impl AppState {
    pub fn new(store: store::Store, hosts: Hosts, operator: Operator) -> Self {
        Self {
            store: Arc::new(store),
            hosts: Arc::new(hosts),
            operator: Arc::new(operator),
            enroll_limiter: Arc::new(Limiter::new(Policy::ENROLL)),
            hub: Arc::new(hub::Hub::new()),
            shutdown: CancellationToken::new(),
            offline_threshold: offline::OFFLINE_THRESHOLD,
            projects: Arc::new(projects::ProjectsCache::new(projects::CACHE_TTL)),
            probe_timeout: projects::PROBE_TIMEOUT,
            vapid: Arc::new(VapidKey::generate()),
            push: Push::detached(),
            sweep_interval: sweep::INTERVAL,
            forgets: Arc::new(forget::InFlight::default()),
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

    /// The session module's part of a hat's purge (plan 9c decision 10d):
    /// `hats::purge_sessions`, which the purge route calls itself for what
    /// it deleted, then the gateway's part (lane L6, plan 8e: its tokens,
    /// stdio servers and connections, with their open streams cut), and
    /// the one checkpoint the deletes owe. Each is idempotent: a purge that
    /// stopped runs both again.
    fn on_hat_purged(&self, hat_id: &str) -> anyhow::Result<()> {
        self.store.owe_checkpoint();
        let purged = hats::purge_sessions(self, hat_id)
            .map(drop)
            .and_then(|()| self.store.purge_gateway_hat(hat_id));
        self.store.checkpoint();
        purged
    }
}

/// Serve until `state.shutdown` is cancelled.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    serve_on(vec![listener], state).await
}

/// Serve `router(state)` on every listener until `state.shutdown` is
/// cancelled (kernel spec §7).
pub async fn serve_on(listeners: Vec<tokio::net::TcpListener>, state: AppState) -> std::io::Result<()> {
    offline::after_startup(&state);
    sweep::after_startup(&state);
    serve_all(listeners, router(state.clone()), state.shutdown.clone()).await
}

/// Serve `app` on every listener, the same router and the same state on
/// each (kernel spec §7), each with its peer's address: enrollment and
/// login rate-limit on it. Until `shutdown` is cancelled; a listener that
/// fails cancels it for the others too.
pub async fn serve_all(
    listeners: Vec<tokio::net::TcpListener>,
    app: Router,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let mut serving = tokio::task::JoinSet::new();
    for listener in listeners {
        let app = app.clone().into_make_service_with_connect_info::<SocketAddr>();
        let shutdown = shutdown.clone();
        serving.spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
    }
    let mut result = Ok(());
    while let Some(done) = serving.join_next().await {
        if let Err(err) = done.map_err(std::io::Error::other).and_then(|served| served) {
            shutdown.cancel();
            if result.is_ok() {
                result = Err(err);
            }
        }
    }
    result
}

/// Every session, host and operator route, the health checks, the host
/// WebSocket and, for every other path, the web UI. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`: enrollment reads
/// the client's address.
pub fn router(state: AppState) -> Router {
    api::router(state.clone())
        .merge(hosts::router(state.clone()))
        .merge(hats::router(state.clone()))
        .merge(projects::router(state.clone()))
        .merge(push::router(state.clone()))
        .merge(hennery_kernel::auth_api::router(state.operator.clone()))
        .merge(hennery_kernel::capabilities::router(state.operator.clone()))
        .merge(hennery_kernel::health::router(state.operator.clone()))
        .merge(ws::router(state))
        // Outside every route's layers: the app holds no data (`web.rs`).
        .fallback(hennery_kernel::web::serve)
        // Last, so it covers every route above and the fallback (kernel
        // spec §7.2).
        .layer(axum::middleware::map_response(hennery_kernel::csp::on_html))
}
