//! The hennery host: runs ACP adapters for one machine and relays their
//! sessions to the collector (ACP core spec §2).

pub mod connection;
pub mod outbox;
pub mod session;
pub mod uplink;

pub use connection::{HostConfig, run};
pub use session::AgentCommand;
