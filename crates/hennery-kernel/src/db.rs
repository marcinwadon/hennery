//! SQLite helpers (kernel spec §1).

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Open (or create) a database with the pragmas every hennery database uses.
///
/// The database is private to its user (mode 0600), whatever the umask: it
/// holds what only the collector may read (pairing-code hashes, decision 5,
/// among them). SQLite gives a new `-wal` and `-shm` the database file's
/// own mode, so the file is made private before it is opened. A `-wal` or
/// `-shm` already there, from an install before this, is made private too.
///
/// None of the three is followed if it is a symlink: the mode is changed
/// through the descriptor of the file itself, never through a path that
/// could name some other file of this user's.
pub fn open(path: &Path) -> Result<Connection> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("open {} (a symlink is refused)", path.display()))?;
    make_private(&file, path)?;
    drop(file);
    let conn = configure(Connection::open(path)?)?;
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let sidecar = Path::new(&name);
        match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(sidecar)
        {
            Ok(file) => make_private(&file, sidecar)?,
            // Gone already is fine: the last connection to close removes them.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("open {} (a symlink is refused)", sidecar.display()));
            }
        }
    }
    Ok(conn)
}

/// `fchmod`, through the open file rather than its path.
fn make_private(file: &std::fs::File, path: &Path) -> Result<()> {
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("make {} private", path.display()))
}

pub fn open_in_memory() -> Result<Connection> {
    configure(Connection::open_in_memory()?)
}

/// Every transaction is `IMMEDIATE`: it takes the write lock when it
/// begins, waiting out the busy timeout if another connection holds it.
/// Several connections write to `hennery.db` (the sessions store, the host
/// registry), and a deferred transaction that reads first
/// fails on its first write, at once and without the busy timeout
/// (`SQLITE_BUSY_SNAPSHOT`), whenever another connection committed after
/// its read. Until one writer thread owns the database (kernel spec §1),
/// this is what serialises them.
fn configure(mut conn: Connection) -> Result<Connection> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // What is deleted is overwritten with zeros, not left in free space
    // for anyone who reads the file (plan 9a A8): a deleted session's
    // title, cwd and timeline must not outlive the delete in the database.
    // The WAL keeps old pages until a checkpoint, which a delete runs.
    conn.pragma_update(None, "secure_delete", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
    Ok(conn)
}

/// Apply `migrations[user_version..]` in order, each in its own transaction.
/// Refuses to run against a database newer than this binary.
///
/// Each step reads the version inside its transaction, which holds the
/// write lock from its start: `IMMEDIATE`, asked for here rather than left
/// to the connection's default (`configure` sets it; a plain connection
/// does not). Another connection migrating the same file meanwhile has
/// either committed the step, and it is skipped, or not begun it. A
/// version read before the lock could be stale by then, and the step
/// would run twice.
pub fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    loop {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: usize = tx.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
        if current > migrations.len() {
            bail!(
                "database schema version {current} is newer than this binary supports ({})",
                migrations.len()
            );
        }
        let Some(sql) = migrations.get(current) else {
            return Ok(());
        };
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (current + 1) as i64)?;
        tx.commit()?;
    }
}

/// Like `migrate`, for one component of a database that several own
/// (kernel spec §1): the kernel's tables share `hennery.db` with the
/// sessions module, which keeps `user_version` for itself. Each component's
/// version is a row of `schema_versions`, and a component newer than this
/// binary is refused the same way. Each step reads the version inside its
/// transaction, as `migrate` does.
pub fn migrate_component(conn: &mut Connection, component: &str, migrations: &[&str]) -> Result<()> {
    loop {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_versions (component TEXT PRIMARY KEY, version INTEGER NOT NULL);",
        )?;
        let current: usize = tx
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
        let Some(sql) = migrations.get(current) else {
            // Commits the `schema_versions` table, if this made it.
            tx.commit()?;
            return Ok(());
        };
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_versions(component, version) VALUES (?1, ?2)
             ON CONFLICT(component) DO UPDATE SET version = excluded.version",
            rusqlite::params![component, (current + 1) as i64],
        )?;
        tx.commit()?;
    }
}

