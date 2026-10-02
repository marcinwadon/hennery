//! The MCP gateway (gateway spec): connections, their credentials at rest,
//! and the hosts they are mounted on. Depends on `hennery-kernel` and
//! `hennery-proto`, never on `hennery-sessions` (umbrella §9).

pub mod api;
pub mod crypto;
pub mod key;
pub mod model;
mod schema;
pub mod store;
