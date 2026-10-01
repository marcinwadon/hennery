//! Collector-side session module (ACP core spec §4, §8, §9).

pub mod api;
pub mod hosts;
pub mod hub;
pub mod offline;
pub mod store;
pub mod ws;

use axum::Router;
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
    /// Cancelled on shutdown; long-lived handlers (host sockets, SSE) end
    /// when it fires so graceful shutdown completes.
    pub shutdown: CancellationToken,
    /// A host gone this long has its sessions presumed parked (ACP core §5.3).
    pub offline_threshold: Duration,
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
    serve_on(vec![listener], state).await
}

/// Serve `router(state)` on every listener until `state.shutdown` is
/// cancelled (kernel spec §7).
pub async fn serve_on(listeners: Vec<tokio::net::TcpListener>, state: AppState) -> std::io::Result<()> {
    offline::after_startup(&state);
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
/// WebSocket and the placeholder page. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`: enrollment reads
/// the client's address.
pub fn router(state: AppState) -> Router {
    api::router(state.clone())
        .merge(hosts::router(state.clone()))
        .merge(hennery_kernel::auth_api::router(state.operator.clone()))
        .merge(hennery_kernel::health::router(state.operator.clone()))
        .merge(ws::router(state))
        .route("/", axum::routing::get(|| async { axum::response::Html(PLACEHOLDER) }))
        // Last, so it covers every route above (kernel spec §7.2).
        .layer(axum::middleware::map_response(hennery_kernel::csp::on_html))
}

/// `GET /` until the frontend is built.
const PLACEHOLDER: &str = "<!doctype html><meta charset=utf-8><title>hennery</title><h1>hennery</h1><p>Walking skeleton. The UI is not built yet.</p>";
