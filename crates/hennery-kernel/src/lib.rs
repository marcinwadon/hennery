//! Shared collector foundations (kernel spec): storage, request auth, and
//! host identity and pairing.

pub mod auth;
pub mod db;
pub mod hosts;
pub mod lifecycle;
pub mod ratelimit;
pub mod secret;
