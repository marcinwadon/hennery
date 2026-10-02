//! A hat's purge, the sessions store's part (kernel spec §5.5; plan 9c
//! decisions 10 and 11, A3, A13): a frozen hat gets no session, no resume
//! and no re-assignment in or out, each refused inside its own transaction;
//! and what a purge reads, the hat's kept sessions and the sessions of no
//! hat.

use hennery_kernel::hats::{HatChange, PurgeStart};
use hennery_kernel::hosts::Hosts;
use hennery_proto::frames::{ParkReason, SessionBody};
use hennery_sessions::store::{Deletion, HatSession, Reassign, ResumeRequest, Store};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const NOW: i64 = 4_000_000_000;

/// The store and the registry over one `hennery.db` in `dir`, and its path:
/// the freeze is the registry's, the refusals the store's.
fn open(dir: &Path) -> (Store, Hosts, PathBuf) {
    let db = dir.join("hennery.db");
    (Store::open(&db).unwrap(), Hosts::open(&db).unwrap(), db)
}

fn new_hat(hosts: &Hosts, name: &str) -> String {
    match hosts.create_hat(name, None, NOW).unwrap() {
        HatChange::Done(hat) => hat.id,
        other => panic!("expected a hat, got {other:?}"),
    }
}

fn freeze(hosts: &Hosts, hat: &str) {
    assert!(matches!(
        hosts.begin_purge(hat, NOW).unwrap(),
        PurgeStart::Frozen { .. }
    ));
}

/// `id` on `h1` in `hat`, started (`active`) and parked by its host.
fn parked(store: &Store, id: &str, hat: &str) {
    assert!(store.create_session(id, "h1", "fake", "/p", hat, None).unwrap());
    store.ingest(id, 1, &SessionBody::session_started("r0", "a0")).unwrap();
    store
        .ingest(
            id,
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
}

/// Every column of a session's row and its events' kinds, as stored.
fn snapshot(conn: &Connection, id: &str) -> (Vec<rusqlite::types::Value>, Vec<String>) {
    let mut stmt = conn.prepare("SELECT * FROM sessions WHERE id = ?1").unwrap();
    let width = stmt.column_count();
    let row = stmt
        .query_row([id], |r| (0..width).map(|i| r.get(i)).collect())
        .unwrap();
    let kinds = conn
        .prepare("SELECT kind FROM events WHERE session_id = ?1 ORDER BY event_id")
        .unwrap()
        .query_map([id], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    (row, kinds)
}

/// Decision 10c: a start that resolved its hat before the freeze and is
/// stored after it is refused, and stores nothing; another hat's is not.
#[test]
fn a_frozen_hat_gets_no_new_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, hosts, _) = open(dir.path());
    let acme = new_hat(&hosts, "Acme");
    let other = new_hat(&hosts, "Other");
    freeze(&hosts, &acme);
    assert!(!store.create_session("s1", "h1", "fake", "/p", &acme, None).unwrap());
    assert_eq!(store.find_session("s1").unwrap(), None);
    assert_eq!(store.session_host("s1").unwrap(), None);
    assert!(store.create_session("s2", "h1", "fake", "/p", &other, None).unwrap());
    assert_eq!(store.find_session("s2").unwrap().unwrap().hat_id, other);
}

/// Decision 10c: a resume of a session of a frozen hat is refused, even
/// when its path still resolves to that hat, and changes nothing.
#[test]
fn a_session_of_a_frozen_hat_does_not_resume() {
    let dir = tempfile::tempdir().unwrap();
    let (store, hosts, db) = open(dir.path());
    let acme = new_hat(&hosts, "Acme");
    parked(&store, "s1", &acme);
    let conn = Connection::open(&db).unwrap();
    let before = snapshot(&conn, "s1");
    freeze(&hosts, &acme);
    assert_eq!(store.request_resume("s1", &acme).unwrap(), ResumeRequest::HatPurging);
    // Its path resolves elsewhere by now (the freeze took the hat's rules):
    // still the purge, not a mismatch, is the answer.
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    assert_eq!(
        store.request_resume("s1", &personal).unwrap(),
        ResumeRequest::HatPurging
    );
    assert_eq!(snapshot(&conn, "s1"), before);
}

