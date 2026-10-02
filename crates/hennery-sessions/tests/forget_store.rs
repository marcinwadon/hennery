//! The agent's own transcript on its host, in the store (plan 9d decisions
//! 1–3, 6, B8, O10): the homes a session records, the records a delete
//! writes, and what becomes of them.

use hennery_proto::frames::{AgentHome, ForgetReason, SessionBody};
use hennery_proto::rest::{HostRemovalState, RemovalPending, RemovalState, TranscriptRemoval};
use hennery_sessions::store::{Deletion, HostForgets, MAX_AGENT_HOMES, Store};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn file_store(dir: &Path) -> (Store, PathBuf) {
    let db = dir.join("hennery.db");
    (Store::open(&db).unwrap(), db)
}

fn home(root: &str) -> AgentHome {
    AgentHome {
        root: root.into(),
        sqlite_root: None,
    }
}

fn started_with(id: &str, agent_session_id: &str, agent_home: Option<AgentHome>) -> SessionBody {
    SessionBody::SessionStarted {
        request_id: format!("r-{id}"),
        agent_session_id: agent_session_id.into(),
        indexed: Default::default(),
        agent_home,
    }
}

/// `id` on `h1`, started by `agent` with `agent_session_id` in `root`.
fn started(store: &Store, id: &str, agent: &str, agent_session_id: &str, root: Option<&str>) {
    store
        .create_session(id, "h1", agent, "/srv/app", "hat-1", None)
        .unwrap();
    store
        .ingest(id, 1, &started_with(id, agent_session_id, root.map(home)))
        .unwrap();
}

fn recorded(conn: &Connection, id: &str) -> Option<serde_json::Value> {
    conn.query_row("SELECT agent_home FROM sessions WHERE id = ?1", [id], |r| {
        r.get::<_, Option<String>>(0)
    })
    .unwrap()
    .map(|raw| serde_json::from_str(&raw).unwrap())
}

/// Close and delete `id`: what it left for its host.
fn delete(store: &Store, id: &str) -> HostForgets {
    store.close_now(id).unwrap();
    match store.delete_session(id, None).unwrap() {
        Deletion::Done { forgets, .. } => *forgets,
        other => panic!("not deleted: {other:?}"),
    }
}

fn pending_result(why: RemovalPending) -> TranscriptRemoval {
    TranscriptRemoval {
        state: RemovalState::Pending,
        pending: Some(why),
        remaining: Vec::new(),
        notes: Vec::new(),
    }
}

/// Decision 1, B8: each distinct (agent session id, roots) a start or
/// resume reports is recorded, once, at most `MAX_AGENT_HOMES`; one that
/// fails the shape check is not; a re-sent `session_started` that changes
/// nothing records nothing.
#[test]
fn a_session_records_each_distinct_agent_home_it_reports_up_to_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let conn = Connection::open(&db).unwrap();
    started(&store, "s1", "claude", "a1", Some("/h/.claude"));
    assert_eq!(
        recorded(&conn, "s1"),
        Some(serde_json::json!([{"agent_session_id": "a1", "root": "/h/.claude"}]))
    );
    // A restart's re-sent `session_started` for the active session: not
    // applied, not recorded.
    store
        .ingest("s1", 2, &started_with("s1", "a1", Some(home("/elsewhere"))))
        .unwrap();
    assert_eq!(recorded(&conn, "s1").unwrap().as_array().unwrap().len(), 1);

    let mut seq = 2;
    let mut resume = |root: &str, id: &str| {
        store.mark_failed("s1", "x").unwrap();
        seq += 1;
        store
            .ingest("s1", seq, &started_with("s1", id, Some(home(root))))
            .unwrap();
    };
    resume("/h/.claude", "a1");
    resume("relative", "a1");
    resume("/h/\0nul", "a1");
    resume("/h/.claude", "a2");
    assert_eq!(
        recorded(&conn, "s1"),
        Some(serde_json::json!([
            {"agent_session_id": "a1", "root": "/h/.claude"},
            {"agent_session_id": "a2", "root": "/h/.claude"}
        ]))
    );
    for n in 0..MAX_AGENT_HOMES + 2 {
        resume(&format!("/root-{n}"), "a1");
    }
    assert_eq!(
        recorded(&conn, "s1").unwrap().as_array().unwrap().len(),
        MAX_AGENT_HOMES
    );
}

