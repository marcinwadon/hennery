//! `/healthz` and `/readyz` (kernel spec §8): whether the process is up, and
//! whether its database answers. For a service manager or a load balancer:
//! they need no session, sit outside the browser rules (kernel spec §3.3),
//! and carry no data, only a fixed word.

use crate::operator::Operator;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use std::sync::Arc;

pub fn router(operator: Arc<Operator>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .with_state(operator)
}

/// 200 `ready` when the database answers a query, 503 `not ready` when it
/// does not (the error is logged, not shown).
async fn ready(State(operator): State<Arc<Operator>>) -> (StatusCode, &'static str) {
    match operator.ping() {
        Ok(()) => (StatusCode::OK, "ready"),
        Err(err) => {
            tracing::warn!(error = %err, "readyz: the database does not answer");
            (StatusCode::SERVICE_UNAVAILABLE, "not ready")
        }
    }
}
