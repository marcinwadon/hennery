use hennery_proto::frames::{Indexed, SessionBody, TurnOutcome};
use hennery_sessions::{hub::Hub, store::Store};
use serde_json::json;

fn start(store: &Store, id: &str) {
    store.create_session(id, "host", "fake", "/tmp", "hat", None).unwrap();
    store
        .ingest(id, 1, &SessionBody::session_started("start", "agent"))
        .unwrap();
}
fn finish(store: &Store, session: &str, turn: &str, seq: u64, outcome: TurnOutcome) -> i64 {
    assert!(
        store
            .open_turn(session, turn, &[json!({"type":"text","text":"work"})])
            .unwrap()
    );
    store
        .ingest(
            session,
            seq,
            &SessionBody::TurnStarted {
                request_id: turn.into(),
                turn_id: turn.into(),
            },
        )
        .unwrap();
    store
        .ingest(
            session,
            seq + 1,
            &SessionBody::AcpUpdate {
                indexed: Indexed::default(),
                payload: json!({"text":"result"}),
            },
        )
        .unwrap();
    store
        .ingest(
            session,
            seq + 2,
            &SessionBody::TurnEnded {
                turn_id: turn.into(),
                outcome,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap()[0]
        .event_id
}

#[test]
fn completed_and_failed_results_wait_until_read_without_a_reply() {
    let store = Store::open_in_memory().unwrap();
    let hub = Hub::new();
    start(&store, "s");
    for (n, outcome) in [TurnOutcome::Completed, TurnOutcome::Failed].into_iter().enumerate() {
        let event = finish(&store, "s", &format!("t{n}"), 2 + n as u64 * 3, outcome);
        let summary = store.family_summary("s").unwrap().unwrap();
        let review = summary.review.unwrap();
        assert_eq!((review.event_id, review.outcome), (event, outcome));
        assert_eq!(summary.operator_attention, 0);
        assert_eq!(store.operator_waiting_count(None).unwrap(), 0);
        assert_eq!(store.operator_waiting_count(Some("different-hat")).unwrap(), 0);
        assert_eq!(store.mark_result_read("s", event, &hub).unwrap(), Some(true));
        assert!(store.family_summary("s").unwrap().unwrap().review.is_none());
        assert_eq!(store.operator_waiting_count(None).unwrap(), 0);
    }
}

#[test]
fn stale_acknowledgements_are_monotonic_and_do_not_consume_newer_results() {
    let store = Store::open_in_memory().unwrap();
    let hub = Hub::new();
    start(&store, "s");
    let first = finish(&store, "s", "t1", 2, TurnOutcome::Completed);
    let second = finish(&store, "s", "t2", 5, TurnOutcome::Completed);
    assert_eq!(store.mark_result_read("s", first, &hub).unwrap(), Some(true));
    assert_eq!(
        store.family_summary("s").unwrap().unwrap().review.unwrap().event_id,
        second
    );
    assert_eq!(store.mark_result_read("s", second, &hub).unwrap(), Some(true));
    let events = store.events("s", 0, 100).unwrap().len();
    assert_eq!(store.mark_result_read("s", first, &hub).unwrap(), Some(true));
    assert_eq!(
        store.events("s", 0, 100).unwrap().len(),
        events,
        "repeated reads write no event"
    );
    assert!(store.family_summary("s").unwrap().unwrap().review.is_none());
    assert_eq!(store.mark_result_read("s", second + 100, &hub).unwrap(), Some(false));
    assert_eq!(store.mark_result_read("s", 1, &hub).unwrap(), Some(false));
    start(&store, "other");
    assert_eq!(store.mark_result_read("other", second, &hub).unwrap(), Some(false));
    assert_eq!(store.mark_result_read("missing", second, &hub).unwrap(), None);
}

#[test]
fn cancelled_interrupted_and_unapplied_ends_do_not_create_review() {
    let store = Store::open_in_memory().unwrap();
    start(&store, "s");
    for (n, outcome) in [TurnOutcome::Cancelled, TurnOutcome::Interrupted]
        .into_iter()
        .enumerate()
    {
        finish(&store, "s", &format!("t{n}"), 2 + n as u64 * 3, outcome);
        assert!(store.family_summary("s").unwrap().unwrap().review.is_none());
    }
    store
        .ingest(
            "s",
            8,
            &SessionBody::TurnEnded {
                turn_id: "missing".into(),
                outcome: TurnOutcome::Completed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap();
    assert_eq!(store.operator_waiting_count(None).unwrap(), 0);
}

#[test]
fn read_state_survives_restart_and_deleted_results_cannot_be_reopened() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db");
    let hub = Hub::new();
    let store = Store::open(&db).unwrap();
    start(&store, "s");
    let event = finish(&store, "s", "t1", 2, TurnOutcome::Completed);
    drop(store);
    let store = Store::open(&db).unwrap();
    assert_eq!(
        store.family_summary("s").unwrap().unwrap().review.unwrap().event_id,
        event
    );
    store.mark_result_read("s", event, &hub).unwrap();
    drop(store);
    let store = Store::open(&db).unwrap();
    assert_eq!(store.operator_waiting_count(None).unwrap(), 0);
    store.ingest("s", 5, &SessionBody::SessionClosed).unwrap();
    store.delete_session("s", None).unwrap();
    assert_eq!(store.mark_result_read("s", event, &hub).unwrap(), None);
    let conn = rusqlite::Connection::open(db).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM session_reviews", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn child_results_stay_with_the_parent_even_when_family_links_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db");
    let store = Store::open(&db).unwrap();
    // The immutable delegated identity is set at creation, before any family.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO sessions(id,host_id,agent,cwd,hat_id,lifecycle,created_at,last_event_at,owner_id,delegated_child)
        VALUES ('child','host','fake','/tmp','hat','starting','t','t',?1,1)",
        [store.owner_id()],
    )
    .unwrap();
    store
        .ingest("child", 1, &SessionBody::session_started("start", "agent"))
        .unwrap();
    finish(&store, "child", "t1", 2, TurnOutcome::Completed);
    assert!(store.family_summary("child").unwrap().unwrap().review.is_none());
    assert_eq!(store.operator_waiting_count(None).unwrap(), 0);
}

#[test]
fn reading_a_result_keeps_formal_questions_and_operator_attention_open() {
    use hennery_proto::frames::{PendingExtract, PendingKind};
    let store = Store::open_in_memory().unwrap();
    start(&store, "s");
    let event = finish(&store, "s", "t1", 2, TurnOutcome::Completed);
    store
        .ingest(
            "s",
            5,
            &SessionBody::PendingOpened {
                pending_id: "permission".into(),
                indexed: Indexed {
                    pending: Some(Box::new(PendingExtract {
                        id: "permission".into(),
                        kind: PendingKind::Permission,
                        option_ids: Some(vec!["allow".into()]),
                        title: None,
                    })),
                    ..Indexed::default()
                },
                payload: json!({"toolCall":{"toolCallId":"call"}}),
            },
        )
        .unwrap();
    store.mark_result_read("s", event, &Hub::new()).unwrap();
    let summary = store.family_summary("s").unwrap().unwrap();
    assert!(summary.review.is_none());
    assert!(summary.question_waits);
    assert_eq!(summary.operator_attention, 1);
    assert_eq!(store.operator_waiting_count(None).unwrap(), 1);
    assert_eq!(store.open_pending("s").unwrap().len(), 1);
}

#[test]
fn another_owners_session_and_event_are_not_readable_or_acknowledgeable() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db");
    let store = Store::open(&db).unwrap();
    start(&store, "s");
    let event = finish(&store, "s", "t1", 2, TurnOutcome::Completed);
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute(
        "INSERT INTO owners(id,created_at) VALUES ('other',9223372036854775807)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions(id,host_id,agent,cwd,hat_id,lifecycle,created_at,last_event_at,owner_id)
        VALUES ('foreign','host','fake','/tmp','hat','active','t','t','other')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO events(owner_id,session_id,kind,body,ts) VALUES ('other','foreign','turn_ended',?1,'t')",
        [json!({"turn_id":"foreign-turn","outcome":"completed"}).to_string()],
    )
    .unwrap();
    let foreign = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO session_reviews(owner_id,session_id,event_id) VALUES ('other','foreign',?1)",
        [foreign],
    )
    .unwrap();
    assert!(store.family_summary("foreign").unwrap().is_none());
    assert_eq!(store.mark_result_read("foreign", foreign, &Hub::new()).unwrap(), None);
    assert_eq!(store.mark_result_read("s", foreign, &Hub::new()).unwrap(), Some(false));
    assert_eq!(
        store.family_summary("s").unwrap().unwrap().review.unwrap().event_id,
        event
    );
    assert_eq!(store.operator_waiting_count(None).unwrap(), 0);
}

