//! The hub's waiters belong to the connection their request went out on
//! (plan A's "After this plan"): an old connection of a host that has
//! already reconnected must not fail or kick what the new one carries.

use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, ParkReason, SessionBody, TurnOutcome};
use hennery_sessions::hub::{Hub, Registration, RequestError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn prompt(request_id: &str) -> CollectorFrame {
    CollectorFrame::Prompt {
        request_id: request_id.into(),
        session_id: "s1".into(),
        turn_id: format!("t-{request_id}"),
        content: vec![serde_json::json!({"type": "text", "text": "hi"})],
    }
}

fn started(request_id: &str) -> SessionBody {
    SessionBody::TurnStarted {
        request_id: request_id.into(),
        turn_id: format!("t-{request_id}"),
    }
}

/// Register and mark ready a connection for host `h`.
fn connect(hub: &Hub) -> (Registration, mpsc::UnboundedReceiver<CollectorFrame>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let registration = hub
        .register("h", tx, Capabilities::default())
        .expect("no live connection for h");
    hub.mark_ready("h", registration.conn_id);
    (registration, rx)
}

#[tokio::test]
async fn an_old_connections_unregister_fails_only_its_own_requests() {
    let hub = Arc::new(Hub::new());
    let (old, mut old_rx) = connect(&hub);
    let on_old = tokio::spawn({
        let hub = hub.clone();
        async move { hub.request("h", "r1", prompt("r1"), Duration::from_secs(5)).await }
    });
    old_rx.recv().await.expect("r1 went out on the old connection");
    // The old socket's writer is gone; its reader has not noticed yet, so
    // the host's reconnect is accepted.
    drop(old_rx);
    let (_new, mut new_rx) = connect(&hub);
    let on_new = tokio::spawn({
        let hub = hub.clone();
        async move { hub.request("h", "r2", prompt("r2"), Duration::from_secs(5)).await }
    });
    new_rx.recv().await.expect("r2 went out on the new connection");

    hub.unregister("h", old.conn_id); // the old reader finally ends
    assert_eq!(on_old.await.unwrap(), Err(RequestError::DeliveryUnknown));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!on_new.is_finished(), "the new connection's request was failed");
    assert!(hub.is_ready("h"), "the old unregister dropped the new connection");
    hub.resolve("r2", started("r2"));
    assert_eq!(on_new.await.unwrap(), Ok(started("r2")));
}

#[tokio::test]
async fn a_timeout_on_an_old_connection_does_not_kick_the_new_one() {
    let hub = Arc::new(Hub::new());
    let (_old, mut old_rx) = connect(&hub);
    let on_old = tokio::spawn({
        let hub = hub.clone();
        async move { hub.request("h", "r1", prompt("r1"), Duration::from_millis(200)).await }
    });
    old_rx.recv().await.expect("r1 went out on the old connection");
    drop(old_rx);
    let (new, _new_rx) = connect(&hub);
    assert_eq!(on_old.await.unwrap(), Err(RequestError::DeliveryUnknown));
    assert!(
        !new.kicked.is_cancelled(),
        "the old request's timeout kicked the new connection"
    );
}

fn cancel(request_id: &str) -> CollectorFrame {
    CollectorFrame::CancelTurn {
        request_id: request_id.into(),
        session_id: "s1".into(),
        turn_id: "t1".into(),
    }
}

fn ended(turn_id: &str, outcome: TurnOutcome) -> SessionBody {
    SessionBody::TurnEnded {
        turn_id: turn_id.into(),
        outcome,
        stop_reason: None,
        error: None,
    }
}