/// Decision 2, B8: a delete writes one record per pair, in its own
/// transaction, with the agent's ids and roots only, and clears the
/// session's home. An agent session id with no home (from before 9d) gets
/// a final record (decision 11); a session with no agent record gets none.
#[test]
fn a_delete_writes_one_record_per_pair_and_keeps_no_content() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let conn = Connection::open(&db).unwrap();
    started(&store, "s1", "claude", "a1", Some("/h/.claude"));
    store.mark_failed("s1", "x").unwrap();
    store
        .ingest("s1", 2, &started_with("s1", "a2", Some(home("/h/other"))))
        .unwrap();
    let forgets = delete(&store, "s1");
    assert!(forgets.had_agent_record);
    assert_eq!(forgets.agent, "claude");
    let pairs: Vec<(String, Option<String>, HostRemovalState)> = forgets
        .records
        .iter()
        .map(|r| {
            (
                r.agent_session_id.clone(),
                r.agent_home.as_ref().map(|h| h.root.clone()),
                r.state,
            )
        })
        .collect();
    assert_eq!(
        pairs,
        [
            ("a1".into(), Some("/h/.claude".into()), HostRemovalState::Pending),
            ("a2".into(), Some("/h/other".into()), HostRemovalState::Pending),
        ]
    );
    assert_eq!(recorded(&conn, "s1"), None);
    assert_eq!(store.forgets_to_send("h1").unwrap(), forgets.records);
    assert_eq!(store.forgets_of_session("s1").unwrap(), forgets.records);
    // The rows hold no cwd, title or content of the session.
    let dump: String = conn
        .query_row(
            "SELECT group_concat(id || host_id || session_id || hat_id || agent || agent_session_id
                     || COALESCE(agent_home, '') || created_at, '|') FROM host_forgets",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!dump.contains("/srv/app"), "{dump}");

    // From before 9d: an agent session id, no home. Final from the start.
    store
        .create_session("s2", "h1", "claude", "/srv/b", "hat-1", None)
        .unwrap();
    store.ingest("s2", 1, &SessionBody::session_started("r", "b1")).unwrap();
    let forgets = delete(&store, "s2");
    let [record] = forgets.records.as_slice() else {
        panic!("{forgets:?}");
    };
    assert_eq!(
        (record.state, record.agent_home.as_ref()),
        (HostRemovalState::Final, None)
    );
    assert_eq!(
        record.last_result.as_ref().unwrap().remaining[0].reason,
        ForgetReason::NoRecordedHome
    );
    assert!(
        store
            .forgets_to_send("h1")
            .unwrap()
            .iter()
            .all(|r| r.session_id == "s1")
    );

    // Never started: nothing on the host.
    store
        .create_session("s3", "h1", "claude", "/srv/c", "hat-1", None)
        .unwrap();
    let forgets = delete(&store, "s3");
    assert!(!forgets.had_agent_record && forgets.records.is_empty());
}

/// B8: no record for an agent session another kept session of the same
/// host and agent refers to, by its id or among its recorded pairs; a
/// tombstone refers to nothing.
#[test]
fn no_record_for_an_agent_session_another_kept_session_refers_to() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _db) = file_store(dir.path());
    started(&store, "s1", "claude", "a1", Some("/h/.claude"));
    started(&store, "s2", "claude", "a1", Some("/h/.claude"));
    let forgets = delete(&store, "s1");
    assert_eq!((forgets.shared, forgets.records.len()), (1, 0));

    // Among the recorded pairs, not the current id.
    started(&store, "s3", "claude", "a3", Some("/h/.claude"));
    store.mark_failed("s3", "x").unwrap();
    store
        .ingest("s3", 2, &started_with("s3", "a4", Some(home("/h/.claude"))))
        .unwrap();
    started(&store, "s4", "claude", "a3", Some("/h/.claude"));
    let forgets = delete(&store, "s4");
    assert_eq!((forgets.shared, forgets.records.len()), (1, 0));

    // By the current id alone: a session from before 9d records no pairs.
    started(&store, "s7", "claude", "a9", None);
    started(&store, "s8", "claude", "a9", Some("/h/.claude"));
    let forgets = delete(&store, "s8");
    assert_eq!((forgets.shared, forgets.records.len()), (1, 0));

    // Another agent or another host does not share it.
    store
        .create_session("s5", "h2", "claude", "/srv", "hat-1", None)
        .unwrap();
    store
        .ingest("s5", 1, &started_with("s5", "a1", Some(home("/h/.claude"))))
        .unwrap();
    store
        .create_session("s6", "h1", "codex", "/srv", "hat-1", None)
        .unwrap();
    store
        .ingest("s6", 1, &started_with("s6", "a1", Some(home("/h/.codex"))))
        .unwrap();
    // s1's tombstone no longer refers to a1: s2 is the last.
    let forgets = delete(&store, "s2");
    assert_eq!((forgets.shared, forgets.records.len()), (0, 1));
}

