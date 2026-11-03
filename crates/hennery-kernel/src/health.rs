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
use tokio::sync::Semaphore;

/// `/readyz` answers 503 when the database has not answered by then.
pub const READY_TIMEOUT: Duration = Duration::from_secs(2);

pub fn router(operator: Arc<Operator>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .with_state(Health {
            operator,
            permits: Arc::new(Semaphore::new(2)),
        })
}
#[derive(Clone)]
struct Health {
    operator: Arc<Operator>,
    permits: Arc<Semaphore>,
}

/// 200 `ready` when the database answers a query within `READY_TIMEOUT`,
/// 503 `not ready` when it does not (the error is logged, not shown). The
/// query runs on a blocking thread: a database held busy must not hold up
/// the runtime's workers, nor the probe past its timeout.
async fn ready(State(health): State<Health>) -> (StatusCode, &'static str) {
    limited_ping(health.permits, move || health.operator.ping()).await
}
async fn limited_ping<F>(permits: Arc<Semaphore>, check: F) -> (StatusCode, &'static str)
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    let Ok(permit) = permits.try_acquire_owned() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "not ready");
    };
    let ping = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        check()
    });
    let why = match tokio::time::timeout(READY_TIMEOUT, ping).await {
        Ok(Ok(Ok(()))) => return (StatusCode::OK, "ready"),
        Ok(Ok(Err(err))) => format!("{err:#}"),
        Ok(Err(err)) => err.to_string(),
        Err(_) => format!("no answer within {READY_TIMEOUT:?}"),
    };
    tracing::warn!(error = %why, "readyz: the database does not answer");
    (StatusCode::SERVICE_UNAVAILABLE, "not ready")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_probe_holds_capacity_until_its_blocking_work_ends() {
        let permits = Arc::new(Semaphore::new(1));
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, hold) = std::sync::mpsc::channel();
        let probe = tokio::spawn(limited_ping(permits.clone(), move || {
            started.send(()).unwrap();
            hold.recv().unwrap();
            Ok(())
        }));
        waiting.await.unwrap();
        probe.abort();
        let _ = probe.await;
        assert_eq!(
            limited_ping(permits.clone(), || panic!("busy probe must not spawn work"))
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(permits.available_permits(), 0);
        release.send(()).unwrap();
        let permit = tokio::time::timeout(Duration::from_secs(5), permits.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
        assert_eq!(limited_ping(permits, || Ok(())).await.0, StatusCode::OK);
    }
}
