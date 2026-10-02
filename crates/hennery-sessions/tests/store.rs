use hennery_proto::frames::{Indexed, SessionBody, TurnOutcome};
use hennery_sessions::store::Store;
use serde_json::json;

fn started(store: &Store) {
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
}

fn update(n: u32) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed::default(),
        payload: json!({ "n": n }),
    }
}

fn ended(turn: &str) -> SessionBody {
    SessionBody::TurnEnded {
        turn_id: turn.into(),
        outcome: TurnOutcome::Completed,
        stop_reason: None,
        error: None,
    }
}

#[test]
fn session_started_activates_the_session() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
}

#[test]
fn ingest_is_idempotent_on_session_and_seq() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    assert_eq!(store.ingest("s1", 2, &update(1)).unwrap().len(), 1);
    assert!(store.ingest("s1", 2, &update(1)).unwrap().is_empty());
    assert_eq!(store.events("s1", 0, 100).unwrap().len(), 2);
    assert_eq!(store.committed_seq("s1").unwrap(), 2);
}

#[test]
fn only_one_turn_can_be_open() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    assert!(
        store
            .open_turn("s1", "t1", &[json!({"type": "text", "text": "a"})])
            .unwrap()
    );
    assert!(
        !store
            .open_turn("s1", "t2", &[json!({"type": "text", "text": "b"})])
            .unwrap()
    );
}

#[test]
fn a_turn_cannot_open_on_a_session_that_is_not_active() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    assert!(
        !store
            .open_turn("s1", "t1", &[json!({"type": "text", "text": "a"})])
            .unwrap()
    );
}

#[test]
fn turn_started_records_the_user_turn_before_the_turns_updates() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .open_turn("s1", "t1", &[json!({"type": "text", "text": "hi"})])
        .unwrap();
    let created = store
        .ingest(
            "s1",
            2,
            &SessionBody::TurnStarted {
                request_id: "r1".into(),
                turn_id: "t1".into(),
            },
        )
        .unwrap();
    let kinds: Vec<&str> = created.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["turn_started", "user_turn"]);
    assert_eq!(created[1].body["content"][0]["text"], "hi");
}

#[test]
fn turn_ended_closes_only_the_open_turn_and_a_late_duplicate_changes_nothing() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .open_turn("s1", "t1", &[json!({"type": "text", "text": "a"})])
        .unwrap();
    store.ingest("s1", 2, &ended("t1")).unwrap();
    assert!(
        store
            .open_turn("s1", "t2", &[json!({"type": "text", "text": "b"})])
            .unwrap()
    );
    store
        .ingest(
            "s1",
            3,
            &SessionBody::TurnStarted {
                request_id: "r2".into(),
                turn_id: "t2".into(),
            },
        )
        .unwrap();
    // A late turn_ended for t1 (e.g. resent after a reconnect) must not close t2,
    // and per ACP core §4.4 must not be pushed since it was not applied.
    let late = store.ingest("s1", 4, &ended("t1")).unwrap();
    assert!(late.is_empty());
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!(s.open_turn_id.as_deref(), Some("t2"));
    assert_eq!(s.activity.as_deref(), Some("running"));
}

#[test]
fn abandon_turn_frees_the_session_for_the_next_prompt() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .open_turn("s1", "t1", &[json!({"type": "text", "text": "a"})])
        .unwrap();
    store.abandon_turn("s1", "t1").unwrap();
    assert!(
        store
            .open_turn("s1", "t2", &[json!({"type": "text", "text": "b"})])
            .unwrap()
    );
}

use hennery_proto::frames::{AttachedSession, ParkReason};

fn turn_started(turn: &str) -> SessionBody {
    SessionBody::TurnStarted {
        request_id: format!("req-{turn}"),
        turn_id: turn.into(),
    }
}

fn attached(session: &str, open_turn: Option<&str>) -> AttachedSession {
    AttachedSession {
        session_id: session.into(),
        last_seq: 0,
        open_turn_id: open_turn.map(str::to_string),
    }
}

fn prompt_text() -> Vec<serde_json::Value> {
    vec![json!({"type": "text", "text": "hi"})]
}

fn kinds(events: &[hennery_proto::rest::EventDto]) -> Vec<&str> {
    events.iter().map(|e| e.kind.as_str()).collect()
}

#[test]
fn a_same_seq_duplicate_with_a_different_body_is_kept_as_a_conflict() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.ingest("s1", 2, &update(1)).unwrap();
    let conflict = store.ingest("s1", 2, &update(2)).unwrap();
    assert_eq!(kinds(&conflict), ["conflict"]);
    assert_eq!(conflict[0].host_seq, None);
    assert_eq!(conflict[0].body["seq"], 2);
    assert_eq!(conflict[0].body["received"]["payload"]["n"], 2);
    // The committed row is untouched and an identical resend is still silent.
    assert!(store.ingest("s1", 2, &update(1)).unwrap().is_empty());
    let all = store.events("s1", 0, 100).unwrap();
    assert_eq!(
        all.iter().find(|e| e.host_seq == Some(2)).unwrap().body["payload"]["n"],
        1
    );
}

#[test]
fn session_parked_and_session_closed_detach_an_active_session() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("parked", None));

    store.create_session("s2", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.ingest("s2", 1, &SessionBody::session_started("r", "a")).unwrap();
    store.ingest("s2", 2, &SessionBody::SessionClosed).unwrap();
    assert_eq!(store.find_session("s2").unwrap().unwrap().lifecycle, "closed");
}

#[test]
fn a_park_that_overtakes_an_operator_close_closes_the_session() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.record_close_request("s1").unwrap();
    store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        )
        .unwrap();
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.close_requested), ("closed", false));
}

#[test]
fn close_now_closes_an_unattached_session_once() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert_eq!(kinds(&store.close_now("s1").unwrap()), ["operator_closed"]);
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "closed");
    assert!(store.close_now("s1").unwrap().is_empty());
}

#[test]
fn reconcile_fails_a_start_the_host_never_received_and_leaves_a_running_start_alone() {
    let store = Store::open_in_memory().unwrap();
    store
        .create_session("lost", "h1", "fake", "/tmp", "hat-1", None)
        .unwrap();
    store
        .create_session("pending", "h1", "fake", "/tmp", "hat-1", None)
        .unwrap();
    store
        .create_session("other-host", "h2", "fake", "/tmp", "hat-1", None)
        .unwrap();
    let r = store.reconcile_host("h1", &[attached("pending", None)]).unwrap();
    assert_eq!(kinds(&r.events), ["start_not_delivered"]);
    let lost = store.find_session("lost").unwrap().unwrap();
    assert_eq!(
        (lost.lifecycle.as_str(), lost.failure_reason.as_deref()),
        ("failed", Some("start_not_delivered"))
    );
    assert_eq!(store.find_session("pending").unwrap().unwrap().lifecycle, "starting");
    assert_eq!(store.find_session("other-host").unwrap().unwrap().lifecycle, "starting");
}

#[test]
fn reconcile_parks_sessions_a_restarted_host_lost_and_interrupts_their_turn() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    let r = store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(kinds(&r.events), ["host_restarted", "turn_ended_synthesized"]);
    assert_eq!(r.events[1].body, json!({"turn_id": "t1", "outcome": "interrupted"}));
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.open_turn_id.as_deref()), ("parked", None));
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("ended"));
    // Reconciling again changes nothing.
    assert_eq!(store.reconcile_host("h1", &[]).unwrap(), Default::default());
}

#[test]
fn reconcile_releases_a_prompt_that_never_reached_the_adapter() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    assert!(
        !store.open_turn("s1", "t2", &prompt_text()).unwrap(),
        "wedged until reconciled"
    );
    let r = store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    assert_eq!(kinds(&r.events), ["turn_not_delivered"]);
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("not_delivered"));
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
    assert!(
        store.open_turn("s1", "t2", &prompt_text()).unwrap(),
        "the next prompt is accepted"
    );
}

#[test]
fn reconcile_keeps_a_turn_the_host_reports_open() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    let r = store.reconcile_host("h1", &[attached("s1", Some("t1"))]).unwrap();
    assert!(r.events.is_empty());
    assert_eq!(
        store.find_session("s1").unwrap().unwrap().open_turn_id.as_deref(),
        Some("t1")
    );
}

#[test]
fn reconcile_interrupts_a_started_turn_the_host_no_longer_reports() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    let r = store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    assert_eq!(kinds(&r.events), ["turn_ended_synthesized"]);
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
}

#[test]
fn a_late_turn_started_reopens_a_turn_marked_not_delivered() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    let created = store.ingest("s1", 2, &turn_started("t1")).unwrap();
    assert_eq!(kinds(&created), ["turn_started", "user_turn"]);
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (s.open_turn_id.as_deref(), s.activity.as_deref()),
        (Some("t1"), Some("running"))
    );
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
}

#[test]
fn reconcile_asks_to_close_attached_sessions_the_operator_closed() {
    let store = Store::open_in_memory().unwrap();
    // Closed while its host was offline, still attached on the host.
    started(&store);
    store.close_now("s1").unwrap();
    // Close requested, delivery unknown, still attached.
    store.create_session("s2", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.ingest("s2", 1, &SessionBody::session_started("r", "a")).unwrap();
    store.record_close_request("s2").unwrap();
    // Close requested, and the host restarted meanwhile.
    store.create_session("s3", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.ingest("s3", 1, &SessionBody::session_started("r", "a")).unwrap();
    store.record_close_request("s3").unwrap();

    let r = store
        .reconcile_host("h1", &[attached("s1", None), attached("s2", None)])
        .unwrap();
    assert_eq!(r.close, ["s1", "s2"]);
    assert_eq!(store.find_session("s3").unwrap().unwrap().lifecycle, "closed");
}

#[test]
fn the_teardown_migration_upgrades_skeleton_turns() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    {
        let store = Store::open(&db).unwrap();
        started(&store);
        store.open_turn("s1", "t1", &prompt_text()).unwrap();
        store.ingest("s1", 2, &turn_started("t1")).unwrap();
    }
    // Roll the file back to the walking skeleton's schema (version 1).
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "DROP TRIGGER events_of_a_tombstone;
             DROP TRIGGER turns_of_a_tombstone;
             DROP TRIGGER pending_of_a_tombstone;
             DROP TRIGGER answers_of_a_tombstone;
             DROP TRIGGER catalog_of_a_tombstone;
             DROP TRIGGER a_tombstone_stays;
             DROP TABLE turn_attachments;
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
             DROP TABLE event_attachments;
             DROP TABLE attachments;
             ALTER TABLE turns DROP COLUMN state;
             ALTER TABLE sessions DROP COLUMN owner_id;
             ALTER TABLE turns DROP COLUMN owner_id;
             ALTER TABLE events DROP COLUMN owner_id;
             ALTER TABLE sessions DROP COLUMN close_requested;
             ALTER TABLE events DROP COLUMN applied;
             ALTER TABLE sessions DROP COLUMN presumed_parked;
             ALTER TABLE sessions DROP COLUMN model;
             ALTER TABLE sessions DROP COLUMN mode;
             ALTER TABLE sessions DROP COLUMN config_axes;
             DROP TABLE session_catalog;
             DROP TABLE answer_queue;
             DROP TABLE pending;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
    assert!(!store.find_session("s1").unwrap().unwrap().close_requested);
    assert!(store.find_session("s1").unwrap().unwrap().config.is_empty());
    // Rows written before migration 3 count as applied.
    assert_eq!(
        kinds(&store.events("s1", 0, 100).unwrap()),
        ["session_started", "turn_started", "user_turn"]
    );
}

// Fix round 1: a late `turn_started` must never leave a turn orphaned with
// no possible end (rulings A-D).

#[test]
fn a_late_turn_started_takes_the_slot_back_from_a_turn_that_never_started() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("not_delivered"));
    assert!(store.open_turn("s1", "t2", &prompt_text()).unwrap());

    let created = store.ingest("s1", 2, &turn_started("t1")).unwrap();
    assert_eq!(kinds(&created), ["turn_started", "turn_not_delivered", "user_turn"]);
    assert_eq!(created[1].body, json!({ "turn_id": "t2" }));
    assert_eq!(store.turn_state("t2").unwrap().as_deref(), Some("not_delivered"));
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
    assert_eq!(
        store.find_session("s1").unwrap().unwrap().open_turn_id.as_deref(),
        Some("t1")
    );

    // The host rejecting t2 (it lost the slot) leaves t1's slot alone.
    store.abandon_turn("s1", "t2").unwrap();
    assert_eq!(
        store.find_session("s1").unwrap().unwrap().open_turn_id.as_deref(),
        Some("t1")
    );
}

#[test]
fn a_turn_displaced_back_into_the_slot_is_still_resolved_by_reconciliation() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    assert!(store.open_turn("s1", "t2", &prompt_text()).unwrap());
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    store.abandon_turn("s1", "t2").unwrap();

    // The host restarts before ever ending t1: reconciliation must still
    // give it exactly one end (previously it was silently orphaned).
    let r = store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(kinds(&r.events), ["host_restarted", "turn_ended_synthesized"]);
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("ended"));
}

#[test]
fn a_late_turn_started_for_an_already_ended_turn_changes_nothing() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    store.ingest("s1", 3, &ended("t1")).unwrap();
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("ended"));
    assert!(store.open_turn("s1", "t2", &prompt_text()).unwrap());

    let created = store.ingest("s1", 4, &turn_started("t1")).unwrap();
    assert!(created.is_empty());
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!(s.open_turn_id.as_deref(), Some("t2"));
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("ended"));
}

#[test]
fn a_turn_ended_for_a_turn_that_never_started_does_not_jump_it_to_ended() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    // No turn_started for t1 ever arrives; a turn_ended lands at the
    // session's open turn anyway. The session slot still clears (the
    // session-level fact), but the turns row must not be corrupted into
    // `ended` for a turn that was never `started` (ruling B).
    store.ingest("s1", 2, &ended("t1")).unwrap();
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("sent"));
}

