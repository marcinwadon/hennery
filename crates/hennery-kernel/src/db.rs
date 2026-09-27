//! SQLite helpers (kernel spec §1).

use anyhow::{Result, bail};
use rusqlite::Connection;
use std::path::Path;

/// Open (or create) a database with the pragmas every hennery database uses.
pub fn open(path: &Path) -> Result<Connection> {
    configure(Connection::open(path)?)
}

pub fn open_in_memory() -> Result<Connection> {
    configure(Connection::open_in_memory()?)
}

fn configure(conn: Connection) -> Result<Connection> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

/// Apply `migrations[user_version..]` in order, each in its own transaction.
/// Refuses to run against a database newer than this binary.
pub fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let current: usize = conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
    if current > migrations.len() {
        bail!(
            "database schema version {current} is newer than this binary supports ({})",
            migrations.len()
        );
    }
    for (index, sql) in migrations.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_once_and_record_the_version() {
        let mut conn = open_in_memory().unwrap();
        let steps = ["CREATE TABLE a (x INTEGER);", "CREATE TABLE b (y INTEGER);"];
        migrate(&mut conn, &steps).unwrap();
        migrate(&mut conn, &steps).unwrap();
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, 2);
    }

    #[test]
    fn a_newer_database_is_refused() {
        let mut conn = open_in_memory().unwrap();
        migrate(
            &mut conn,
            &["CREATE TABLE a (x INTEGER);", "CREATE TABLE b (y INTEGER);"],
        )
        .unwrap();
        let err = migrate(&mut conn, &["CREATE TABLE a (x INTEGER);"]).unwrap_err();
        assert!(err.to_string().contains("newer"), "{err}");
    }
}
