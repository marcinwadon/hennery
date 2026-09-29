use hennery_proto::frames::{Indexed, SessionBody, TurnOutcome};
use hennery_sessions::store::Store;
use serde_json::json;

fn started(store: &Store) {
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
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
    store.ingest("s2", 1, &SessionBody::session_started("r", "a")).unwrap();
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
    store.ingest("s2", 1, &SessionBody::session_started("r", "a")).unwrap();
    store.record_close_request("s2").unwrap();
    // Close requested, and the host restarted meanwhile.
    store.create_session("s3", "h1", "fake", "/tmp").unwrap();
    store.ingest("s3", 1, &SessionBody::session_started("r", "a")).unwrap();
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
             ALTER TABLE sessions DROP COLUMN presumed_parked;
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
        ..
    } = store.request_resume("s1").unwrap()
    else {
        panic!("not resumable");
    };
    assert_eq!(kinds(&events), ["operator_resumed"]);
    assert_eq!((agent_session_id.as_str(), committed_seq), ("a1", 3));
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("starting", None));
    store
        .ingest("s1", 4, &SessionBody::session_started("r9", "a1"))
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
    store.ingest("s2", 1, &SessionBody::session_started("r", "a2")).unwrap();
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
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
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
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(
        store.session("s1").unwrap().unwrap().failure_reason.as_deref(),
        Some("start_not_delivered")
    );
    let created = store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
    assert_eq!(kinds(&created), ["session_started"]);
    let s = store.session("s1").unwrap().unwrap();
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
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(
        store.session("s1").unwrap().unwrap().failure_reason.as_deref(),
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
    let s = store.session("s1").unwrap().unwrap();
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
    store.create_session("s2", "h2", "fake", "/tmp").unwrap();
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
    let s1 = store.session("s1").unwrap().unwrap();
    assert_eq!(
        (s1.lifecycle.as_str(), s1.presumed_parked, s1.open_turn_id.as_deref()),
        ("parked", true, Some("t1"))
    );
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
    assert_eq!(store.session("s2").unwrap().unwrap().lifecycle, "active");
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
    let s1 = store.session("s1").unwrap().unwrap();
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
    let s1 = store.session("s1").unwrap().unwrap();
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
    let s1 = store.session("s1").unwrap().unwrap();
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
        store.request_resume("s1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    assert!(!store.session("s1").unwrap().unwrap().presumed_parked);

    store.presume_parked("h2").unwrap();
    store.close_now("s2").unwrap();
    let s2 = store.session("s2").unwrap().unwrap();
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
    let s1 = store.session("s1").unwrap().unwrap();
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
    let s2 = store.session("s2").unwrap().unwrap();
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
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "closed");

    // Closed, then resumed: the late rejection must leave the start alone.
    assert!(matches!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
    let before = store.events("s1", 0, 100).unwrap();
    assert!(store.close_after_rejected_reconcile_close("s1").unwrap().is_empty());
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "starting");
    assert_eq!(store.events("s1", 0, 100).unwrap(), before);
}

#[test]
fn a_failed_resume_fails_only_a_session_still_starting() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.mark_failed_if_starting("s1", "not_attached").unwrap();
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!(
        (s.lifecycle.as_str(), s.failure_reason.as_deref()),
        ("active", None),
        "an active session was overwritten"
    );
    parked(&store, 2);
    store.request_resume("s1").unwrap();
    store.mark_failed_if_starting("s1", "not_attached").unwrap();
    let s = store.session("s1").unwrap().unwrap();
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
        store.request_resume("s1").unwrap(),
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
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "starting");
    store
        .ingest("s1", 5, &SessionBody::session_started("r9", "a1"))
        .unwrap();
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
}