#[test]
fn close_now_resolves_a_started_turn_before_closing_and_reconcile_finds_nothing_left() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();

    let events = store.close_now("s1").unwrap();
    assert_eq!(kinds(&events), ["turn_ended_synthesized", "operator_closed"]);
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("ended"));

    // Nothing left for reconciliation to resolve, and no second end.
    let r = store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(r, Default::default());
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("ended"));
}

#[test]
fn a_resent_identical_conflicting_frame_does_not_duplicate_the_conflict_event() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.ingest("s1", 2, &update(1)).unwrap();
    let first = store.ingest("s1", 2, &update(2)).unwrap();
    assert_eq!(kinds(&first), ["conflict"]);

    let resent = store.ingest("s1", 2, &update(2)).unwrap();
    assert!(resent.is_empty());
    let conflicts = store
        .events("s1", 0, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "conflict")
        .count();
    assert_eq!(conflicts, 1);
}

// Final review F1: a host fact that is stored (it is the (session_id, seq)
// idempotency key) but not applied must not be listed by `events()` (the
// REST listing and the SSE replay), or a turn can show two ends.

fn ends_of(store: &Store, turn: &str) -> Vec<String> {
    store
        .events("s1", 0, 1000)
        .unwrap()
        .into_iter()
        .filter(|e| {
            matches!(
                e.kind.as_str(),
                "turn_ended" | "turn_ended_synthesized" | "turn_not_delivered"
            ) && e.body["turn_id"] == turn
        })
        .map(|e| e.kind)
        .collect()
}

#[test]
fn a_real_turn_ended_after_an_offline_close_is_not_listed_as_a_second_end() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    // The host is offline: the operator closes now, the turn is synthesized.
    store.close_now("s1").unwrap();
    // The host reconnects and resends the turn's real end.
    let created = store.ingest("s1", 3, &ended("t1")).unwrap();
    assert!(created.is_empty());
    assert_eq!(ends_of(&store, "t1"), ["turn_ended_synthesized"]);
    // A resend of the unapplied fact is still recognised as a duplicate,
    // and it still counts towards the committed seq (hello_ack).
    assert!(store.ingest("s1", 3, &ended("t1")).unwrap().is_empty());
    assert_eq!(store.committed_seq("s1").unwrap(), 3);
}

#[test]
fn a_real_turn_ended_after_reconcile_synthesis_is_not_listed_as_a_second_end() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    assert!(store.ingest("s1", 3, &ended("t1")).unwrap().is_empty());
    assert_eq!(ends_of(&store, "t1"), ["turn_ended_synthesized"]);
}

#[test]
fn an_unapplied_late_turn_started_is_not_listed() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    // Closed before the host's turn_started arrived: t1 is not_delivered.
    store.close_now("s1").unwrap();
    assert!(store.ingest("s1", 2, &turn_started("t1")).unwrap().is_empty());
    let listed = store.events("s1", 0, 1000).unwrap();
    assert!(
        !listed.iter().any(|e| e.kind == "turn_started" || e.kind == "user_turn"),
        "{:?}",
        kinds(&listed)
    );
    assert!(store.ingest("s1", 2, &turn_started("t1")).unwrap().is_empty());
}

// Plan B: resume, and no turn left open across a detach.

use hennery_sessions::store::ResumeRequest;

fn parked(store: &Store, seq: u64) {
    store
        .ingest(
            "s1",
            seq,
            &SessionBody::SessionParked {
                reason: ParkReason::Operator,
            },
        )
        .unwrap();
}

#[test]
fn a_resume_moves_a_parked_session_to_starting_with_what_the_host_needs() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.ingest("s1", 2, &update(1)).unwrap();
    parked(&store, 3);
    let ResumeRequest::Starting {
        events,
        agent_session_id,
        committed_seq,
        ..
    } = store.request_resume("s1", "hat-1").unwrap()
    else {
        panic!("not resumable");
    };
    assert_eq!(kinds(&events), ["operator_resumed"]);
    assert_eq!((agent_session_id.as_str(), committed_seq), ("a1", 3));
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("starting", None));
    store
        .ingest("s1", 4, &SessionBody::session_started("r9", "a1"))
        .unwrap();
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
}

#[test]
fn a_second_concurrent_resume_is_refused_while_the_first_is_starting() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    assert_eq!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Busy("active".into())
    );
    parked(&store, 2);
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    assert_eq!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Busy("starting".into())
    );
    assert_eq!(
        kinds(&store.events("s1", 0, 100).unwrap())
            .iter()
            .filter(|k| **k == "operator_resumed")
            .count(),
        1
    );
}

#[test]
fn a_resume_of_a_failed_or_closed_session_clears_what_stopped_it() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .ingest(
            "s1",
            2,
            &SessionBody::StartFailed {
                request_id: "r".into(),
                code: "agent_not_logged_in".into(),
                message: "log in".into(),
            },
        )
        .unwrap();
    // Not applied: the session is active, not starting.
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
    store.close_now("s1").unwrap();
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    store
        .ingest(
            "s1",
            3,
            &SessionBody::StartFailed {
                request_id: "r2".into(),
                code: "agent_has_no_record".into(),
                message: "gone".into(),
            },
        )
        .unwrap();
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (s.lifecycle.as_str(), s.failure_reason.as_deref()),
        ("failed", Some("agent_has_no_record"))
    );
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    assert_eq!(store.find_session("s1").unwrap().unwrap().failure_reason, None);
}

#[test]
fn a_session_the_agent_never_created_cannot_be_resumed() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.mark_failed("s1", "start_not_delivered").unwrap();
    assert_eq!(store.request_resume("s1", "hat-1").unwrap(), ResumeRequest::NoRecord);
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "failed");
    assert_eq!(store.request_resume("nope", "hat-1").unwrap(), ResumeRequest::NotFound);
}

#[test]
fn detaching_releases_a_prompt_the_host_never_acknowledged() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    // The prompt went out; the host's `not_attached` answer was lost.
    assert!(store.open_turn("s1", "t1", &prompt_text()).unwrap());
    let created = store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        )
        .unwrap();
    assert_eq!(kinds(&created), ["session_parked", "turn_not_delivered"]);
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("not_delivered"));
    assert_eq!(store.find_session("s1").unwrap().unwrap().open_turn_id, None);

    store.create_session("s2", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.ingest("s2", 1, &SessionBody::session_started("r", "a2")).unwrap();
    assert!(store.open_turn("s2", "t2", &prompt_text()).unwrap());
    let created = store.ingest("s2", 2, &SessionBody::SessionClosed).unwrap();
    assert_eq!(kinds(&created), ["session_closed", "turn_not_delivered"]);
    assert_eq!(store.find_session("s2").unwrap().unwrap().open_turn_id, None);
}

#[test]
fn a_resume_releases_a_turn_an_older_database_left_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    {
        let store = Store::open(&db).unwrap();
        started(&store);
        assert!(store.open_turn("s1", "t1", &prompt_text()).unwrap());
    }
    // What plan A could leave behind: parked with a `sent` turn still open.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute("UPDATE sessions SET lifecycle = 'parked', activity = NULL", [])
        .unwrap();
    let store = Store::open(&db).unwrap();
    let ResumeRequest::Starting { events, .. } = store.request_resume("s1", "hat-1").unwrap() else {
        panic!("not resumable");
    };
    assert_eq!(kinds(&events), ["turn_not_delivered", "operator_resumed"]);
    assert_eq!(store.find_session("s1").unwrap().unwrap().open_turn_id, None);
}

// Plan B: one rule for every stored-but-unapplied host fact (decision 7).

fn listed(store: &Store, session: &str) -> Vec<String> {
    store
        .events(session, 0, 1000)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

fn turn_update(turn: Option<&str>) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed {
            turn_id: turn.map(str::to_string),
            ..Indexed::default()
        },
        payload: json!({ "update": { "sessionUpdate": "agent_message_chunk" } }),
    }
}

#[test]
fn a_re_emitted_session_started_for_an_active_session_is_stored_but_not_listed() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let again = SessionBody::session_started("r5", "a1");
    assert!(store.ingest("s1", 2, &again).unwrap().is_empty());
    assert_eq!(listed(&store, "s1"), ["session_started"]);
    assert_eq!(store.committed_seq("s1").unwrap(), 2);
    // Still the idempotency key: the resend is a duplicate, not a conflict.
    assert!(store.ingest("s1", 2, &again).unwrap().is_empty());
}

#[test]
fn a_start_failed_that_changes_nothing_is_not_listed() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let failed = SessionBody::StartFailed {
        request_id: "r".into(),
        code: "start_failed".into(),
        message: "late".into(),
    };
    assert!(store.ingest("s1", 2, &failed).unwrap().is_empty());
    assert_eq!(listed(&store, "s1"), ["session_started"]);
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
}

#[test]
fn a_hosts_detach_of_a_session_already_closed_is_not_listed() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    // Closed while its host was offline; the host comes back, is sent
    // `close_session`, and confirms. Or its adapter had died meanwhile.
    store.close_now("s1").unwrap();
    let before = listed(&store, "s1");
    for (seq, body) in [
        (2, SessionBody::SessionClosed),
        (
            3,
            SessionBody::AdapterExited {
                code: Some(1),
                signal: None,
                stderr_tail: String::new(),
            },
        ),
        (
            4,
            SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        ),
        (
            5,
            SessionBody::HostNote {
                note: "n".into(),
                text: "t".into(),
            },
        ),
        (6, turn_update(None)),
    ] {
        assert!(store.ingest("s1", seq, &body).unwrap().is_empty(), "seq {seq}");
    }
    assert_eq!(listed(&store, "s1"), before);
    assert_eq!(store.committed_seq("s1").unwrap(), 6);
}

#[test]
fn a_late_update_after_a_synthesized_end_is_not_listed() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    assert_eq!(
        kinds(&store.ingest("s1", 3, &turn_update(Some("t1"))).unwrap()),
        ["acp_update"]
    );
    // The host's snapshot missed the turn (decision 3 of plan A): the
    // collector ends it; the session stays active.
    store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    assert!(store.ingest("s1", 4, &turn_update(Some("t1"))).unwrap().is_empty());
    // An update outside any turn on the active session is still listed.
    assert_eq!(
        kinds(&store.ingest("s1", 5, &turn_update(None)).unwrap()),
        ["acp_update"]
    );
    let listed = listed(&store, "s1");
    let end = listed.iter().position(|k| k == "turn_ended_synthesized").unwrap();
    assert_eq!(listed[end + 1..], ["acp_update"], "{listed:?}");
}

#[test]
fn a_start_that_ran_after_all_revives_the_session_without_its_failure_reason() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(
        store.find_session("s1").unwrap().unwrap().failure_reason.as_deref(),
        Some("start_not_delivered")
    );
    let created = store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
    assert_eq!(kinds(&created), ["session_started"]);
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.failure_reason), ("active", None));
}

// Controller ruling on the Task 6 review, folded into this same task: the
// collector's `start_not_delivered` guess (reconciliation never having heard
// back) is only ever a guess. When the host's real answer to that same start
// finally arrives, it must replace the guess, not be swallowed as a fact
// that "changes nothing" because the session is already `failed`.
#[test]
fn a_start_failed_replaces_a_reconciled_guess_of_start_not_delivered() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(
        store.find_session("s1").unwrap().unwrap().failure_reason.as_deref(),
        Some("start_not_delivered")
    );
    let created = store
        .ingest(
            "s1",
            1,
            &SessionBody::StartFailed {
                request_id: "r0".into(),
                code: "agent_not_logged_in".into(),
                message: "please log in".into(),
            },
        )
        .unwrap();
    assert_eq!(kinds(&created), ["start_failed"]);
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (s.lifecycle.as_str(), s.failure_reason.as_deref()),
        ("failed", Some("agent_not_logged_in"))
    );
}

// Plan B: host offline, presumed park, reattached (ACP core §5.3).

/// s1 active on h1 with a started turn t1, s2 active on another host.
fn two_hosts(store: &Store) {
    started(store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    store.create_session("s2", "h2", "fake", "/tmp", "hat-1", None).unwrap();
    store.ingest("s2", 1, &SessionBody::session_started("r", "a2")).unwrap();
}

#[test]
fn presume_parked_parks_the_hosts_active_sessions_and_keeps_their_open_turn() {
    let store = Store::open_in_memory().unwrap();
    two_hosts(&store);
    assert_eq!(store.hosts_with_active_sessions().unwrap(), ["h1", "h2"]);
    let events = store.presume_parked("h1").unwrap();
    assert_eq!(kinds(&events), ["presumed_parked"]);
    assert_eq!(events[0].body["reason"], "host_offline");
    let s1 = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (s1.lifecycle.as_str(), s1.presumed_parked, s1.open_turn_id.as_deref()),
        ("parked", true, Some("t1"))
    );
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
    assert_eq!(store.find_session("s2").unwrap().unwrap().lifecycle, "active");
    // Idempotent: nothing is active on h1 any more.
    assert!(store.presume_parked("h1").unwrap().is_empty());
    assert_eq!(store.hosts_with_active_sessions().unwrap(), ["h2"]);
}

#[test]
fn reconcile_reattaches_a_presumed_session_the_host_still_has() {
    let store = Store::open_in_memory().unwrap();
    two_hosts(&store);
    store.presume_parked("h1").unwrap();
    let done = store.reconcile_host("h1", &[attached("s1", Some("t1"))]).unwrap();
    assert_eq!(kinds(&done.events), ["reattached"]);
    let s1 = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (s1.lifecycle.as_str(), s1.presumed_parked, s1.open_turn_id.as_deref()),
        ("active", false, Some("t1"))
    );
    // The turn the host kept running ends for real.
    assert_eq!(kinds(&store.ingest("s1", 3, &ended("t1")).unwrap()), ["turn_ended"]);
}

#[test]
fn reconcile_of_a_presumed_session_the_host_lost_is_a_host_restart() {
    let store = Store::open_in_memory().unwrap();
    two_hosts(&store);
    store.presume_parked("h1").unwrap();
    let done = store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(kinds(&done.events), ["host_restarted", "turn_ended_synthesized"]);
    let s1 = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (s1.lifecycle.as_str(), s1.presumed_parked, s1.open_turn_id),
        ("parked", false, None)
    );
}