#[test]
fn acknowledgement_publishes_its_committed_receipt_once() {
    let store = Store::open_in_memory().unwrap();
    let hub = Hub::new();
    start(&store, "s");
    let event = finish(&store, "s", "t1", 2, TurnOutcome::Completed);
    let mut notifications = hub.subscribe();
    store.mark_result_read("s", event, &hub).unwrap();
    let receipt = notifications
        .try_recv()
        .expect("the read receipt must ring list subscribers");
    assert_eq!(receipt.kind, "result_read");
    assert_eq!(receipt.session_id, "s");
    assert_eq!(receipt.body, json!({"event_id":event}));
    assert!(store.family_summary("s").unwrap().unwrap().review.is_none());
    assert_eq!(store.events("s", receipt.event_id - 1, 1).unwrap()[0], receipt);
    store.mark_result_read("s", event, &hub).unwrap();
    assert!(notifications.try_recv().is_err());
}

#[test]
fn upgrading_from_v30_starts_empty_and_keeps_historical_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db");
    let store = Store::open(&db).unwrap();
    start(&store, "s");
    finish(&store, "s", "old", 2, TurnOutcome::Completed);
    let history = store.events("s", 0, 100).unwrap();
    drop(store);
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(
            "ALTER TABLE sessions DROP COLUMN mcp_source_requested; ALTER TABLE sessions DROP COLUMN mcp_source_applied;
             DROP INDEX sessions_sidebar_pinned; ALTER TABLE sessions DROP COLUMN sidebar_pinned;
            ALTER TABLE sessions DROP COLUMN unseen_event_id; ALTER TABLE sessions DROP COLUMN seen_event_id;
             DROP TRIGGER review_delete; DROP TABLE session_reviews; PRAGMA user_version=30;",
        )
        .unwrap();
    let upgraded = Store::open(&db).unwrap();
    assert_eq!(upgraded.events("s", 0, 100).unwrap(), history);
    assert!(upgraded.family_summary("s").unwrap().unwrap().review.is_none());
    assert_eq!(upgraded.operator_waiting_count(None).unwrap(), 0);
    finish(&upgraded, "s", "new", 5, TurnOutcome::Completed);
    assert_eq!(
        upgraded.family_summary("s").unwrap().unwrap().review.unwrap().turn_id,
        "new"
    );
}
