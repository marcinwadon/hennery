//! SQLite helpers (kernel spec §1).

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension};
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

/// Like `migrate`, for one component of a database that several own
/// (kernel spec §1): the kernel's tables share `hennery.db` with the
/// sessions module, which keeps `user_version` for itself. Each component's
/// version is a row of `schema_versions`, and a component newer than this
/// binary is refused the same way.
pub fn migrate_component(conn: &mut Connection, component: &str, migrations: &[&str]) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_versions (component TEXT PRIMARY KEY, version INTEGER NOT NULL);",
    )?;
    let current: usize = conn
        .query_row(
            "SELECT version FROM schema_versions WHERE component = ?1",
            [component],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0) as usize;
    if current > migrations.len() {
        bail!(
            "{component} schema version {current} is newer than this binary supports ({})",
            migrations.len()
        );
    }
    for (index, sql) in migrations.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_versions(component, version) VALUES (?1, ?2)
             ON CONFLICT(component) DO UPDATE SET version = excluded.version",
            rusqlite::params![component, (index + 1) as i64],
        )?;
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

    #[test]
    fn components_keep_their_own_versions_beside_user_version() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn, &["CREATE TABLE a (x INTEGER);"]).unwrap();
        migrate_component(
            &mut conn,
            "kernel",
            &["CREATE TABLE k1 (x INTEGER);", "CREATE TABLE k2 (x INTEGER);"],
        )
        .unwrap();
        migrate_component(
            &mut conn,
            "kernel",
            &["CREATE TABLE k1 (x INTEGER);", "CREATE TABLE k2 (x INTEGER);"],
        )
        .unwrap();
        migrate_component(&mut conn, "other", &["CREATE TABLE o1 (x INTEGER);"]).unwrap();
        let user_version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        let kernel: i64 = conn
            .query_row(
                "SELECT version FROM schema_versions WHERE component = 'kernel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((user_version, kernel), (1, 2));
        // `migrate` still sees its own version, untouched by the components.
        migrate(&mut conn, &["CREATE TABLE a (x INTEGER);"]).unwrap();
    }

    #[test]
    fn a_newer_component_is_refused() {
        let mut conn = open_in_memory().unwrap();
        migrate_component(
            &mut conn,
            "kernel",
            &["CREATE TABLE k1 (x INTEGER);", "CREATE TABLE k2 (x INTEGER);"],
        )
        .unwrap();
        let err = migrate_component(&mut conn, "kernel", &["CREATE TABLE k1 (x INTEGER);"]).unwrap_err();
        assert!(err.to_string().contains("kernel schema version 2 is newer"), "{err}");
    }
}
