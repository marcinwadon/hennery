//! `owner_id` in the sessions store (kernel spec §1; plan 3b-iii): every
//! row carries it, the migration fills it in, and every query names it.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{AttachedSession, SessionBody};
use hennery_proto::rest::AnswerRequest;
use hennery_sessions::store::{AnswerSubmission, ListQuery, Reconciliation, ResumeRequest, Store};
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
        // Back to the store's schema before this plan (version 6): plan
        // 9a's migration (version 13), 10b-iii's (version 11), 5c's (version
        // 10) and 6b's (version 9) undone first, their indexes on `owner_id`
        // included; the events index (version 12) is made again if missing.
        conn.execute_batch(
            "
            DROP TRIGGER events_of_a_tombstone;
            DROP TRIGGER turns_of_a_tombstone;
            DROP TRIGGER pending_of_a_tombstone;
            DROP TRIGGER answers_of_a_tombstone;
            DROP TRIGGER catalog_of_a_tombstone;
            DROP TRIGGER a_tombstone_stays;
            DROP TABLE turn_attachments;
            DROP TABLE host_forgets;
            ALTER TABLE sessions DROP COLUMN agent_home;
            DROP INDEX events_by_kind;
            ALTER TABLE pending DROP COLUMN opened_event_id;
            DROP INDEX sessions_by_hat;
            ALTER TABLE sessions DROP COLUMN hat_id;
            ALTER TABLE sessions DROP COLUMN hat_rule_id;
            DROP INDEX sessions_by_recency;
            ALTER TABLE sessions DROP COLUMN title;
            ALTER TABLE sessions DROP COLUMN git_branch;
            ALTER TABLE sessions DROP COLUMN git_dirty;
            ALTER TABLE sessions DROP COLUMN git_worktree;
            ALTER TABLE sessions DROP COLUMN base_commit;
            ALTER TABLE sessions DROP COLUMN last_event_id;
            ALTER TABLE sessions DROP COLUMN mcp_delivery_mode;
            ALTER TABLE sessions DROP COLUMN mcp_delivery_servers;
            ALTER TABLE sessions DROP COLUMN mcp_delivery_at;
            ALTER TABLE session_catalog DROP COLUMN commands;
            ",
        )
        .unwrap();
        for table in TABLES {
            conn.execute_batch(&format!("ALTER TABLE {table} DROP COLUMN owner_id;"))
                .unwrap();
        }
        // And without the tables of the migrations after it.
        conn.execute_batch("DROP TABLE event_attachments; DROP TABLE attachments;")
            .unwrap();
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
    assert!(store.find_session("session-a").unwrap().is_some());
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

    // Both owners' sessions in one hat, for the hat filter.
    conn.execute(
        "UPDATE sessions SET hat_id = 'hat-x' WHERE id IN ('session-a', 'session-b')",
        [],
    )
    .unwrap();

    // The control: the owner's world is there.
    assert!(store.find_session("session-a").unwrap().is_some());
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
    assert_eq!(store.find_session("session-b").unwrap(), None);
    assert_eq!(store.find_session_item("session-b").unwrap(), None);
    // The list, and a search that would match only the other owner's.
    let listed: Vec<String> = store
        .list(&ListQuery::default())
        .unwrap()
        .sessions
        .into_iter()
        .map(|s| s.session_id)
        .collect();
    assert_eq!(listed, ["session-a"]);
    let search = ListQuery {
        search: Some("session-b"),
        ..ListQuery::default()
    };
    assert!(store.list(&search).unwrap().sessions.is_empty());
    // And a hat filter naming a hat both owners' sessions carry.
    let by_hat = ListQuery {
        hat: Some("hat-x"),
        ..ListQuery::default()
    };
    let listed: Vec<String> = store
        .list(&by_hat)
        .unwrap()
        .sessions
        .into_iter()
        .map(|s| s.session_id)
        .collect();
    assert_eq!(listed, ["session-a"]);
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
    assert_eq!(
        store.request_resume("session-b", "hat-1").unwrap(),
        ResumeRequest::NotFound
    );
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
    assert_eq!(store.find_session("session-a").unwrap().unwrap().lifecycle, "parked");
}

