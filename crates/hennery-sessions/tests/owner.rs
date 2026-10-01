//! `owner_id` in the sessions store (kernel spec §1; plan 3b-iii): every
//! row carries it, the migration fills it in, and every query names it.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{AttachedSession, SessionBody};
use hennery_proto::rest::AnswerRequest;
use hennery_sessions::store::{AnswerSubmission, Reconciliation, ResumeRequest, Store};
use rusqlite::types::Value;
use serde_json::json;

/// The store's tables, each of which carries `owner_id`.
const TABLES: &[&str] = &[
    "sessions",
    "turns",
    "events",
    "session_catalog",
    "pending",
    "answer_queue",
];

/// A second owner, written straight into the database: nothing in v1
/// makes one. It is never the database's owner: see `OTHER_CREATED_AT`.
const OTHER: &str = "owner-00000000000000b2";
/// `OTHER`'s `created_at` and `set_up_at`: later than any real clock
/// reaches, since the kernel's migration 3 stamps the real owner with
/// `unixepoch()` (3b-iii review, A5).
const OTHER_CREATED_AT: i64 = i64::MAX;

/// Every row of `table`, as stored, of `owner`'s or of everyone's.
fn rows(conn: &rusqlite::Connection, table: &str, owner: Option<&str>) -> Vec<Vec<Value>> {
    let filter = if owner.is_some() {
        "WHERE owner_id = ?1"
    } else {
        "WHERE ?1 IS NULL"
    };
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {table} {filter} ORDER BY rowid"))
        .unwrap();
    let width = stmt.column_count();
    stmt.query_map([owner], |r| (0..width).map(|i| r.get::<_, Value>(i)).collect())
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn all_rows(conn: &rusqlite::Connection, owner: Option<&str>) -> Vec<Vec<Vec<Value>>> {
    TABLES.iter().map(|t| rows(conn, t, owner)).collect()
}

/// A session of `owner`'s on host `host-<x>`, as the store writes one:
/// active and blocked in turn `turn-<x>`, with its first fact, its
/// catalogue, an open question and an answer queued for it.
fn write_world(conn: &rusqlite::Connection, owner: &str, x: &str) {
    let ts = "2026-10-01T00:00:00Z";
    conn.execute_batch(&format!(
        "
        INSERT INTO sessions(id, host_id, agent, cwd, agent_session_id, lifecycle, activity, open_turn_id,
                             created_at, last_event_at, owner_id)
            VALUES ('session-{x}', 'host-{x}', 'fake', '/tmp', 'agent-{x}', 'active', 'blocked', 'turn-{x}',
                    '{ts}', '{ts}', '{owner}');
        INSERT INTO turns(turn_id, session_id, content, created_at, state, owner_id)
            VALUES ('turn-{x}', 'session-{x}', '[]', '{ts}', 'started', '{owner}');
        INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
            VALUES ('session-{x}', 1, 'session_started', '{{}}', '{ts}', '{owner}');
        INSERT INTO session_catalog(session_id, config_options, updated_at, owner_id)
            VALUES ('session-{x}', '[]', '{ts}', '{owner}');
        INSERT INTO pending(pending_id, session_id, kind, turn_id, option_ids, payload, state, opened_at, owner_id)
            VALUES ('pending-{x}', 'session-{x}', 'permission', 'turn-{x}', '[\"allow\"]', '{{}}', 'open', '{ts}',
                    '{owner}');
        INSERT INTO answer_queue(pending_id, session_id, request_id, answer, submitted_at, owner_id)
            VALUES ('pending-{x}', 'session-{x}', 'request-{x}', '{{\"kind\":\"permission\",\"option_id\":\"allow\"}}',
                    '{ts}', '{owner}');
        "
    ))
    .unwrap();
}

fn allow() -> AnswerRequest {
    AnswerRequest::Permission {
        option_id: "allow".into(),
    }
}

fn update() -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Default::default(),
        payload: json!({ "n": 1 }),
    }
}

/// The store binds to the kernel's owner (3b-iii decision 2), whichever of
/// them opens a new database first: one owner, the same for all.
#[test]
fn the_store_and_the_kernel_agree_on_the_owner_whichever_opens_first() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("store-first.db");
    let store = Store::open(&first).unwrap();
    assert_eq!(Operator::open(&first).unwrap().owner_id(), store.owner_id());
    assert_eq!(Hosts::open(&first).unwrap().owner_id(), store.owner_id());
    let last = dir.path().join("store-last.db");
    let operator = Operator::open(&last).unwrap();
    assert_eq!(Store::open(&last).unwrap().owner_id(), operator.owner_id());
    for db in [first, last] {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let owners: i64 = conn.query_row("SELECT count(*) FROM owners", [], |r| r.get(0)).unwrap();
        assert_eq!(owners, 1);
    }
}

