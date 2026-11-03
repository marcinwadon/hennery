use hennery_proto::{
    frames::{Indexed, SessionBody, TurnOutcome},
    usage::*,
};
use hennery_sessions::store::{ResumeRequest, Store};
use serde_json::json;

fn seed(store: &Store, id: &str, hat: &str) {
    store.create_session(id, "host", "claude", "/tmp", hat, None).unwrap();
    let mut started = SessionBody::session_started("start", "agent-id");
    if let SessionBody::SessionStarted { indexed, .. } = &mut started {
        indexed.cache_retention = Some(CacheRetention::Inherit);
    }
    store.ingest(id, 1, &started).unwrap();
}
fn usage() -> Box<TurnUsage> {
    Box::new(TurnUsage {
        source: "claude".into(),
        tokens: TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 100,
            cache_read_tokens: Some(80),
            cache_write_tokens: Some(5),
        },
        models: vec![],
    })
}

#[test]
fn external_migrations_preserve_v37_usage_and_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upgrade.db");
    let store = Store::open(&path).unwrap();
    seed(&store, "s", "hat");
    finish(&store, "s", "turn", 2, Some(usage()), TurnOutcome::Completed);
    let before = serde_json::to_value(store.session_usage("s").unwrap()).unwrap();
    assert_eq!(before["session"]["tokens"]["total_tokens"], 100);
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    // Main's last schema already owns the usage columns and indexes. Only the
    // two unpublished external migrations are absent from an upgrading DB.
    conn.execute_batch(concat!(
        include_str!("fixtures/undo_external.sql"),
        "PRAGMA user_version=37;"
    ))
    .unwrap();
    for _ in 0..2 {
        let upgraded = Store::open(&path).unwrap();
        assert_eq!(
            serde_json::to_value(upgraded.session_usage("s").unwrap()).unwrap(),
            before
        );
        let count: u32 = conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN (
              'external_clients','external_client_roots','external_client_sessions',
              'external_oauth_clients','external_oauth_requests','external_oauth_tokens','external_oauth_refresh_history')",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 7);
    }
}

