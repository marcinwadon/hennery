//! Shared collector foundations (kernel spec): storage, request auth, and
//! the operator and their setup, host identity and pairing, and the admin
//! socket.

pub mod admin;
pub mod auth;
pub mod auth_api;
pub mod capabilities;
pub mod client_address;
pub mod coordination;
pub mod csp;
pub mod db;
pub mod delivery;
pub mod egress;
pub mod hats;
pub mod health;
pub mod hosts;
pub mod json;
pub mod lifecycle;
pub mod logo;
pub mod operator;
pub mod origin;
pub mod passkeys;
pub mod plugin_context;
pub mod push;
mod push_crypto;
pub mod ratelimit;
pub mod recents;
pub mod redact;
mod schema;
pub mod secret;
pub mod server;
pub mod web;

pub mod session_defaults;
