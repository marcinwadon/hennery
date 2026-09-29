//! The host socket's reader loop must never ack past a frame it failed to
//! ingest: acks are cumulative and the host prunes its outbox on ack, so
//! acking a later frame would tell the host the failed one is safe to
//! discard forever. A store error (lookup or ingest) must instead drop the
//! connection, the same as any other disconnect.
//!
//! Forced here with a second SQLite connection holding an exclusive write
//! lock on the same database file: the collector's own ingest blocks behind
//! it and, after the store's fixed 5s busy timeout, returns a real
//! `SQLITE_BUSY` error — no invasive hook into `Store` required.

use futures::{SinkExt, StreamExt};
use hennery_kernel::auth::DevToken;
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{HostFrame, SessionBody};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "dev-token";

#[tokio::test]
async fn a_failed_ingest_drops_the_connection_instead_of_acking_past_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let store = Store::open(&db).unwrap();
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();

    let state = AppState::new(store, DevToken::new(TOKEN));
    let shutdown = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(hennery_sessions::serve(listener, state));

    // A second connection holding the database's one write lock: the
    // collector's own ingest attempt blocks behind it until the store's
    // hard-coded 5s busy timeout elapses and rusqlite returns an error.
    let locker = tokio::task::spawn_blocking({
        let db = db.clone();
        move || {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE;").unwrap();
            conn
        }
    })
    .await
    .unwrap();

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
        .await
        .unwrap();
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::text(
        serde_json::to_string(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "0".into(),
            host_id: "host-1".into(),
            token: TOKEN.into(),
            capabilities: Default::default(),
            attached_sessions: vec![],
        })
        .unwrap(),
    ))
    .await
    .unwrap();
    // hello_ack: a plain read, unaffected by the writer lock (WAL readers
    // never block on a writer).
    assert!(
        matches!(stream.next().await, Some(Ok(Message::Text(_)))),
        "expected hello_ack"
    );

    sink.send(Message::text(
        serde_json::to_string(&HostFrame::Session {
            session_id: "s1".into(),
            seq: 1,
            body: SessionBody::SessionStarted {
                request_id: "r0".into(),
                agent_session_id: "a1".into(),
            },
        })
        .unwrap(),
    ))
    .await
    .unwrap();

    // No ack ever arrives: the write blocks behind the lock, then errors,
    // and the reader must drop the connection rather than skip silently.
    let outcome = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(t))) => panic!("unexpected frame while ingest was blocked: {t}"),
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return,
            }
        }
    })
    .await;
    assert!(outcome.is_ok(), "server kept the connection open after a failed ingest");

    drop(locker); // release the exclusive lock (rolls back, nothing was committed)
    shutdown.cancel();
    server.await.unwrap().unwrap();
}
