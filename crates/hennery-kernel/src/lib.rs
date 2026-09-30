//! Shared collector foundations (kernel spec): storage, request auth, and
//! the operator and their setup, and host identity and pairing.

pub mod auth;
pub mod auth_api;
pub mod db;
pub mod hosts;
pub mod lifecycle;
pub mod operator;
pub mod origin;
pub mod ratelimit;
mod schema;
pub mod secret;
mod setup_page;