/// Decision 6, O10: a complete removal deletes its record; a result that
/// no retry changes makes it final; anything else keeps it pending, its
/// attempts counted only when sent; a dismissed record is gone.
#[test]
fn a_records_attempts_decide_whether_it_stays() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _db) = file_store(dir.path());
    for (id, a) in [("s1", "a1"), ("s2", "a2"), ("s3", "a3")] {
        started(&store, id, "claude", a, Some("/h/.claude"));
        delete(&store, id);
    }
    let ids: Vec<String> = store.forgets_to_send("h1").unwrap().into_iter().map(|r| r.id).collect();
    let [r1, r2, r3] = ids.as_slice() else {
        panic!("{ids:?}");
    };
    store
        .forget_attempted(r1, &pending_result(RemovalPending::HostOffline), false, false)
        .unwrap();
    store
        .forget_attempted(r1, &pending_result(RemovalPending::NoReply), true, false)
        .unwrap();
    let removed = TranscriptRemoval {
        state: RemovalState::Removed,
        pending: None,
        remaining: Vec::new(),
        notes: Vec::new(),
    };
    store.forget_attempted(r2, &removed, true, true).unwrap();
    let left = hennery_sessions::store::final_result(ForgetReason::Symlink);
    store.forget_attempted(r3, &left, true, true).unwrap();

    let all = store.host_removals().unwrap();
    let states: Vec<(&str, HostRemovalState, u32)> = all.iter().map(|r| (r.id.as_str(), r.state, r.attempts)).collect();
    assert_eq!(
        states,
        [
            (r1.as_str(), HostRemovalState::Pending, 1),
            (r3.as_str(), HostRemovalState::Final, 1)
        ]
    );
    assert_eq!(all[0].last_result, Some(pending_result(RemovalPending::NoReply)));
    // A final record is not retried, nor changed by a late attempt.
    store
        .forget_attempted(r3, &pending_result(RemovalPending::NoReply), true, false)
        .unwrap();
    assert_eq!(store.host_removals().unwrap()[1].last_result, Some(left));
    assert_eq!(store.forgets_to_send("h1").unwrap().len(), 1);
    assert!(store.dismiss_forget(r3).unwrap());
    assert!(!store.dismiss_forget(r3).unwrap());
    assert_eq!(store.host_removals().unwrap().len(), 1);
}

/// O10: a revoked host's records are final, those it had and those a
/// delete writes after.
#[test]
fn a_revoked_hosts_records_are_final() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let hosts = hennery_kernel::hosts::Hosts::open(&db).unwrap();
    let enrollment = hennery_kernel::hosts::Enrollment {
        public_key: "aa".repeat(32),
        name: "h".into(),
        host_version: "t".into(),
        platform: "t".into(),
    };
    hosts.register("h1", &enrollment, 0).unwrap();
    started(&store, "s1", "claude", "a1", Some("/h/.claude"));
    started(&store, "s2", "claude", "a2", Some("/h/.claude"));
    delete(&store, "s1");
    hosts.revoke("h1", 1).unwrap();
    store.revoke_host("h1").unwrap();
    store.close_now("s2").unwrap();
    let Deletion::Done { forgets, .. } = store.delete_session("s2", None).unwrap() else {
        panic!("not deleted");
    };
    assert_eq!(forgets.records[0].state, HostRemovalState::Final);
    let all = store.host_removals().unwrap();
    assert!(all.iter().all(|r| r.state == HostRemovalState::Final), "{all:?}");
    assert!(
        all.iter()
            .all(|r| r.last_result.as_ref().unwrap().remaining[0].reason == ForgetReason::HostRevoked)
    );
    assert!(store.forgets_to_send("h1").unwrap().is_empty());
}

fn stored_home(db: &Path, id: &str) -> Option<String> {
    Connection::open(db)
        .unwrap()
        .query_row("SELECT agent_home FROM host_forgets WHERE id = ?1", [id], |r| r.get(0))
        .unwrap()
}

/// The review's item 12: a record keeps its roots only while it may still
/// be sent: a final one, by an attempt, a revoke or at its writing, has
/// none.
#[test]
fn a_final_record_keeps_no_roots() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    for (id, a) in [("s1", "a1"), ("s2", "a2"), ("s3", "a3")] {
        started(&store, id, "claude", a, Some("/h/.claude"));
    }
    delete(&store, "s1");
    delete(&store, "s2");
    let records = store.forgets_to_send("h1").unwrap();
    let (r1, r2) = (&records[0].id, &records[1].id);
    assert!(stored_home(&db, r1).is_some());
    let left = hennery_sessions::store::final_result(ForgetReason::Symlink);
    store.forget_attempted(r1, &left, true, true).unwrap();
    assert_eq!(stored_home(&db, r1), None);
    store
        .forget_attempted(r2, &pending_result(RemovalPending::NoReply), true, false)
        .unwrap();
    assert!(stored_home(&db, r2).is_some());
    let hosts = hennery_kernel::hosts::Hosts::open(&db).unwrap();
    let enrollment = hennery_kernel::hosts::Enrollment {
        public_key: "aa".repeat(32),
        name: "h".into(),
        host_version: "t".into(),
        platform: "t".into(),
    };
    hosts.register("h1", &enrollment, 0).unwrap();
    hosts.revoke("h1", 1).unwrap();
    store.revoke_host("h1").unwrap();
    assert_eq!(stored_home(&db, r2), None);
    let written = delete(&store, "s3");
    assert_eq!(written.records[0].agent_home, None);
    assert_eq!(stored_home(&db, &written.records[0].id), None);
}