#[test]
fn facts_a_returning_host_resends_still_apply_to_a_presumed_session() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    // A prompt went out just before the host dropped off.
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.presume_parked("h1").unwrap();
    // The resend: the prompt had reached the adapter, and the host reaped
    // the session while it was away.
    assert_eq!(
        kinds(&store.ingest("s1", 2, &turn_started("t1")).unwrap()),
        ["turn_started", "user_turn"]
    );
    store.ingest("s1", 3, &ended("t1")).unwrap();
    let parked = store
        .ingest(
            "s1",
            4,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert_eq!(kinds(&parked), ["session_parked"]);
    let s1 = store.find_session("s1").unwrap().unwrap();
    assert_eq!((s1.lifecycle.as_str(), s1.presumed_parked), ("parked", false));
    // Reconciliation leaves a real park alone.
    assert!(store.reconcile_host("h1", &[]).unwrap().events.is_empty());
}

#[test]
fn closing_or_resuming_a_presumed_session_ends_the_presumption() {
    let store = Store::open_in_memory().unwrap();
    two_hosts(&store);
    store.presume_parked("h1").unwrap();
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    assert!(!store.find_session("s1").unwrap().unwrap().presumed_parked);

    store.presume_parked("h2").unwrap();
    store.close_now("s2").unwrap();
    let s2 = store.find_session("s2").unwrap().unwrap();
    assert_eq!((s2.lifecycle.as_str(), s2.presumed_parked), ("closed", false));
}

// The brief's tests above never send `presume_parked` a session whose open
// turn is still genuinely open when a *real* detach fact later arrives (the
// only test that reaches `session_parked` with `presumed_parked = 1` first
// ends the turn for real, so `release_turn_on_detach` finds nothing to
// release). Cover that path directly: `release_turn_on_detach`,
// `SessionParked`'s guard, `SessionClosed`'s guard and `fact_applies` must
// all treat a presumed-parked session as attached, not just `reconcile_host`
// and the explicitly-listed store methods.

#[test]
fn a_real_session_parked_fact_still_releases_a_presumed_sessions_open_turn() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    store.presume_parked("h1").unwrap();
    // The host, still holding the turn the presumption left it, reports a
    // real park before any reconciliation happens.
    let events = store
        .ingest(
            "s1",
            3,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert_eq!(kinds(&events), ["session_parked", "turn_ended_synthesized"]);
    let s1 = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (s1.lifecycle.as_str(), s1.presumed_parked, s1.open_turn_id),
        ("parked", false, None)
    );
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("ended"));
}

#[test]
fn a_real_session_closed_fact_still_applies_to_a_presumed_session() {
    let store = Store::open_in_memory().unwrap();
    two_hosts(&store);
    store.presume_parked("h2").unwrap();
    let events = store.ingest("s2", 2, &SessionBody::SessionClosed).unwrap();
    assert_eq!(kinds(&events), ["session_closed"]);
    let s2 = store.find_session("s2").unwrap().unwrap();
    assert_eq!((s2.lifecycle.as_str(), s2.presumed_parked), ("closed", false));
}

#[test]
fn an_acp_update_for_its_open_turn_still_applies_to_a_presumed_session() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    store.presume_parked("h1").unwrap();
    let update = turn_update(Some("t1"));
    assert_eq!(kinds(&store.ingest("s1", 3, &update).unwrap()), ["acp_update"]);
}

// Final review fix wave.

#[test]
fn a_rejected_reconcile_close_closes_only_a_session_still_waiting_on_that_close() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.record_close_request("s1").unwrap();
    assert_eq!(
        kinds(&store.close_after_rejected_reconcile_close("s1").unwrap()),
        Vec::<&str>::new(),
        "operator_closed is already recorded"
    );
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "closed");

    // Closed, then resumed: the late rejection must leave the start alone.
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    let before = store.events("s1", 0, 100).unwrap();
    assert!(store.close_after_rejected_reconcile_close("s1").unwrap().is_empty());
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "starting");
    assert_eq!(store.events("s1", 0, 100).unwrap(), before);
}

#[test]
fn a_failed_resume_fails_only_a_session_still_starting() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.mark_failed_if_starting("s1", "not_attached").unwrap();
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (s.lifecycle.as_str(), s.failure_reason.as_deref()),
        ("active", None),
        "an active session was overwritten"
    );
    parked(&store, 2);
    store.request_resume("s1", "hat-1").unwrap();
    store.mark_failed_if_starting("s1", "not_attached").unwrap();
    let s = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (s.lifecycle.as_str(), s.failure_reason.as_deref()),
        ("failed", Some("not_attached"))
    );
}

/// Review Focus 1, collector half: the old actor's detach facts landing
/// after a resume has begun change nothing, and the fresh start still
/// attaches.
#[test]
fn an_old_actors_detach_after_a_resume_began_changes_nothing() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    parked(&store, 2);
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    let before = store.events("s1", 0, 100).unwrap();
    assert!(store.ingest("s1", 3, &SessionBody::SessionClosed).unwrap().is_empty());
    assert!(
        store
            .ingest(
                "s1",
                4,
                &SessionBody::SessionParked {
                    reason: ParkReason::Idle,
                },
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.events("s1", 0, 100).unwrap(), before);
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "starting");
    store
        .ingest("s1", 5, &SessionBody::session_started("r9", "a1"))
        .unwrap();
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
}

// Plan B2b: the catalogue and the stored config (ACP core §3.2, §8).

use hennery_proto::frames::{ConfigValue, SessionConfig};

/// Catalogue extracts reporting `model` and `mode`, with one other axis.
fn catalogue(model: &str, mode: &str) -> Indexed {
    Indexed {
        config_options: Some(vec![
            json!({"id": "model", "currentValue": model}),
            json!({"id": "mode"}),
        ]),
        current_model: Some(model.into()),
        current_mode: Some(mode.into()),
        current_axes: Some([("fast".to_string(), ConfigValue::Bool(true))].into_iter().collect()),
        ..Indexed::default()
    }
}

fn config(model: &str, mode: &str) -> SessionConfig {
    SessionConfig {
        model: Some(model.into()),
        mode: Some(mode.into()),
        axes: [("fast".to_string(), ConfigValue::Bool(true))].into_iter().collect(),
    }
}

fn started_with(store: &Store, indexed: Indexed) {
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store
        .ingest(
            "s1",
            1,
            &SessionBody::SessionStarted {
                request_id: "r0".into(),
                agent_session_id: "a1".into(),
                indexed,
            },
        )
        .unwrap();
}

fn applied(indexed: Indexed) -> SessionBody {
    SessionBody::ConfigApplied {
        request_id: "rc".into(),
        indexed,
    }
}

fn stored(store: &Store) -> SessionConfig {
    store.find_session("s1").unwrap().unwrap().config
}

#[test]
fn session_started_stores_the_announced_catalogue_and_its_current_values() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("large", "plan"));
    assert_eq!(stored(&store), config("large", "plan"));
    let catalog = store.catalog("s1").unwrap().unwrap();
    assert_eq!(catalog.config_options.len(), 2);
    assert_eq!(catalog.current, config("large", "plan"));
    assert_eq!(store.catalog("nope").unwrap(), None);
}

#[test]
fn a_session_whose_host_reported_no_catalogue_has_an_empty_one() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let catalog = store.catalog("s1").unwrap().unwrap();
    assert!(
        catalog.config_options.is_empty() && catalog.current.is_empty(),
        "{catalog:?}"
    );
}

#[test]
fn a_config_applied_or_a_live_update_replaces_the_catalogue() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("small", "default"));
    let events = store.ingest("s1", 2, &applied(catalogue("large", "default"))).unwrap();
    assert_eq!(kinds(&events), ["config_applied"]);
    assert_eq!(stored(&store), config("large", "default"));
    let live = SessionBody::AcpUpdate {
        indexed: catalogue("large", "bypass"),
        payload: json!({"update": {"sessionUpdate": "config_option_update"}}),
    };
    store.ingest("s1", 3, &live).unwrap();
    assert_eq!(stored(&store), config("large", "bypass"));
    // An update with no catalogue changes nothing.
    store.ingest("s1", 4, &update(1)).unwrap();
    assert_eq!(stored(&store), config("large", "bypass"));
}

/// An adapter's unparseable or empty answer is no read-back: the stored
/// model and mode must survive it, or the next resume would re-apply
/// nothing (P-13 through another door).
#[test]
fn an_empty_or_absent_read_back_keeps_the_stored_catalogue() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("large", "plan"));
    let empty = Indexed {
        config_options: Some(vec![]),
        ..Indexed::default()
    };
    let events = store.ingest("s1", 2, &applied(empty)).unwrap();
    assert_eq!(
        kinds(&events),
        ["config_applied"],
        "still listed: the switch was accepted"
    );
    store.ingest("s1", 3, &applied(Indexed::default())).unwrap();
    assert_eq!(stored(&store), config("large", "plan"));
    assert_eq!(store.catalog("s1").unwrap().unwrap().config_options.len(), 2);
}

#[test]
fn a_late_config_applied_for_a_detached_session_is_not_applied() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("small", "default"));
    parked(&store, 2);
    let events = store.ingest("s1", 3, &applied(catalogue("large", "plan"))).unwrap();
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(stored(&store), config("small", "default"));
    assert!(!kinds(&store.events("s1", 0, 100).unwrap()).contains(&"config_applied"));
}

#[test]
fn a_resume_hands_back_the_config_to_re_apply() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("large", "plan"));
    parked(&store, 2);
    let ResumeRequest::Starting { config: wanted, .. } = store.request_resume("s1", "hat-1").unwrap() else {
        panic!("not resumable");
    };
    assert_eq!(wanted, config("large", "plan"));
}

// Plan (2): the pending set and the answer queue (ACP core §4.6, §5, §8).

use hennery_proto::frames::{
    CollectorFrame, ElicitationAction, PendingExtract, PendingKind, PendingReason, PendingResolution,
};
use hennery_proto::rest::{AnswerRequest, PendingState};
use hennery_sessions::store::AnswerSubmission;

/// `s1` active with turn `t1` running (seqs 1 and 2).
fn running(store: &Store) {
    started(store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
}

fn permission(pending_id: &str) -> SessionBody {
    SessionBody::PendingOpened {
        pending_id: pending_id.into(),
        indexed: Indexed {
            turn_id: Some("t1".into()),
            pending: Some(Box::new(PendingExtract {
                id: pending_id.into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into(), "reject".into()]),
                title: None,
            })),
            ..Indexed::default()
        },
        payload: json!({"toolCall": {"toolCallId": "call-1"}}),
    }
}

fn elicitation(pending_id: &str) -> SessionBody {
    SessionBody::PendingOpened {
        pending_id: pending_id.into(),
        indexed: Indexed {
            turn_id: Some("t1".into()),
            pending: Some(Box::new(PendingExtract {
                id: pending_id.into(),
                kind: PendingKind::Elicitation,
                option_ids: None,
                title: None,
            })),
            ..Indexed::default()
        },
        payload: json!({"mode": "form"}),
    }
}

fn resolved(pending_id: &str, reason: Option<PendingReason>) -> SessionBody {
    SessionBody::PendingResolved {
        pending_id: pending_id.into(),
        resolution: if reason.is_some() {
            PendingResolution::Cancelled
        } else {
            PendingResolution::Delivered
        },
        reason,
    }
}

fn verdict(pending_id: &str, delivered: bool) -> SessionBody {
    SessionBody::AnswerResult {
        pending_id: pending_id.into(),
        request_id: "whatever".into(),
        delivered,
    }
}

fn choose(option_id: &str) -> AnswerRequest {
    AnswerRequest::Permission {
        option_id: option_id.into(),
    }
}

fn activity(store: &Store) -> Option<String> {
    store.find_session("s1").unwrap().unwrap().activity
}

fn state_of(store: &Store, pending_id: &str) -> (PendingState, Option<PendingReason>) {
    let item = store.pending_item(pending_id).unwrap().unwrap();
    (item.state, item.reason)
}

#[test]
fn a_running_turn_is_blocked_until_its_last_open_question_is_resolved() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.ingest("s1", 4, &elicitation("p2")).unwrap();
    assert_eq!(activity(&store).as_deref(), Some("blocked"));
    let open: Vec<String> = store
        .open_pending("s1")
        .unwrap()
        .into_iter()
        .map(|p| p.pending_id)
        .collect();
    assert_eq!(open, ["p1", "p2"], "oldest first");
    store.ingest("s1", 5, &resolved("p1", None)).unwrap();
    assert_eq!(activity(&store).as_deref(), Some("blocked"), "p2 is still open");
    store
        .ingest("s1", 6, &resolved("p2", Some(PendingReason::TurnCancelled)))
        .unwrap();
    assert_eq!(activity(&store).as_deref(), Some("running"));
    assert_eq!(
        state_of(&store, "p2"),
        (PendingState::Cancelled, Some(PendingReason::TurnCancelled))
    );
    assert!(store.open_pending("s1").unwrap().is_empty());
    // A second resolution of the same request is stored, not applied.
    assert!(store.ingest("s1", 7, &resolved("p2", None)).unwrap().is_empty());
    assert_eq!(state_of(&store, "p2").0, PendingState::Cancelled);
}

#[test]
fn a_question_for_a_detached_session_or_an_ended_turn_is_not_applied() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &ended("t1")).unwrap();
    assert!(store.ingest("s1", 4, &permission("p1")).unwrap().is_empty());
    assert!(store.pending_item("p1").unwrap().is_none());
    parked(&store, 5);
    assert!(store.ingest("s1", 6, &permission("p2")).unwrap().is_empty());
}

