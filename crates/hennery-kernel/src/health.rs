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
use std::time::Duration;

/// `/readyz` answers 503 when the database has not answered by then.
pub const READY_TIMEOUT: Duration = Duration::from_secs(2);

pub fn router(operator: Arc<Operator>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .with_state(operator)
}

/// 200 `ready` when the database answers a query within `READY_TIMEOUT`,
/// 503 `not ready` when it does not (the error is logged, not shown). The
/// query runs on a blocking thread: a database held busy must not hold up
/// the runtime's workers, nor the probe past its timeout.
async fn ready(State(operator): State<Arc<Operator>>) -> (StatusCode, &'static str) {
    let ping = tokio::task::spawn_blocking(move || operator.ping());
    let why = match tokio::time::timeout(READY_TIMEOUT, ping).await {
        Ok(Ok(Ok(()))) => return (StatusCode::OK, "ready"),
        Ok(Ok(Err(err))) => format!("{err:#}"),
        Ok(Err(err)) => err.to_string(),
        Err(_) => format!("no answer within {READY_TIMEOUT:?}"),
    };
    tracing::warn!(error = %why, "readyz: the database does not answer");
    (StatusCode::SERVICE_UNAVAILABLE, "not ready")
}
