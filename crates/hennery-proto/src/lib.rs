//! hennery's own wire types: host<->collector frames and REST payloads.
//!
//! This crate is the single source of truth (architecture spec §5.4). JSON
//! Schema and TypeScript are generated from these types by the `gen` binary;
//! ACP payloads travel inside them as raw JSON and are never modelled here.

pub mod frames;
pub mod rest;

/// Version of the host<->collector protocol, `MAJOR.MINOR`. Sent only in
/// `hello` / `hello_ack`; minor versions are additive (architecture spec §5.3).
pub const PROTOCOL_VERSION: &str = "1.0";

/// The major part of a `MAJOR.MINOR` version string, if well formed.
pub fn protocol_major(version: &str) -> Option<u32> {
    let (major, minor) = version.split_once('.')?;
    minor.parse::<u32>().ok()?;
    major.parse().ok()
}
pub mod codegen;
