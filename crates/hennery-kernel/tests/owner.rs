//! `owner_id` everywhere (kernel spec §1, umbrella §7.4; plan 3b-iii): the
//! owner exists from the first start, every row carries it, and every
//! query filters by it.

use hennery_kernel::operator::{Operator, SetupOutcome};
use std::path::Path;

const NOW: i64 = 1_800_000_000;
const PASSWORD: &str = "correct horse battery";

/// The owner of `a_3b_ii_database`, when it is set up.
const OLD_OWNER: &str = "owner-00000000000000a1";

/// `hennery.db` as 3b-ii left it: the kernel's first two migrations,
/// verbatim, with a host paired and a pairing code minted (as `up` does
/// before setup), and the owner set up if `set_up`.
fn a_3b_ii_database(path: &Path, set_up: bool) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE schema_versions (component TEXT PRIMARY KEY, version INTEGER NOT NULL);
        CREATE TABLE hosts (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            public_key TEXT NOT NULL UNIQUE,
            platform TEXT NOT NULL,
            host_version TEXT NOT NULL,
            capabilities TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER,
            revoked_at INTEGER);
        CREATE TABLE pairing_codes (
            code_hash TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            used_at INTEGER);
        CREATE TABLE owners (
            id TEXT PRIMARY KEY,
            contact TEXT,
            created_at INTEGER NOT NULL);
        CREATE TABLE password_credentials (
            owner_id TEXT PRIMARY KEY REFERENCES owners(id),
            phc TEXT NOT NULL,
            updated_at INTEGER NOT NULL);
        CREATE TABLE auth_sessions (
            id_hash TEXT PRIMARY KEY,
            owner_id TEXT NOT NULL REFERENCES owners(id),
            user_agent TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            last_step_up_at INTEGER,
            expires_at INTEGER NOT NULL);
        CREATE TABLE settings (
            owner_id TEXT NOT NULL REFERENCES owners(id),
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (owner_id, key));
        INSERT INTO schema_versions VALUES ('kernel', 2);
        INSERT INTO hosts(id, name, public_key, platform, host_version, created_at)
            VALUES ('host-old', 'laptop', '00', 'macos-aarch64', '0.0.0', 1700000000);
        INSERT INTO pairing_codes VALUES ('code-old', 1700000000, 1700000600, NULL);
        ",
    )
    .unwrap();
    if set_up {
        conn.execute(
            "INSERT INTO owners(id, created_at) VALUES (?1, 1700000000)",
            [OLD_OWNER],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO password_credentials VALUES (?1, ?2, 1700000000)",
            [OLD_OWNER, &password_auth::generate_hash(PASSWORD)],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings VALUES (?1, 'public_url', 'https://hennery.example')",
            [OLD_OWNER],
        )
        .unwrap();
    }
}

/// 3b-iii decision 1: the owner exists from the first start, before setup,
/// so what is written before setup (`up`'s pairing code and host) has an
/// owner to carry. Setup marks that owner set up rather than creating one,
/// and a restart finds the same owner, set up or not.
#[test]
fn the_owner_exists_from_the_first_start_and_setup_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let op = Operator::open(&db).unwrap();
    let owner = op.owner_id().to_string();
    assert!(owner.starts_with("owner-") && owner.len() == 22, "{owner}");
    assert!(!op.is_set_up().unwrap());
    drop(op);
    let op = Operator::open(&db).unwrap();
    assert_eq!(op.owner_id(), owner);
    assert!(!op.is_set_up().unwrap());
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { owner_id, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap()
    else {
        panic!("setup failed");
    };
    assert_eq!(owner_id, owner);
    drop(op);
    let op = Operator::open(&db).unwrap();
    assert_eq!(op.owner_id(), owner);
    assert!(op.is_set_up().unwrap());
    let conn = rusqlite::Connection::open(&db).unwrap();
    let owners: i64 = conn.query_row("SELECT count(*) FROM owners", [], |r| r.get(0)).unwrap();
    assert_eq!(owners, 1);
}

/// The backfill (3b-iii decision 1): a database 3b-ii set up keeps its
/// owner, set up, its password and its `public_url`. One 3b-ii left before
/// setup gets an owner, not set up, which setup then keeps.
#[test]
fn a_3b_ii_database_keeps_its_owner_or_gets_one() {
    for set_up in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        a_3b_ii_database(&db, set_up);
        let op = Operator::open(&db).unwrap();
        assert_eq!(op.is_set_up().unwrap(), set_up);
        let owner = op.owner_id().to_string();
        if set_up {
            assert_eq!(owner, OLD_OWNER);
            assert!(op.verify_password(PASSWORD).unwrap().is_some());
            assert_eq!(op.public_url().unwrap().origin(), "https://hennery.example");
            assert_eq!(op.issue_setup_token(NOW).unwrap(), None);
        } else {
            assert!(owner.starts_with("owner-") && owner != OLD_OWNER, "{owner}");
            let token = op.issue_setup_token(NOW).unwrap().unwrap();
            let SetupOutcome::Done { owner_id, .. } =
                op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap()
            else {
                panic!("setup failed");
            };
            assert_eq!(owner_id, owner);
        }
    }
}