/// Plan 6a: an attachment is the owner's who sent it. Another owner's row,
/// its file on disk under the same name, is neither served nor counted;
/// the owner's own copy of the same image is a row of its own.
#[test]
fn another_owners_attachment_is_neither_served_nor_counted() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let store = Store::open(&db).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, ?2, ?2)",
        rusqlite::params![OTHER, OTHER_CREATED_AT],
    )
    .unwrap();
    let bytes = b"GIF89a, another owner's".to_vec();
    let image = json!({
        "type": "image", "mimeType": "image/gif",
        "data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes),
    });
    let checked = hennery_sessions::content::check(vec![image]).unwrap();
    let sha = checked.images[0].sha256.clone();
    conn.execute(
        "INSERT INTO attachments(owner_id, sha256, mime, size, created_at) VALUES (?1, ?2, 'image/gif', ?3, ?4)",
        rusqlite::params![OTHER, sha, bytes.len() as i64, "2026-10-01T00:00:00Z"],
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("attachments")).unwrap();
    std::fs::write(dir.path().join("attachments").join(&sha), &bytes).unwrap();
    assert!(store.attachment(&sha).unwrap().is_none());
    assert_eq!(store.attachment_usage().unwrap().count, 0);

    // The owner sends the same image: a row of their own, and it is served.
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
    store.save_images(&checked.images).unwrap();
    assert!(store.open_prompt("s1", "t1", &checked).unwrap());
    assert_eq!(store.attachment(&sha).unwrap().unwrap().bytes, bytes);
    assert_eq!(store.attachment_usage().unwrap().count, 1);
    let owners: Vec<String> = conn
        .prepare("SELECT owner_id FROM attachments ORDER BY owner_id = ?1")
        .unwrap()
        .query_map([OTHER], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(owners, [store.owner_id(), OTHER]);
}

