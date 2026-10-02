//! The hennery host: runs ACP adapters for one machine and relays their
//! sessions to the collector (ACP core spec §2).

pub mod adapter;
pub mod connection;
pub mod identity;
pub mod outbox;
pub mod pairing;
pub mod paths;
pub mod session;
pub mod uplink;

pub use adapter::AgentCommand;
pub use connection::{HostConfig, run, run_until};
