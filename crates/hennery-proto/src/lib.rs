//! hennery's own wire types: host<->collector frames and REST payloads.
//!
//! This crate is the single source of truth (architecture spec §5.4). JSON
//! Schema and TypeScript are generated from these types by the `gen` binary;
//! ACP payloads travel inside them as raw JSON and are never modelled here.

pub mod frames;
pub mod paths;
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

/// Name of the WebSocket upgrade response header that carries the
/// collector's nonce for `hello.proof` (ACP core §3.5): 32 random bytes,
/// lowercase hex.
pub const HELLO_NONCE_HEADER: &str = "hennery-hello-nonce";

/// The bytes a host signs for `hello.proof` (ACP core §3.5): the
/// collector's nonce, the host id and the protocol version, behind a
/// domain-separation label and each prefixed with its length (u32, big
/// endian), so no two different triples produce the same message.
pub fn hello_proof_message(nonce: &[u8], host_id: &str, protocol_version: &str) -> Vec<u8> {
    const LABEL: &[u8] = b"hennery hello proof v1";
    let mut out = Vec::with_capacity(LABEL.len() + 12 + nonce.len() + host_id.len() + protocol_version.len());
    out.extend_from_slice(LABEL);
    for part in [nonce, host_id.as_bytes(), protocol_version.as_bytes()] {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}
pub mod codegen;
