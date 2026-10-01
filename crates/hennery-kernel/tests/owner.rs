//! `owner_id` everywhere (kernel spec §1, umbrella §7.4; plan 3b-iii): the
//! owner exists from the first start, every row carries it, and every
//! query filters by it.

use ed25519_dalek::{Signer, SigningKey};
use hennery_kernel::hosts::{
    EnrollOutcome, Enrollment, HelloCheck, Hosts, PAIRING_CODE_TTL_SECS, Revoke, normalize_code,
};
use hennery_kernel::operator::{Operator, Reset, SESSION_TTL_SECS, SetupOutcome};
use hennery_kernel::secret::sha256_hex;
use hennery_proto::frames::Capabilities;
use hennery_proto::hello_proof_message;
use rusqlite::types::Value;
use std::path::Path;
use std::sync::Arc;

const NOW: i64 = 1_800_000_000;
const PASSWORD: &str = "correct horse battery";

/// The owner of `a_3b_ii_database`, when it is set up.
const OLD_OWNER: &str = "owner-00000000000000a1";

/// `hennery.db` as 3b-ii left it: the kernel's first two migrations,
/// verbatim, with a host paired and a pairing code minted (as `up` does
/// before setup), and the owner set up if `set_up`. The host and the code
/// are the rows the backfill rebuilds, so every column of theirs holds a
/// value of its own, none `NULL` or the default: one the rebuild dropped,
/// nulled or swapped would show (`HOST_OLD`, `CODE_OLD`).
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
        INSERT INTO hosts(id, name, public_key, platform, host_version, capabilities, created_at,
                          last_seen_at, revoked_at)
            VALUES ('host-old', 'laptop', '00', 'macos-aarch64', '0.0.0', '[\"fixture\"]', 1700000000,
                    1700000100, 1700000200);
        INSERT INTO pairing_codes VALUES ('code-old', 1700000300, 1700000600, 1700000400);
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

/// `a_3b_ii_database`'s host, column by column, as inserted.
fn host_old() -> Vec<(&'static str, Value)> {
    vec![
        ("id", Value::Text("host-old".into())),
        ("name", Value::Text("laptop".into())),
        ("public_key", Value::Text("00".into())),
        ("platform", Value::Text("macos-aarch64".into())),
        ("host_version", Value::Text("0.0.0".into())),
        ("capabilities", Value::Text("[\"fixture\"]".into())),
        ("created_at", Value::Integer(1_700_000_000)),
        ("last_seen_at", Value::Integer(1_700_000_100)),
        ("revoked_at", Value::Integer(1_700_000_200)),
    ]
}

/// `a_3b_ii_database`'s pairing code, column by column, as inserted.
fn code_old() -> Vec<(&'static str, Value)> {
    vec![
        ("code_hash", Value::Text("code-old".into())),
        ("created_at", Value::Integer(1_700_000_300)),
        ("expires_at", Value::Integer(1_700_000_600)),
        ("used_at", Value::Integer(1_700_000_400)),
    ]
}

