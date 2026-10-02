//! The MCP gateway (gateway spec): connections, their credentials at rest,
//! and the hosts they are mounted on. Depends on `hennery-kernel` and
//! `hennery-proto`, never on `hennery-sessions` (umbrella §9).

pub mod api;
pub mod crypto;
pub mod key;
pub mod model;
mod schema;
pub mod store;

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

/// The gateway on the collector's `hennery.db`: its store, and the master
/// key from `keys`, made at the first start. Before serving, and after the
/// admin socket's bind (the one-collector guard).
pub fn open(db: &Path, keys: &key::KeySource, operator: Arc<Operator>) -> Result<api::GatewayState> {
    let store = store::GatewayStore::open(db)?;
    let stored = store.has_ciphertext()?;
    let (key, origin) = key::load_or_create(keys, stored).map_err(|err| err.context(KeyUnavailable))?;
    store.check_key(&key).map_err(|err| err.context(KeyUnavailable))?;
    tracing::info!(source = ?origin, "the gateway's master key is loaded");
    Ok(api::GatewayState {
        store: Arc::new(store),
        key: Arc::new(key),
        operator,
    })
}