#[test]
fn an_answer_is_queued_once_and_only_if_it_fits_the_question() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.ingest("s1", 4, &elicitation("p2")).unwrap();
    let invalid = |answer: AnswerRequest, pending: &str| {
        matches!(
            store.submit_answer("s1", pending, &answer).unwrap(),
            AnswerSubmission::Invalid(_)
        )
    };
    assert!(invalid(choose("maybe"), "p1"), "an option it does not offer");
    let decline_with_content = AnswerRequest::Elicitation {
        action: ElicitationAction::Decline,
        content: Some(json!({"name": "x"})),
    };
    assert!(invalid(decline_with_content, "p2"));
    assert!(invalid(choose("allow"), "p2"), "an option for an elicitation");
    let AnswerSubmission::Queued(queued) = store.submit_answer("s1", "p1", &choose("allow")).unwrap() else {
        panic!("not queued");
    };
    assert_eq!(
        (queued.event.kind.as_str(), queued.host_id.as_str()),
        ("answer_submitted", "h1")
    );
    assert_eq!(queued.event.body["pending_id"], "p1");
    assert_eq!(
        queued.frame,
        CollectorFrame::AnswerPermission {
            request_id: queued.request_id.clone(),
            session_id: "s1".into(),
            pending_id: "p1".into(),
            option_id: "allow".into(),
        }
    );
    assert_eq!(
        store.submit_answer("s1", "p1", &choose("reject")).unwrap(),
        AnswerSubmission::AlreadyAnswered
    );
    assert_eq!(
        store.submit_answer("s1", "nope", &choose("allow")).unwrap(),
        AnswerSubmission::NotFound
    );
    assert_eq!(
        store.submit_answer("other", "p1", &choose("allow")).unwrap(),
        AnswerSubmission::NotFound,
        "a pending id is looked up within its own session"
    );
    store
        .ingest("s1", 5, &resolved("p2", Some(PendingReason::TurnCancelled)))
        .unwrap();
    let accept = AnswerRequest::Elicitation {
        action: ElicitationAction::Accept,
        content: Some(json!({"name": "x"})),
    };
    assert_eq!(
        store.submit_answer("s1", "p2", &accept).unwrap(),
        AnswerSubmission::NotOpen
    );
    let item = store.pending_item("p1").unwrap().unwrap();
    assert_eq!((item.answered, item.delivered), (true, None));
}

#[test]
fn an_empty_option_ids_is_treated_like_no_option_ids() {
    // Decision 4: every option lacked a string optionId, so the extract's
    // `option_ids` is `Some(&[])` rather than `None`. That must still steer
    // the operator to "stop, park or close the session" -- not report that
    // the request offers no option "allow", which would suggest a retry
    // with a different option id would help.
    let store = Store::open_in_memory().unwrap();
    running(&store);
    let empty_options = SessionBody::PendingOpened {
        pending_id: "p1".into(),
        indexed: Indexed {
            turn_id: Some("t1".into()),
            pending: Some(Box::new(PendingExtract {
                id: "p1".into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec![]),
                title: None,
            })),
            ..Indexed::default()
        },
        payload: json!({"toolCall": {"toolCallId": "call-1"}}),
    };
    store.ingest("s1", 3, &empty_options).unwrap();
    let AnswerSubmission::Invalid(why) = store.submit_answer("s1", "p1", &choose("allow")).unwrap() else {
        panic!("expected Invalid");
    };
    assert!(
        why.contains("stop, park or close the session"),
        "expected the stop/park/close guidance, got: {why}"
    );
    assert!(
        !why.contains("offers no option"),
        "should not blame the option id: {why}"
    );
}

#[test]
fn a_delivered_verdict_sticks_and_a_later_false_does_not_overwrite_it() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.submit_answer("s1", "p1", &choose("allow")).unwrap();
    store.ingest("s1", 4, &verdict("p1", true)).unwrap();
    store.ingest("s1", 5, &resolved("p1", None)).unwrap();
    // A resent answer the host no longer had a waiter for: a verdict that
    // changes nothing is stored but not applied (decision 14).
    assert!(store.ingest("s1", 6, &verdict("p1", false)).unwrap().is_empty());
    let item = store.pending_item("p1").unwrap().unwrap();
    assert_eq!((item.state, item.delivered), (PendingState::Delivered, Some(true)));
}

#[test]
fn a_later_true_verdict_upgrades_an_earlier_false_one() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.submit_answer("s1", "p1", &choose("allow")).unwrap();
    // The first verdict finds no live waiter (an earlier adapter, say).
    let created = store.ingest("s1", 4, &verdict("p1", false)).unwrap();
    assert_eq!(kinds(&created), ["answer_result"]);
    assert_eq!(store.pending_item("p1").unwrap().unwrap().delivered, Some(false));
    // A later one does reach a waiter: `false` is not a verdict that sticks.
    let created = store.ingest("s1", 5, &verdict("p1", true)).unwrap();
    assert_eq!(kinds(&created), ["answer_result"]);
    assert_eq!(store.pending_item("p1").unwrap().unwrap().delivered, Some(true));
}

#[test]
fn only_answers_still_waiting_for_a_verdict_on_an_open_question_are_sent() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.ingest("s1", 4, &permission("p2")).unwrap();
    store.ingest("s1", 5, &permission("p3")).unwrap();
    for p in ["p1", "p2", "p3"] {
        let queued = store.submit_answer("s1", p, &choose("allow")).unwrap();
        assert!(matches!(queued, AnswerSubmission::Queued(_)), "{p} not queued");
    }
    let pending_ids = |frames: Vec<CollectorFrame>| -> Vec<String> {
        frames
            .into_iter()
            .map(|f| match f {
                CollectorFrame::AnswerPermission { pending_id, .. } => pending_id,
                other => panic!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(pending_ids(store.answers_to_send("h1").unwrap()), ["p1", "p2", "p3"]);
    assert!(store.answers_to_send("another-host").unwrap().is_empty());
    // p1 got its verdict; p2's question was cancelled before its answer
    // could be sent, so the answer can never be delivered.
    store.ingest("s1", 6, &verdict("p1", true)).unwrap();
    store
        .ingest("s1", 7, &resolved("p2", Some(PendingReason::TurnCancelled)))
        .unwrap();
    assert_eq!(pending_ids(store.answers_to_send("h1").unwrap()), ["p3"]);
    assert_eq!(store.pending_item("p2").unwrap().unwrap().delivered, Some(false));
    // p3's goes again after every handshake until its verdict comes.
    assert_eq!(pending_ids(store.answers_to_send("h1").unwrap()), ["p3"]);
}

#[test]
fn a_host_restart_cancels_open_questions_and_a_presumed_park_keeps_them() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.presume_parked("h1").unwrap();
    assert_eq!(
        state_of(&store, "p1").0,
        PendingState::Open,
        "the host may still hold it"
    );
    store.reconcile_host("h1", &[attached("s1", Some("t1"))]).unwrap();
    assert_eq!(state_of(&store, "p1").0, PendingState::Open, "reattached, intact");
    // Away again, and this time back without the session: a restart found
    // through a presumed park.
    store.presume_parked("h1").unwrap();
    let r = store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(
        kinds(&r.events),
        ["host_restarted", "turn_ended_synthesized", "pending_cancelled"]
    );
    assert_eq!(
        r.events[2].body,
        json!({"pending_id": "p1", "reason": "host_restarted"})
    );
    assert_eq!(
        state_of(&store, "p1"),
        (PendingState::Cancelled, Some(PendingReason::HostRestarted))
    );
}

#[test]
fn a_detach_or_an_unattached_close_cancels_whatever_is_still_open() {
    // The host resolves its questions before it detaches; a question it
    // left open is cancelled with the detach.
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    let created = store
        .ingest(
            "s1",
            4,
            &SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        )
        .unwrap();
    assert_eq!(kinds(&created).last(), Some(&"pending_cancelled"));
    assert_eq!(
        state_of(&store, "p1"),
        (PendingState::Cancelled, Some(PendingReason::AdapterLost))
    );
    // Closing a session whose host is away closes its questions too.
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.presume_parked("h1").unwrap();
    let events = store.close_now("s1").unwrap();
    assert!(kinds(&events).contains(&"pending_cancelled"), "{:?}", kinds(&events));
    assert_eq!(
        state_of(&store, "p1"),
        (PendingState::Cancelled, Some(PendingReason::SessionClosed))
    );
}

// Plan 3a: the kernel's tables share `hennery.db` with this store.

#[test]
fn the_session_store_and_the_host_registry_share_one_database_in_either_order() {
    use hennery_kernel::hosts::Hosts;
    for kernel_first in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let (store, hosts) = if kernel_first {
            let hosts = Hosts::open(&db).unwrap();
            (Store::open(&db).unwrap(), hosts)
        } else {
            let store = Store::open(&db).unwrap();
            (store, Hosts::open(&db).unwrap())
        };
        started(&store);
        hosts.mint_pairing_code(0).unwrap();
        drop((store, hosts));
        // Reopened, each finds its own tables and migrates nothing twice.
        let store = Store::open(&db).unwrap();
        let hosts = Hosts::open(&db).unwrap();
        assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
        assert!(hosts.list().unwrap().is_empty());
    }
}

// Plan 3a: host revoke (kernel spec §4.3).

#[test]
fn a_revoked_hosts_sessions_are_parked_for_good_and_what_they_held_is_cancelled() {
    let store = Store::open_in_memory().unwrap();
    // s3: presumed parked while its host was away, its turn still open.
    store.create_session("s3", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store
        .ingest("s3", 1, &SessionBody::session_started("r3", "a3"))
        .unwrap();
    store.open_turn("s3", "t3", &prompt_text()).unwrap();
    store.ingest("s3", 2, &turn_started("t3")).unwrap();
    store.presume_parked("h1").unwrap();
    // s1: running, with a question whose answer is queued.
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    assert!(matches!(
        store.submit_answer("s1", "p1", &choose("allow")).unwrap(),
        AnswerSubmission::Queued(_)
    ));
    // s2: still starting; s5: active, the operator's close not confirmed;
    // s4: on another host.
    store.create_session("s2", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.create_session("s5", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store
        .ingest("s5", 1, &SessionBody::session_started("r5", "a5"))
        .unwrap();
    store.record_close_request("s5").unwrap();
    store.create_session("s4", "h2", "fake", "/tmp", "hat-1", None).unwrap();
    store
        .ingest("s4", 1, &SessionBody::session_started("r4", "a4"))
        .unwrap();

    let events = store.revoke_host("h1").unwrap();
    assert_eq!(
        kinds(&events),
        [
            // s2 (`starting`) fails first (minor 3): the timeline gets an
            // event too, consistent with reconciliation's own
            // `start_not_delivered`.
            "start_not_delivered",
            "presumed_parked",
            "turn_ended_synthesized",
            "pending_cancelled",
            "presumed_parked",
            "turn_ended_synthesized",
            "presumed_parked"
        ]
    );
    assert_eq!(events[1].body, json!({ "reason": "host_revoked" }));
    for id in ["s1", "s3"] {
        let row = store.find_session(id).unwrap().unwrap();
        assert_eq!(
            (
                row.lifecycle.as_str(),
                row.presumed_parked,
                row.open_turn_id,
                row.activity
            ),
            ("parked", true, None, None),
            "{id}"
        );
    }
    assert_eq!(
        state_of(&store, "p1"),
        (PendingState::Cancelled, Some(PendingReason::HostRevoked))
    );
    assert_eq!(store.pending_item("p1").unwrap().unwrap().delivered, Some(false));
    let s2 = store.find_session("s2").unwrap().unwrap();
    assert_eq!(
        (s2.lifecycle.as_str(), s2.failure_reason.as_deref()),
        ("failed", Some("host_revoked"))
    );
    let s5 = store.find_session("s5").unwrap().unwrap();
    assert_eq!((s5.lifecycle.as_str(), s5.presumed_parked), ("closed", false));
    assert_eq!(store.find_session("s4").unwrap().unwrap().lifecycle, "active");

    // A repeated revoke finds nothing left to do.
    assert!(store.revoke_host("h1").unwrap().is_empty());
}

/// Fix round 1 (F1): `revoke_host`'s idempotency check must look at the
/// session's *current* state, not just the reason its last `presumed_parked`
/// event carries. If a revoke's wait for the connection times out, that
/// connection is still live for a little longer: its `resend_complete` can
/// reconcile the session it was told to park right back to `active`
/// (`reconcile_host` treats `presumed_parked` as reattachable), and it can
/// still deliver a turnless question on top of that. A repeated revoke must
/// still converge both.
fn turnless_permission(pending_id: &str) -> SessionBody {
    SessionBody::PendingOpened {
        pending_id: pending_id.into(),
        indexed: Indexed {
            turn_id: None,
            pending: Some(Box::new(PendingExtract {
                id: pending_id.into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into(), "reject".into()]),
                title: None,
            })),
            ..Indexed::default()
        },
        payload: json!({"toolCall": {"toolCallId": "call-1"}}),
    }
}

#[test]
fn a_revoke_converges_even_after_its_wait_timed_out_and_reconciliation_reattached_it() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    let events = store.revoke_host("h1").unwrap();
    assert_eq!(kinds(&events), ["presumed_parked", "turn_ended_synthesized"]);

    // The wait for the connection to close timed out: it is still live, and
    // its resend_complete reconciles the session back as the active one it
    // once was.
    store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");

    // The zombie connection keeps talking: a turnless question opens.
    store.ingest("s1", 3, &turnless_permission("p1")).unwrap();
    assert!(matches!(
        store.submit_answer("s1", "p1", &choose("allow")).unwrap(),
        AnswerSubmission::Queued(_)
    ));

    // A repeated revoke must still converge it: parked for good, its
    // question cancelled and its queued answer given up as undelivered.
    let events = store.revoke_host("h1").unwrap();
    assert!(kinds(&events).contains(&"pending_cancelled"), "{:?}", kinds(&events));
    let row = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (row.lifecycle.as_str(), row.presumed_parked, row.open_turn_id),
        ("parked", true, None)
    );
    assert_eq!(
        state_of(&store, "p1"),
        (PendingState::Cancelled, Some(PendingReason::HostRevoked))
    );
    assert_eq!(store.pending_item("p1").unwrap().unwrap().delivered, Some(false));
}

// Plan 6b: a session's recency (the list's sort key, ACP core §9) moves
// only with an event the timeline lists (decision 6).

fn recency(store: &Store) -> (String, Option<i64>) {
    let s = store.find_session("s1").unwrap().unwrap();
    (s.last_event_at, s.last_event_id)
}

