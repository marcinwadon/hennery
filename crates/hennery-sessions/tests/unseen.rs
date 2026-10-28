use hennery_proto::frames::{Indexed, SessionBody, TurnOutcome};
use hennery_sessions::{hub::Hub, store::Store};
use serde_json::json;

fn start(store: &Store, id: &str) {
    store.create_session(id, "host", "fake", "/tmp", "hat", None).unwrap();
    store
        .ingest(id, 1, &SessionBody::session_started("start", "agent"))
        .unwrap();
}
fn output(store: &Store, id: &str, seq: u64) -> i64 {
    store.ingest(id, seq, &SessionBody::AcpUpdate {
        indexed: Indexed::default(),
        payload: json!({"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Result"}}}),
    }).unwrap()[0].event_id
}
fn unseen(store: &Store, id: &str) -> Option<i64> {
    store.family_summary(id).unwrap().unwrap().unseen.map(|a| a.event_id)
}

#[test]
fn output_is_unseen_before_completion_and_receipts_do_not_move_recency() {
    let store = Store::open_in_memory().unwrap();
    let hub = Hub::new();
    start(&store, "s");
    assert_eq!(unseen(&store, "s"), None);
    let event = output(&store, "s", 2);
    let before = store.family_summary("s").unwrap().unwrap();
    assert_eq!(unseen(&store, "s"), Some(event));
    assert_eq!(store.operator_waiting_count(None).unwrap(), 0);
    let mut subscription = hub.subscribe();
    assert_eq!(store.mark_session_seen("s", event, &hub).unwrap(), Some(true));
    let receipt = subscription.try_recv().unwrap();
    assert_eq!(receipt.kind, "session_seen");
    assert_eq!(unseen(&store, "s"), None);
    let after = store.family_summary("s").unwrap().unwrap();
    assert_eq!(after.session.last_event_at, before.session.last_event_at);
    assert_eq!(after.operator_attention, before.operator_attention);
    assert_eq!(store.mark_session_seen("s", event, &hub).unwrap(), Some(true));
    assert!(subscription.try_recv().is_err());
}

#[test]
fn stale_reads_duplicates_and_foreign_events_never_consume_new_activity() {
    let store = Store::open_in_memory().unwrap();
    let hub = Hub::new();
    start(&store, "s");
    start(&store, "other");
    let first = output(&store, "s", 2);
    let second = output(&store, "s", 3);
    let foreign = output(&store, "other", 2);
    assert_eq!(store.mark_session_seen("s", foreign, &hub).unwrap(), Some(false));
    assert_eq!(store.mark_session_seen("s", second + 100, &hub).unwrap(), Some(false));
    assert_eq!(store.mark_session_seen("missing", first, &hub).unwrap(), None);
    assert_eq!(store.mark_session_seen("s", first, &hub).unwrap(), Some(true));
    assert_eq!(unseen(&store, "s"), Some(second));
    store.mark_session_seen("s", second, &hub).unwrap();
    assert!(store.ingest("s", 3, &SessionBody::AcpUpdate {
        indexed: Indexed::default(),
        payload: json!({"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Result"}}}),
    }).unwrap().is_empty()); // Host replay is idempotent.
    store.mark_session_seen("s", first, &hub).unwrap();
    assert_eq!(unseen(&store, "s"), None);
}

