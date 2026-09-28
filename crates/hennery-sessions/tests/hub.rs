//! The hub's waiters belong to the connection their request went out on
//! (plan A's "After this plan"): an old connection of a host that has
//! already reconnected must not fail or kick what the new one carries.

use hennery_proto::frames::{CollectorFrame, SessionBody};
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
    let registration = hub.register("h", tx).expect("no live connection for h");
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
