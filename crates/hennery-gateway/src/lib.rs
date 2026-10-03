//! The MCP gateway (gateway spec): connections, their credentials at rest,
//! the hosts they are mounted on, session tokens, and the proxy that
//! forwards a token's requests to its connections. Depends on
//! `hennery-kernel` and `hennery-proto`, never on `hennery-sessions`
//! (umbrella §9).

pub mod api;
pub mod callback;
pub mod crypto;
pub mod flows;
mod jsonrpc;
pub mod key;
pub mod model;
pub mod notify;
pub mod oauth;
mod oauth_api;
pub mod probe;
pub mod proxy;
pub mod refresh;
pub mod runtime;
mod schema;
pub mod scope;
pub mod store;
pub mod tokens;

use anyhow::Result;
use hennery_kernel::operator::Operator;
use std::path::Path;
use std::sync::Arc;

/// The master key could not be had: missing while credentials are stored,
/// unreadable, refused, or not the one that sealed them (plan 8a decision
/// 8). It is the context of every such error from `open`, so a caller can
/// tell it from a store that does not open. The collector refuses to start
/// on it today; serving with the gateway off (its routes 503, sessions
/// untouched) would match on it and serve a stand-in router instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the MCP gateway's master key is unavailable")]
pub struct KeyUnavailable;

/// The gateway on the collector's `hennery.db`: its stores, and the master
/// key from `keys`, made at the first start, sending through the
/// collector's one `egress` and telling `notifier` of status transitions.
/// Before serving, and after the admin socket's bind (the one-collector
/// guard). The state's `runtime` is the one the proxy takes too
/// (`proxy::ProxyState::for_sessions`).
pub fn open(
    db: &Path,
    keys: &key::KeySource,
    operator: Arc<Operator>,
    egress: hennery_kernel::egress::Egress,
    notifier: Arc<dyn notify::Notifier>,
) -> Result<api::GatewayState> {
    let store = store::GatewayStore::open(db)?;
    let stored = store.has_ciphertext()?;
    let (key, origin) = key::load_or_create(keys, stored).map_err(|err| err.context(KeyUnavailable))?;
    store.check_key(&key).map_err(|err| err.context(KeyUnavailable))?;
    tracing::info!(source = ?origin, "the gateway's master key is loaded");
    let runtime = runtime::Runtime::new(
        Arc::new(store),
        Arc::new(scope::ProxyStore::open(db)?),
        Arc::new(key),
        egress,
        notifier,
    );
    Ok(api::GatewayState {
        runtime: Arc::new(runtime),
        operator,
    })
}
