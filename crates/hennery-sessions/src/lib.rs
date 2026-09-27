//! Collector-side session module (ACP core spec §4, §8, §9).

pub mod api;
pub mod hub;
pub mod store;
pub mod ws;

use axum::Router;
use hennery_kernel::auth::DevToken;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<store::Store>,
    pub hub: Arc<hub::Hub>,
    pub token: DevToken,
    /// Cancelled on shutdown; long-lived handlers (host sockets, SSE) end
    /// when it fires so graceful shutdown completes.
    pub shutdown: CancellationToken,
}

impl AppState {
    pub fn new(store: store::Store, token: DevToken) -> Self {
        Self {
            store: Arc::new(store),
            hub: Arc::new(hub::Hub::new()),
            token,
            shutdown: CancellationToken::new(),
        }
    }
}

/// Serve until `state.shutdown` is cancelled.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    let shutdown = state.shutdown.clone();
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

/// Every session route plus the host WebSocket.
pub fn router(state: AppState) -> Router {
    api::router(state.clone()).merge(ws::router(state))
}