/// `cancel_turn` is completed by its turn's end, whatever the outcome; a
/// fact about another turn, only about the session, or naming the right
/// turn_id under the wrong session does not complete it. Without the
/// session check a host could complete another session's cancel waiter with
/// a fabricated outcome merely by naming that turn_id under a session it
/// owns (final review M1).
#[tokio::test]
async fn a_turn_waiter_resolves_only_on_that_sessions_turn_end() {
    let hub = Arc::new(Hub::new());
    let (_conn, mut rx) = connect(&hub);
    let call = tokio::spawn({
        let hub = hub.clone();
        async move {
            hub.request_for_turn("h", "rc", "s1", "t1", cancel("rc"), Duration::from_secs(5))
                .await
        }
    });
    rx.recv().await.expect("the cancel went out");
    hub.resolve_turn("s1", "t0", ended("t0", TurnOutcome::Completed));
    // Same turn_id, but a different session: must not complete `s1`'s waiter.
    hub.resolve_turn("s-other", "t1", ended("t1", TurnOutcome::Completed));
    hub.resolve_session(
        "s1",
        SessionBody::SessionParked {
            reason: ParkReason::Operator,
        },
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!call.is_finished(), "completed by a fact about something else");
    hub.resolve_turn("s1", "t1", ended("t1", TurnOutcome::Completed));
    assert_eq!(call.await.unwrap(), Ok(ended("t1", TurnOutcome::Completed)));

    // A rejection still matches the request id.
    let call = tokio::spawn({
        let hub = hub.clone();
        async move {
            hub.request_for_turn("h", "rc2", "s1", "t1", cancel("rc2"), Duration::from_secs(5))
                .await
        }
    });
    rx.recv().await.expect("the second cancel went out");
    hub.reject("rc2", "not_running".into(), "no".into());
    assert_eq!(
        call.await.unwrap(),
        Err(RequestError::Rejected {
            code: "not_running".into(),
            message: "no".into()
        })
    );
}

#[tokio::test]
async fn capabilities_belong_to_the_hosts_current_connection() {
    let hub = Hub::new();
    assert!(!hub.has_capability("h", Capability::Park), "an unknown host has none");
    let (tx, _rx) = mpsc::unbounded_channel();
    let first = hub.register("h", tx, Capabilities(vec![Capability::Park])).unwrap();
    assert!(hub.has_capability("h", Capability::Park));
    assert!(!hub.has_capability("h", Capability::Images));
    hub.unregister("h", first.conn_id);
    assert!(!hub.has_capability("h", Capability::Park), "a gone host has none");
    let (tx, _rx) = mpsc::unbounded_channel();
    hub.register("h", tx, Capabilities::default()).unwrap();
    assert!(
        !hub.has_capability("h", Capability::Park),
        "an older build of the host that cannot park reconnected"
    );
}

/// Decision 6: the hub owns every waiter's deadline. A handler dropped
/// mid-request (its client left) must not leave its waiter behind, nor
/// spare a connection that never answered.
#[tokio::test]
async fn a_dropped_requests_deadline_still_kicks_the_connection_and_frees_its_waiter() {
    let hub = Arc::new(Hub::new());
    let (conn, mut rx) = connect(&hub);
    let call = tokio::spawn({
        let hub = hub.clone();
        async move { hub.request("h", "r1", prompt("r1"), Duration::from_millis(300)).await }
    });
    rx.recv().await.expect("r1 went out");
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    assert_eq!(hub.pending_requests(), 1, "the waiter outlives its handler");
    assert!(!conn.kicked.is_cancelled());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(conn.kicked.is_cancelled(), "a connection that never answered was kept");
    assert_eq!(hub.pending_requests(), 0, "the waiter leaked");
}

// Plan 3a: a revoke waits for the host's socket task to let go.

#[tokio::test]
async fn disconnect_and_wait_returns_once_the_socket_task_has_let_go() {
    let hub = Arc::new(Hub::new());
    assert!(
        hub.disconnect_and_wait("h", Duration::from_millis(10)).await,
        "no connection: nothing to wait for"
    );
    let (registration, _rx) = connect(&hub);
    // The socket task: it notices the kick, and unregisters a little later.
    let socket_task = tokio::spawn({
        let hub = hub.clone();
        async move {
            registration.kicked.cancelled().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            hub.unregister("h", registration.conn_id);
        }
    });
    assert!(hub.disconnect_and_wait("h", Duration::from_secs(10)).await);
    assert!(!hub.is_ready("h"));
    socket_task.await.unwrap();
}

#[tokio::test]
async fn disconnect_and_wait_gives_up_after_its_bound() {
    let hub = Hub::new();
    let (registration, _rx) = connect(&hub);
    assert!(!hub.disconnect_and_wait("h", Duration::from_millis(50)).await);
    assert!(registration.kicked.is_cancelled());
}
