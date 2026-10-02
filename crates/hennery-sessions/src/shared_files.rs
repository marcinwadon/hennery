//! The one read across owners (plan 9a decision 6, A6): attachment files
//! are shared by hash, so the same image sent by two owners is one file,
//! each owner holding a row of its own (plan 6a decision 5). Whether a
//! file may go is whether *any* owner's row still names it. This reads
//! nothing but that existence, changes nothing, and is the owner audit's
//! listed exemption (`crates/hennery-testkit/tests/owner_filter.rs`).

use anyhow::Result;
use rusqlite::Connection;

/// The read, on `attachments_by_hash`.
const NAMED_BY_ANY_OWNER: &str = "SELECT EXISTS(SELECT 1 FROM attachments WHERE sha256 = ?1)";

/// Whether an `attachments` row of any owner names `sha256`.
pub(crate) fn hash_named_by_any_owner(conn: &Connection, sha256: &str) -> Result<bool> {
    Ok(conn.query_row(NAMED_BY_ANY_OWNER, [sha256], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    /// Plan 9a: the read walks the hash's own index, not the whole table.
    #[test]
    fn the_read_uses_the_hash_index() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        crate::store::Store::open(&db).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        let plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", super::NAMED_BY_ANY_OWNER))
            .unwrap()
            .query_map(["0".repeat(64)], |r| r.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(plan.iter().any(|p| p.contains("attachments_by_hash")), "{plan:?}");
    }
}