#[test]
fn late_usage_after_detach_repairs_accounting_without_reopening_a_turn() {
    let store = Store::open_in_memory().unwrap();
    seed(&store, "s", "hat");
    store
        .open_turn("s", "late", &[json!({"type":"text","text":"test"})])
        .unwrap();
    store
        .ingest(
            "s",
            2,
            &SessionBody::TurnStarted {
                request_id: "r".into(),
                turn_id: "late".into(),
            },
        )
        .unwrap();
    store
        .ingest(
            "s",
            3,
            &SessionBody::SessionParked {
                reason: hennery_proto::frames::ParkReason::Idle,
            },
        )
        .unwrap();
    store
        .ingest(
            "s",
            4,
            &SessionBody::TurnEnded {
                turn_id: "late".into(),
                outcome: TurnOutcome::Completed,
                stop_reason: None,
                error: None,
                usage: Some(usage()),
            },
        )
        .unwrap();
    let u = store.session_usage("s").unwrap().unwrap();
    assert_eq!(u.session.reported_turns, 1);
    assert_eq!(u.session.tokens.unwrap().total_tokens, 100);
    assert_eq!(store.find_session("s").unwrap().unwrap().lifecycle, "parked");
}
fn finish(store: &Store, id: &str, turn: &str, seq: u64, data: Option<Box<TurnUsage>>, outcome: TurnOutcome) {
    assert!(
        store
            .open_turn(id, turn, &[json!({"type":"text","text":"test"})])
            .unwrap()
    );
    store
        .ingest(
            id,
            seq,
            &SessionBody::TurnStarted {
                request_id: turn.into(),
                turn_id: turn.into(),
            },
        )
        .unwrap();
    store
        .ingest(
            id,
            seq + 1,
            &SessionBody::TurnEnded {
                turn_id: turn.into(),
                outcome,
                stop_reason: None,
                error: None,
                usage: data,
            },
        )
        .unwrap();
}
#[test]
fn old_and_interrupted_turns_are_unknown_and_replays_never_add_usage_twice() {
    let store = Store::open_in_memory().unwrap();
    seed(&store, "s", "hat");
    finish(&store, "s", "old", 2, None, TurnOutcome::Completed);
    finish(&store, "s", "known", 4, Some(usage()), TurnOutcome::Completed);
    let report = SessionBody::TurnEnded {
        turn_id: "known".into(),
        outcome: TurnOutcome::Completed,
        stop_reason: None,
        error: None,
        usage: Some(usage()),
    };
    assert!(store.ingest("s", 5, &report).unwrap().is_empty());
    assert!(store.ingest("s", 6, &report).unwrap().is_empty());
    finish(&store, "s", "cancelled", 7, None, TurnOutcome::Cancelled);
    let u = store.session_usage("s").unwrap().unwrap();
    assert_eq!(u.session.tokens.unwrap().total_tokens, 100);
    assert_eq!((u.session.turns, u.session.reported_turns), (3, 1));
    assert!(u.session.partial);
    assert_eq!(u.last_turn.unwrap().usage, None);
    assert_eq!(u.family.reporting_sessions, 1);
    assert_eq!(u.family.totals.tokens.unwrap().total_tokens, 100);
}
#[test]
fn forged_or_invalid_counts_cannot_be_attributed_to_another_sessions_turn() {
    let store = Store::open_in_memory().unwrap();
    seed(&store, "a", "hat");
    seed(&store, "b", "hat");
    finish(&store, "a", "turn", 2, Some(usage()), TurnOutcome::Completed);
    let mut invalid = usage();
    invalid.tokens.total_tokens = 1;
    finish(&store, "b", "bad", 2, Some(invalid), TurnOutcome::Completed);
    store
        .ingest(
            "b",
            4,
            &SessionBody::TurnEnded {
                turn_id: "turn".into(),
                outcome: TurnOutcome::Completed,
                usage: Some(usage()),
                stop_reason: None,
                error: None,
            },
        )
        .unwrap();
    let b = store.session_usage("b").unwrap().unwrap();
    assert!(b.session.tokens.is_none());
    assert!(b.session.partial);
    assert_eq!(store.session_usage("a").unwrap().unwrap().session.reported_turns, 1);
}
#[test]
fn context_replay_does_not_refresh_observations_and_cumulative_cost_is_not_summed() {
    let store = Store::open_in_memory().unwrap();
    seed(&store, "s", "hat");
    let update = |early, amount, used| SessionBody::AcpUpdate {
        indexed: Indexed {
            early,
            ..Default::default()
        },
        payload: json!({"update":{"sessionUpdate":"usage_update","used":used,"size":200000,"cost":{"amount":amount,"currency":"USD"}}}),
    };
    store.ingest("s", 2, &update(false, 1.0, 100)).unwrap();
    let first = store.session_usage("s").unwrap().unwrap();
    store.ingest("s", 3, &update(true, 99.0, 900)).unwrap();
    let replay = store.session_usage("s").unwrap().unwrap();
    assert_eq!(first.context, replay.context);
    assert_eq!(replay.session.cost.unwrap().amount, 1.0);
    store.ingest("s", 4, &update(false, 2.0, 200)).unwrap();
    let final_read = store.session_usage("s").unwrap().unwrap();
    assert_eq!(final_read.context.unwrap().used, 200);
    assert_eq!(final_read.session.cost.unwrap().amount, 2.0);
}
#[test]
fn totals_and_retention_survive_restart_and_resume_carries_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let store = Store::open(&db).unwrap();
    seed(&store, "s", "hat");
    finish(&store, "s", "turn", 2, Some(usage()), TurnOutcome::Cancelled);
    assert!(store.set_cache_retention("s", CacheRetention::OneHour).unwrap());
    assert_eq!(
        store.session_usage("s").unwrap().unwrap().retention.attached_request,
        Some(CacheRetention::Inherit)
    );
    store
        .ingest(
            "s",
            4,
            &SessionBody::SessionParked {
                reason: hennery_proto::frames::ParkReason::Idle,
            },
        )
        .unwrap();
    drop(store);
    let store = Store::open(&db).unwrap();
    let u = store.session_usage("s").unwrap().unwrap();
    assert_eq!(u.session.tokens.unwrap().total_tokens, 100);
    assert_eq!(u.retention.requested, CacheRetention::OneHour);
    let ResumeRequest::Starting { config, .. } = store.request_resume("s", "hat").unwrap() else {
        panic!("resume refused")
    };
    assert_eq!(config.cache_retention, CacheRetention::OneHour);
}
#[test]
fn unsupported_and_deleted_sessions_do_not_accept_preferences_or_expose_usage() {
    let store = Store::open_in_memory().unwrap();
    store
        .create_session("old", "host", "generic", "/tmp", "hat", None)
        .unwrap();
    store.ingest("old", 1, &SessionBody::session_started("r", "a")).unwrap();
    assert!(!store.set_cache_retention("old", CacheRetention::OneHour).unwrap());
    seed(&store, "s", "hat");
    finish(&store, "s", "t", 2, Some(usage()), TurnOutcome::Completed);
    store
        .ingest(
            "s",
            4,
            &SessionBody::SessionParked {
                reason: hennery_proto::frames::ParkReason::Idle,
            },
        )
        .unwrap();
    store
        .delete_session(
            "s",
            Some(&hennery_sessions::store::Unattached {
                lifecycle: "parked".into(),
                presumed_parked: false,
            }),
        )
        .unwrap();
    assert!(store.session_usage("s").unwrap().is_none());
    assert!(!store.set_cache_retention("s", CacheRetention::FiveMinutes).unwrap());
}
#[test]
fn family_aggregation_respects_hat_boundaries_and_does_not_count_missing_as_zero() {
    use hennery_kernel::{hats::HatChange, hosts::Hosts};
    use rusqlite::params;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let hat = hosts.default_hat_for_new_hosts().unwrap();
    let HatChange::Done(other) = hosts.create_hat("Other", None, 1).unwrap() else {
        panic!()
    };
    let store = Store::open(&db).unwrap();
    seed(&store, "parent", &hat);
    for id in ["child", "missing"] {
        store.create_session(id, "host", "claude", "/tmp", &hat, None).unwrap();
    }
    let raw = rusqlite::Connection::open(&db).unwrap();
    for id in ["child", "missing"] {
        raw.execute("INSERT INTO session_families(owner_id,child_id,parent_id,root_id,hat_id,created_event_id) VALUES (?1,?2,'parent','parent',?3,1)",params![store.owner_id(),id,hat]).unwrap();
    }
    for id in ["child", "missing"] {
        store
            .ingest(id, 1, &SessionBody::session_started("r", "agent"))
            .unwrap();
    }
    finish(&store, "parent", "p", 2, Some(usage()), TurnOutcome::Completed);
    finish(&store, "child", "c", 2, Some(usage()), TurnOutcome::Completed);
    let u = store.session_usage("parent").unwrap().unwrap();
    assert_eq!((u.family.sessions, u.family.reporting_sessions), (3, 2));
    assert!(u.family.totals.partial);
    assert_eq!(u.family.totals.tokens.unwrap().total_tokens, 200);
    raw.execute(
        "UPDATE sessions SET hat_id=?3 WHERE owner_id=?1 AND id=?2",
        params![store.owner_id(), "child", other.id],
    )
    .unwrap();
    let p = store.session_usage("parent").unwrap().unwrap();
    assert_eq!(p.family.totals.tokens.unwrap().total_tokens, 100);
    let c = store.session_usage("child").unwrap().unwrap();
    assert_eq!(c.family.sessions, 1);
    assert_eq!(c.family.totals.tokens.unwrap().total_tokens, 100);
}
