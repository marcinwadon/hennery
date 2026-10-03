//! The hennery host: runs ACP adapters for one machine and relays their
//! sessions to the collector (ACP core spec §2).

pub mod adapter;
pub mod agent_home;
pub mod availability;
pub mod connection;
pub mod forget;
pub mod forget_codex;
pub mod git;
pub mod identity;
pub mod logging;
pub mod outbox;
pub mod pairing;
pub mod paths;
pub mod profile;
pub mod projects;
pub mod runtime;
pub mod session;
pub mod uplink;
pub mod walk;

pub use adapter::AgentCommand;
pub use connection::{HostConfig, run, run_until};
