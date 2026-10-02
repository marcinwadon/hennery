//! Push triggers (ACP core §10; plan 10b): the edges a host fact crosses,
//! read by the store in the fact's own transaction. Edge-triggered only: a duplicate, a fact not applied, an already
//! ended turn and a second question cross nothing.

use hennery_proto::frames::{Indexed, PendingExtract, PendingKind, SessionBody, TurnOutcome};
use hennery_sessions::store::{EdgeSession, PushEdge, Store};
use serde_json::json;

/// `s1`, started, with turn `t1` open and running (facts 1 and 2).
fn running(store: &Store) {
    store
        .create_session("s1", "h1", "fake", "/home/me/project", "hat-1", None)
        .unwrap();
    store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
    assert!(
        store
            .open_turn("s1", "t1", &[json!({"type": "text", "text": "hi"})])
            .unwrap()
    );
    store
        .ingest(
            "s1",
            2,
            &SessionBody::TurnStarted {
                request_id: "req-t1".into(),
                turn_id: "t1".into(),
            },
        )
        .unwrap();
}

fn question(pending_id: &str, turn: Option<&str>, title: Option<&str>) -> SessionBody {
    SessionBody::PendingOpened {
        pending_id: pending_id.into(),
        indexed: Indexed {
            turn_id: turn.map(str::to_string),
            pending: Some(Box::new(PendingExtract {
                id: pending_id.into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into()]),
                title: title.map(str::to_string),
            })),
            ..Indexed::default()
        },
        payload: json!({"toolCall": {"toolCallId": "call-1"}}),
    }
}

fn ended(turn: &str, outcome: TurnOutcome) -> SessionBody {
    SessionBody::TurnEnded {
        turn_id: turn.into(),
        outcome,
        stop_reason: None,
        error: None,
    }
}

fn edge(store: &Store, seq: u64, body: &SessionBody) -> Option<PushEdge> {
    store.ingest_fact("s1", seq, body).unwrap().edge.map(|e| e.kind)
}

#[test]
fn the_first_question_of_a_turn_blocks_it_and_a_second_does_not() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    assert_eq!(
        edge(&store, 3, &question("p1", Some("t1"), Some("Run\n  cargo test"))),
        Some(PushEdge::Blocked {
            pending_id: "p1".into(),
            title: Some("Run cargo test".into()),
        })
    );
    assert_eq!(edge(&store, 4, &question("p2", Some("t1"), None)), None);
    // The same fact again is a duplicate: nothing.
    assert_eq!(
        edge(&store, 3, &question("p1", Some("t1"), Some("Run cargo test"))),
        None
    );
}

#[test]
fn a_question_outside_a_turn_is_an_edge_of_its_own() {
    let store = Store::open_in_memory().unwrap();
    store
        .create_session("s1", "h1", "fake", "/home/me/project", "hat-1", None)
        .unwrap();
    store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
    assert_eq!(
        edge(&store, 2, &question("p1", None, Some("Which branch?"))),
        Some(PushEdge::QuestionOutsideTurn {
            pending_id: "p1".into(),
            title: Some("Which branch?".into()),
        })
    );
    // It left the activity alone.
    assert_eq!(store.session("s1").unwrap().unwrap().activity.as_deref(), Some("idle"));
}

#[test]
fn a_turn_ends_once() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    assert_eq!(
        edge(&store, 3, &ended("t1", TurnOutcome::Completed)),
        Some(PushEdge::TurnEnded(TurnOutcome::Completed))
    );
    // A late duplicate for the ended turn is stored, not applied: nothing.
    assert_eq!(edge(&store, 4, &ended("t1", TurnOutcome::Completed)), None);
    // A turn that is not the open one: nothing.
    assert_eq!(edge(&store, 5, &ended("t9", TurnOutcome::Failed)), None);
}

#[test]
fn a_park_mid_turn_ends_it_without_an_edge() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    let parked = store
        .ingest_fact(
            "s1",
            3,
            &SessionBody::SessionParked {
                reason: hennery_proto::frames::ParkReason::Idle,
            },
        )
        .unwrap();
    // The turn is ended for it (`turn_ended_synthesized`), which is not a
    // host's `turn_ended`: no push (ACP core §10, P-25).
    assert!(parked.events.iter().any(|e| e.kind == "turn_ended_synthesized"));
    assert_eq!(parked.edge, None);
}

#[test]
fn a_question_for_a_closed_session_is_not_applied_and_crosses_nothing() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &SessionBody::SessionClosed).unwrap();
    assert_eq!(edge(&store, 4, &question("p1", Some("t1"), None)), None);
}

#[test]
fn reconciliation_and_the_resend_after_it_cross_nothing() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    // The host restarted: reconciliation ends the turn, writing no fact.
    let reconciled = store.reconcile_host("h1", &[]).unwrap();
    assert!(reconciled.events.iter().any(|e| e.kind == "turn_ended_synthesized"));
    // Its outbox resent after: the facts it already had are duplicates.
    assert_eq!(
        edge(
            &store,
            2,
            &SessionBody::TurnStarted {
                request_id: "req-t1".into(),
                turn_id: "t1".into()
            }
        ),
        None
    );
    assert_eq!(edge(&store, 3, &ended("t1", TurnOutcome::Completed)), None);
}

/// 10b-i's review, A2: asked, withdrawn by the agent, asked again in the
/// same turn: the second ask blocks the turn but notifies nothing.
#[test]
fn a_question_asked_again_after_a_withdrawal_crosses_nothing() {
    use hennery_proto::frames::{PendingReason, PendingResolution};
    let store = Store::open_in_memory().unwrap();
    running(&store);
    assert!(matches!(
        edge(&store, 3, &question("p1", Some("t1"), None)),
        Some(PushEdge::Blocked { .. })
    ));
    let withdrawn = SessionBody::PendingResolved {
        pending_id: "p1".into(),
        resolution: PendingResolution::Cancelled,
        reason: Some(PendingReason::AgentWithdrew),
    };
    store.ingest("s1", 4, &withdrawn).unwrap();
    assert_eq!(edge(&store, 5, &question("p2", Some("t1"), None)), None);
    assert_eq!(
        store.session("s1").unwrap().unwrap().activity.as_deref(),
        Some("blocked")
    );
}

/// 10b-i's review, A3: the edge carries the session as its fact left it.
#[test]
fn an_edge_carries_its_session_as_the_fact_left_it() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    let edge = store
        .ingest_fact("s1", 3, &question("p1", Some("t1"), None))
        .unwrap()
        .edge
        .unwrap();
    assert_eq!(
        edge.session,
        EdgeSession {
            id: "s1".into(),
            hat_id: "hat-1".into(),
            title: None,
            cwd: "/home/me/project".into(),
        }
    );
}

/// Decision 1: the activity decides, not the turn id. A question with no
/// turn id that blocks a running turn is `Blocked`.
#[test]
fn a_question_without_a_turn_id_that_blocks_a_running_turn_is_blocked() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    assert!(matches!(
        edge(&store, 3, &question("p1", None, None)),
        Some(PushEdge::Blocked { .. })
    ));
}

#[test]
fn a_close_mid_turn_ends_it_without_an_edge() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    let closed = store.ingest_fact("s1", 3, &SessionBody::SessionClosed).unwrap();
    assert!(closed.events.iter().any(|e| e.kind == "turn_ended_synthesized"));
    assert_eq!(closed.edge, None);
}