/// The view's reads (plan 4a-ii; A-10) name the owner too: the other
/// owner's events, group starts, questions and sessions are not there for
/// them, even where they would be the newest or the only ones.
#[test]
fn the_views_reads_see_only_the_owners_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let store = Store::open(&db).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, ?2, ?2)",
        rusqlite::params![OTHER, OTHER_CREATED_AT],
    )
    .unwrap();
    write_world(&conn, store.owner_id(), "a");
    write_world(&conn, OTHER, "b");
    // Each owner's session gets a turn, a question with no answer (and one
    // cancelled, which no one can answer) and a turn not delivered, the
    // other owner's last: its events are the newest of all.
    for (owner, x) in [(store.owner_id(), "a"), (OTHER, "b")] {
        conn.execute_batch(&format!(
            "
            INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
                VALUES ('session-{x}', NULL, 'user_turn', '{{\"turn_id\":\"turn-{x}\",\"content\":[]}}',
                        '2026-10-01T00:00:01Z', '{owner}');
            INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
                VALUES ('session-{x}', 2, 'pending_opened', '{{\"pending_id\":\"open-{x}\"}}',
                        '2026-10-01T00:00:02Z', '{owner}');
            INSERT INTO pending(pending_id, session_id, kind, turn_id, payload, state, opened_at, owner_id)
                VALUES ('open-{x}', 'session-{x}', 'permission', 'turn-{x}', '{{}}', 'open',
                        '2026-10-01T00:00:02Z', '{owner}');
            INSERT INTO pending(pending_id, session_id, kind, turn_id, payload, state, opened_at, owner_id)
                VALUES ('gone-{x}', 'session-{x}', 'permission', 'turn-{x}', '{{}}', 'cancelled',
                        '2026-10-01T00:00:02Z', '{owner}');
            INSERT INTO turns(turn_id, session_id, content, created_at, owner_id, state)
                VALUES ('lost-{x}', 'session-{x}', '[]', '2026-10-01T00:00:03Z', '{owner}', 'not_delivered');
            "
        ))
        .unwrap();
    }
    let last = |x: &str| -> i64 {
        conn.query_row(
            "SELECT MAX(event_id) FROM events WHERE session_id = ?1",
            [format!("session-{x}")],
            |r| r.get(0),
        )
        .unwrap()
    };
    let (a, b) = (last("a"), last("b"));
    assert!(b > a);

    // The control: the owner's rows are found.
    assert_eq!(store.revision("session-a").unwrap(), a);
    assert_eq!(store.max_event_id().unwrap(), a);
    assert_eq!(store.group_starts("session-a", i64::MAX, 10).unwrap(), [a - 1]);
    assert_eq!(store.turn_start("session-a", "turn-a").unwrap(), Some(a - 1));
    assert_eq!(
        store.group_start_at("session-a", a).unwrap(),
        (a - 1, Some("turn-a".into()))
    );
    assert_eq!(store.events_between("session-a", 0, i64::MAX, 10).unwrap().len(), 3);
    assert_eq!(store.count_after("session-a", 0, 10, 10).unwrap(), (3, 1));
    assert!(store.session_pending("session-a", "open-a").unwrap().is_some());
    assert_eq!(store.pending_last_event("session-a", "open-a").unwrap(), Some(a));
    assert_eq!(store.pending_touched_after("session-a", 0).unwrap(), ["open-a"]);
    assert_eq!(store.sessions_waiting().unwrap(), ["session-a".to_string()].into());
    // The count: `session-a` waits; the other owner's (blocked, with an
    // open question) does not count, nor does the other owner's open
    // question naming a session of the owner's that waits for nothing.
    conn.execute_batch(&format!(
        "
        INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, activity, created_at, last_event_at, owner_id)
            VALUES ('session-c', 'host-a', 'fake', '/tmp', 'active', 'idle', '2026-10-01T00:00:00Z',
                    '2026-10-01T00:00:00Z', '{owner}');
        INSERT INTO pending(pending_id, session_id, kind, turn_id, payload, state, opened_at, owner_id)
            VALUES ('sneak', 'session-c', 'permission', 'turn-c', '{{}}', 'open', '2026-10-01T00:00:02Z', '{OTHER}');
        ",
        owner = store.owner_id()
    ))
    .unwrap();
    assert_eq!(store.waiting_count(None).unwrap(), 1);
    assert_eq!(store.waiting_count(Some("")).unwrap(), 1);
    assert_eq!(store.sessions_changed_after(0).unwrap(), ["session-a"]);
    assert!(store.event_ts(a).unwrap().is_some());
    assert_eq!(
        store.undelivered_turn("session-a", "lost-a").unwrap().as_deref(),
        Some("[]")
    );
    // A question's opening event: by its body when its record names none
    // (a question from before plan 10b-iii), else the one its record names.
    let opened_a = store.pending_opened_at("session-a", "open-a").unwrap();
    assert_eq!(opened_a.as_ref().map(|(id, _)| *id), Some(a));
    let name = |event_id: i64| {
        conn.execute(
            "UPDATE pending SET opened_event_id = ?1 WHERE pending_id = 'open-a'",
            [event_id],
        )
        .unwrap();
    };
    name(a);
    assert_eq!(store.pending_opened_at("session-a", "open-a").unwrap(), opened_a);
    // A record naming another owner's event finds nothing.
    name(b);
    assert_eq!(store.pending_opened_at("session-a", "open-a").unwrap(), None);
    name(a);

    // The other owner's.
    assert_eq!(store.revision("session-b").unwrap(), 0);
    assert!(store.group_starts("session-b", i64::MAX, 10).unwrap().is_empty());
    assert_eq!(store.turn_start("session-b", "turn-b").unwrap(), None);
    assert_eq!(store.group_start_at("session-b", b).unwrap(), (0, None));
    assert_eq!(store.first_event_after("session-b", 0).unwrap(), None);
    assert!(!store.has_events_before("session-b", b + 1).unwrap());
    assert!(store.events_between("session-b", 0, i64::MAX, 10).unwrap().is_empty());
    assert_eq!(store.count_after("session-b", 0, 10, 10).unwrap(), (0, 0));
    assert_eq!(store.session_pending("session-b", "open-b").unwrap(), None);
    // Nor through the owner's own session.
    assert_eq!(store.session_pending("session-a", "open-b").unwrap(), None);
    assert_eq!(store.pending_last_event("session-b", "open-b").unwrap(), None);
    assert_eq!(store.pending_opened_at("session-b", "open-b").unwrap(), None);
    assert!(store.pending_touched_after("session-b", 0).unwrap().is_empty());
    assert_eq!(store.event_ts(b).unwrap(), None);
    assert_eq!(store.undelivered_turn("session-b", "lost-b").unwrap(), None);
    assert_eq!(store.undelivered_turn("session-a", "lost-b").unwrap(), None);
}
