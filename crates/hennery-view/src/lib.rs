//! The client view layer (client view spec §2): one session's stored
//! events folded into display items, the same for every client.
//!
//! Pure: no I/O, no clock, no database. The collector's view API (in
//! `hennery-sessions`) reads the events and the pending set, and serves
//! what this crate makes. ACP payloads are read as JSON values, field by
//! field, so a shape that drifts degrades one field, never a whole update;
//! an update the fold cannot place becomes an `unrecognised` item and is
//! never dropped (D3).

pub mod cap;
pub mod fold;
pub mod item;
pub mod question;
pub mod tool;

pub use fold::{Answerable, Fold, Items, fold};
pub use item::*;

/// The fold's rules' version. Ordinal ids and versions follow the rules:
/// raise it whenever they change what an event makes, so a client holding
/// items of the old rules resyncs.
pub const FOLD_VERSION: u32 = 1;
