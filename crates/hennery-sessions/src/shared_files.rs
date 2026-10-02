//! The one read across owners (plan 9a decision 6, A6): attachment files
//! are shared by hash, so the same image sent by two owners is one file,
//! each owner holding a row of its own (plan 6a decision 5). Whether a
//! file may go is whether *any* owner's row still names it. This reads
//! nothing but that existence, changes nothing, and is the owner audit's
//! listed exemption (`crates/hennery-testkit/tests/owner_filter.rs`).

use anyhow::Result;
use rusqlite::Connection;

/// Whether an `attachments` row of any owner names `sha256`.
pub(crate) fn hash_named_by_any_owner(conn: &Connection, sha256: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM attachments WHERE sha256 = ?1)",
        [sha256],
        |r| r.get(0),
    )?)
}
