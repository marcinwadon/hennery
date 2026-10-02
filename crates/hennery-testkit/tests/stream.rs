//! The session stream, `GET /api/stream/sessions/{id}` (ACP core §9), as
//! smoke test #1 found it (F2): 404 for a session the owner does not have,
//! and a replay read a page at a time that hands over to the live events
//! without losing or repeating one.

use futures::StreamExt;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Indexed, SessionBody};
use hennery_proto::rest::ApiError;
use hennery_sessions::AppState;
use hennery_sessions::api::REPLAY_PAGE;
use hennery_sessions::store::Store;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;

/// Another owner, written straight into the database as `owner.rs` does:
/// nothing in v1 makes one.
const OTHER: &str = "owner-00000000000000b2";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    client: reqwest::Client,
    db: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        let client = hennery_testkit::operator_client(&state.operator);
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            addr,
            state,
            client,
            db,
            _dir: dir,
        }
    }

    /// The stream's answer, within a bound: a stream that hangs fails.
    async fn open(&self, session: &str) -> reqwest::Response {
        tokio::time::timeout(
            Duration::from_secs(10),
            self.client
                .get(format!("http://{}/api/stream/sessions/{session}", self.addr))
                .send(),
        )
        .await
        .expect("the stream answered")
        .unwrap()
    }

    /// The timeline's answer, `GET /api/sessions/{session}/events`.
    async fn events(&self, session: &str) -> reqwest::Response {
        self.client
            .get(format!("http://{}/api/sessions/{session}/events", self.addr))
            .send()
            .await
            .unwrap()
    }
}

fn update(n: usize) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed::default(),
        payload: json!({"update": {"sessionUpdate": "agent_message_chunk", "n": n}}),
    }
}

/// The `id:` of every timeline event in the SSE text `body`, in order.
fn event_ids(body: &str) -> Vec<i64> {
    body.split("\n\n")
        .filter(|m| m.lines().any(|l| l == "event: event"))
        .filter_map(|m| m.lines().find_map(|l| l.strip_prefix("id: ")))
        .map(|id| id.parse().unwrap())
        .collect()
}

async fn not_found(resp: reqwest::Response) {
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.json::<ApiError>().await.unwrap().code, "not_found");
}

/// The stream and the timeline alike (ACP core §9, "Common answers").
#[tokio::test]
async fn an_unknown_session_is_404() {
    let c = Collector::start().await;
    not_found(c.open("does-not-exist").await).await;
    not_found(c.events("does-not-exist").await).await;
}

/// No existence oracle: another owner's session answers as an unknown one.
#[tokio::test]
async fn another_owners_session_is_404() {
    let c = Collector::start().await;
    let conn = rusqlite::Connection::open(&c.db).unwrap();
    conn.execute(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, ?2, ?2)",
        rusqlite::params![OTHER, i64::MAX],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at, owner_id)
             VALUES ('theirs', 'h1', 'fake', '/tmp', 'active', ?1, ?1, ?2)",
        ["2026-10-01T00:00:00Z", OTHER],
    )
    .unwrap();
    not_found(c.open("theirs").await).await;
    not_found(c.events("theirs").await).await;
}

/// A history of more than two pages replays every event once, in order;
/// an event both replayed and published live is sent once; and a live
/// event after the replay follows it.
#[tokio::test]
async fn a_replay_of_several_pages_hands_over_to_the_live_events() {
    let c = Collector::start().await;
    let store = &c.state.store;
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
    let total = 2 * REPLAY_PAGE as usize + 7;
    for n in 2..=total {
        store.ingest("s1", n as u64, &update(n)).unwrap();
    }
    let stored = store.events("s1", 0, u32::MAX).unwrap();
    assert_eq!(stored.len(), total);
    let resp = c.open("s1").await;
    assert_eq!(resp.status(), 200);
    // Published once the stream is subscribed, as an event stored just
    // before a page read it would be: the replay sends it, and the live
    // stream must not again. Whatever the replay has read by now, this one
    // is in a page.
    c.state.hub.publish(stored[stored.len() - 2].clone());
    let stored: Vec<i64> = stored.iter().map(|e| e.event_id).collect();
    let last = *stored.last().unwrap();

    let mut body = resp.bytes_stream();
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while event_ids(&buf).last() != Some(&last) {
        let chunk = tokio::time::timeout_at(deadline, body.next())
            .await
            .expect("the stream stalled")
            .unwrap()
            .unwrap();
        buf.push_str(&String::from_utf8_lossy(&chunk));
    }
    // One more, live after the replay.
    let live = store.ingest("s1", total as u64 + 2, &update(total + 2)).unwrap();
    let next = live.last().unwrap().event_id;
    for event in live {
        c.state.hub.publish(event);
    }
    while event_ids(&buf).last() != Some(&next) {
        let chunk = tokio::time::timeout_at(deadline, body.next())
            .await
            .expect("the live event never came")
            .unwrap()
            .unwrap();
        buf.push_str(&String::from_utf8_lossy(&chunk));
    }
    let mut expected = stored;
    expected.extend(store.events("s1", last, u32::MAX).unwrap().iter().map(|e| e.event_id));
    assert_eq!(event_ids(&buf), expected);
}
