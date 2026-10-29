//! The `serve` command's router: MCP, the optional UI and client attribution.
use crate::{http, store::Store, ui};
use anyhow::Result;
use axum::{Extension, Router, middleware};
use hennery_kernel::{
    client_address::{ProxyPolicy, resolve_request},
    operator::Operator,
};
use std::sync::Arc;

/// Serve with `ConnectInfo<SocketAddr>`. The collector's exact-peer policy
/// resolves every request's address before routing: the default trusts no
/// proxy header, so every limiter is keyed by the TCP peer.
pub fn app(store: Arc<Store>, operator: Option<Arc<Operator>>, policy: ProxyPolicy) -> Result<Router> {
    let mut router = http::router(store.clone());
    if let Some(operator) = operator {
        router = router.merge(ui::router(store, operator)?);
    }
    Ok(router
        .layer(middleware::from_fn(resolve_request))
        .layer(Extension(policy)))
}