#[test]
fn own_prompts_and_metadata_do_not_create_unseen_but_completion_does() {
    let store = Store::open_in_memory().unwrap();
    start(&store, "s");
    assert!(
        store
            .open_turn("s", "t", &[json!({"type":"text","text":"work"})])
            .unwrap()
    );
    store
        .ingest(
            "s",
            2,
            &SessionBody::TurnStarted {
                request_id: "t".into(),
                turn_id: "t".into(),
            },
        )
        .unwrap();
    store
        .ingest(
            "s",
            3,
            &SessionBody::AcpUpdate {
                indexed: Indexed::default(),
                payload: json!({"update":{"sessionUpdate":"current_mode_update","currentModeId":"code"}}),
            },
        )
        .unwrap();
    assert_eq!(unseen(&store, "s"), None);
    let end = store
        .ingest(
            "s",
            4,
            &SessionBody::TurnEnded {
                turn_id: "t".into(),
                outcome: TurnOutcome::Completed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap()[0]
        .event_id;
    assert_eq!(unseen(&store, "s"), Some(end));
    assert!(store.family_summary("s").unwrap().unwrap().review.is_some());
    assert_eq!(store.operator_waiting_count(None).unwrap(), 0);
    store.mark_session_seen("s", end, &Hub::new()).unwrap();
    assert!(store.family_summary("s").unwrap().unwrap().review.is_none());
    assert_eq!(unseen(&store, "s"), None);
    // A late turn end is stored but unapplied.
    store
        .ingest(
            "s",
            5,
            &SessionBody::TurnEnded {
                turn_id: "missing".into(),
                outcome: TurnOutcome::Completed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap();
    assert_eq!(unseen(&store, "s"), None);
}

#[test]
fn seen_cursor_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let store = Store::open(&path).unwrap();
    start(&store, "s");
    let event = output(&store, "s", 2);
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(unseen(&store, "s"), Some(event));
    store.mark_session_seen("s", event, &Hub::new()).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(unseen(&store, "s"), None);
    output(&store, "s", 3);
    assert!(unseen(&store, "s").is_some());
}

#[test]
fn seeing_a_question_does_not_answer_it_or_remove_needs_you() {
    use hennery_proto::frames::{PendingExtract, PendingKind};
    let store = Store::open_in_memory().unwrap();
    start(&store, "s");
    let event = store
        .ingest(
            "s",
            2,
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
        .unwrap()[0]
        .event_id;
    assert_eq!(unseen(&store, "s"), Some(event));
    assert_eq!(store.operator_waiting_count(None).unwrap(), 1);
    store.mark_session_seen("s", event, &Hub::new()).unwrap();
    assert_eq!(unseen(&store, "s"), None);
    let summary = store.family_summary("s").unwrap().unwrap();
    assert!(summary.question_waits);
    assert_eq!(summary.operator_attention, 1);
    assert_eq!(store.operator_waiting_count(None).unwrap(), 1);
    assert_eq!(store.open_pending("s").unwrap().len(), 1);
}

#[test]
fn upgrade_preserves_explicit_unread_results_without_backfilling_old_activity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let store = Store::open(&path).unwrap();
    start(&store, "result");
    start(&store, "history");
    output(&store, "history", 2);
    store
        .open_turn("result", "t", &[json!({"type":"text","text":"work"})])
        .unwrap();
    store
        .ingest(
            "result",
            2,
            &SessionBody::TurnStarted {
                request_id: "t".into(),
                turn_id: "t".into(),
            },
        )
        .unwrap();
    let event = store
        .ingest(
            "result",
            3,
            &SessionBody::TurnEnded {
                turn_id: "t".into(),
                outcome: TurnOutcome::Completed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap()[0]
        .event_id;
    drop(store);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "ALTER TABLE sessions DROP COLUMN mcp_source_requested; ALTER TABLE sessions DROP COLUMN mcp_source_applied;
             DROP INDEX sessions_sidebar_pinned; ALTER TABLE sessions DROP COLUMN sidebar_pinned;
         ALTER TABLE sessions DROP COLUMN unseen_event_id;
         ALTER TABLE sessions DROP COLUMN seen_event_id; PRAGMA user_version=31;",
        )
        .unwrap();
    let upgraded = Store::open(&path).unwrap();
    assert_eq!(unseen(&upgraded, "result"), Some(event));
    assert_eq!(unseen(&upgraded, "history"), None);
    let before = upgraded.family_summary("result").unwrap().unwrap();
    upgraded.mark_result_read("result", event, &Hub::new()).unwrap();
    assert_eq!(unseen(&upgraded, "result"), None);
    assert_eq!(
        upgraded
            .family_summary("result")
            .unwrap()
            .unwrap()
            .session
            .last_event_at,
        before.session.last_event_at
    );
}