#[test]
fn recency_moves_only_with_a_listed_event() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    let (created_at, none) = recency(&store);
    assert_eq!(none, None);
    let first = store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
    let after_start = recency(&store);
    assert_eq!(after_start, (first[0].ts.clone(), Some(first[0].event_id)));
    assert!(after_start.0 >= created_at);
    // Later than any stamp so far, so a wrongly moved recency would show.
    std::thread::sleep(std::time::Duration::from_millis(5));
    // An exact duplicate, and a fact that does not apply (an end for a turn
    // that is not open), move neither.
    assert!(
        store
            .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
            .unwrap()
            .is_empty()
    );
    assert!(store.ingest("s1", 2, &ended("t-none")).unwrap().is_empty());
    assert_eq!(recency(&store), after_start);
    // A listed update moves both.
    let listed = store.ingest("s1", 3, &update(1)).unwrap();
    assert_eq!(recency(&store), (listed[0].ts.clone(), Some(listed[0].event_id)));
    assert!(listed[0].ts > after_start.0);
}

#[test]
fn a_fact_that_writes_collector_events_leaves_recency_at_the_last_of_them() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    let created = store.ingest("s1", 2, &turn_started("t1")).unwrap();
    assert_eq!(kinds(&created), ["turn_started", "user_turn"]);
    assert!(created[1].event_id > created[0].event_id);
    assert_eq!(recency(&store).1, Some(created[1].event_id));
    // A collector event of its own moves it too.
    let parked = store.record_park_request("s1").unwrap();
    assert_eq!(recency(&store), (parked.ts.clone(), Some(parked.event_id)));
}

// Plan 6b: the title and the commands, from their extracts (ACP core §3.2,
// §7, §8).

fn titled(title: &str) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed {
            title: Some(title.into()),
            ..Indexed::default()
        },
        payload: json!({ "update": { "sessionUpdate": "session_info_update" } }),
    }
}

fn commands(names: &[&str]) -> SessionBody {
    let list = names.iter().map(|n| json!({ "name": n, "description": n })).collect();
    SessionBody::AcpUpdate {
        indexed: Indexed {
            commands: Some(list),
            ..Indexed::default()
        },
        payload: json!({ "update": { "sessionUpdate": "available_commands_update" } }),
    }
}

fn title_of(store: &Store) -> Option<String> {
    store.find_session("s1").unwrap().unwrap().title
}

/// Decision 1: the title is kept on one line and capped, for the list; an
/// empty one (the agent cleared it) clears it; one in an update that does
/// not apply changes nothing. The event keeps what the agent sent.
#[test]
fn a_title_is_stored_on_one_line_and_capped_and_an_empty_one_clears_it() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let created = store
        .ingest("s1", 2, &titled("  Fix\nthe\tlogin \u{1}  bug  "))
        .unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("Fix the login bug"));
    assert_eq!(created[0].body["indexed"]["title"], "  Fix\nthe\tlogin \u{1}  bug  ");
    store.ingest("s1", 3, &titled(&"x".repeat(300))).unwrap();
    assert_eq!(title_of(&store), Some("x".repeat(120)));
    // `"` takes two bytes in JSON: 80 of them is the byte cap (160).
    store.ingest("s1", 4, &titled(&"\"".repeat(150))).unwrap();
    assert_eq!(title_of(&store), Some("\"".repeat(80)));
    store.ingest("s1", 5, &titled("")).unwrap();
    assert_eq!(title_of(&store), None);
    store.ingest("s1", 6, &titled("Kept")).unwrap();
    store.close_now("s1").unwrap();
    assert!(store.ingest("s1", 7, &titled("Too late")).unwrap().is_empty());
    assert_eq!(title_of(&store).as_deref(), Some("Kept"));
}

fn early_titled(title: &str) -> SessionBody {
    let SessionBody::AcpUpdate { mut indexed, payload } = titled(title) else {
        unreachable!()
    };
    indexed.early = true;
    SessionBody::AcpUpdate { indexed, payload }
}

/// Decision 2: a title the adapter sent before the session was announced
/// (replayed by a load) may be older than the stored one: it only fills an
/// empty title. A live one always wins.
#[test]
fn an_early_title_only_fills_an_empty_one() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.ingest("s1", 2, &early_titled("Old")).unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("Old"));
    // Still listed: only the title is left as it was.
    assert_eq!(store.ingest("s1", 3, &early_titled("Older")).unwrap().len(), 1);
    assert_eq!(title_of(&store).as_deref(), Some("Old"));
    store.ingest("s1", 4, &titled("New")).unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("New"));
    // An early clear clears nothing.
    store.ingest("s1", 5, &early_titled("")).unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("New"));
    store.ingest("s1", 6, &titled("")).unwrap();
    store.ingest("s1", 7, &early_titled("Replayed")).unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("Replayed"));
}

/// Decision 3: the latest list replaces the stored one (an empty list too),
/// and commands never touch the config catalogue or its current values.
#[test]
fn commands_replace_the_stored_list_and_never_touch_the_config() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    let snapshot = Indexed {
        config_options: Some(vec![json!({"id": "mode", "currentValue": "plan"})]),
        current_mode: Some("plan".into()),
        current_axes: Some(Default::default()),
        ..Indexed::default()
    };
    store
        .ingest(
            "s1",
            1,
            &SessionBody::SessionStarted {
                request_id: "r0".into(),
                agent_session_id: "a1".into(),
                indexed: snapshot,
            },
        )
        .unwrap();
    let before = store.catalog("s1").unwrap().unwrap();
    assert!(before.commands.is_empty());
    store.ingest("s1", 2, &commands(&["review", "plan"])).unwrap();
    let after = store.catalog("s1").unwrap().unwrap();
    assert_eq!(
        after.commands,
        [
            json!({"name": "review", "description": "review"}),
            json!({"name": "plan", "description": "plan"})
        ]
    );
    assert_eq!(
        (&after.config_options, &after.current),
        (&before.config_options, &before.current)
    );
    assert_eq!(store.find_session("s1").unwrap().unwrap().config, before.current);
    store.ingest("s1", 3, &commands(&[])).unwrap();
    let emptied = store.catalog("s1").unwrap().unwrap();
    assert!(emptied.commands.is_empty());
    assert_eq!(emptied.config_options, before.config_options);
}

/// Commands reported before any config (or by an adapter that has none)
/// are served with an empty catalogue.
#[test]
fn commands_reported_before_any_config_are_served_with_an_empty_catalogue() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.ingest("s1", 2, &commands(&["review"])).unwrap();
    let catalog = store.catalog("s1").unwrap().unwrap();
    assert_eq!(catalog.commands, [json!({"name": "review", "description": "review"})]);
    assert!(catalog.config_options.is_empty() && catalog.current.is_empty());
}

// Plan 6b: the list item (ACP core §8, §9).

/// Decision 9: the item is the row, read from `sessions` alone, with the
/// title and the current model and mode (B2b's "model / mode in the list
/// and detail items"), unbounded: the detail serves it as it is.
#[test]
fn a_session_item_is_the_row_with_its_title_and_current_model_and_mode() {
    let store = Store::open_in_memory().unwrap();
    let cwd = format!("/home/someone/{}", "deep/".repeat(40));
    store.create_session("s1", "h1", "fake", &cwd, "hat-1", None).unwrap();
    let snapshot = Indexed {
        config_options: Some(vec![json!({"id": "model"}), json!({"id": "mode"})]),
        current_model: Some("opus".into()),
        current_mode: Some("plan".into()),
        current_axes: Some(Default::default()),
        ..Indexed::default()
    };
    store
        .ingest(
            "s1",
            1,
            &SessionBody::SessionStarted {
                request_id: "r0".into(),
                agent_session_id: "a1".into(),
                indexed: snapshot,
            },
        )
        .unwrap();
    store.ingest("s1", 2, &titled("Fix the login bug")).unwrap();
    let row = store.find_session("s1").unwrap().unwrap();
    let item = store.find_session_item("s1").unwrap().unwrap();
    assert_eq!(
        item,
        hennery_proto::rest::SessionItem {
            session_id: "s1".into(),
            host_id: "h1".into(),
            agent: "fake".into(),
            cwd,
            hat_id: "hat-1".into(),
            title: Some("Fix the login bug".into()),
            lifecycle: "active".into(),
            activity: Some("idle".into()),
            failure_reason: None,
            presumed_parked: false,
            git_branch: None,
            git_dirty: None,
            model: Some("opus".into()),
            mode: Some("plan".into()),
            created_at: item.created_at.clone(),
            last_event_at: row.last_event_at,
        }
    );
    assert_eq!(item.created_at.len(), 24);
    assert!(store.find_session_item("nope").unwrap().is_none());
}

// Plan 6b: the session list (ACP core §9; frontend §5).

use hennery_sessions::store::{Cursor, ListQuery};

