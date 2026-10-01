//! Shared collector foundations (kernel spec): storage, request auth, and
//! the operator and their setup, host identity and pairing, and the admin
//! socket.

pub mod admin;
pub mod auth;
pub mod auth_api;
pub mod db;
pub mod health;
pub mod hosts;
pub mod json;
pub mod lifecycle;
pub mod operator;
pub mod origin;
pub mod passkeys;
pub mod ratelimit;
mod schema;
pub mod secret;
mod setup_page;