/// The backfill (3b-iii decision 4): rows written before the store had
/// `owner_id` get the database's owner, and stay visible.
#[test]
fn the_owner_id_migration_gives_every_row_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let owner = Store::open(&db).unwrap().owner_id().to_string();
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        write_world(&conn, &owner, "a");
        // Back to the store's schema before this plan (version 6).
        for table in TABLES {
            conn.execute_batch(&format!("ALTER TABLE {table} DROP COLUMN owner_id;"))
                .unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 6;").unwrap();
    }
    let store = Store::open(&db).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    for table in TABLES {
        let owners: Vec<String> = conn
            .prepare(&format!("SELECT owner_id FROM {table}"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(owners, vec![owner.clone()], "{table}");
    }
    assert!(store.session("session-a").unwrap().is_some());
    assert_eq!(store.events("session-a", 0, 10).unwrap().len(), 1);
    assert_eq!(store.open_pending("session-a").unwrap().len(), 1);
}

/// Kernel spec §1: every query of the store's filters by the owner.
/// Another owner's session, written the same way as the real owner's, in
/// the same database, is not there for any of the store's methods: each
/// finds nothing, changes nothing and writes nothing (every table,
/// whoever's rows, is as it was). The real owner's world, read the same
/// way, is found (the control).
#[test]
fn another_owners_sessions_are_invisible_to_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let store = Store::open(&db).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, ?2, ?2)",
        rusqlite::params![OTHER, OTHER_CREATED_AT],
    )
    .unwrap();
    write_world(&conn, OTHER, "b");
    write_world(&conn, store.owner_id(), "a");
    // And a session of the other owner's on the owner's own host, which the
    // host does not report: a reconcile of `host-a` that did not filter by
    // the owner would end it as one the host restarted without (ED1).
    conn.execute(
        "INSERT INTO sessions(id, host_id, agent, cwd, agent_session_id, lifecycle, activity, created_at,
                              last_event_at, owner_id)
             VALUES ('session-c', 'host-a', 'fake', '/tmp', 'agent-c', 'active', 'idle', ?1, ?1, ?2)",
        ["2026-10-01T00:00:00Z", OTHER],
    )
    .unwrap();

    // The control: the owner's world is there.
    assert!(store.session("session-a").unwrap().is_some());
    assert!(store.catalog("session-a").unwrap().is_some());
    assert_eq!(store.open_pending("session-a").unwrap().len(), 1);
    assert!(store.pending_item("pending-a").unwrap().is_some());
    assert_eq!(store.answers_to_send("host-a").unwrap().len(), 1);
    assert_eq!(store.turn_state("turn-a").unwrap().as_deref(), Some("started"));
    assert_eq!(store.committed_seq("session-a").unwrap(), 1);
    assert_eq!(store.events("session-a", 0, 10).unwrap().len(), 1);
    assert_eq!(store.hosts_with_active_sessions().unwrap(), ["host-a"]);
    assert_eq!(
        store.submit_answer("session-a", "pending-a", &allow()).unwrap(),
        AnswerSubmission::AlreadyAnswered
    );

    let before = all_rows(&conn, None);
    assert_eq!(store.session("session-b").unwrap(), None);
    assert_eq!(store.catalog("session-b").unwrap(), None);
    assert!(store.open_pending("session-b").unwrap().is_empty());
    assert_eq!(store.pending_item("pending-b").unwrap(), None);
    assert_eq!(
        store.submit_answer("session-b", "pending-b", &allow()).unwrap(),
        AnswerSubmission::NotFound
    );
    assert!(store.answers_to_send("host-b").unwrap().is_empty());
    assert_eq!(store.turn_state("turn-b").unwrap(), None);
    assert_eq!(store.ended_turn_outcome("turn-b").unwrap(), None);
    assert_eq!(store.committed_seq("session-b").unwrap(), 0);
    assert!(store.events("session-b", 0, 10).unwrap().is_empty());
    assert!(!store.open_turn("session-b", "turn-new", &[]).unwrap());
    store.abandon_turn("session-b", "turn-b").unwrap();
    store.mark_failed("session-b", "nope").unwrap();
    store.mark_failed_if_starting("session-b", "nope").unwrap();
    assert!(store.record_park_request("session-b").is_err());
    assert!(store.record_close_request("session-b").is_err());
    assert!(store.close_now("session-b").unwrap().is_empty());
    assert!(
        store
            .close_after_rejected_reconcile_close("session-b")
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.request_resume("session-b").unwrap(), ResumeRequest::NotFound);
    assert!(store.presume_parked("host-b").unwrap().is_empty());
    assert!(store.revoke_host("host-b").unwrap().is_empty());
    assert_eq!(store.reconcile_host("host-b", &[]).unwrap(), Reconciliation::default());
    assert!(store.ingest("session-b", 2, &update()).is_err());
    assert_eq!(all_rows(&conn, None), before);
    assert_eq!(rows(&conn, "sessions", Some(OTHER)).len(), 2);

    // The owner's own host naming the other owner's session (3b-iii review,
    // O3): theirs, `session-b` and `session-c` on `host-a` itself (ED1), are
    // untouched, and the owner's is reconciled as one the host no longer has.
    let theirs = all_rows(&conn, Some(OTHER));
    let attached = [AttachedSession {
        session_id: "session-b".into(),
        last_seq: 1,
        open_turn_id: Some("turn-b".into()),
    }];
    let reconciled = store.reconcile_host("host-a", &attached).unwrap();
    assert!(reconciled.close.is_empty());
    assert!(!reconciled.events.is_empty());
    assert!(
        reconciled.events.iter().all(|e| e.session_id == "session-a"),
        "{reconciled:?}"
    );
    assert_eq!(all_rows(&conn, Some(OTHER)), theirs);
    assert_eq!(store.session("session-a").unwrap().unwrap().lifecycle, "parked");
}