/// A store over a file, with a session per `(id, last_event_at, title,
/// cwd)`, its recency set by hand so the order is known.
fn listed_store(dir: &std::path::Path, sessions: &[(&str, &str, Option<&str>, &str)]) -> Store {
    let db = dir.join("hennery.db");
    let store = Store::open(&db).unwrap();
    for (id, _, _, cwd) in sessions {
        store.create_session(id, "h1", "fake", cwd, "hat-1", None).unwrap();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    for (id, at, title, _) in sessions {
        conn.execute(
            "UPDATE sessions SET last_event_at = ?2, title = ?3 WHERE id = ?1",
            rusqlite::params![id, at, title],
        )
        .unwrap();
    }
    store
}

fn ids(page: &hennery_proto::rest::SessionPage) -> Vec<&str> {
    page.sessions.iter().map(|s| s.session_id.as_str()).collect()
}

/// Stamps from before any real clock this test runs on.
const T: &str = "2020-01-07T12:00:0";

/// One sort key, newest `last_event_at` first, the id breaking ties
/// (decision 8); pages follow an opaque cursor, and a session that moves to
/// the top meanwhile does not shift the next page.
#[test]
fn the_list_is_newest_first_and_pages_by_keyset() {
    let dir = tempfile::tempdir().unwrap();
    let at = |s: u8| format!("{T}{s}.000Z");
    let (a1, a2, a3, a5) = (at(1), at(2), at(3), at(5));
    let store = listed_store(
        dir.path(),
        &[
            ("s1", &a1, None, "/tmp"),
            ("s2", &a3, None, "/tmp"),
            ("s3", &a3, None, "/tmp"),
            ("s4", &a2, None, "/tmp"),
            ("s5", &a5, None, "/tmp"),
        ],
    );
    assert_eq!(
        ids(&store.list(&ListQuery::default()).unwrap()),
        ["s5", "s3", "s2", "s4", "s1"]
    );
    let first = store
        .list(&ListQuery {
            limit: 2,
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(ids(&first), ["s5", "s3"]);
    let cursor = Cursor::decode(first.next_cursor.as_deref().unwrap()).unwrap();
    // s1 moves to the top: the next page is still the one after s3.
    store.ingest("s1", 1, &SessionBody::session_started("r", "a")).unwrap();
    let second = store
        .list(&ListQuery {
            after: Some(&cursor),
            limit: 2,
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(ids(&second), ["s2", "s4"]);
    // s1 now sorts first, so nothing follows s4: no further page.
    assert_eq!(second.next_cursor, None);
    let all = store
        .list(&ListQuery {
            limit: 6,
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(ids(&all), ["s1", "s5", "s3", "s2", "s4"]);
    assert_eq!(all.next_cursor, None);
}

/// Decision 8: `q` matches a substring of the title, cwd, branch or id, with
/// `%`, `_` and `\` taken literally and ASCII case ignored; while it is set,
/// the lifecycle filter is bypassed (frontend §5, F-10).
#[test]
fn search_matches_title_cwd_branch_and_id_literally_across_every_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let at = format!("{T}1.000Z");
    let store = listed_store(
        dir.path(),
        &[
            ("percent", &at, Some("100% done"), "/tmp"),
            ("plain", &at, Some("1000 done"), "/tmp"),
            ("under", &at, None, "/src/my_app"),
            ("nounder", &at, None, "/src/myXapp"),
            ("slash", &at, Some("a\\b"), "/tmp"),
            ("branchy", &at, None, "/tmp"),
        ],
    );
    rusqlite::Connection::open(dir.path().join("hennery.db"))
        .unwrap()
        .execute(
            "UPDATE sessions SET git_branch = 'feat/list-search' WHERE id = 'branchy'",
            [],
        )
        .unwrap();
    store.close_now("percent").unwrap();
    let found = |q: &str| {
        let active = ["starting"];
        let mut found = ids(&store
            .list(&ListQuery {
                search: Some(q),
                lifecycles: Some(&active),
                ..ListQuery::default()
            })
            .unwrap())
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        found.sort();
        found
    };
    assert_eq!(found("100%"), ["percent"]);
    assert_eq!(found("DONE"), ["percent", "plain"]);
    assert_eq!(found("my_app"), ["under"]);
    assert_eq!(found("a\\b"), ["slash"]);
    assert_eq!(found("LIST-SEARCH"), ["branchy"]);
    assert_eq!(found("nounde"), ["nounder"]);
    assert_eq!(found("%"), ["percent"]);
}

/// Without `q`, only the named lifecycles are listed ("Hide closed" names
/// all but `closed`); a presumed park is `parked`.
#[test]
fn the_lifecycle_filter_keeps_only_the_named_lifecycles() {
    let dir = tempfile::tempdir().unwrap();
    let at = format!("{T}1.000Z");
    let store = listed_store(dir.path(), &[("open", &at, None, "/tmp"), ("shut", &at, None, "/tmp")]);
    store.close_now("shut").unwrap();
    let only = |lifecycles: &[&str]| {
        ids(&store
            .list(&ListQuery {
                lifecycles: Some(lifecycles),
                ..ListQuery::default()
            })
            .unwrap())
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>()
    };
    assert_eq!(only(&["starting", "active", "parked", "failed"]), ["open"]);
    assert_eq!(only(&["closed"]), ["shut"]);
}

/// The list serves each item bounded (the review's A1); the detail's is as
/// stored.
#[test]
fn the_list_serves_items_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = format!("/home/someone/{}webapp", "deep/".repeat(40));
    let store = listed_store(dir.path(), &[("s1", &format!("{T}1.000Z"), None, &cwd)]);
    let listed = store.list(&ListQuery::default()).unwrap().sessions.remove(0);
    assert!(
        listed.cwd.starts_with('…') && listed.cwd.ends_with("deep/webapp"),
        "{}",
        listed.cwd
    );
    assert_eq!(store.find_session_item("s1").unwrap().unwrap().cwd, cwd);
}

// Plan 6b-ii: the git state (ACP core §3.2, §7, §8).

fn git(branch: Option<&str>, dirty: bool, base: Option<&str>) -> SessionBody {
    SessionBody::GitState {
        branch: branch.map(str::to_string),
        dirty,
        worktree: false,
        head: Some("c0ffee".into()),
        base_commit: base.map(str::to_string),
    }
}

/// Decision 11: a `git_state` fills the git columns, the branch on one
/// line and capped like the title; `base_commit` is recorded once; one
/// that changes nothing is stored but not listed (the review's O1), and
/// does not move the session up the list.
#[test]
fn a_git_state_fills_the_git_columns_and_records_the_base_commit_once() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let created = store.ingest("s1", 2, &git(Some("main"), true, Some("c0ffee"))).unwrap();
    assert_eq!(kinds(&created), ["git_state"]);
    let row = store.find_session("s1").unwrap().unwrap();
    assert_eq!(
        (row.git_worktree, row.base_commit.as_deref()),
        (Some(false), Some("c0ffee"))
    );
    let item = store.find_session_item("s1").unwrap().unwrap();
    assert_eq!((item.git_branch.as_deref(), item.git_dirty), (Some("main"), Some(true)));

    let recency = store.find_session("s1").unwrap().unwrap().last_event_id;
    assert!(
        store
            .ingest("s1", 3, &git(Some("main"), true, Some("decade")))
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.find_session("s1").unwrap().unwrap().last_event_id, recency);
    assert_eq!(listed(&store, "s1"), ["session_started", "git_state"]);

    store
        .ingest(
            "s1",
            4,
            &git(
                Some(&format!("feat/\u{202E}{}", "x".repeat(200))),
                false,
                Some("decade"),
            ),
        )
        .unwrap();
    let row = store.find_session("s1").unwrap().unwrap();
    assert_eq!(row.base_commit.as_deref(), Some("c0ffee"));
    let item = store.find_session_item("s1").unwrap().unwrap();
    assert_eq!(item.git_branch, Some(format!("feat/{}", "x".repeat(115))));
    assert_eq!(item.git_dirty, Some(false));
    // Detached: no branch.
    store.ingest("s1", 5, &git(None, false, None)).unwrap();
    assert_eq!(store.find_session_item("s1").unwrap().unwrap().git_branch, None);
}

/// The task review's gap: a state that differs only in `worktree`, or only
/// by a first base commit, changes a column, so it applies; the same state
/// again does not.
#[test]
fn a_git_state_that_changes_only_the_worktree_or_the_first_base_applies() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.ingest("s1", 2, &git(Some("main"), false, None)).unwrap();
    let linked = SessionBody::GitState {
        branch: Some("main".into()),
        dirty: false,
        worktree: true,
        head: Some("c0ffee".into()),
        base_commit: None,
    };
    assert_eq!(kinds(&store.ingest("s1", 3, &linked).unwrap()), ["git_state"]);
    assert_eq!(store.find_session("s1").unwrap().unwrap().git_worktree, Some(true));
    assert!(store.ingest("s1", 4, &linked).unwrap().is_empty());
    let SessionBody::GitState {
        branch,
        dirty,
        worktree,
        head,
        ..
    } = linked
    else {
        unreachable!()
    };
    let based = SessionBody::GitState {
        branch,
        dirty,
        worktree,
        head,
        base_commit: Some("c0ffee".into()),
    };
    assert_eq!(kinds(&store.ingest("s1", 5, &based).unwrap()), ["git_state"]);
    assert_eq!(
        store.find_session("s1").unwrap().unwrap().base_commit.as_deref(),
        Some("c0ffee")
    );
    assert!(store.ingest("s1", 6, &based).unwrap().is_empty());
}

/// A git state for a closed session changes nothing; a base commit that is
/// not a commit id is not recorded.
#[test]
fn a_git_state_for_a_closed_session_or_with_a_strange_base_changes_nothing() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .ingest("s1", 2, &git(Some("main"), false, Some("not a commit")))
        .unwrap();
    assert_eq!(store.find_session("s1").unwrap().unwrap().base_commit, None);
    store.close_now("s1").unwrap();
    assert!(
        store
            .ingest("s1", 3, &git(Some("other"), true, Some("c0ffee")))
            .unwrap()
            .is_empty()
    );
    let item = store.find_session_item("s1").unwrap().unwrap();
    assert_eq!(
        (item.git_branch.as_deref(), item.git_dirty),
        (Some("main"), Some(false))
    );
}

/// ACP core §4.3: a resume whose path now resolves to another hat is
/// refused with the stored hat, and nothing changes; the same hat goes on.
#[test]
fn a_resume_in_another_hat_than_the_sessions_is_refused_and_changes_nothing() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-a", None).unwrap();
    store
        .ingest("s1", 1, &SessionBody::session_started("r1", "agent-1"))
        .unwrap();
    store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert_eq!(
        store.request_resume("s1", "hat-b").unwrap(),
        ResumeRequest::HatMismatch("hat-a".into())
    );
    let row = store.find_session("s1").unwrap().unwrap();
    assert_eq!((row.lifecycle.as_str(), row.hat_id.as_str()), ("parked", "hat-a"));
    assert!(matches!(
        store.request_resume("s1", "hat-a").unwrap(),
        ResumeRequest::Starting { .. }
    ));
}

/// Plan 5c decision 1: a session from before hats gets its host's default
/// hat; one whose host is gone, the owner's default for new hosts.
#[test]
fn the_hat_migration_gives_each_session_its_hosts_default_hat() {
    use hennery_kernel::hats::HatChange;
    use hennery_kernel::hosts::{Enrollment, Hosts};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    let HatChange::Done(acme) = hosts.create_hat("Acme", None, 1).unwrap() else {
        panic!("no hat");
    };
    let enrollment = Enrollment {
        public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
        name: "laptop".into(),
        host_version: "0".into(),
        platform: "linux".into(),
    };
    hosts.register("h1", &enrollment, 1).unwrap();
    hosts.update_host("h1", None, Some(&acme.id)).unwrap();
    {
        let store = Store::open(&db).unwrap();
        store
            .create_session("s-on-h1", "h1", "fake", "/tmp", "x", None)
            .unwrap();
        store
            .create_session("s-gone", "h-gone", "fake", "/tmp", "x", None)
            .unwrap();
    }
    // Back to the store's schema before hats (version 9): plan 10b-iii's
    // migration (version 11) and plan 9a's (version 13) undone too.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "DROP TRIGGER events_of_a_tombstone;
         DROP TRIGGER turns_of_a_tombstone;
         DROP TRIGGER pending_of_a_tombstone;
         DROP TRIGGER answers_of_a_tombstone;
         DROP TRIGGER catalog_of_a_tombstone;
         DROP TRIGGER a_tombstone_stays;
         DROP TABLE turn_attachments;
         DROP INDEX attachments_by_hash;
         DROP INDEX events_by_kind;
         ALTER TABLE pending DROP COLUMN opened_event_id;
         DROP INDEX sessions_by_hat;
         ALTER TABLE sessions DROP COLUMN hat_id;
         ALTER TABLE sessions DROP COLUMN hat_rule_id;
         PRAGMA user_version = 9;",
    )
    .unwrap();
    let store = Store::open(&db).unwrap();
    assert_eq!(store.find_session("s-on-h1").unwrap().unwrap().hat_id, acme.id);
    assert_eq!(store.find_session("s-gone").unwrap().unwrap().hat_id, personal);
}