/// Migrate the kernel's tables (`schema`), then return the database's
/// owner (plan 3b-iii decisions 1 and 2): the oldest row of `owners`,
/// which the kernel's migrations create at the first start. Every store on
/// `hennery.db` binds to it when it opens, so they agree whichever opens
/// first. This is the one query that does not filter by the owner: it
/// finds the owner.
pub fn kernel_owner(conn: &mut Connection) -> Result<String> {
    migrate_component(conn, crate::schema::COMPONENT, crate::schema::MIGRATIONS)?;
    conn.query_row("SELECT id FROM owners ORDER BY created_at, id LIMIT 1", [], |r| {
        r.get(0)
    })
    .optional()?
    .context("the database has no owner")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// Final review I2, an install from before the fix: a database and its
    /// WAL files left readable by others become private when opened.
    #[test]
    fn opening_makes_an_existing_database_and_its_wal_files_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hennery.db");
        // Held open, so the `-wal` and `-shm` stay there.
        let first = open(&path).unwrap();
        first
            .execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();
        let files = [
            path.clone(),
            dir.path().join("hennery.db-wal"),
            dir.path().join("hennery.db-shm"),
        ];
        for file in &files {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let _second = open(&path).unwrap();
        for file in &files {
            assert_eq!(mode(file), 0o600, "{}", file.display());
        }
    }

    /// Review of the fix wave: `open` changes the mode of the file it is
    /// given, so it must never follow a symlink to some other file of its
    /// user's and make that one 0600, or open it as a database.
    #[test]
    fn a_symlinked_database_is_refused_and_its_target_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        std::fs::write(&target, b"not a database").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let db = dir.path().join("hennery.db");
        std::os::unix::fs::symlink(&target, &db).unwrap();
        let err = open(&db).expect_err("a symlinked database was opened");
        assert!(format!("{err:#}").contains("a symlink is refused"), "{err:#}");
        assert_eq!(mode(&target), 0o644);
        assert_eq!(std::fs::read(&target).unwrap(), b"not a database");
    }

    /// A transaction holds the write lock from its start, so
    /// another connection's write waits for it (here, with no busy timeout,
    /// is refused) instead of committing between the transaction's read and
    /// its write and failing that write with `SQLITE_BUSY_SNAPSHOT`.
    #[test]
    fn a_transaction_holds_the_write_lock_from_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hennery.db");
        let mut first = open(&path).unwrap();
        first.execute_batch("CREATE TABLE t (x INTEGER);").unwrap();
        let second = open(&path).unwrap();
        second.busy_timeout(std::time::Duration::ZERO).unwrap();

        let tx = first.transaction().unwrap();
        let count: i64 = tx.query_row("SELECT count(*) FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
        let err = second.execute("INSERT INTO t VALUES (1)", []).unwrap_err();
        assert_eq!(
            err.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy),
            "{err}"
        );
        tx.execute("INSERT INTO t VALUES (2)", []).unwrap();
        tx.commit().unwrap();
        second.execute("INSERT INTO t VALUES (3)", []).unwrap();
    }

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

    /// A connection `configure` never touched: a busy timeout, but no
    /// `IMMEDIATE` default. The migration functions must take the write
    /// lock themselves (3b-iii review, O1).
    fn unconfigured(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        conn
    }

    /// Two connections migrating one file at once, as two components of
    /// one collector, or two collectors, can. `first` holds the write lock
    /// and applies the step meanwhile, as the other migrator would.
    /// `migrate`, on a connection of its own (`unconfigured`), must read
    /// the version once it holds the lock, then see the step done and skip
    /// it. A version read before it waits for the lock is stale, and the
    /// step would run twice. If `migrate` reaches its read only after
    /// `first` commits (under load, say), the test passes falsely; it never
    /// fails falsely.
    #[test]
    fn a_migration_racing_another_connection_is_applied_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hennery.db");
        let steps = ["CREATE TABLE a (x INTEGER);"];
        let mut first = open(&path).unwrap();
        let tx = first.transaction().unwrap();
        let racing = {
            let path = path.clone();
            std::thread::spawn(move || migrate(&mut unconfigured(&path), &steps))
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        tx.execute_batch(steps[0]).unwrap();
        tx.pragma_update(None, "user_version", 1).unwrap();
        tx.commit().unwrap();
        racing.join().unwrap().expect("the racing migration failed");
        let v: i64 = first.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, 1);
    }

    /// The same race for a component's version (`schema_versions`).
    #[test]
    fn a_component_migration_racing_another_connection_is_applied_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hennery.db");
        let steps = ["CREATE TABLE k1 (x INTEGER);"];
        let mut first = open(&path).unwrap();
        migrate_component(&mut first, "kernel", &[]).unwrap();
        let tx = first.transaction().unwrap();
        let racing = {
            let path = path.clone();
            std::thread::spawn(move || migrate_component(&mut unconfigured(&path), "kernel", &steps))
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        tx.execute_batch(steps[0]).unwrap();
        tx.execute(
            "INSERT INTO schema_versions(component, version) VALUES ('kernel', 1)",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        racing.join().unwrap().expect("the racing migration failed");
        let v: i64 = first
            .query_row(
                "SELECT version FROM schema_versions WHERE component = 'kernel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, 1);
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
