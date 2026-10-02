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
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
use hennery_proto::rest::HostItem;
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

/// A registry with `host-1` paired, in memory: the tests below lock the
/// session database, and the registry must not wait on that lock.
fn paired_hosts() -> Hosts {
    let hosts = Hosts::open_in_memory().unwrap();
    let enrollment = Enrollment {
        public_key: host_key().public_key_hex(),
        name: "test".into(),
        host_version: "test".into(),
        platform: "test".into(),
    };
    hosts.register("host-1", &enrollment, 0).unwrap();
    hosts
}

/// On several threads: the collector's ingest blocks the thread it runs on
/// in SQLite's busy wait, and on the test's only thread that would stop the
/// deadline below from firing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_ingest_drops_the_connection_instead_of_acking_past_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let store = Store::open(&db).unwrap();
    store
        .create_session("s1", "host-1", "fake", "/tmp", "hat-1", None)
        .unwrap();

    let state = AppState::new(store, paired_hosts(), Operator::open_in_memory().unwrap());
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

    let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
        .await
        .unwrap();
    let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::text(
        serde_json::to_string(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "0".into(),
            host_id: "host-1".into(),
            proof: host_key().sign_hello(&nonce, "host-1", PROTOCOL_VERSION),
            capabilities: Default::default(),
            workspace_roots: vec![],
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
            body: SessionBody::session_started("r0", "a1"),
        })
        .unwrap(),
    ))
    .await
    .unwrap();

    // No ack ever arrives: the write blocks behind the lock, then errors,
    // and the reader must drop the connection rather than skip silently.
    // The ingest is not the only writer kept waiting: the attachment sweep
    // the collector runs at startup (plan 9b) holds the store's connection
    // through its own busy wait first. Both together took about 23 s on a
    // loaded macOS machine, past the 15 s this once had (PR #82's CI).
    // So as long as the undo test below: the drop ends the wait anyway.
    let outcome = tokio::time::timeout(Duration::from_secs(120), async {
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

/// A host rejection's undo (F1, final review) must not be answered as a
/// plain `Rejected`: if applying the undo itself fails, the store's state no
/// longer matches what the rejection announced, so the caller must see
/// delivery-unknown (as for a dropped connection) and the socket must drop
/// so the next handshake's reconciliation settles it.
///
/// Forced with the same second-SQLite-connection technique as the ingest
/// test above: `mark_failed_if_starting` (the undo for a rejected
/// `start_session`) blocks behind an exclusive write lock and, after the
/// store's fixed 5s busy timeout, returns a real `SQLITE_BUSY` error.
#[tokio::test]
async fn an_undo_error_answers_delivery_unknown_and_drops_the_connection() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let store = Store::open(&db).unwrap();

    let state = AppState::new(store, paired_hosts(), Operator::open_in_memory().unwrap());
    let shutdown = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = hennery_testkit::operator_client(&state.operator);
    let server = tokio::spawn(hennery_sessions::serve(listener, state));

    let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
        .await
        .unwrap();
    let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::text(
        serde_json::to_string(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "0".into(),
            host_id: "host-1".into(),
            proof: host_key().sign_hello(&nonce, "host-1", PROTOCOL_VERSION),
            // It starts a session, so it resolves paths (plan 5c).
            capabilities: Capabilities(vec![Capability::ResolvePath]),
            workspace_roots: vec![],
            attached_sessions: vec![],
        })
        .unwrap(),
    ))
    .await
    .unwrap();
    assert!(
        matches!(stream.next().await, Some(Ok(Message::Text(_)))),
        "expected hello_ack"
    );
    sink.send(Message::text(
        serde_json::to_string(&HostFrame::ResendComplete).unwrap(),
    ))
    .await
    .unwrap();

    // The host is not `ready` until `resend_complete` is processed: wait for
    // it via `/api/hosts` rather than racing the HTTP call below against it.
    let hosts_url = format!("http://{addr}/api/hosts");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let hosts: Vec<HostItem> = client.get(&hosts_url).send().await.unwrap().json().await.unwrap();
        if hosts.iter().any(|h| h.host_id == "host-1" && h.connected) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "host never became ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let post = tokio::spawn({
        let client = client.clone();
        let url = format!("http://{addr}/api/sessions");
        async move {
            client
                .post(url)
                .json(&json!({ "host_id": "host-1", "agent": "fake", "cwd": "/tmp" }))
                .send()
                .await
                .unwrap()
        }
    });

    // The cwd is resolved on the host first (plan 5c): to itself.
    match stream.next().await {
        Some(Ok(Message::Text(t))) => match serde_json::from_str::<CollectorFrame>(&t).unwrap() {
            CollectorFrame::ResolvePath { request_id, path } => sink
                .send(Message::text(
                    serde_json::to_string(&HostFrame::ResolvedPath {
                        request_id,
                        canonical: path,
                        exists: true,
                        is_dir: true,
                    })
                    .unwrap(),
                ))
                .await
                .unwrap(),
            other => panic!("expected resolve_path, got {other:?}"),
        },
        other => panic!("expected resolve_path frame, got {other:?}"),
    }

    // Only once `create_session` has committed and the request is on its
    // way (proven by receiving the frame) does the lock go up: the start
    // must already be in flight before the undo it exercises can block.
    let request_id = match stream.next().await {
        Some(Ok(Message::Text(t))) => match serde_json::from_str::<CollectorFrame>(&t).unwrap() {
            CollectorFrame::StartSession { request_id, .. } => request_id,
            other => panic!("expected start_session, got {other:?}"),
        },
        other => panic!("expected start_session frame, got {other:?}"),
    };

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

    sink.send(Message::text(
        serde_json::to_string(&HostFrame::Error {
            request_id,
            code: "unknown_agent".into(),
            message: "no such agent".into(),
        })
        .unwrap(),
    ))
    .await
    .unwrap();

    let resp = post.await.unwrap();
    assert_eq!(
        resp.status(),
        503,
        "expected delivery_unknown, not the host's rejection"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "delivery_unknown");
    let session_id = body["session_id"]
        .as_str()
        .expect("delivery_unknown carries the session_id")
        .to_string();

    // The undo's failure must drop the connection, same as a failed ingest.
    let outcome = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(t))) => panic!("unexpected frame after the undo failed: {t}"),
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return,
            }
        }
    })
    .await;
    assert!(outcome.is_ok(), "server kept the connection open after a failed undo");

    drop(locker); // release the exclusive lock (rolls back, nothing was committed)

    // The undo never applied: the session is exactly as `create_session`
    // left it, not failed with the host's (wrong) rejection code.
    let detail: serde_json::Value = client
        .get(format!("http://{addr}/api/sessions/{session_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["lifecycle"], "starting");

    shutdown.cancel();
    server.await.unwrap().unwrap();
}