/// Plan 5c (plan 6b's "After this plan"): the list is filtered by the hat
/// a session belongs to, with the lifecycle filter, and with a search,
/// which bypasses every filter but the hat's (frontend §5); its pages
/// follow the cursor within the hat.
#[test]
fn the_list_filters_by_hat_with_or_without_a_search() {
    let store = Store::open_in_memory().unwrap();
    for (id, cwd, hat) in [
        ("s1", "/src/alpha", "hat-a"),
        ("s2", "/src/alpha-b", "hat-b"),
        ("s3", "/src/gamma", "hat-a"),
        ("s4", "/src/old", ""),
    ] {
        store.create_session(id, "h1", "fake", cwd, hat, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    store.close_now("s1").unwrap();
    let list = |hat: Option<&str>, lifecycles: Option<&[&str]>, search: Option<&str>, limit: u32| {
        store
            .list(&ListQuery {
                hat,
                lifecycles,
                search,
                limit,
                ..ListQuery::default()
            })
            .unwrap()
    };
    let page = list(Some("hat-a"), None, None, 50);
    assert_eq!(ids(&page), ["s1", "s3"]);
    assert!(page.sessions.iter().all(|s| s.hat_id == "hat-a"));
    assert_eq!(ids(&list(Some("hat-a"), Some(&["starting"]), None, 50)), ["s3"]);
    assert_eq!(
        ids(&list(Some("hat-a"), Some(&["starting"]), Some("alpha"), 50)),
        ["s1"]
    );
    assert_eq!(ids(&list(Some("hat-b"), None, Some("alpha"), 50)), ["s2"]);
    assert_eq!(ids(&list(Some(""), None, None, 50)), ["s4"]);
    assert!(ids(&list(Some("hat-x"), None, None, 50)).is_empty());
    assert_eq!(list(None, None, None, 50).sessions.len(), 4);
    let first = list(Some("hat-a"), None, None, 1);
    assert_eq!(ids(&first), ["s1"]);
    let cursor = Cursor::decode(first.next_cursor.as_deref().unwrap()).unwrap();
    let rest = store
        .list(&ListQuery {
            after: Some(&cursor),
            limit: 1,
            hat: Some("hat-a"),
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(ids(&rest), ["s3"]);
    assert_eq!(rest.next_cursor, None);
}

/// ACP core §4.9, plan 5d decision 1: a session with no running adapter
/// moves to another of the owner's hats, with a `hat_reassigned` event;
/// one that may still run (`starting`, `active`, or presumed parked while
/// its host is away) does not, nor to a hat that is not the owner's.
#[test]
fn a_session_with_no_running_adapter_is_reassigned_to_another_hat() {
    use hennery_kernel::hats::HatChange;
    use hennery_kernel::hosts::Hosts;
    use hennery_sessions::store::Reassign;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    let HatChange::Done(acme) = hosts.create_hat("Acme", None, 1).unwrap() else {
        panic!("no hat");
    };
    let store = Store::open(&db).unwrap();
    store
        .create_session("s1", "h1", "fake", "/tmp", &personal, Some("rule-1"))
        .unwrap();
    // `starting`, then `active`: an adapter may run.
    assert_eq!(
        store.reassign_hat("s1", &acme.id).unwrap(),
        Reassign::Attached("starting".into())
    );
    store
        .ingest("s1", 1, &SessionBody::session_started("r1", "agent-1"))
        .unwrap();
    assert_eq!(
        store.reassign_hat("s1", &acme.id).unwrap(),
        Reassign::Attached("active".into())
    );
    // Presumed parked: its host is away and may still run it.
    store.presume_parked("h1").unwrap();
    assert_eq!(
        store.reassign_hat("s1", &acme.id).unwrap(),
        Reassign::Attached("presumed_parked".into())
    );
    store.close_now("s1").unwrap();

    assert_eq!(store.reassign_hat("s1", "hat-nope").unwrap(), Reassign::UnknownHat);
    // Nor to another owner's hat.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES ('owner-00000000000000b2', 9223372036854775807, 9223372036854775807);
         INSERT INTO hats(id, owner_id, name, colour, created_at)
             VALUES ('hat-theirs', 'owner-00000000000000b2', 'Theirs', '#000000', 9);",
    )
    .unwrap();
    let events = store.events("s1", 0, 100).unwrap().len();
    assert_eq!(store.reassign_hat("s1", "hat-theirs").unwrap(), Reassign::UnknownHat);
    assert_eq!(store.events("s1", 0, 100).unwrap().len(), events, "nothing written");
    assert_eq!(store.reassign_hat("s1", &personal).unwrap(), Reassign::Unchanged);
    let Reassign::Done(event) = store.reassign_hat("s1", &acme.id).unwrap() else {
        panic!("not re-assigned");
    };
    assert_eq!(
        (event.kind.as_str(), event.body.clone()),
        ("hat_reassigned", json!({ "from": personal, "to": acme.id }))
    );
    assert_eq!(store.find_session("s1").unwrap().unwrap().hat_id, acme.id);
    let rule: Option<String> = conn
        .query_row("SELECT hat_rule_id FROM sessions WHERE id = 's1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rule, None, "the rule that decided the old hat no longer does");
    assert_eq!(store.reassign_hat("s-nope", &acme.id).unwrap(), Reassign::NotFound);
}

// Plan 9a: session delete (ACP core §4.10). What is left of a deleted
// session is its row, scrubbed, as a tombstone (decision 1), and one
// `session_deleted` event; nothing writes to it again (decision 2, A1).

use base64::Engine;
use hennery_proto::rest::{AttachmentUsage, EventDto};
use hennery_sessions::content;
use hennery_sessions::store::{Deletion, Reassign, Unattached};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// A store over `hennery.db` in `dir`, and that path.
fn file_store(dir: &Path) -> (Store, PathBuf) {
    let db = dir.join("hennery.db");
    (Store::open(&db).unwrap(), db)
}

/// Delete `id`, which is closed: its `session_deleted` event.
fn delete(store: &Store, id: &str) -> EventDto {
    match store.delete_session(id, None).unwrap() {
        Deletion::Done {
            event,
            unconfirmed: false,
        } => event,
        other => panic!("not deleted: {other:?}"),
    }
}

/// `id` on `host`, started, closed and deleted: a tombstone.
fn tombstone(store: &Store, id: &str, host: &str) {
    store.create_session(id, host, "fake", "/tmp", "hat-1", None).unwrap();
    store.ingest(id, 1, &SessionBody::session_started("r0", "a0")).unwrap();
    store.close_now(id).unwrap();
    delete(store, id);
}

/// Every column of a session's row, as stored.
fn raw_row(conn: &Connection, id: &str) -> Vec<rusqlite::types::Value> {
    let mut stmt = conn.prepare("SELECT * FROM sessions WHERE id = ?1").unwrap();
    let width = stmt.column_count();
    stmt.query_row([id], |r| (0..width).map(|i| r.get(i)).collect())
        .unwrap()
}

fn event_kinds(conn: &Connection, id: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT kind FROM events WHERE session_id = ?1 ORDER BY event_id")
        .unwrap();
    stmt.query_map([id], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// `len` bytes of a PNG, different for each `seed`.
fn png(seed: u8, len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..len - 8).map(|i| seed.wrapping_add(i as u8)));
    bytes
}

fn image(bytes: &[u8]) -> serde_json::Value {
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    json!({ "type": "image", "mimeType": "image/png", "data": data })
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Check `content`, save its images and open `turn` of `session` with it,
/// as the prompt route does.
fn prompt_in(store: &Store, session: &str, turn: &str, content: Vec<serde_json::Value>) {
    let checked = content::check(content).unwrap();
    store.save_images(&checked.images).unwrap();
    assert!(store.open_prompt(session, turn, &checked).unwrap());
}

/// `id` on `h1`, active, in `cwd`.
fn active(store: &Store, id: &str, cwd: &str) {
    store.create_session(id, "h1", "fake", cwd, "hat-1", None).unwrap();
    store.ingest(id, 1, &SessionBody::session_started("r0", "a0")).unwrap();
}

fn has_file(db: &Path, sha256: &str) -> bool {
    db.parent().unwrap().join("attachments").join(sha256).exists()
}

/// The tables that hold something of a session (a `session_id` column or
/// a foreign key to `sessions`), plus the image tables, each with how many
/// rows of it they hold: the session's own row, its events, turns and
/// images by the ids and hashes it had.
fn rows_of(conn: &Connection, id: &str, events: &[i64], turns: &[String], hashes: &[String]) -> Vec<(String, i64)> {
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let list = |items: Vec<String>| items.join(", ");
    let quoted = |items: &[String]| list(items.iter().map(|s| format!("'{s}'")).collect());
    let mut out = Vec::new();
    for table in tables {
        let columns: Vec<String> = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let refers: bool = conn
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_list('{table}') WHERE \"table\" = 'sessions')"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        let filter = match table.as_str() {
            "sessions" => format!("id = '{id}'"),
            "event_attachments" => format!("event_id IN ({})", list(events.iter().map(i64::to_string).collect())),
            "turn_attachments" => format!("turn_id IN ({})", quoted(turns)),
            "attachments" => format!("sha256 IN ({})", quoted(hashes)),
            _ if columns.iter().any(|c| c == "session_id") => format!("session_id = '{id}'"),
            _ => {
                assert!(!refers, "{table} refers to sessions with no session_id: walk it here");
                continue;
            }
        };
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table} WHERE {filter}"), [], |r| {
                r.get(0)
            })
            .unwrap();
        out.push((table, count));
    }
    out
}

/// Decision 1: everything of the session goes but its row and one
/// `session_deleted` event, walked over the schema, so a table added later
/// with a `session_id` is walked too. Another session keeps all of its own.
#[test]
fn a_deleted_session_leaves_only_its_tombstone_row_and_one_event() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let (a, b, kept) = (png(1, 300), png(2, 300), png(3, 300));
    for id in ["s1", "s2"] {
        active(&store, id, "/srv/app");
        store.ingest(id, 2, &titled("a title")).unwrap();
        store.ingest(id, 3, &commands(&["build"])).unwrap();
        store.ingest(id, 4, &git(Some("main"), true, Some("c0ffee"))).unwrap();
    }
    // s1: a started turn with an image (its event links it), a question
    // with an answer queued; then a second turn that never started, with
    // another image (only the turn links it).
    prompt_in(
        &store,
        "s1",
        "t1",
        vec![json!({"type": "text", "text": "see"}), image(&a)],
    );
    store.ingest("s1", 5, &turn_started("t1")).unwrap();
    store.ingest("s1", 6, &permission("p1")).unwrap();
    assert!(matches!(
        store.submit_answer("s1", "p1", &choose("allow")).unwrap(),
        AnswerSubmission::Queued(_)
    ));
    store.ingest("s1", 7, &ended("t1")).unwrap();
    prompt_in(&store, "s1", "t2", vec![image(&b)]);
    prompt_in(&store, "s2", "t9", vec![image(&kept)]);
    store.ingest("s2", 5, &turn_started("t9")).unwrap();
    store.close_now("s1").unwrap();

    let conn = Connection::open(&db).unwrap();
    let ids = |sql: &str, id: &str| -> Vec<String> {
        conn.prepare(sql)
            .unwrap()
            .query_map([id], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let events: Vec<i64> = conn
        .prepare("SELECT event_id FROM events WHERE session_id = 's1'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let turns = ids("SELECT turn_id FROM turns WHERE session_id = ?1", "s1");
    let hashes = vec![sha(&a), sha(&b)];
    let before = rows_of(&conn, "s1", &events, &turns, &hashes);
    let walked: Vec<&str> = before.iter().map(|(t, _)| t.as_str()).collect();
    for table in [
        "answer_queue",
        "attachments",
        "event_attachments",
        "events",
        "pending",
        "session_catalog",
        "sessions",
        "turn_attachments",
        "turns",
    ] {
        assert!(walked.contains(&table), "{table} not walked: {walked:?}");
    }
    assert!(
        before.iter().all(|(_, n)| *n > 0),
        "nothing to delete somewhere: {before:?}"
    );
    let s2_before = {
        let s2_events: Vec<i64> = conn
            .prepare("SELECT event_id FROM events WHERE session_id = 's2'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let s2_turns = ids("SELECT turn_id FROM turns WHERE session_id = ?1", "s2");
        (
            s2_events.clone(),
            s2_turns.clone(),
            rows_of(&conn, "s2", &s2_events, &s2_turns, &[sha(&kept)]),
        )
    };

    let event = delete(&store, "s1");
    assert_eq!((event.kind.as_str(), &event.body), ("session_deleted", &json!({})));
    let after = rows_of(&conn, "s1", &events, &turns, &hashes);
    for (table, n) in &after {
        let left = match table.as_str() {
            "sessions" | "events" => 1,
            _ => 0,
        };
        assert_eq!(*n, left, "{table}: {after:?}");
    }
    assert_eq!(event_kinds(&conn, "s1"), ["session_deleted"]);
    assert!(!has_file(&db, &sha(&a)) && !has_file(&db, &sha(&b)));
    let (s2_events, s2_turns, s2_rows) = s2_before;
    assert_eq!(rows_of(&conn, "s2", &s2_events, &s2_turns, &[sha(&kept)]), s2_rows);
    assert!(has_file(&db, &sha(&kept)));
    // A tombstone is no session: deleted again, it is not found.
    assert!(matches!(store.delete_session("s1", None).unwrap(), Deletion::NotFound));
    assert!(matches!(
        store.delete_session("s-nope", None).unwrap(),
        Deletion::NotFound
    ));
}

/// Decision 1: the tombstone keeps its id, owner, host, hat, creation and
/// recency (its `session_deleted` event); every column that could hold
/// client data is cleared. Decision 3: no accessor and no list finds it.
#[test]
fn a_tombstone_is_scrubbed_and_found_by_no_accessor_or_list() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    store
        .create_session("s1", "h1", "fake", "/srv/app", "hat-1", Some("rule-1"))
        .unwrap();
    store
        .ingest(
            "s1",
            1,
            &SessionBody::SessionStarted {
                request_id: "r0".into(),
                agent_session_id: "agent-1".into(),
                indexed: catalogue("opus", "plan"),
            },
        )
        .unwrap();
    store.ingest("s1", 2, &titled("a title")).unwrap();
    store.ingest("s1", 3, &git(Some("main"), true, Some("c0ffee"))).unwrap();
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.presume_parked("h1").unwrap();
    // Every column that can hold client data holds some.
    let conn = Connection::open(&db).unwrap();
    conn.execute("UPDATE sessions SET failure_reason = 'x' WHERE id = 's1'", [])
        .unwrap();
    let created_at: String = conn
        .query_row("SELECT created_at FROM sessions WHERE id = 's1'", [], |r| r.get(0))
        .unwrap();
    let Deletion::Done { event, unconfirmed } = store
        .delete_session(
            "s1",
            Some(&Unattached {
                lifecycle: "parked".into(),
                presumed_parked: true,
            }),
        )
        .unwrap()
    else {
        panic!("not deleted");
    };
    assert!(unconfirmed);

    type Scrubbed = (
        (String, String, String, String, String, String),
        (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<bool>,
            Option<bool>,
            Option<String>,
        ),
        (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
        (Option<String>, bool, bool),
        (String, Option<i64>, bool),
    );
    let row: Scrubbed = conn
        .query_row(
            "SELECT host_id, hat_id, agent, cwd, lifecycle, created_at,
                    title, git_branch, base_commit, git_dirty, git_worktree, model,
                    mode, config_axes, agent_session_id, failure_reason, hat_rule_id, open_turn_id,
                    activity, presumed_parked, close_requested,
                    last_event_at, last_event_id, owner_id = ?1
             FROM sessions WHERE id = 's1'",
            [store.owner_id()],
            |r| {
                Ok((
                    (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?),
                    (r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?),
                    (r.get(12)?, r.get(13)?, r.get(14)?, r.get(15)?, r.get(16)?, r.get(17)?),
                    (r.get(18)?, r.get(19)?, r.get(20)?),
                    (r.get(21)?, r.get(22)?, r.get(23)?),
                ))
            },
        )
        .unwrap();
    assert_eq!(
        row,
        (
            (
                "h1".into(),
                "hat-1".into(),
                String::new(),
                String::new(),
                "deleted".into(),
                created_at
            ),
            (None, None, None, None, None, None),
            (None, None, None, None, None, None),
            (None, false, false),
            (event.ts.clone(), Some(event.event_id), true),
        )
    );

    // The columns no closed session holds set are cleared too.
    store
        .create_session("s2", "h1", "fake", "/srv/b", "hat-1", None)
        .unwrap();
    store.close_now("s2").unwrap();
    conn.execute(
        "UPDATE sessions SET open_turn_id = 't9', activity = 'idle', presumed_parked = 1, close_requested = 1
         WHERE id = 's2'",
        [],
    )
    .unwrap();
    delete(&store, "s2");
    let flags: (Option<String>, Option<String>, bool, bool) = conn
        .query_row(
            "SELECT open_turn_id, activity, presumed_parked, close_requested FROM sessions WHERE id = 's2'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(flags, (None, None, false, false));

    assert_eq!(store.find_session("s1").unwrap(), None);
    assert_eq!(store.find_session_item("s1").unwrap(), None);
    for query in [
        ListQuery::default(),
        ListQuery {
            search: Some("s1"),
            ..ListQuery::default()
        },
        ListQuery {
            hat: Some("hat-1"),
            ..ListQuery::default()
        },
    ] {
        assert!(store.list(&query).unwrap().sessions.is_empty(), "{query:?}");
    }
}

/// A8: once deleted, a session's title and cwd are in neither the database
/// file nor its WAL: `secure_delete` zeroes what is deleted, and a
/// checkpoint folds the WAL back.
#[test]
fn a_deleted_sessions_title_and_cwd_are_not_left_in_the_database_files() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let (title, cwd) = ("zq-title-91c2be", "/srv/zq-cwd-7f3e0a");
    active(&store, "s1", cwd);
    store.ingest("s1", 2, &titled(title)).unwrap();
    store.ingest("s1", 3, &git(Some("main"), false, None)).unwrap();
    store.close_now("s1").unwrap();
    let files = || -> Vec<u8> {
        let mut bytes = std::fs::read(&db).unwrap();
        bytes.extend(std::fs::read(db.with_extension("db-wal")).unwrap_or_default());
        bytes
    };
    let holds = |bytes: &[u8], needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
    let before = files();
    assert!(
        holds(&before, title) && holds(&before, cwd),
        "not written to begin with"
    );

    delete(&store, "s1");
    // The delete's own checkpoint folded the WAL back already.
    let now = files();
    assert!(!holds(&now, title) && !holds(&now, cwd), "left in the WAL");
    drop(store);
    let conn = Connection::open(&db).unwrap();
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    drop(conn);
    let after = files();
    assert!(!holds(&after, title), "the title is still in the files");
    assert!(!holds(&after, cwd), "the cwd is still in the files");
}

/// Decision 6: an image is the owner's while a turn or an event of a kept
/// session shows it; its row and its file go with the last of them, and
/// the usage drops by what went.
#[test]
fn an_image_two_sessions_show_stays_until_the_second_is_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let (shared, own) = (png(1, 1000), png(2, 500));
    active(&store, "s1", "/srv/a");
    active(&store, "s2", "/srv/b");
    prompt_in(&store, "s1", "t1", vec![image(&shared), image(&own)]);
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    prompt_in(&store, "s2", "t2", vec![image(&shared)]);
    store.ingest("s2", 2, &turn_started("t2")).unwrap();
    assert_eq!(
        store.attachment_usage().unwrap(),
        AttachmentUsage { count: 2, bytes: 1500 }
    );
    for id in ["s1", "s2"] {
        store.close_now(id).unwrap();
    }
    // s2 shows the shared image by its event alone: its turn's link gone,
    // as for a turn of a database from before the links were kept.
    let conn = Connection::open(&db).unwrap();
    conn.execute("DELETE FROM turn_attachments WHERE turn_id = 't2'", [])
        .unwrap();

    delete(&store, "s1");
    assert_eq!(
        store.attachment_usage().unwrap(),
        AttachmentUsage { count: 1, bytes: 1000 }
    );
    assert!(store.attachment(&sha(&shared)).unwrap().is_some());
    assert!(store.attachment(&sha(&own)).unwrap().is_none());
    assert!(has_file(&db, &sha(&shared)) && !has_file(&db, &sha(&own)));

    delete(&store, "s2");
    assert_eq!(
        store.attachment_usage().unwrap(),
        AttachmentUsage { count: 0, bytes: 0 }
    );
    assert!(!has_file(&db, &sha(&shared)));
}

/// Decision 6: a turn that never started (no `user_turn` event) still
/// shows its image, so another session's delete keeps it.
#[test]
fn an_image_only_a_turn_shows_is_still_referenced() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let img = png(7, 400);
    active(&store, "s1", "/srv/a");
    active(&store, "s2", "/srv/b");
    prompt_in(&store, "s1", "t1", vec![image(&img)]);
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    // s2's turn is sent, never started.
    prompt_in(&store, "s2", "t2", vec![image(&img)]);
    store.close_now("s1").unwrap();
    delete(&store, "s1");
    assert_eq!(store.attachment_usage().unwrap().count, 1);
    assert!(has_file(&db, &sha(&img)));
}

/// Decision 6: an abandoned turn's images go with it, unless something
/// else still shows them.
#[test]
fn abandoning_a_turn_removes_an_image_only_it_showed() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let (only, shared) = (png(1, 300), png(2, 200));
    active(&store, "s1", "/srv/a");
    active(&store, "s2", "/srv/b");
    prompt_in(&store, "s2", "t2", vec![image(&shared)]);
    prompt_in(&store, "s1", "t1", vec![image(&only), image(&shared)]);
    store.abandon_turn("s1", "t1").unwrap();
    assert_eq!(
        store.attachment_usage().unwrap(),
        AttachmentUsage { count: 1, bytes: 200 }
    );
    assert!(!has_file(&db, &sha(&only)) && has_file(&db, &sha(&shared)));
    assert_eq!(store.turn_state("t1").unwrap(), None);
}

/// Decision 6: a prompt whose image file went missing since it was saved
/// (a delete in between) writes it again as it opens the turn, and links
/// the turn to each image at its block's index.
#[test]
fn opening_a_prompt_rewrites_a_missing_image_and_links_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let img = png(5, 300);
    active(&store, "s1", "/srv/a");
    let checked = content::check(vec![json!({"type": "text", "text": "x"}), image(&img), image(&img)]).unwrap();
    store.save_images(&checked.images).unwrap();
    std::fs::remove_file(db.parent().unwrap().join("attachments").join(sha(&img))).unwrap();
    assert!(store.open_prompt("s1", "t1", &checked).unwrap());
    assert!(has_file(&db, &sha(&img)));
    assert_eq!(store.attachment(&sha(&img)).unwrap().unwrap().bytes, img);
    let conn = Connection::open(&db).unwrap();
    let links: Vec<(String, i64)> = conn
        .prepare("SELECT sha256, position FROM turn_attachments WHERE turn_id = 't1' ORDER BY position")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(links, [(sha(&img), 1), (sha(&img), 2)]);
}

/// Decision 5, A4: only a closed session is deleted as it is. One the
/// route judged to have no adapter it can reach is closed first, if it is
/// still exactly what the route saw (compare-and-set); else refused, and
/// nothing changes.
#[test]
fn a_session_not_closed_is_deleted_only_as_the_route_judged_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let conn = Connection::open(&db).unwrap();
    let judged = |lifecycle: &str, presumed_parked: bool| Unattached {
        lifecycle: lifecycle.into(),
        presumed_parked,
    };
    let refused = |id: &str, unattached: Option<&Unattached>, lifecycle: &str| {
        let row = raw_row(&conn, id);
        let kinds = event_kinds(&conn, id);
        match store.delete_session(id, unattached).unwrap() {
            Deletion::Refused(found) => assert_eq!(found, lifecycle),
            other => panic!("not refused: {other:?}"),
        }
        assert_eq!((raw_row(&conn, id), event_kinds(&conn, id)), (row, kinds));
    };
    // `unconfirmed`: its host may still run it (A13).
    let deleted =
        |id: &str, unattached: &Unattached, may_run: bool| match store.delete_session(id, Some(unattached)).unwrap() {
            Deletion::Done { event, unconfirmed } => {
                assert_eq!(unconfirmed, may_run, "{id}");
                assert_eq!(event.kind, "session_deleted");
                assert_eq!(event_kinds(&conn, id), ["session_deleted"]);
            }
            other => panic!("not deleted: {other:?}"),
        };

    store
        .create_session("s1", "h1", "fake", "/srv/a", "hat-1", None)
        .unwrap();
    refused("s1", None, "starting");
    refused("s1", Some(&judged("active", false)), "starting");
    deleted("s1", &judged("starting", false), true);

    active(&store, "s2", "/srv/b");
    store.open_turn("s2", "t2", &prompt_text()).unwrap();
    refused("s2", None, "active");
    refused("s2", Some(&judged("active", true)), "active");
    store.presume_parked("h1").unwrap();
    // The route saw it active on an offline host; it is presumed parked by now.
    refused("s2", Some(&judged("active", false)), "parked");
    deleted("s2", &judged("parked", true), true);

    active(&store, "s3", "/srv/c");
    store
        .ingest(
            "s3",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    refused("s3", None, "parked");
    refused("s3", Some(&judged("parked", true)), "parked");
    deleted("s3", &judged("parked", false), false);

    active(&store, "s4", "/srv/d");
    refused("s4", Some(&judged("active", true)), "active");
    deleted("s4", &judged("active", false), true);

    store
        .create_session("s5", "h1", "fake", "/srv/e", "hat-1", None)
        .unwrap();
    store.mark_failed("s5", "spawn").unwrap();
    refused("s5", None, "failed");
    deleted("s5", &judged("failed", false), false);
}

/// Decision 2, A9: the schema itself refuses anything new for a tombstone,
/// whoever writes it.
#[test]
fn the_schema_refuses_writes_for_a_tombstone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    tombstone(&store, "s1", "h1");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    let owner = store.owner_id();
    for sql in [
        "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id) VALUES ('s1', 9, 'x', '{}', 't', ?1)",
        "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
         VALUES ('s1', NULL, 'session_deleted', '{}', 't', ?1)",
        "INSERT INTO turns(turn_id, session_id, content, created_at, owner_id) VALUES ('t9', 's1', '[]', 't', ?1)",
        "INSERT INTO pending(pending_id, session_id, kind, payload, state, opened_at, owner_id)
         VALUES ('p9', 's1', 'permission', '{}', 'open', 't', ?1)",
        "INSERT INTO answer_queue(pending_id, session_id, request_id, answer, submitted_at, owner_id)
         VALUES ('p9', 's1', 'r9', '{}', 't', ?1)",
        "INSERT INTO session_catalog(session_id, config_options, updated_at, owner_id) VALUES ('s1', '[]', 't', ?1)",
        "UPDATE sessions SET title = 'back' WHERE id = 's1' AND ?1 = ?1",
        "UPDATE sessions SET lifecycle = 'closed' WHERE id = 's1' AND ?1 = ?1",
    ] {
        let err = conn.execute(sql, [owner]).unwrap_err().to_string();
        assert!(err.contains("a deleted session"), "{sql}: {err}");
    }
    assert_eq!(event_kinds(&conn, "s1"), ["session_deleted"]);
}