/// A3: a session is not re-assigned out of a frozen hat, nor into one, and
/// nothing changes.
#[test]
fn a_session_does_not_move_in_or_out_of_a_frozen_hat() {
    let dir = tempfile::tempdir().unwrap();
    let (store, hosts, db) = open(dir.path());
    let acme = new_hat(&hosts, "Acme");
    let other = new_hat(&hosts, "Other");
    parked(&store, "in-acme", &acme);
    parked(&store, "in-other", &other);
    let conn = Connection::open(&db).unwrap();
    let before = (snapshot(&conn, "in-acme"), snapshot(&conn, "in-other"));
    freeze(&hosts, &acme);
    assert_eq!(store.reassign_hat("in-acme", &other).unwrap(), Reassign::HatPurging);
    assert_eq!(store.reassign_hat("in-other", &acme).unwrap(), Reassign::HatPurging);
    assert_eq!(store.reassign_hat("in-acme", &acme).unwrap(), Reassign::HatPurging);
    assert_eq!((snapshot(&conn, "in-acme"), snapshot(&conn, "in-other")), before);
    // Control: between two hats that are not frozen, it moves.
    let third = new_hat(&hosts, "Third");
    assert!(matches!(
        store.reassign_hat("in-other", &third).unwrap(),
        Reassign::Done(_)
    ));
}

/// Decisions 10 and 11, the 5d hand-on: a hat's sessions are those of
/// every lifecycle and host whose hat it is now, re-assigned in included,
/// re-assigned out not; never a tombstone.
#[test]
fn a_hats_sessions_are_its_kept_sessions_of_every_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let (store, hosts, _) = open(dir.path());
    let acme = new_hat(&hosts, "Acme");
    let other = new_hat(&hosts, "Other");
    assert!(
        store
            .create_session("starting", "h1", "fake", "/p", &acme, None)
            .unwrap()
    );
    assert!(store.create_session("active", "h2", "fake", "/p", &acme, None).unwrap());
    store
        .ingest("active", 1, &SessionBody::session_started("r0", "a0"))
        .unwrap();
    assert!(
        store
            .create_session("presumed", "h3", "fake", "/p", &acme, None)
            .unwrap()
    );
    store
        .ingest("presumed", 1, &SessionBody::session_started("r0", "a0"))
        .unwrap();
    store.presume_parked("h3").unwrap();
    parked(&store, "moved-in", &other);
    assert!(matches!(
        store.reassign_hat("moved-in", &acme).unwrap(),
        Reassign::Done(_)
    ));
    parked(&store, "moved-out", &acme);
    assert!(matches!(
        store.reassign_hat("moved-out", &other).unwrap(),
        Reassign::Done(_)
    ));
    assert!(store.create_session("failed", "h1", "fake", "/p", &acme, None).unwrap());
    store.mark_failed("failed", "spawn").unwrap();
    parked(&store, "gone", &acme);
    store.close_now("gone").unwrap();
    assert!(matches!(
        store.delete_session("gone", None).unwrap(),
        Deletion::Done { .. }
    ));

    let session = |id: &str, lifecycle: &str, presumed_parked: bool, host_id: &str| HatSession {
        id: id.into(),
        lifecycle: lifecycle.into(),
        presumed_parked,
        host_id: host_id.into(),
    };
    assert_eq!(
        store.hat_sessions(&acme).unwrap(),
        [
            session("active", "active", false, "h2"),
            session("failed", "failed", false, "h1"),
            session("moved-in", "parked", false, "h1"),
            session("presumed", "parked", true, "h3"),
            session("starting", "starting", false, "h1"),
        ]
    );
    assert_eq!(
        store.hat_sessions(&other).unwrap(),
        [session("moved-out", "parked", false, "h1")]
    );
}

/// Decision 11, the 5c hand-on: the sessions of no hat are listed for the
/// operator, the newest first, at most as many as asked, with how many
/// there are; never a tombstone.
#[test]
fn the_sessions_of_no_hat_are_listed_with_their_count() {
    let dir = tempfile::tempdir().unwrap();
    let (store, hosts, _) = open(dir.path());
    let acme = new_hat(&hosts, "Acme");
    for id in ["none-1", "none-2", "none-3"] {
        assert!(store.create_session(id, "h1", "fake", "/p", "", None).unwrap());
    }
    parked(&store, "none-gone", "");
    store.close_now("none-gone").unwrap();
    assert!(matches!(
        store.delete_session("none-gone", None).unwrap(),
        Deletion::Done { .. }
    ));
    assert!(store.create_session("hatted", "h1", "fake", "/p", &acme, None).unwrap());

    let (listed, count) = store.unassigned(2).unwrap();
    let ids: Vec<&str> = listed.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!((ids, count), (vec!["none-3", "none-2"], 3));
    assert!(listed.iter().all(|s| s.hat_id.is_empty()));
    let (listed, count) = store.unassigned(100).unwrap();
    assert_eq!((listed.len(), count), (3, 3));
}
