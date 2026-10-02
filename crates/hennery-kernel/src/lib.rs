//! Shared collector foundations (kernel spec): storage, request auth, and
//! the operator and their setup, host identity and pairing, and the admin
//! socket.

pub mod admin;
pub mod auth;
pub mod auth_api;
pub mod csp;
pub mod db;
pub mod egress;
pub mod hats;
pub mod health;
pub mod hosts;
pub mod json;
pub mod lifecycle;
pub mod operator;
pub mod origin;
pub mod passkeys;
pub mod push;
pub mod ratelimit;
pub mod recents;
mod schema;
pub mod secret;
mod setup_page;