/// A1, decision 2: the store's own writers leave a tombstone alone and
/// answer as they would for a session with nothing to do; one that would
/// write an event for it fails as for an unknown session.
#[test]
fn the_stores_writers_leave_a_tombstone_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    tombstone(&store, "s1", "h1");
    let conn = Connection::open(&db).unwrap();
    let row = raw_row(&conn, "s1");

    assert!(store.ingest("s1", 2, &update(1)).unwrap().is_empty());
    assert!(
        store
            .ingest("s1", 1, &SessionBody::session_started("r0", "a0"))
            .unwrap()
            .is_empty()
    );
    assert!(store.ingest("s1", 3, &SessionBody::SessionClosed).unwrap().is_empty());
    assert!(store.close_now("s1").unwrap().is_empty());
    assert!(store.close_after_rejected_reconcile_close("s1").unwrap().is_empty());
    store.mark_failed("s1", "late").unwrap();
    store.mark_failed_if_starting("s1", "late").unwrap();
    assert!(!store.open_turn("s1", "t1", &prompt_text()).unwrap());
    for err in [
        store.record_park_request("s1").unwrap_err().to_string(),
        store.record_close_request("s1").unwrap_err().to_string(),
    ] {
        assert!(err.contains("no session s1"), "{err}");
    }
    assert_eq!(store.request_resume("s1", "hat-1").unwrap(), ResumeRequest::NotFound);
    assert_eq!(store.reassign_hat("s1", "hat-1").unwrap(), Reassign::NotFound);
    assert_eq!(store.catalog("s1").unwrap(), None);
    assert!(matches!(
        store.submit_answer("s1", "p1", &choose("allow")).unwrap(),
        AnswerSubmission::NotFound
    ));
    // Its host's frames still find it, to ack and discard them (decision 4).
    assert_eq!(store.session_host("s1").unwrap().as_deref(), Some("h1"));
    assert_eq!(store.session_host("s-nope").unwrap(), None);
    assert_eq!(raw_row(&conn, "s1"), row);
    assert_eq!(event_kinds(&conn, "s1"), ["session_deleted"]);
}

/// A1: a tombstone, listed by its host as attached or not, goes through
/// every statement over a host's sessions untouched, and is no reason to
/// think its host runs anything.
#[test]
fn a_tombstone_goes_through_the_bulk_functions_untouched() {
    for listed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (store, db) = file_store(dir.path());
        tombstone(&store, "s1", "h1");
        active(&store, "s2", "/srv/b");
        let conn = Connection::open(&db).unwrap();
        let row = raw_row(&conn, "s1");
        let attached: Vec<AttachedSession> = if listed {
            vec![attached("s1", None), attached("s2", None)]
        } else {
            vec![attached("s2", None)]
        };
        assert_eq!(store.hosts_with_active_sessions().unwrap(), ["h1"]);
        store.presume_parked("h1").unwrap();
        // Decision 4: listed, its host is told to close its adapter; its
        // answer, either way, changes nothing.
        let reconciled = store.reconcile_host("h1", &attached).unwrap();
        assert!(reconciled.events.iter().all(|e| e.session_id != "s1"), "{reconciled:?}");
        assert_eq!(reconciled.close.contains(&"s1".to_string()), listed, "{reconciled:?}");
        assert!(store.ingest("s1", 2, &SessionBody::SessionClosed).unwrap().is_empty());
        assert!(store.close_after_rejected_reconcile_close("s1").unwrap().is_empty());
        store.revoke_host("h1").unwrap();
        assert!(store.hosts_with_active_sessions().unwrap().is_empty());
        let again = store.reconcile_host("h1", &attached).unwrap();
        assert_eq!(again.close.contains(&"s1".to_string()), listed, "{again:?}");
        assert_eq!(raw_row(&conn, "s1"), row, "listed: {listed}");
        assert_eq!(event_kinds(&conn, "s1"), ["session_deleted"], "listed: {listed}");
    }
}

/// R1–R4: a delete removes its project recent (host, hat, cwd: exact
/// bytes) unless another session kept of that host and hat has that cwd.
#[test]
fn a_delete_removes_its_recent_unless_another_kept_session_has_that_cwd() {
    use hennery_kernel::hosts::{Enrollment, Hosts};
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let hosts = Hosts::open(&db).unwrap();
    let mut hat = String::new();
    for (host, key) in [
        ("h1", "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"),
        ("h2", "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"),
    ] {
        let enrollment = Enrollment {
            public_key: key.into(),
            name: "test".into(),
            host_version: "0".into(),
            platform: "test".into(),
        };
        hosts.register(host, &enrollment, 1_800_000_000).unwrap();
        hat = hosts.host(host).unwrap().unwrap().default_hat_id;
    }
    for (id, host, cwd) in [
        ("s1", "h1", "/p/a"),
        ("s2", "h1", "/p/a"),
        ("s3", "h1", "/p/b"),
        ("s4", "h2", "/p/b"),
        ("s5", "h1", "/p/A"),
    ] {
        store.create_session(id, host, "fake", cwd, &hat, None).unwrap();
        store.close_now(id).unwrap();
        assert!(hosts.remember(host, &hat, cwd, 1_800_000_000).unwrap());
    }
    let conn = Connection::open(&db).unwrap();
    let recents = || -> Vec<(String, String)> {
        conn.prepare("SELECT host_id, path FROM project_recents ORDER BY host_id, path")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let recent = |host: &str, path: &str| (host.to_string(), path.to_string());
    delete(&store, "s1");
    assert!(recents().contains(&recent("h1", "/p/a")), "s2 still has it");
    // h2's s4 is another host's: h1's recent goes.
    delete(&store, "s3");
    assert_eq!(
        recents(),
        [recent("h1", "/p/A"), recent("h1", "/p/a"), recent("h2", "/p/b")]
    );
    // s5's `/p/A` is not `/p/a`, and s1's tombstone keeps nothing.
    delete(&store, "s2");
    assert_eq!(recents(), [recent("h1", "/p/A"), recent("h2", "/p/b")]);
}

/// Decision 6, A6: files are shared by hash, so the owner's row goes with
/// the owner's last reference, but the file stays while another owner's
/// row names it.
#[test]
fn an_image_file_another_owner_holds_stays_when_the_owners_row_goes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let img = png(9, 300);
    active(&store, "s1", "/srv/a");
    prompt_in(&store, "s1", "t1", vec![image(&img)]);
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO owners(id, created_at, set_up_at)
         VALUES ('owner-00000000000000b2', 9223372036854775807, 9223372036854775807)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attachments(owner_id, sha256, mime, size, created_at)
         VALUES ('owner-00000000000000b2', ?1, 'image/png', 300, 't')",
        [sha(&img)],
    )
    .unwrap();
    store.close_now("s1").unwrap();
    delete(&store, "s1");
    assert_eq!(store.attachment_usage().unwrap().count, 0);
    assert!(has_file(&db, &sha(&img)), "another owner's image lost its file");
}
