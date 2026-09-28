use hennery_proto::frames::{Indexed, SessionBody, TurnOutcome};
use hennery_sessions::store::Store;
use serde_json::json;

fn started(store: &Store) {
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    store
        .ingest(
            "s1",
            1,
            &SessionBody::SessionStarted {
                request_id: "r0".into(),
                agent_session_id: "a1".into(),
            },
        )
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
    let s = store.session("s1").unwrap().unwrap();
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
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
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
    let s = store.session("s1").unwrap().unwrap();
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
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("parked", None));

    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
    store
        .ingest(
            "s2",
            1,
            &SessionBody::SessionStarted {
                request_id: "r".into(),
                agent_session_id: "a".into(),
            },
        )
        .unwrap();
    store.ingest("s2", 2, &SessionBody::SessionClosed).unwrap();
    assert_eq!(store.session("s2").unwrap().unwrap().lifecycle, "closed");
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
    let s = store.session("s1").unwrap().unwrap();
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
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "closed");
    assert!(store.close_now("s1").unwrap().is_empty());
}

#[test]
fn reconcile_fails_a_start_the_host_never_received_and_leaves_a_running_start_alone() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("lost", "h1", "fake", "/tmp").unwrap();
    store.create_session("pending", "h1", "fake", "/tmp").unwrap();
    store.create_session("other-host", "h2", "fake", "/tmp").unwrap();
    let r = store.reconcile_host("h1", &[attached("pending", None)]).unwrap();
    assert_eq!(kinds(&r.events), ["start_not_delivered"]);
    let lost = store.session("lost").unwrap().unwrap();
    assert_eq!(
        (lost.lifecycle.as_str(), lost.failure_reason.as_deref()),
        ("failed", Some("start_not_delivered"))
    );
    assert_eq!(store.session("pending").unwrap().unwrap().lifecycle, "starting");
    assert_eq!(store.session("other-host").unwrap().unwrap().lifecycle, "starting");
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
    let s = store.session("s1").unwrap().unwrap();
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
    let s = store.session("s1").unwrap().unwrap();
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
        store.session("s1").unwrap().unwrap().open_turn_id.as_deref(),
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
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
}

#[test]
fn a_late_turn_started_reopens_a_turn_marked_not_delivered() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    let created = store.ingest("s1", 2, &turn_started("t1")).unwrap();
    assert_eq!(kinds(&created), ["turn_started", "user_turn"]);
    let s = store.session("s1").unwrap().unwrap();
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
    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
    store
        .ingest(
            "s2",
            1,
            &SessionBody::SessionStarted {
                request_id: "r".into(),
                agent_session_id: "a".into(),
            },
        )
        .unwrap();
    store.record_close_request("s2").unwrap();
    // Close requested, and the host restarted meanwhile.
    store.create_session("s3", "h1", "fake", "/tmp").unwrap();
    store
        .ingest(
            "s3",
            1,
            &SessionBody::SessionStarted {
                request_id: "r".into(),
                agent_session_id: "a".into(),
            },
        )
        .unwrap();
    store.record_close_request("s3").unwrap();

    let r = store
        .reconcile_host("h1", &[attached("s1", None), attached("s2", None)])
        .unwrap();
    assert_eq!(r.close, ["s1", "s2"]);
    assert_eq!(store.session("s3").unwrap().unwrap().lifecycle, "closed");
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
            "ALTER TABLE turns DROP COLUMN state;
             ALTER TABLE sessions DROP COLUMN close_requested;
             ALTER TABLE events DROP COLUMN applied;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
    assert!(!store.session("s1").unwrap().unwrap().close_requested);
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
        store.session("s1").unwrap().unwrap().open_turn_id.as_deref(),
        Some("t1")
    );

    // The host rejecting t2 (it lost the slot) leaves t1's slot alone.
    store.abandon_turn("s1", "t2").unwrap();
    assert_eq!(
        store.session("s1").unwrap().unwrap().open_turn_id.as_deref(),
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
    let s = store.session("s1").unwrap().unwrap();
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
    } = store.request_resume("s1").unwrap()
    else {
        panic!("not resumable");
    };
    assert_eq!(kinds(&events), ["operator_resumed"]);
    assert_eq!((agent_session_id.as_str(), committed_seq), ("a1", 3));
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("starting", None));
    store
        .ingest(
            "s1",
            4,
            &SessionBody::SessionStarted {
                request_id: "r9".into(),
                agent_session_id: "a1".into(),
            },
        )
        .unwrap();
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
}

#[test]
fn a_second_concurrent_resume_is_refused_while_the_first_is_starting() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    assert_eq!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Busy("active".into())
    );
    parked(&store, 2);
    assert!(matches!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    assert_eq!(
        store.request_resume("s1").unwrap(),
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
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
    store.close_now("s1").unwrap();
    assert!(matches!(
        store.request_resume("s1").unwrap(),
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
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!(
        (s.lifecycle.as_str(), s.failure_reason.as_deref()),
        ("failed", Some("agent_has_no_record"))
    );
    assert!(matches!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    assert_eq!(store.session("s1").unwrap().unwrap().failure_reason, None);
}

#[test]
fn a_session_the_agent_never_created_cannot_be_resumed() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    store.mark_failed("s1", "start_not_delivered").unwrap();
    assert_eq!(store.request_resume("s1").unwrap(), ResumeRequest::NoRecord);
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "failed");
    assert_eq!(store.request_resume("nope").unwrap(), ResumeRequest::NotFound);
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
    assert_eq!(store.session("s1").unwrap().unwrap().open_turn_id, None);

    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
    store
        .ingest(
            "s2",
            1,
            &SessionBody::SessionStarted {
                request_id: "r".into(),
                agent_session_id: "a2".into(),
            },
        )
        .unwrap();
    assert!(store.open_turn("s2", "t2", &prompt_text()).unwrap());
    let created = store.ingest("s2", 2, &SessionBody::SessionClosed).unwrap();
    assert_eq!(kinds(&created), ["session_closed", "turn_not_delivered"]);
    assert_eq!(store.session("s2").unwrap().unwrap().open_turn_id, None);
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
    let ResumeRequest::Starting { events, .. } = store.request_resume("s1").unwrap() else {
        panic!("not resumable");
    };
    assert_eq!(kinds(&events), ["turn_not_delivered", "operator_resumed"]);
    assert_eq!(store.session("s1").unwrap().unwrap().open_turn_id, None);
}