/// Every row of `table`, in order, each as its columns by name.
fn named_rows(conn: &rusqlite::Connection, table: &str) -> Vec<std::collections::BTreeMap<String, Value>> {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
    let names: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
    stmt.query_map([], |r| {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| Ok((name.clone(), r.get::<_, Value>(i)?)))
            .collect()
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// The `owner_id` of every row of `table`, in order.
fn owners_of(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("SELECT owner_id FROM {table} ORDER BY rowid"))
        .unwrap();
    stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

/// The backfill (3b-iii decision 1): a database 3b-ii set up keeps its
/// owner, set up, its password and its `public_url`. One 3b-ii left before
/// setup gets an owner, not set up, which setup then keeps. Either way,
/// the host and the pairing code written before carry that owner, and the
/// rebuild keeps every other column of theirs as it was.
#[test]
fn a_3b_ii_database_keeps_its_owner_or_gets_one() {
    for set_up in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        a_3b_ii_database(&db, set_up);
        let op = Operator::open(&db).unwrap();
        assert_eq!(op.is_set_up().unwrap(), set_up);
        let owner = op.owner_id().to_string();
        // The host and the pairing code written before carry it.
        let conn = rusqlite::Connection::open(&db).unwrap();
        for table in ["hosts", "pairing_codes"] {
            assert_eq!(owners_of(&conn, table), vec![owner.clone()], "{table}");
        }
        // Every column of the rebuilt rows, by name: what was inserted,
        // and the owner.
        for (table, inserted) in [("hosts", host_old()), ("pairing_codes", code_old())] {
            let mut expected: std::collections::BTreeMap<String, Value> = inserted
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect();
            expected.insert("owner_id".into(), Value::Text(owner.clone()));
            assert_eq!(named_rows(&conn, table), vec![expected], "{table}");
        }
        let hosts = Hosts::open(&db).unwrap();
        assert_eq!(hosts.owner_id(), owner);
        let listed: Vec<String> = hosts.list().unwrap().into_iter().map(|h| h.id).collect();
        assert_eq!(listed, ["host-old"]);
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

/// A second owner, written straight into the database: nothing in v1
/// makes one. It is never the database's owner (`db::kernel_owner`):
/// see `OTHER_CREATED_AT`.
const OTHER: &str = "owner-00000000000000b2";
/// `OTHER`'s `created_at` and `set_up_at`: later than any real clock
/// reaches, since migration 3 stamps the real owner with `unixepoch()`
/// (3b-iii review, A5). A fixed date would lose to the clock one day.
const OTHER_CREATED_AT: i64 = i64::MAX;
const OTHER_PASSWORD: &str = "the other owner's password";
const OTHER_TOKEN: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
const OTHER_EXPIRED_TOKEN: &str = "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3";

/// A signed-in session of `owner`'s, as `open_session` writes one, whose
/// cookie is `token`.
fn write_session(conn: &rusqlite::Connection, owner: &str, token: &str, expires_at: i64) {
    conn.execute(
        "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, last_step_up_at, expires_at)
         VALUES (?1, ?2, 'written', ?3, ?3, ?3, ?4)",
        rusqlite::params![sha256_hex(token.as_bytes()), owner, NOW, expires_at],
    )
    .unwrap();
}

/// `OTHER`, set up, with a password, a `public_url`, a live session and an
/// expired one.
fn write_other_owner(conn: &rusqlite::Connection) {
    conn.execute(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, ?2, ?2)",
        rusqlite::params![OTHER, OTHER_CREATED_AT],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO password_credentials VALUES (?1, ?2, ?3)",
        rusqlite::params![OTHER, password_auth::generate_hash(OTHER_PASSWORD), OTHER_CREATED_AT],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO settings VALUES (?1, 'public_url', 'https://other.example')",
        [OTHER],
    )
    .unwrap();
    write_session(conn, OTHER, OTHER_TOKEN, NOW + SESSION_TTL_SECS);
    write_session(conn, OTHER, OTHER_EXPIRED_TOKEN, NOW - 1);
}

/// Every row of `owner`'s in `tables`, as stored.
fn rows_of(conn: &rusqlite::Connection, tables: &[&str], owner: &str) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for table in tables {
        let column = if *table == "owners" { "id" } else { "owner_id" };
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {table} WHERE {column} = ?1 ORDER BY rowid"))
            .unwrap();
        let width = stmt.column_count();
        let rows = stmt
            .query_map([owner], |r| (0..width).map(|i| r.get::<_, Value>(i)).collect())
            .unwrap();
        out.extend(rows.map(Result::unwrap));
    }
    out
}

const OPERATOR_TABLES: &[&str] = &["owners", "password_credentials", "auth_sessions", "settings"];

/// Kernel spec §1: every query of the operator's filters by the owner.
/// Another owner's password, sessions and `public_url`, in the same
/// database, are neither seen nor changed, by any of them. They are
/// written before the real owner's, so a query that took the first row it
/// found would find theirs. The real owner's session, written the same
/// way, is seen (the control).
#[tokio::test]
async fn another_owners_password_and_sessions_are_invisible_to_the_operator() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let op = Arc::new(Operator::open(&db).unwrap());
    let conn = rusqlite::Connection::open(&db).unwrap();
    write_other_owner(&conn);
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    let mine = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
    write_session(&conn, op.owner_id(), mine, NOW + SESSION_TTL_SECS);
    let theirs = rows_of(&conn, OPERATOR_TABLES, OTHER);
    let other_session = sha256_hex(OTHER_TOKEN.as_bytes());
    let other_phc: String = conn
        .query_row(
            "SELECT phc FROM password_credentials WHERE owner_id = ?1",
            [OTHER],
            |r| r.get(0),
        )
        .unwrap();

    assert_eq!(op.authenticate(mine, NOW).unwrap().unwrap().owner_id, op.owner_id());
    assert_eq!(op.verify_password(PASSWORD).unwrap(), Some(phc.clone()));
    // The controls for the refusals below (3b-iii deferred minor): on the
    // owner's own session, `step_up` and `session_expires_at` take effect.
    // `NOW + 7`, not `NOW`: `write_session` stores `NOW` already.
    let my_session = sha256_hex(mine.as_bytes());
    assert!(op.step_up(&my_session, NOW + 7).unwrap());
    assert_eq!(
        op.authenticate(mine, NOW).unwrap().unwrap().last_step_up_at,
        Some(NOW + 7)
    );
    assert_eq!(
        op.session_expires_at(&my_session, NOW).unwrap(),
        Some(NOW + SESSION_TTL_SECS)
    );

    assert_eq!(op.verify_password(OTHER_PASSWORD).unwrap(), None);
    assert_eq!(op.authenticate(OTHER_TOKEN, NOW).unwrap(), None);
    assert!(!op.step_up(&other_session, NOW).unwrap());
    assert_eq!(op.session_expires_at(&other_session, NOW).unwrap(), None);
    let listed: Vec<String> = op.sessions(NOW).unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(listed, [sha256_hex(mine.as_bytes())]);
    // The control for the refusal below: a spare session of the owner's,
    // written only now so the listing above stays the owner's one, is
    // revoked, and no longer authenticates.
    let spare = "d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4";
    write_session(&conn, op.owner_id(), spare, NOW + SESSION_TTL_SECS);
    assert!(op.authenticate(spare, NOW).unwrap().is_some());
    assert!(op.revoke_session(&sha256_hex(spare.as_bytes()), NOW).unwrap());
    assert_eq!(op.authenticate(spare, NOW).unwrap(), None);
    assert!(!op.revoke_session(&other_session, NOW).unwrap());
    assert_eq!(op.open_session("theirs", &other_phc, NOW).unwrap(), None);
    // Opening one sweeps the owner's expired sessions, not theirs.
    assert!(op.open_session("mine", &phc, NOW).unwrap().is_some());
    // The resets end the owner's sessions only, and replace only the
    // owner's `public_url` and password.
    assert_eq!(
        op.reset_public_url("https://moved.example").unwrap(),
        Reset::Done {
            sessions_ended: 2,
            passkeys_removed: 0
        }
    );
    assert_eq!(
        op.reset_password("a new long password".into(), NOW).await.unwrap(),
        Reset::Done {
            sessions_ended: 0,
            passkeys_removed: 0
        }
    );
    assert_eq!(rows_of(&conn, OPERATOR_TABLES, OTHER), theirs);
    drop(op);
    let reopened = Operator::open(&db).unwrap();
    assert_eq!(reopened.public_url().unwrap().origin(), "https://moved.example");
    assert!(reopened.verify_password("a new long password").unwrap().is_some());
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn enrollment(seed: u8) -> Enrollment {
    Enrollment {
        public_key: hex::encode(key(seed).verifying_key().as_bytes()),
        name: "laptop".into(),
        host_version: "0.0.0".into(),
        platform: "macos-aarch64".into(),
    }
}

/// A paired host of `owner`'s, as `enroll` writes one, keyed by `seed`.
fn write_host(conn: &rusqlite::Connection, owner: &str, host_id: &str, seed: u8) {
    conn.execute(
        "INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, created_at)
         VALUES (?1, ?2, 'laptop', ?3, 'macos-aarch64', '0.0.0', ?4)",
        rusqlite::params![host_id, owner, enrollment(seed).public_key, NOW],
    )
    .unwrap();
}

/// A live pairing code of `owner`'s, as `mint_pairing_code` writes one.
fn write_code(conn: &rusqlite::Connection, owner: &str, code: &str) {
    let hash = sha256_hex(normalize_code(code).unwrap().as_bytes());
    conn.execute(
        "INSERT INTO pairing_codes(code_hash, owner_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![hash, owner, NOW, NOW + PAIRING_CODE_TTL_SECS],
    )
    .unwrap();
}

fn proof(seed: u8, nonce: &[u8], host_id: &str) -> String {
    hex::encode(key(seed).sign(&hello_proof_message(nonce, host_id, "1.0")).to_bytes())
}

/// 3b-iii decision 1, the question it answers: `up` mints a pairing code
/// and enrolls its host before setup. Both carry the owner that setup
/// then sets up, and the registry lists the host after setup.
#[test]
fn a_host_paired_before_setup_belongs_to_the_owner_setup_sets_up() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let code = hosts.mint_pairing_code(NOW).unwrap();
    let EnrollOutcome::Enrolled { host_id } = hosts.enroll(&code.code, &enrollment(1), NOW).unwrap() else {
        panic!("the host did not enroll");
    };
    let op = Operator::open(&db).unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { owner_id, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap()
    else {
        panic!("setup failed");
    };
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(owners_of(&conn, "hosts"), vec![owner_id.clone()]);
    assert_eq!(owners_of(&conn, "pairing_codes"), vec![owner_id.clone()]);
    assert_eq!(hosts.owner_id(), owner_id);
    let listed: Vec<String> = hosts.list().unwrap().into_iter().map(|h| h.id).collect();
    assert_eq!(listed, [host_id]);
}

const HOST_TABLES: &[&str] = &["hosts", "pairing_codes"];

/// Kernel spec §1: every query of the host registry's filters by the
/// owner. Another owner's host and pairing code, in the same database,
/// are neither seen nor changed: its host's valid proof is refused like an
/// unknown host's, it is not listed, cannot be revoked and, revoked
/// already, is not seen as revoked either, and its code pairs nothing.
/// The real owner's, written the same way, work (the control). The other
/// owner's key cannot be paired again under this
/// owner either: keys are unique across owners (3b-iii decision 7).
#[test]
fn another_owners_hosts_and_codes_are_invisible_to_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    write_other_owner(&conn);
    write_host(&conn, OTHER, "host-b2", 2);
    // Revoked, so that only the owner filter keeps `is_revoked` from
    // seeing it (3b-iii final review, T4-M1): it gates the host socket.
    conn.execute("UPDATE hosts SET revoked_at = ?1 WHERE id = 'host-b2'", [NOW])
        .unwrap();
    write_code(&conn, OTHER, "BBBB-BBBB");
    write_host(&conn, hosts.owner_id(), "host-a1", 1);
    write_code(&conn, hosts.owner_id(), "AAAA-AAAA");
    let theirs = rows_of(&conn, HOST_TABLES, OTHER);
    let nonce = [7u8; 32];

    assert_eq!(
        hosts
            .check_hello("host-a1", &nonce, "1.0", &proof(1, &nonce, "host-a1"))
            .unwrap(),
        HelloCheck::Accepted
    );
    assert!(hosts.host("host-a1").unwrap().is_some());

    assert_eq!(
        hosts
            .check_hello("host-b2", &nonce, "1.0", &proof(2, &nonce, "host-b2"))
            .unwrap(),
        HelloCheck::BadProof
    );
    assert_eq!(hosts.host("host-b2").unwrap(), None);
    let listed: Vec<String> = hosts.list().unwrap().into_iter().map(|h| h.id).collect();
    assert_eq!(listed, ["host-a1"]);
    assert_eq!(hosts.revoke("host-b2", NOW).unwrap(), Revoke::NotFound);
    assert!(!hosts.is_revoked("host-b2").unwrap());
    hosts
        .record_hello("host-b2", "9.9.9", &Capabilities::default(), NOW + 5)
        .unwrap();
    assert_eq!(
        hosts.enroll("BBBB-BBBB", &enrollment(3), NOW).unwrap(),
        EnrollOutcome::InvalidCode
    );
    assert!(hosts.enroll("AAAA-AAAA", &enrollment(2), NOW).is_err());
    assert_eq!(rows_of(&conn, HOST_TABLES, OTHER), theirs);

    // The owner's code is still live: the refused pairing above spent
    // nothing.
    assert!(matches!(
        hosts.enroll("AAAA-AAAA", &enrollment(4), NOW).unwrap(),
        EnrollOutcome::Enrolled { .. }
    ));
    assert!(!hosts.is_revoked("host-a1").unwrap());
    assert_eq!(hosts.revoke("host-a1", NOW).unwrap(), Revoke::Revoked);
    assert!(hosts.is_revoked("host-a1").unwrap());
}
