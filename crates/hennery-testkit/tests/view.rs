//! The view API over HTTP (client view spec §4, §7; plan 4a-ii): a real
//! collector, with the test writing events straight into its database, as
//! ingest would, and ringing the hub as ingest does.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_proto::rest::{EventDto, StoredBlock};
use hennery_sessions::AppState;
use hennery_sessions::store::{Deletion, Store};
use hennery_sessions::view::{COALESCE, LIST_COALESCE, RESUME_MAX_EVENTS, RESUME_MAX_GROUPS};
use hennery_view::api::{ItemPage, SessionSummary, SummaryPage, TurnContent, WaitingChanged};
use hennery_view::{Body, Item, fold};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::Instant;

/// A second owner, written straight into the database, as `owner.rs`
/// does: nothing in v1 makes one.
const OTHER: &str = "owner-00000000000000b2";

/// How long a test waits for anything the collector sends.
const WAIT: Duration = Duration::from_secs(20);

struct Collector {
    addr: SocketAddr,
    state: AppState,
    db: PathBuf,
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
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            addr,
            state,
            db,
            _dir: dir,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn client(&self) -> reqwest::Client {
        hennery_testkit::operator_client(&self.state.operator)
    }

    fn owner(&self) -> String {
        self.state.store.owner_id().to_string()
    }

    /// A connection of the test's own to the collector's database.
    fn conn(&self) -> Connection {
        let conn = Connection::open(&self.db).unwrap();
        conn.busy_timeout(Duration::from_secs(10)).unwrap();
        conn
    }

    /// A session of `owner`'s, as `create_session` writes one.
    fn session(&self, owner: &str, id: &str) {
        self.conn()
            .execute(
                "INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at, owner_id)
                 VALUES (?1, 'host-1', 'fake', '/tmp', 'active', ?2, ?2, ?3)",
                params![id, "2026-10-01T00:00:00.000Z", owner],
            )
            .unwrap();
    }

    /// The second owner, and a session of theirs with one event.
    fn other_owner(&self, id: &str) {
        self.conn()
            .execute(
                "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, ?2, ?2)",
                params![OTHER, i64::MAX],
            )
            .unwrap();
        self.session(OTHER, id);
        self.write(OTHER, id, "user_turn", &turn("theirs"));
    }

    /// Store an event of `session`'s, stamped now, as the collector does,
    /// with the session's recency; nothing is told of it.
    fn write(&self, owner: &str, session: &str, kind: &str, body: &Value) -> EventDto {
        self.write_as(owner, session, kind, body, true)
    }

    fn write_as(&self, owner: &str, session: &str, kind: &str, body: &Value, recency: bool) -> EventDto {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
             VALUES (?1, NULL, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?4)",
            params![session, kind, body.to_string(), owner],
        )
        .unwrap();
        let event_id = conn.last_insert_rowid();
        let ts: String = conn
            .query_row("SELECT ts FROM events WHERE event_id = ?1", [event_id], |r| r.get(0))
            .unwrap();
        if recency {
            conn.execute(
                "UPDATE sessions SET last_event_at = ?1, last_event_id = ?2 WHERE id = ?3",
                params![ts, event_id, session],
            )
            .unwrap();
        }
        EventDto {
            event_id,
            session_id: session.to_string(),
            host_seq: None,
            kind: kind.to_string(),
            body: body.clone(),
            ts,
        }
    }

    /// Store `bodies` as acp updates of `session`'s in one transaction, and
    /// ring the hub once, for the last.
    fn events_at_once(&self, session: &str, bodies: &[Value]) -> EventDto {
        let mut conn = self.conn();
        let tx = conn.transaction().unwrap();
        let mut last = 0;
        for body in bodies {
            tx.execute(
                "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
                 VALUES (?1, NULL, 'acp_update', ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?3)",
                params![session, body.to_string(), self.owner()],
            )
            .unwrap();
            last = tx.last_insert_rowid();
        }
        tx.commit().unwrap();
        let event = EventDto {
            event_id: last,
            session_id: session.to_string(),
            host_seq: None,
            kind: "acp_update".into(),
            body: bodies.last().unwrap().clone(),
            ts: String::new(),
        };
        self.state.hub.publish(event.clone());
        event
    }

    /// Store an event of the owner's and ring the hub, as ingest does.
    fn event(&self, session: &str, kind: &str, body: &Value) -> EventDto {
        let event = self.write(&self.owner(), session, kind, body);
        self.state.hub.publish(event.clone());
        event
    }

    /// A recorded session (`hennery-view`'s fixtures) stored as the
    /// owner's session `session`, read back as stored.
    fn recorded(&self, fixture: &str, session: &str) -> Vec<EventDto> {
        self.session(&self.owner(), session);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../hennery-view/tests/fixtures")
            .join(format!("{fixture}.jsonl"));
        let text = std::fs::read_to_string(path).unwrap();
        for line in text.lines() {
            let event: EventDto = serde_json::from_str(line).unwrap();
            self.write(&self.owner(), session, &event.kind, &event.body);
        }
        self.state.store.events(session, 0, u32::MAX).unwrap()
    }

    async fn page(&self, session: &str, query: &str) -> ItemPage {
        let resp = self
            .client()
            .get(self.url(&format!("/api/view/sessions/{session}{query}")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        resp.json().await.unwrap()
    }

    async fn summaries(&self, query: &str) -> SummaryPage {
        let resp = self
            .client()
            .get(self.url(&format!("/api/view/sessions{query}")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        resp.json().await.unwrap()
    }

    /// An SSE stream, with `Last-Event-ID` if given.
    async fn stream(&self, path: &str, last_event_id: Option<&str>) -> Sse {
        self.stream_as(&self.client(), path, last_event_id).await
    }

    /// An SSE stream of the operator's session `client` holds.
    async fn stream_as(&self, client: &reqwest::Client, path: &str, last_event_id: Option<&str>) -> Sse {
        let mut req = client.get(self.url(path));
        if let Some(id) = last_event_id {
            req = req.header("last-event-id", id);
        }
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), 200);
        Sse {
            resp,
            buf: String::new(),
        }
    }
}

fn turn(turn_id: &str) -> Value {
    json!({"turn_id": turn_id, "content": [{"type": "text", "text": "go"}]})
}

fn acp(update: Value) -> Value {
    json!({"kind": "acp_update", "indexed": {}, "payload": {"sessionId": "a", "update": update}})
}

fn chunk(text: &str) -> Value {
    acp(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}))
}

fn tool(call: &str, status: &str) -> Value {
    acp(json!({"sessionUpdate": "tool_call", "toolCallId": call, "title": "Run", "status": status}))
}

/// One SSE message.
#[derive(Debug)]
struct Message {
    event: String,
    data: String,
    id: Option<String>,
}

impl Message {
    fn item(&self) -> Item {
        assert_eq!(self.event, "item", "{self:?}");
        serde_json::from_str(&self.data).unwrap()
    }

    fn summary(&self) -> SessionSummary {
        assert_eq!(self.event, "session_upsert", "{self:?}");
        serde_json::from_str(&self.data).unwrap()
    }
}

struct Sse {
    resp: reqwest::Response,
    buf: String,
}

impl Sse {
    /// The next message, keep-alives skipped; `None` once the stream ends.
    async fn next(&mut self) -> Option<Message> {
        let deadline = Instant::now() + WAIT;
        loop {
            while let Some(end) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..end + 2).collect();
                let mut message = Message {
                    event: "message".into(),
                    data: String::new(),
                    id: None,
                };
                let mut fields = false;
                for line in block.lines() {
                    let Some((field, value)) = line.split_once(':') else {
                        continue;
                    };
                    let value = value.strip_prefix(' ').unwrap_or(value).to_string();
                    match field {
                        "event" => message.event = value,
                        "data" => message.data = value,
                        "id" => message.id = Some(value),
                        _ => continue,
                    }
                    fields = true;
                }
                if fields {
                    return Some(message);
                }
            }
            let chunk = tokio::time::timeout_at(deadline, self.resp.chunk())
                .await
                .expect("an SSE message in time")
                .unwrap()?;
            self.buf.push_str(std::str::from_utf8(&chunk).unwrap());
        }
    }

    async fn message(&mut self) -> Message {
        self.next().await.expect("the stream ended")
    }

    /// The list stream's next message is `waiting_changed`: its count and
    /// its id.
    async fn counted(&mut self) -> (u32, Option<String>) {
        let message = self.message().await;
        assert_eq!(message.event, "waiting_changed", "{message:?}");
        let count: WaitingChanged = serde_json::from_str(&message.data).unwrap();
        (count.count, message.id)
    }

    /// The stream sends `resync_required`, and ends.
    async fn resyncs(&mut self) {
        let message = self.message().await;
        assert_eq!(
            (message.event.as_str(), message.data.as_str()),
            ("resync_required", "{}")
        );
        assert!(self.next().await.is_none(), "the stream goes on after a resync");
    }
}

fn none_answerable(_: &str) -> bool {
    false
}

const RECORDINGS: [&str; 2] = ["claude-agent-acp-0.81.0", "codex-acp-1.13.0"];

/// A2: the tail page is the fold of the session's last `limit` groups, and
/// pages before it walk back to the start: every item once, and the
/// session's whole fold in all.
#[tokio::test]
async fn a_page_is_the_fold_of_its_last_turns_and_pages_walk_back_to_the_start() {
    let collector = Collector::start().await;
    for (n, recording) in RECORDINGS.iter().enumerate() {
        let session = format!("session-{n}");
        let events = collector.recorded(recording, &session);
        let starts: Vec<usize> = (0..events.len()).filter(|&i| events[i].kind == "user_turn").collect();
        let page = collector.page(&session, "?limit=3").await;
        let third_last = starts[starts.len() - 3];
        assert_eq!(page.items, fold(&events[third_last..], &none_answerable), "{recording}");
        assert!(page.older);
        assert_eq!(page.revision, events.last().unwrap().event_id);
        assert!(!page.epoch.is_empty());

        let mut pages = vec![collector.page(&session, "?limit=2").await];
        while pages.last().unwrap().older {
            let first = &pages.last().unwrap().items[0];
            let turn = first.turn_id.as_deref().expect("an older page's turn");
            pages.push(collector.page(&session, &format!("?limit=2&before_turn={turn}")).await);
            assert!(pages.len() <= starts.len(), "{recording}: pages never end");
        }
        let walked: Vec<Item> = pages.into_iter().rev().flat_map(|page| page.items).collect();
        assert_eq!(walked, fold(&events, &none_answerable), "{recording}");
    }
}

/// A2: a `before_turn` that names no turn of the session is refused.
#[tokio::test]
async fn a_page_refuses_a_turn_the_session_does_not_have() {
    let collector = Collector::start().await;
    collector.recorded(RECORDINGS[0], "s");
    for query in ["?before_turn=nope", "?before_turn=start", "?before_turn=event-1"] {
        let resp = collector
            .client()
            .get(collector.url(&format!("/api/view/sessions/s{query}")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "{query}");
        assert_eq!(resp.json::<Value>().await.unwrap()["code"], "invalid", "{query}");
    }
}

/// A2: a `limit` that is not a whole number is refused.
#[tokio::test]
async fn a_page_refuses_a_limit_that_is_not_a_number() {
    let collector = Collector::start().await;
    collector.recorded(RECORDINGS[0], "s");
    for query in ["?limit=x", "?limit=-1"] {
        let resp = collector
            .client()
            .get(collector.url(&format!("/api/view/sessions/s{query}")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "{query}");
        assert_eq!(resp.json::<Value>().await.unwrap()["code"], "invalid", "{query}");
    }
}

/// A1, F2: an unknown session and another owner's answer 404 on the page
/// and on the item stream, and read no event to say so.
#[tokio::test]
async fn an_unknown_or_foreign_session_is_404_and_reads_nothing() {
    let collector = Collector::start().await;
    collector.other_owner("theirs");
    let c = collector.client();
    let before = collector.state.store.view_reads();
    for session in ["nope", "theirs"] {
        for path in [
            format!("/api/view/sessions/{session}"),
            format!("/api/stream/view/sessions/{session}"),
            format!("/api/view/sessions/{session}/turns/lost"),
        ] {
            let resp = c.get(collector.url(&path)).send().await.unwrap();
            assert_eq!(resp.status(), 404, "{path}");
            assert_eq!(resp.json::<Value>().await.unwrap()["code"], "not_found", "{path}");
        }
    }
    assert_eq!(collector.state.store.view_reads(), before);
    // The control: the owner's own session is read.
    collector.session(&collector.owner(), "mine");
    collector.page("mine", "").await;
    assert!(collector.state.store.view_reads() > before);
}

/// Plan 4a-i's hand-off (frontend §6.1 "send again"): the prompt of a turn
/// that was not delivered is served whole, as stored; any other turn,
/// another session's included, is 404.
#[tokio::test]
async fn a_turn_not_delivered_is_served_to_send_again() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "s");
    collector.session(&owner, "other");
    let long = "x".repeat(hennery_view::cap::TEXT_MAX_BYTES + 1);
    let image = json!({"type": "image", "mimeType": "image/png", "sha256": "ab".repeat(32), "size": 3});
    for (turn, session, state, content) in [
        (
            "lost",
            "s",
            "not_delivered",
            json!([{"type": "text", "text": "go"}, image]),
        ),
        ("long", "s", "not_delivered", json!([{"type": "text", "text": long}])),
        ("sent", "s", "ended", json!([{"type": "text", "text": "go"}])),
        (
            "elsewhere",
            "other",
            "not_delivered",
            json!([{"type": "text", "text": "go"}]),
        ),
    ] {
        collector
            .conn()
            .execute(
                "INSERT INTO turns(turn_id, session_id, content, created_at, owner_id, state)
                 VALUES (?1, ?2, ?3, '2026-10-01T00:00:00.000Z', ?4, ?5)",
                params![turn, session, content.to_string(), owner, state],
            )
            .unwrap();
    }
    let get = |turn: &str| {
        collector
            .client()
            .get(collector.url(&format!("/api/view/sessions/s/turns/{turn}")))
            .send()
    };
    let resp = get("lost").await.unwrap();
    assert_eq!(resp.status(), 200);
    let lost: TurnContent = resp.json().await.unwrap();
    assert_eq!(
        serde_json::to_value(&lost).unwrap(),
        json!({"turn_id": "lost", "content": [{"type": "text", "text": "go"}, image]})
    );
    // Whole, past the cap a `user_turn` item has (the security review's
    // B-4): sent again, it is the prompt as written.
    let long: TurnContent = get("long").await.unwrap().json().await.unwrap();
    assert!(
        matches!(&long.content[..], [StoredBlock::Text { text }] if text.len() == hennery_view::cap::TEXT_MAX_BYTES + 1)
    );
    for turn in ["sent", "elsewhere", "nope"] {
        let resp = get(turn).await.unwrap();
        assert_eq!(resp.status(), 404, "{turn}");
        assert_eq!(resp.json::<Value>().await.unwrap()["code"], "not_found", "{turn}");
    }
}

/// Client view §7: one owner never sees another's summaries, on the page
/// or on the list stream.
#[tokio::test]
async fn one_owner_never_sees_anothers_summaries() {
    let collector = Collector::start().await;
    collector.other_owner("theirs");
    collector.session(&collector.owner(), "mine");
    collector.write(&collector.owner(), "mine", "user_turn", &turn("t1"));
    let page = collector.summaries("").await;
    let listed: Vec<&str> = page.sessions.iter().map(|s| s.session.session_id.as_str()).collect();
    assert_eq!(listed, ["mine"]);
    assert!(collector.summaries("?q=theirs").await.sessions.is_empty());

    let after = format!("{}:{}", page.epoch, page.revision);
    let mut stream = collector
        .stream(&format!("/api/stream/sessions?after={after}"), None)
        .await;
    assert_eq!(stream.counted().await, (0, Some(after.clone())));
    let theirs = collector.write(OTHER, "theirs", "user_turn", &turn("t2"));
    collector.state.hub.publish(theirs);
    collector.event("mine", "user_turn", &turn("t3"));
    let message = stream.message().await;
    assert_eq!(message.summary().session.session_id, "mine");
}

/// A7, the security review's B-5: `question_waits` is set by an open
/// question, an answer queued for it or not; a question no longer open
/// sets none.
#[tokio::test]
async fn a_summary_says_whether_a_question_waits() {
    let collector = Collector::start().await;
    for session in ["asks", "answered", "quiet", "withdrawn"] {
        collector.session(&collector.owner(), session);
        collector.write(&collector.owner(), session, "user_turn", &turn(session));
    }
    question(&collector, "asks", "p1", "asks");
    question(&collector, "answered", "p2", "answered");
    question(&collector, "withdrawn", "p3", "withdrawn");
    collector
        .conn()
        .execute("UPDATE pending SET state = 'cancelled' WHERE pending_id = 'p3'", [])
        .unwrap();
    let resp = collector
        .client()
        .post(collector.url("/api/sessions/answered/pending/p2/answer"))
        .json(&json!({"option_id": "allow"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    let page = collector.summaries("").await;
    let mut waits: Vec<(&str, bool)> = page
        .sessions
        .iter()
        .map(|s| (s.session.session_id.as_str(), s.question_waits))
        .collect();
    waits.sort();
    // Queued, not delivered: the agent still waits on it.
    assert_eq!(
        waits,
        [
            ("answered", true),
            ("asks", true),
            ("quiet", false),
            ("withdrawn", false)
        ]
    );
}

/// A permission question `pending_id` of `session`'s, asked in `turn`:
/// its event and its record, as ingest writes them; nothing is told.
fn question(collector: &Collector, session: &str, pending_id: &str, turn: &str) -> EventDto {
    let payload = json!({"toolCall": {"toolCallId": "c1", "title": "Write a.txt"},
                         "options": [{"optionId": "allow", "name": "Yes", "kind": "allow_once"}]});
    let event = collector.write(
        &collector.owner(),
        session,
        "pending_opened",
        &json!({"kind": "pending_opened", "pending_id": pending_id,
                "indexed": {"turn_id": turn, "pending": {"id": pending_id, "kind": "permission", "option_ids": ["allow"]}},
                "payload": payload}),
    );
    collector
        .conn()
        .execute(
            "INSERT INTO pending(pending_id, session_id, kind, turn_id, option_ids, payload, state, opened_at, owner_id)
             VALUES (?1, ?2, 'permission', ?3, '[\"allow\"]', ?4, 'open', ?5, ?6)",
            params![pending_id, session, turn, payload.to_string(), event.ts, collector.owner()],
        )
        .unwrap();
    event
}

/// A session with a turn, whose page has been read: the stream's anchor.
async fn opened(collector: &Collector, session: &str) -> String {
    collector.session(&collector.owner(), session);
    collector.write(&collector.owner(), session, "user_turn", &turn("t1"));
    let page = collector.page(session, "").await;
    format!("{}:{}", page.epoch, page.revision)
}

/// Wait until `stream` follows live events: past its resume, which reads
/// whatever is stored by then and sends it at once, in one burst.
async fn live(collector: &Collector, stream: &mut Sse) {
    let event = collector.event("s", "acp_update", &tool("live", "pending"));
    let item = stream.message().await.item();
    assert_eq!((item.id.ends_with(":tool:live"), item.version), (true, event.event_id));
}

fn epoch_of(anchor: &str) -> &str {
    anchor.rsplit_once(':').unwrap().0
}

/// A-1: a stream opened at the page's revision sends nothing for what
/// the page has, then each change, its id the event that made it.
#[tokio::test]
async fn a_fresh_stream_sends_nothing_until_an_event_then_its_upsert() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let mut stream = collector
        .stream(&format!("/api/stream/view/sessions/s?after={anchor}"), None)
        .await;
    let event = collector.event("s", "acp_update", &tool("c1", "pending"));
    let message = stream.message().await;
    let item = message.item();
    assert_eq!(item.id, "t1:tool:c1");
    assert_eq!(item.version, event.event_id);
    assert_eq!(message.id, Some(format!("{}:{}", epoch_of(&anchor), event.event_id)));
}

/// The stream subscribes before it reads anything: an event stored and
/// rung the moment its headers arrive is sent.
#[tokio::test]
async fn an_event_right_after_the_stream_opens_is_sent() {
    let collector = Collector::start().await;
    opened(&collector, "s").await;
    for n in 0..20 {
        let page = collector.page("s", "").await;
        let anchor = format!("{}:{}", page.epoch, page.revision);
        let mut stream = collector
            .stream(&format!("/api/stream/view/sessions/s?after={anchor}"), None)
            .await;
        let call = format!("c{n}");
        collector.event("s", "acp_update", &tool(&call, "pending"));
        assert_eq!(stream.message().await.item().id, format!("t1:tool:{call}"));
    }
}

/// A5, client view §7: a reconnect gets exactly what changed since its
/// id: each item once, as it stands, only the last message carrying the
/// id, which is the session's revision.
#[tokio::test]
async fn a_resume_sends_exactly_the_missed_items_once() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let owner = collector.owner();
    let tool_first = collector.write(&owner, "s", "acp_update", &tool("x", "pending"));
    collector.write(&owner, "s", "acp_update", &chunk("y1"));
    collector.write(&owner, "s", "acp_update", &chunk("y2"));
    collector.write(
        &owner,
        "s",
        "acp_update",
        &acp(json!({"sessionUpdate": "usage_update"})),
    );
    let tool_last = collector.write(&owner, "s", "acp_update", &tool("x", "completed"));
    let plan = collector.write(
        &owner,
        "s",
        "acp_update",
        &acp(json!({"sessionUpdate": "plan", "entries": [{"content": "z"}]})),
    );
    assert!(tool_first.event_id < tool_last.event_id);
    let mut stream = collector.stream("/api/stream/view/sessions/s", Some(&anchor)).await;
    let mut got = Vec::new();
    loop {
        let message = stream.message().await;
        let last = message.id.is_some();
        got.push(message);
        if last {
            break;
        }
    }
    let items: Vec<(String, i64)> = got.iter().map(|m| (m.item().id, m.item().version)).collect();
    assert_eq!(
        items,
        [
            ("t1:tool:x".to_string(), tool_last.event_id),
            ("t1:1".to_string(), tool_first.event_id + 2),
            ("t1:plan".to_string(), plan.event_id),
        ]
    );
    let Body::Message(text) = &got[1].item().body else {
        panic!("a message");
    };
    assert_eq!(text.text, "y1y2");
    assert_eq!(
        got.last().unwrap().id,
        Some(format!("{}:{}", epoch_of(&anchor), plan.event_id))
    );
}

const ITEMS: &str = "/api/stream/view/sessions/s";

/// A-1: an item stream with no anchor cannot resume: it resyncs.
#[tokio::test]
async fn an_item_stream_with_no_anchor_resyncs() {
    let collector = Collector::start().await;
    opened(&collector, "s").await;
    collector.stream(ITEMS, None).await.resyncs().await;
}

/// A-1: an anchor that is not `<epoch>:<revision>` resyncs.
#[tokio::test]
async fn an_item_stream_with_an_unreadable_anchor_resyncs() {
    let collector = Collector::start().await;
    opened(&collector, "s").await;
    collector
        .stream(&format!("{ITEMS}?after=nonsense"), None)
        .await
        .resyncs()
        .await;
}

/// A-1: an anchor of another collector process, or fold, resyncs.
#[tokio::test]
async fn an_item_stream_anchored_in_another_epoch_resyncs() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let revision = anchor.rsplit_once(':').unwrap().1;
    collector
        .stream(ITEMS, Some(&format!("1.0000000000000000:{revision}")))
        .await
        .resyncs()
        .await;
}

/// A5: a revision the session never reached resyncs.
#[tokio::test]
async fn an_item_stream_anchored_past_the_revision_resyncs() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let revision: i64 = anchor.rsplit_once(':').unwrap().1.parse().unwrap();
    collector
        .stream(ITEMS, Some(&format!("{}:{}", epoch_of(&anchor), revision + 1)))
        .await
        .resyncs()
        .await;
}

/// The review's A-1: `Last-Event-ID` decides, whatever `?after=` says.
#[tokio::test]
async fn the_last_event_id_header_wins_over_after() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    collector
        .stream(&format!("{ITEMS}?after={anchor}"), Some("nonsense"))
        .await
        .resyncs()
        .await;
}

/// A-10: past `RESUME_MAX_GROUPS` turns since the anchor, a resume
/// resyncs; at it, it is sent.
#[tokio::test]
async fn a_resume_past_its_turn_cap_resyncs() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let epoch = epoch_of(&anchor).to_string();
    for n in 0..=RESUME_MAX_GROUPS {
        collector.write(&collector.owner(), "s", "user_turn", &turn(&format!("g{n}")));
    }
    collector.stream(ITEMS, Some(&anchor)).await.resyncs().await;
    let groups = collector.page("s", "").await;
    let anchor = format!("{epoch}:{}", groups.revision);
    for n in 0..RESUME_MAX_GROUPS {
        collector.write(&collector.owner(), "s", "user_turn", &turn(&format!("h{n}")));
    }
    let mut stream = collector.stream(ITEMS, Some(&anchor)).await;
    assert_eq!(stream.message().await.item().id, "h0:0");
}

/// A-10: past `RESUME_MAX_EVENTS` events since the anchor, a resume
/// resyncs.
#[tokio::test]
async fn a_resume_past_its_event_cap_resyncs() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    collector
        .conn()
        .execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i <= ?1)
             INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
             SELECT 's', NULL, 'git_state', '{}', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?2 FROM n",
            params![RESUME_MAX_EVENTS, collector.owner()],
        )
        .unwrap();
    collector.stream(ITEMS, Some(&anchor)).await.resyncs().await;
}

/// A5: chunks read within `COALESCE` of the first are merged into one
/// upsert, sent no sooner than `COALESCE` after it.
#[tokio::test]
async fn chunks_are_held_and_sent_merged() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let mut stream = collector
        .stream(&format!("/api/stream/view/sessions/s?after={anchor}"), None)
        .await;
    live(&collector, &mut stream).await;
    // Stored together and rung once (the plan review's 6): the stream
    // reads all three at once, whatever the machine's load.
    let start = Instant::now();
    let last = collector.events_at_once("s", &[chunk("a"), chunk("b"), chunk("c")]);
    let message = stream.message().await;
    assert!(start.elapsed() >= COALESCE, "sent after {:?}", start.elapsed());
    let item = message.item();
    let Body::Message(text) = &item.body else {
        panic!("a message: {item:?}");
    };
    assert_eq!((text.text.as_str(), item.version), ("abc", last.event_id));
    assert_eq!(message.id, Some(format!("{}:{}", epoch_of(&anchor), last.event_id)));
}

/// A5: anything else flushes the held chunks first, in one burst.
#[tokio::test]
async fn a_tool_call_after_held_chunks_comes_after_their_message() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let mut stream = collector
        .stream(&format!("/api/stream/view/sessions/s?after={anchor}"), None)
        .await;
    live(&collector, &mut stream).await;
    collector.event("s", "acp_update", &chunk("a"));
    let call = collector.event("s", "acp_update", &tool("c1", "pending"));
    assert_eq!(stream.message().await.item().id, "t1:1");
    let message = stream.message().await;
    assert_eq!(message.item().id, "t1:tool:c1");
    assert_eq!(message.id, Some(format!("{}:{}", epoch_of(&anchor), call.event_id)));
}

/// Plan 4a-i decision 12: events read together across a new turn send
/// the ended turn's changes first, their id the event before the turn's:
/// the fold never holds two turns' changes.
#[tokio::test]
async fn a_new_turn_sends_what_the_last_turn_changed_first() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let mut stream = collector
        .stream(&format!("/api/stream/view/sessions/s?after={anchor}"), None)
        .await;
    live(&collector, &mut stream).await;
    let owner = collector.owner();
    let said = collector.write(&owner, "s", "acp_update", &chunk("a"));
    collector.write(&owner, "s", "user_turn", &turn("t2"));
    // One ring for the three: they are read in one batch.
    let call = collector.event("s", "acp_update", &tool("c1", "pending"));
    let message = stream.message().await;
    assert_eq!(message.item().id, "t1:1");
    assert_eq!(message.id, Some(format!("{}:{}", epoch_of(&anchor), said.event_id)));
    let message = stream.message().await;
    assert_eq!((message.item().id.as_str(), message.id.as_deref()), ("t2:0", None));
    let message = stream.message().await;
    assert_eq!(message.item().id, "t2:tool:c1");
    assert_eq!(message.id, Some(format!("{}:{}", epoch_of(&anchor), call.event_id)));
}

/// A4, the review's A-8: a question asked in a turn before the current
/// one, answered now, is sent as the store holds it: answered, no longer
/// answerable, in its own turn. The page shows it so too.
#[tokio::test]
async fn a_question_answered_after_the_next_prompt_is_sent_from_the_store() {
    let collector = Collector::start().await;
    collector.session(&collector.owner(), "s");
    collector.write(&collector.owner(), "s", "user_turn", &turn("t1"));
    question(&collector, "s", "p1", "t1");
    // The record's turn id is not where it is shown: the fold places a
    // question in the group its `pending_opened` is in, and so does the
    // store's (4a-i's whole-branch review).
    collector
        .conn()
        .execute("UPDATE pending SET turn_id = 'elsewhere' WHERE pending_id = 'p1'", [])
        .unwrap();
    collector.write(&collector.owner(), "s", "user_turn", &turn("t2"));
    let page = collector.page("s", "").await;
    let anchor = format!("{}:{}", page.epoch, page.revision);
    let asked = page.items.iter().find(|i| i.id == "question:p1").unwrap();
    let Body::Question(q) = &asked.body else { panic!() };
    assert!(q.answerable && !q.answered);

    let mut stream = collector
        .stream(&format!("/api/stream/view/sessions/s?after={anchor}"), None)
        .await;
    live(&collector, &mut stream).await;
    let resp = collector
        .client()
        .post(collector.url("/api/sessions/s/pending/p1/answer"))
        .json(&json!({"option_id": "allow"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    let submitted = collector.state.store.revision("s").unwrap();
    let item = stream.message().await.item();
    assert_eq!((item.id.as_str(), item.turn_id.as_deref()), ("question:p1", Some("t1")));
    assert_eq!(item.version, submitted);
    let Body::Question(q) = &item.body else { panic!() };
    assert!(q.answered && !q.answerable);

    let page = collector.page("s", "").await;
    let shown = page.items.iter().find(|i| i.id == "question:p1").unwrap();
    assert_eq!(shown, &item);
    // A page of its own turn alone, which holds no event of the answer:
    // the store's record says it is answered (A4).
    let earlier = collector.page("s", "?before_turn=t2").await;
    let shown = earlier.items.iter().find(|i| i.id == "question:p1").unwrap();
    assert_eq!(shown, &item);
    // A resume from before the answer: the question, from the store.
    let mut resumed = collector.stream("/api/stream/view/sessions/s", Some(&anchor)).await;
    let mut sent = Vec::new();
    loop {
        let message = resumed.message().await;
        let last = message.id.is_some();
        sent.push(message.item());
        if last {
            break;
        }
    }
    assert!(sent.contains(&item), "{sent:?}");
}

/// A8, ACP core §9: a resume sends one upsert per session changed since
/// its id, its last carrying the owner's revision; a summary equal to the
/// one this stream last sent is not sent again.
#[tokio::test]
async fn the_list_stream_resumes_once_per_session_and_never_repeats_a_summary() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    for session in ["a", "b", "c"] {
        collector.session(&owner, session);
        collector.write(&owner, session, "user_turn", &turn(session));
    }
    let page = collector.summaries("").await;
    let anchor = format!("{}:{}", page.epoch, page.revision);
    collector.write(&owner, "a", "acp_update", &chunk("1"));
    collector.write(&owner, "b", "acp_update", &chunk("1"));
    let last = collector.write(&owner, "a", "acp_update", &chunk("2"));
    let mut stream = collector.stream("/api/stream/sessions", Some(&anchor)).await;
    let first = stream.message().await;
    let second = stream.message().await;
    let sent: Vec<String> = [&first, &second]
        .iter()
        .map(|m| m.summary().session.session_id)
        .collect();
    assert_eq!(sent, ["b", "a"]);
    assert_eq!((first.id, second.id.as_deref()), (None, None));
    // The burst ends with the count, which carries the id.
    assert_eq!(
        stream.counted().await,
        (0, Some(format!("{}:{}", page.epoch, last.event_id)))
    );
    // The latest summary (the plan review's 3c): as of `a`'s last event.
    assert_eq!(second.summary().session.last_event_at, last.ts);

    // An event that leaves `a`'s summary as it was sends nothing for it.
    let same = collector.write_as(&owner, "a", "git_state", &json!({}), false);
    collector.state.hub.publish(same);
    let changed = collector.event("c", "acp_update", &chunk("1"));
    let message = stream.message().await;
    assert_eq!(message.summary().session.session_id, "c");
    assert_eq!(message.id, Some(format!("{}:{}", page.epoch, changed.event_id)));
}

const LIST: &str = "/api/stream/sessions";

/// A session `a` of the owner's with one event, stamped long ago, and
/// the list's epoch.
async fn old_session(collector: &Collector) -> (EventDto, String) {
    let owner = collector.owner();
    collector.session(&owner, "a");
    let old = collector.write(&owner, "a", "user_turn", &turn("t1"));
    collector
        .conn()
        .execute(
            "UPDATE events SET ts = '2026-01-01T00:00:00.000Z' WHERE event_id = ?1",
            [old.event_id],
        )
        .unwrap();
    (old, collector.summaries("").await.epoch)
}

/// ACP core §9: an anchor with nothing since is followed, however old.
#[tokio::test]
async fn the_list_stream_follows_an_old_anchor_with_nothing_since() {
    let collector = Collector::start().await;
    let (old, epoch) = old_session(&collector).await;
    let mut idle = collector.stream(LIST, Some(&format!("{epoch}:{}", old.event_id))).await;
    // Nothing to replay: the count alone, at the anchor.
    assert_eq!(idle.counted().await, (0, Some(format!("{epoch}:{}", old.event_id))));
    collector.event("a", "acp_update", &chunk("1"));
    assert_eq!(idle.message().await.summary().session.session_id, "a");
}

/// ACP core §9: an anchor older than the 24 h window, with events since,
/// resyncs.
#[tokio::test]
async fn the_list_stream_resyncs_past_its_window() {
    let collector = Collector::start().await;
    let (old, epoch) = old_session(&collector).await;
    collector.write(&collector.owner(), "a", "acp_update", &chunk("1"));
    collector
        .stream(LIST, Some(&format!("{epoch}:{}", old.event_id)))
        .await
        .resyncs()
        .await;
}

/// The review's A-4: an anchor of another epoch resyncs, even with
/// nothing since.
#[tokio::test]
async fn the_list_stream_resyncs_for_another_epoch() {
    let collector = Collector::start().await;
    old_session(&collector).await;
    let revision = collector.state.store.max_event_id().unwrap();
    collector
        .stream(LIST, Some(&format!("1.0000000000000000:{revision}")))
        .await
        .resyncs()
        .await;
}

/// A-1: a list stream with no anchor resyncs.
#[tokio::test]
async fn the_list_stream_resyncs_with_no_anchor() {
    let collector = Collector::start().await;
    old_session(&collector).await;
    collector.stream(LIST, None).await.resyncs().await;
}

/// A-1: an anchor that is not `<epoch>:<revision>` resyncs.
#[tokio::test]
async fn the_list_stream_resyncs_with_an_unreadable_anchor() {
    let collector = Collector::start().await;
    old_session(&collector).await;
    collector.stream(LIST, Some("nonsense")).await.resyncs().await;
}

/// A8: an anchor past the owner's greatest event id resyncs.
#[tokio::test]
async fn the_list_stream_resyncs_past_the_owners_revision() {
    let collector = Collector::start().await;
    let (_, epoch) = old_session(&collector).await;
    let revision = collector.state.store.max_event_id().unwrap();
    collector
        .stream(LIST, Some(&format!("{epoch}:{}", revision + 1)))
        .await
        .resyncs()
        .await;
}

/// A8: an anchor that is no event of the owner's (another owner's, or
/// one gone with its session) resyncs: its age cannot be told.
#[tokio::test]
async fn the_list_stream_resyncs_when_its_anchor_is_no_event_of_the_owners() {
    let collector = Collector::start().await;
    collector.other_owner("theirs");
    let theirs: i64 = collector
        .conn()
        .query_row("SELECT MAX(event_id) FROM events", [], |r| r.get(0))
        .unwrap();
    collector.session(&collector.owner(), "a");
    collector.write(&collector.owner(), "a", "user_turn", &turn("t1"));
    let epoch = collector.summaries("").await.epoch;
    collector
        .stream(LIST, Some(&format!("{epoch}:{theirs}")))
        .await
        .resyncs()
        .await;
}

/// A5, A8 (3b decision 7): both streams end with the operator's session
/// that opened them, and when the collector shuts down.
#[tokio::test]
async fn the_streams_end_with_their_operator_session_and_on_shutdown() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let list = collector.summaries("").await;
    let list_anchor = format!("{}:{}", list.epoch, list.revision);
    let open = |client: reqwest::Client| {
        let collector = &collector;
        let (anchor, list_anchor) = (anchor.clone(), list_anchor.clone());
        async move {
            [
                collector
                    .stream_as(&client, "/api/stream/view/sessions/s", Some(&anchor))
                    .await,
                collector
                    .stream_as(&client, "/api/stream/sessions", Some(&list_anchor))
                    .await,
            ]
        }
    };
    let kept = collector.client();
    let mut streams = open(kept.clone()).await;
    streams[1].counted().await;
    let signed_out = collector.client();
    let mut ended = open(signed_out.clone()).await;
    ended[1].counted().await;
    let resp = signed_out.post(collector.url("/api/auth/logout")).send().await.unwrap();
    assert_eq!(resp.status(), 204);
    for stream in &mut ended {
        assert!(
            stream.next().await.is_none(),
            "a signed-out session's stream stayed open"
        );
    }
    // The control: another session's streams still follow.
    collector.event("s", "acp_update", &tool("c1", "pending"));
    assert_eq!(streams[0].message().await.item().id, "t1:tool:c1");
    assert_eq!(streams[1].message().await.summary().session.session_id, "s");

    collector.state.shutdown.cancel();
    for stream in &mut streams {
        assert!(stream.next().await.is_none(), "a stream outlived the collector");
    }
}

/// Plan 9a: a deleted session is 404 on every view route and gone from
/// the summaries; an item stream open on it ends; the list stream says
/// `session_removed` with the tombstone's event id.
#[tokio::test]
async fn a_deleted_session_leaves_the_view() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    let anchor = opened(&collector, "s").await;
    collector.session(&owner, "kept");
    collector.write(&owner, "kept", "user_turn", &turn("k"));
    let mut items = collector
        .stream(&format!("/api/stream/view/sessions/s?after={anchor}"), None)
        .await;
    live(&collector, &mut items).await;
    let list = collector.summaries("").await;
    let mut sessions = collector
        .stream(
            &format!("/api/stream/sessions?after={}:{}", list.epoch, list.revision),
            None,
        )
        .await;
    sessions.counted().await;

    collector
        .conn()
        .execute("UPDATE sessions SET lifecycle = 'closed' WHERE id = 's'", [])
        .unwrap();
    let Deletion::Done { event: deleted, .. } = collector.state.store.delete_session("s", None).unwrap() else {
        panic!("not deleted");
    };
    let rung = Instant::now();
    collector.state.hub.publish(deleted.clone());
    // Its last message says so, with no id (the security review's B-3).
    let gone = items.message().await;
    assert_eq!(
        (gone.event.as_str(), gone.data.as_str(), gone.id.as_deref()),
        ("session_removed", r#"{"session_id":"s"}"#, None)
    );
    assert!(items.next().await.is_none(), "the item stream outlived its session");
    let removed = sessions.message().await;
    // Read at once, not after the list's hold (the plan review's 3b).
    assert!(rung.elapsed() < LIST_COALESCE, "sent after {:?}", rung.elapsed());
    assert_eq!(
        (removed.event.as_str(), removed.data.as_str()),
        ("session_removed", r#"{"session_id":"s"}"#)
    );
    assert_eq!(removed.id, Some(format!("{}:{}", list.epoch, deleted.event_id)));
    // The list stream goes on.
    collector.event("kept", "acp_update", &chunk("1"));
    assert_eq!(sessions.message().await.summary().session.session_id, "kept");

    // A tombstone is not found, and nothing of it is read to say so (the
    // security review's B-6).
    let before = collector.state.store.view_reads();
    for path in [
        "/api/view/sessions/s",
        "/api/stream/view/sessions/s",
        "/api/view/sessions/s/turns/t1",
    ] {
        let resp = collector.client().get(collector.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), 404, "{path}");
        assert_eq!(resp.json::<Value>().await.unwrap()["code"], "not_found", "{path}");
    }
    assert_eq!(collector.state.store.view_reads(), before);
    let listed: Vec<String> = collector
        .summaries("")
        .await
        .sessions
        .into_iter()
        .map(|s| s.session.session_id)
        .collect();
    assert_eq!(listed, ["kept"]);
    // A search matching its id too (a search lists every lifecycle).
    assert!(collector.summaries("?q=s").await.sessions.is_empty());
}

/// 4a-i's whole-branch review: a question whose id the fold refuses (past
/// `ID_MAX_BYTES`) is never built from the store's record either.
#[tokio::test]
async fn a_question_the_fold_refuses_is_never_sent_from_the_store() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "s");
    collector.write(&owner, "s", "user_turn", &turn("t1"));
    let long = "p".repeat(hennery_view::cap::ID_MAX_BYTES + 1);
    question(&collector, "s", &long, "t1");
    collector.write(&owner, "s", "user_turn", &turn("t2"));
    let page = collector.page("s", "?limit=100").await;
    assert!(
        page.items.iter().all(|i| !matches!(i.body, Body::Question(_))),
        "{:?}",
        page.items
    );
    let anchor = format!("{}:{}", page.epoch, page.revision);
    // Its verdict comes after the anchor, then something else.
    collector.write(
        &owner,
        "s",
        "pending_cancelled",
        &json!({"pending_id": long, "reason": "agent_withdrew"}),
    );
    let call = collector.write(&owner, "s", "acp_update", &tool("c1", "pending"));
    let mut stream = collector.stream("/api/stream/view/sessions/s", Some(&anchor)).await;
    let message = stream.message().await;
    assert_eq!(message.item().id, "t2:tool:c1");
    assert_eq!(message.id, Some(format!("{}:{}", page.epoch, call.event_id)));
}

/// Plan 4a-i's hand-off (its review's 13): a question asked live is sent
/// answerable: the pending set is read after the event that opened it.
#[tokio::test]
async fn a_question_asked_live_is_answerable() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let mut stream = collector
        .stream(&format!("/api/stream/view/sessions/s?after={anchor}"), None)
        .await;
    live(&collector, &mut stream).await;
    let asked = question(&collector, "s", "p1", "t1");
    collector.state.hub.publish(asked.clone());
    let item = stream.message().await.item();
    assert_eq!((item.id.as_str(), item.version), ("question:p1", asked.event_id));
    let Body::Question(q) = &item.body else {
        panic!("{item:?}")
    };
    assert!(q.answerable && !q.answered);
}

/// The security review's B-2: past `GROUP_MAX_QUESTIONS` questions of
/// earlier turns changed at once, a stream resyncs rather than leave one
/// out, on a resume and live.
#[tokio::test]
async fn too_many_questions_of_earlier_turns_resync() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "s");
    collector.write(&owner, "s", "user_turn", &turn("t1"));
    let asked: Vec<String> = (0..=hennery_view::cap::GROUP_MAX_QUESTIONS)
        .map(|n| format!("p{n}"))
        .collect();
    for pending_id in &asked {
        question(&collector, "s", pending_id, "t1");
    }
    collector.write(&owner, "s", "user_turn", &turn("t2"));
    let page = collector.page("s", "").await;
    let anchor = format!("{}:{}", page.epoch, page.revision);
    let mut live_stream = collector
        .stream(&format!("/api/stream/view/sessions/s?after={anchor}"), None)
        .await;
    live(&collector, &mut live_stream).await;
    let mut last = None;
    for pending_id in &asked {
        last = Some(collector.write(
            &owner,
            "s",
            "pending_cancelled",
            &json!({"pending_id": pending_id, "reason": "agent_withdrew"}),
        ));
        collector
            .conn()
            .execute(
                "UPDATE pending SET state = 'cancelled' WHERE pending_id = ?1",
                [pending_id],
            )
            .unwrap();
    }
    collector.state.hub.publish(last.unwrap());
    live_stream.resyncs().await;
    collector
        .stream("/api/stream/view/sessions/s", Some(&anchor))
        .await
        .resyncs()
        .await;
}

/// A7: the summaries refuse what `GET /api/sessions` refuses, with its
/// codes (the shared `ListFilter::parse`).
#[tokio::test]
async fn the_summaries_refuse_a_filter_they_cannot_honour() {
    let collector = Collector::start().await;
    let long = format!("?q={}", "q".repeat(201));
    for (query, code) in [
        ("?cursor=zz", "invalid_cursor"),
        ("?limit=x", "invalid"),
        ("?limit=-1", "invalid"),
        ("?lifecycle=active,bogus", "invalid"),
        ("?hat=", "invalid"),
        (long.as_str(), "invalid"),
        ("?q=a%00b", "invalid"),
    ] {
        let resp = collector
            .client()
            .get(collector.url(&format!("/api/view/sessions{query}")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "{query}");
        assert_eq!(resp.json::<Value>().await.unwrap()["code"], code, "{query}");
    }
    // The control: one it can honour.
    collector.summaries("?limit=1&lifecycle=active&hat=h&q=x").await;
}

/// The kind of an item as served, and of its marker if it is one.
fn kind_of(item: &Item) -> String {
    let value = serde_json::to_value(item).unwrap();
    match value["marker"].as_str() {
        Some(marker) => format!("marker/{marker}"),
        None => value["kind"].as_str().unwrap().to_string(),
    }
}

/// Client view §3 (the fleet's outcome rule): every item kind and every
/// marker kind the fold makes is served, on a page and on a resume, each
/// from the event that makes it.
#[tokio::test]
async fn every_item_and_marker_kind_is_served() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "s");
    let w = |kind: &str, body: Value| {
        collector.write(&owner, "s", kind, &body);
    };
    w("user_turn", turn("t1"));
    let mut expected = vec!["user_turn"];
    let mut each = |kind: &str, body: Value, served: &'static str| {
        w(kind, body);
        expected.push(served);
    };
    each("acp_update", chunk("said"), "message");
    each(
        "acp_update",
        acp(json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hm"}})),
        "thinking",
    );
    each("acp_update", tool("c1", "pending"), "tool_call");
    each(
        "acp_update",
        acp(json!({"sessionUpdate": "plan", "entries": [{"content": "z"}]})),
        "plan",
    );
    each("acp_update", acp(json!({"sessionUpdate": "mystery"})), "unrecognised");
    each("session_parked", json!({"reason": "idle"}), "marker/parked");
    each("operator_resumed", json!({}), "marker/resumed");
    each("operator_closed", json!({}), "marker/closed");
    each("host_restarted", json!({}), "marker/host_restarted");
    each(
        "presumed_parked",
        json!({"reason": "host_offline"}),
        "marker/host_offline",
    );
    each("reattached", json!({}), "marker/host_back");
    each(
        "turn_ended",
        json!({"turn_id": "t1", "outcome": "interrupted"}),
        "marker/turn_interrupted",
    );
    each(
        "turn_ended_synthesized",
        json!({"turn_id": "t1"}),
        "marker/turn_interrupted",
    );
    each(
        "turn_ended",
        json!({"turn_id": "t1", "outcome": "failed", "error": "boom"}),
        "marker/turn_failed",
    );
    each(
        "turn_ended",
        json!({"turn_id": "t1", "outcome": "cancelled"}),
        "marker/turn_cancelled",
    );
    each(
        "turn_not_delivered",
        json!({"turn_id": "t0"}),
        "marker/turn_not_delivered",
    );
    each("start_not_delivered", json!({}), "marker/start_not_delivered");
    each(
        "start_failed",
        json!({"code": "spawn_failed", "message": "no"}),
        "marker/start_failed",
    );
    each(
        "adapter_exited",
        json!({"code": 1, "stderr_tail": "x"}),
        "marker/adapter_exited",
    );
    each(
        "transcript_gap",
        json!({"from_seq": 1, "to_seq": 2}),
        "marker/transcript_gap",
    );
    each(
        "host_note",
        json!({"note": "config_failed", "text": "x"}),
        "marker/host_note",
    );
    each("conflict", json!({"seq": 3, "received": {}}), "marker/conflict");
    each(
        "hat_reassigned",
        json!({"from": "a", "to": "b"}),
        "marker/hat_reassigned",
    );
    question(&collector, "s", "p1", "t1");
    expected.push("question");
    // A turn past its item budget: one `elided` marker, then nothing.
    w("user_turn", turn("t2"));
    collector
        .conn()
        .execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i <= ?1)
             INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
             SELECT 's', NULL, 'operator_resumed', '{}', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?2 FROM n",
            params![hennery_view::cap::GROUP_MAX_ITEMS as i64, owner],
        )
        .unwrap();

    let page = collector.page("s", "").await;
    let first: Vec<String> = page
        .items
        .iter()
        .filter(|i| i.turn_id.as_deref() == Some("t1"))
        .map(kind_of)
        .collect();
    assert_eq!(first, expected);
    let second: Vec<String> = page
        .items
        .iter()
        .filter(|i| i.turn_id.as_deref() == Some("t2"))
        .map(kind_of)
        .collect();
    assert_eq!(second.last().map(String::as_str), Some("marker/elided"));
    let mut served: Vec<String> = page.items.iter().map(kind_of).collect();
    served.sort();
    served.dedup();
    // Seven item kinds besides `marker`, and its eighteen kinds.
    assert_eq!(served.len(), 7 + 18, "{served:?}");

    // A resume from the session's start serves the same.
    let mut stream = collector.stream(ITEMS, Some(&format!("{}:0", page.epoch))).await;
    let mut resumed = Vec::new();
    loop {
        let message = stream.message().await;
        let last = message.id.is_some();
        resumed.push(message.item());
        if last {
            break;
        }
    }
    let ids = |items: &[Item]| -> Vec<(String, String)> {
        let mut ids: Vec<(String, String)> = items.iter().map(|i| (i.id.clone(), kind_of(i))).collect();
        ids.sort();
        ids
    };
    assert_eq!(ids(&resumed), ids(&page.items));
}

/// The plan review's 1a: a verdict on a question of the turn that just
/// ended, live, is sent from the store, in its own turn.
#[tokio::test]
async fn a_verdict_on_the_last_turns_question_is_sent_live() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let mut stream = collector.stream(&format!("{ITEMS}?after={anchor}"), None).await;
    live(&collector, &mut stream).await;
    let asked = question(&collector, "s", "p1", "t1");
    collector.state.hub.publish(asked);
    assert_eq!(stream.message().await.item().id, "question:p1");
    collector.event("s", "user_turn", &turn("t2"));
    assert_eq!(stream.message().await.item().id, "t2:0");
    let cancelled = cancel(&collector, "p1");
    let item = stream.message().await.item();
    assert_eq!(
        (item.id.as_str(), item.turn_id.as_deref(), item.version),
        ("question:p1", Some("t1"), cancelled.event_id)
    );
}

/// The plan review's 1b: after a resume that read a new turn, a verdict
/// on a question of the turn before is sent from the store.
#[tokio::test]
async fn a_verdict_after_a_resume_into_a_new_turn_is_sent() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    question(&collector, "s", "p1", "t1");
    collector.write(&collector.owner(), "s", "user_turn", &turn("t2"));
    let mut stream = collector.stream(ITEMS, Some(&anchor)).await;
    loop {
        if stream.message().await.id.is_some() {
            break;
        }
    }
    let cancelled = cancel(&collector, "p1");
    let item = stream.message().await.item();
    assert_eq!(
        (item.id.as_str(), item.turn_id.as_deref(), item.version),
        ("question:p1", Some("t1"), cancelled.event_id)
    );
}

/// Cancel the owner's question `pending_id` of session `s`, as the
/// collector does: its record and its event, rung.
fn cancel(collector: &Collector, pending_id: &str) -> EventDto {
    collector
        .conn()
        .execute(
            "UPDATE pending SET state = 'cancelled' WHERE pending_id = ?1",
            [pending_id],
        )
        .unwrap();
    collector.event(
        "s",
        "pending_cancelled",
        &json!({"pending_id": pending_id, "reason": "agent_withdrew"}),
    )
}

/// The plan review's 2: a page of exactly `limit` turns says `older` when
/// the session has items before its first turn, and the page before holds
/// them.
#[tokio::test]
async fn a_page_of_every_turn_says_older_when_the_preamble_has_items() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "s");
    collector.write(&owner, "s", "operator_resumed", &json!({}));
    collector.write(&owner, "s", "user_turn", &turn("t1"));
    collector.write(&owner, "s", "user_turn", &turn("t2"));
    let page = collector.page("s", "?limit=2").await;
    assert!(page.older);
    let before = collector.page("s", "?before_turn=t1").await;
    let ids: Vec<(&str, Option<&str>)> = before
        .items
        .iter()
        .map(|i| (i.id.as_str(), i.turn_id.as_deref()))
        .collect();
    assert_eq!(ids, [("start:0", None)]);
    assert!(!before.older);
}

/// The plan review's 5: a question asked before the first turn is shown,
/// from the store, with no turn.
#[tokio::test]
async fn a_verdict_on_a_question_before_the_first_turn_has_no_turn() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "s");
    question(&collector, "s", "p1", "");
    collector.write(&owner, "s", "user_turn", &turn("t1"));
    let page = collector.page("s", "").await;
    let mut stream = collector
        .stream(&format!("{ITEMS}?after={}:{}", page.epoch, page.revision), None)
        .await;
    live(&collector, &mut stream).await;
    cancel(&collector, "p1");
    let item = stream.message().await.item();
    assert_eq!((item.id.as_str(), item.turn_id), ("question:p1", None));
}

/// The plan review's 5: a question id the fold refuses never counts
/// toward `GROUP_MAX_QUESTIONS`: exactly that many shown ones, and one
/// refused, are sent with no resync.
#[tokio::test]
async fn a_refused_question_id_counts_toward_no_cap() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "s");
    collector.write(&owner, "s", "user_turn", &turn("t1"));
    let mut asked: Vec<String> = (0..hennery_view::cap::GROUP_MAX_QUESTIONS)
        .map(|n| format!("p{n}"))
        .collect();
    asked.push("p".repeat(hennery_view::cap::ID_MAX_BYTES + 1));
    for pending_id in &asked {
        question(&collector, "s", pending_id, "t1");
    }
    collector.write(&owner, "s", "user_turn", &turn("t2"));
    let page = collector.page("s", "").await;
    let mut stream = collector
        .stream(&format!("{ITEMS}?after={}:{}", page.epoch, page.revision), None)
        .await;
    live(&collector, &mut stream).await;
    let mut last = None;
    for pending_id in &asked {
        collector
            .conn()
            .execute(
                "UPDATE pending SET state = 'cancelled' WHERE pending_id = ?1",
                [pending_id],
            )
            .unwrap();
        last = Some(collector.write(
            &owner,
            "s",
            "pending_cancelled",
            &json!({"pending_id": pending_id, "reason": "agent_withdrew"}),
        ));
    }
    collector.state.hub.publish(last.unwrap());
    let mut sent = 0;
    loop {
        let message = stream.message().await;
        assert_eq!(message.event, "item", "{message:?}");
        sent += 1;
        if message.id.is_some() {
            break;
        }
    }
    assert_eq!(sent, hennery_view::cap::GROUP_MAX_QUESTIONS);
}

/// The plan review's 5: a question of an earlier turn touched by many
/// events before a send counts once toward the cap, and is sent once.
#[tokio::test]
async fn a_question_touched_many_times_counts_once() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "s");
    collector.write(&owner, "s", "user_turn", &turn("t1"));
    question(&collector, "s", "p1", "t1");
    collector.write(&owner, "s", "user_turn", &turn("t2"));
    let page = collector.page("s", "").await;
    let mut stream = collector
        .stream(&format!("{ITEMS}?after={}:{}", page.epoch, page.revision), None)
        .await;
    live(&collector, &mut stream).await;
    let mut last = None;
    for _ in 0..=hennery_view::cap::GROUP_MAX_QUESTIONS {
        last = Some(collector.write(
            &owner,
            "s",
            "answer_result",
            &json!({"pending_id": "p1", "delivered": false}),
        ));
    }
    collector.state.hub.publish(last.unwrap());
    let message = stream.message().await;
    assert_eq!(message.item().id, "question:p1");
    assert!(message.id.is_some(), "one message, the burst's last");
}

/// The plan review's 3: a burst of events is held `LIST_COALESCE` and
/// sent as one upsert per session, with the latest summary, and no event
/// after it is needed for it to go (3a).
#[tokio::test]
async fn the_list_stream_holds_a_burst_into_one_upsert() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "a");
    collector.write(&owner, "a", "user_turn", &turn("t1"));
    let page = collector.summaries("").await;
    let mut stream = collector
        .stream(&format!("{LIST}?after={}:{}", page.epoch, page.revision), None)
        .await;
    stream.counted().await;
    let start = Instant::now();
    let mut last = None;
    for n in 0..5 {
        last = Some(collector.event("a", "acp_update", &chunk(&n.to_string())));
    }
    let last = last.unwrap();
    let message = stream.message().await;
    assert!(start.elapsed() >= LIST_COALESCE, "sent after {:?}", start.elapsed());
    assert_eq!(message.summary().session.last_event_at, last.ts);
    assert_eq!(message.id, Some(format!("{}:{}", page.epoch, last.event_id)));
    // Nothing more for `a`: the next message is another session's.
    collector.session(&owner, "b");
    collector.event("b", "user_turn", &turn("t1"));
    assert_eq!(stream.message().await.summary().session.session_id, "b");
}

/// The plan review's 3b: a delete is read at once, never behind a held
/// upsert, and no upsert for the session follows it.
#[tokio::test]
async fn the_list_stream_sends_a_removal_at_once_and_nothing_after_it() {
    let collector = Collector::start().await;
    let owner = collector.owner();
    collector.session(&owner, "a");
    collector.write(&owner, "a", "user_turn", &turn("t1"));
    let page = collector.summaries("").await;
    let mut stream = collector
        .stream(&format!("{LIST}?after={}:{}", page.epoch, page.revision), None)
        .await;
    stream.counted().await;
    // An upsert sent, then one held, then the delete.
    collector.event("a", "acp_update", &chunk("0"));
    assert_eq!(stream.message().await.summary().session.session_id, "a");
    let start = Instant::now();
    collector.event("a", "acp_update", &chunk("1"));
    collector
        .conn()
        .execute("UPDATE sessions SET lifecycle = 'closed' WHERE id = 'a'", [])
        .unwrap();
    let Deletion::Done { event: deleted, .. } = collector.state.store.delete_session("a", None).unwrap() else {
        panic!("not deleted");
    };
    collector.state.hub.publish(deleted);
    let removed = stream.message().await;
    assert!(start.elapsed() < LIST_COALESCE, "sent after {:?}", start.elapsed());
    assert_eq!(
        (removed.event.as_str(), removed.data.as_str()),
        ("session_removed", r#"{"session_id":"a"}"#)
    );
    // The next message is another session's: no stale upsert for `a`.
    collector.session(&owner, "b");
    collector.event("b", "user_turn", &turn("t1"));
    let next = stream.message().await;
    assert_eq!(next.summary().session.session_id, "b");
}

/// An `available_commands_update` of session `s`'s naming `command`, with
/// the catalogue as ingest stores it; nothing is told.
fn commands(collector: &Collector, command: &str) -> EventDto {
    let list = json!([{"name": command, "description": "x"}]);
    let event = collector.write(
        &collector.owner(),
        "s",
        "acp_update",
        &json!({"kind": "acp_update", "indexed": {"commands": list},
                "payload": {"sessionId": "a", "update": {"sessionUpdate": "available_commands_update",
                                                       "availableCommands": list}}}),
    );
    collector
        .conn()
        .execute(
            "INSERT INTO session_catalog(session_id, config_options, updated_at, owner_id, commands)
             VALUES ('s', '[]', '2026-10-01T00:00:00.000Z', ?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET commands = excluded.commands",
            params![collector.owner(), list.to_string()],
        )
        .unwrap();
    event
}

/// Frontend §4.1 (4c's request): an event that changes the catalogue is
/// followed on the item stream by `catalog_changed`, the whole catalogue
/// as it stands, the burst's last message.
#[tokio::test]
async fn a_catalogue_change_is_sent_live_as_catalog_changed() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let mut stream = collector.stream(&format!("{ITEMS}?after={anchor}"), None).await;
    live(&collector, &mut stream).await;
    // An item and the change, rung once: one burst, the catalogue last.
    collector.write(&collector.owner(), "s", "acp_update", &tool("c1", "pending"));
    let changed = commands(&collector, "review");
    collector.state.hub.publish(changed.clone());
    let item = stream.message().await;
    assert_eq!((item.item().id.as_str(), item.id), ("t1:tool:c1", None));
    let message = stream.message().await;
    assert_eq!(message.event, "catalog_changed");
    let catalog: Value = serde_json::from_str(&message.data).unwrap();
    assert_eq!(catalog["session_id"], "s");
    assert_eq!(catalog["commands"][0]["name"], "review");
    assert_eq!(message.id, Some(format!("{}:{}", epoch_of(&anchor), changed.event_id)));
}

/// A resume whose events changed the catalogue sends one
/// `catalog_changed`, after the items, as it stands.
#[tokio::test]
async fn a_resume_over_catalogue_changes_sends_one_catalog_changed() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    commands(&collector, "first");
    collector.write(&collector.owner(), "s", "acp_update", &tool("c1", "pending"));
    let last = commands(&collector, "second");
    let mut stream = collector.stream(ITEMS, Some(&anchor)).await;
    let mut got = Vec::new();
    loop {
        let message = stream.message().await;
        let end = message.id.is_some();
        got.push(message);
        if end {
            break;
        }
    }
    let events: Vec<&str> = got.iter().map(|m| m.event.as_str()).collect();
    assert_eq!(events, ["item", "catalog_changed"]);
    let catalog: Value = serde_json::from_str(&got[1].data).unwrap();
    assert_eq!(catalog["commands"][0]["name"], "second");
    assert_eq!(got[1].id, Some(format!("{}:{}", epoch_of(&anchor), last.event_id)));
}

/// No event changed the catalogue: no `catalog_changed`, live or on a
/// resume.
#[tokio::test]
async fn no_catalogue_change_sends_no_catalog_changed() {
    let collector = Collector::start().await;
    opened(&collector, "s").await;
    commands(&collector, "before");
    let page = collector.page("s", "").await;
    let anchor = format!("{}:{}", page.epoch, page.revision);
    collector.write(&collector.owner(), "s", "acp_update", &tool("c1", "pending"));
    // A resume from after the catalogue's change sends only the item.
    let mut resumed = collector.stream(ITEMS, Some(&anchor)).await;
    let message = resumed.message().await;
    assert_eq!((message.event.as_str(), message.id.is_some()), ("item", true));
    // Live: each item alone, nothing after it.
    let page = collector.page("s", "").await;
    let mut stream = collector
        .stream(&format!("{ITEMS}?after={}:{}", page.epoch, page.revision), None)
        .await;
    for call in ["c2", "c3"] {
        collector.event("s", "acp_update", &tool(call, "pending"));
        let message = stream.message().await;
        assert_eq!((message.event.as_str(), message.id.is_some()), ("item", true));
    }
}

/// 4b's router fallback answers unknown `/api/*` paths: the view's routes
/// are routes, so an unknown session is the view's own 404, never the
/// fallback's.
#[tokio::test]
async fn the_view_routes_answer_their_own_404_not_the_fallbacks() {
    let collector = Collector::start().await;
    let c = collector.client();
    for path in [
        "/api/view/sessions/nope",
        "/api/stream/view/sessions/nope",
        "/api/view/sessions/nope/turns/t1",
    ] {
        let resp = c.get(collector.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), 404, "{path}");
        let body: Value = resp.json().await.unwrap();
        assert_eq!(
            (body["code"].as_str(), body["message"].as_str()),
            (Some("not_found"), Some("no such session")),
            "{path}"
        );
    }
    // The control: a path no route claims is the fallback's.
    let resp = c
        .get(collector.url("/api/view/nothing/here/at/all"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.json::<Value>().await.unwrap()["message"], "no such route");
}

/// 4c's request: every id the item stream sends resumes it, the id of a
/// burst that `catalog_changed` ends included (live, or a resume's): a
/// reconnect with it is not resynced, and follows.
#[tokio::test]
async fn a_catalog_changed_id_resumes_the_item_stream() {
    let collector = Collector::start().await;
    let anchor = opened(&collector, "s").await;
    let mut stream = collector.stream(&format!("{ITEMS}?after={anchor}"), None).await;
    live(&collector, &mut stream).await;
    // Live, the change rung alone: `catalog_changed` is the whole burst.
    let changed = commands(&collector, "review");
    collector.state.hub.publish(changed.clone());
    let message = stream.message().await;
    assert_eq!(message.event, "catalog_changed");
    let id = format!("{}:{}", epoch_of(&anchor), changed.event_id);
    assert_eq!(message.id.as_deref(), Some(id.as_str()));
    // A resume from just before it: the same burst, the same id.
    let before = format!("{}:{}", epoch_of(&anchor), changed.event_id - 1);
    let mut resumed = collector.stream(ITEMS, Some(&before)).await;
    let message = resumed.message().await;
    assert_eq!(
        (message.event.as_str(), message.id.as_deref()),
        ("catalog_changed", Some(id.as_str()))
    );
    // Reconnected with it: nothing to replay, then the next item.
    let mut again = collector.stream(ITEMS, Some(&id)).await;
    let event = collector.event("s", "acp_update", &tool("next", "pending"));
    let message = again.message().await;
    assert_eq!(message.item().version, event.event_id);
    assert_eq!(message.id, Some(format!("{}:{}", epoch_of(&anchor), event.event_id)));
}

/// A session `id` of the owner's, in `hat`, with a turn.
fn in_hat(collector: &Collector, id: &str, hat: &str) {
    collector.session(&collector.owner(), id);
    collector
        .conn()
        .execute("UPDATE sessions SET hat_id = ?1 WHERE id = ?2", params![hat, id])
        .unwrap();
    collector.write(&collector.owner(), id, "user_turn", &turn(id));
}

/// Session `id`'s agent waits on the operator in a turn; nothing is told.
fn block(collector: &Collector, id: &str) {
    collector
        .conn()
        .execute("UPDATE sessions SET activity = 'blocked' WHERE id = ?1", [id])
        .unwrap();
}

/// 4c's request: `waiting` counts the owner's sessions that wait for the
/// operator, blocked or with an open question, each once, in every
/// lifecycle but deleted, within the hat when one is named, whatever the
/// cursor, the search and the lifecycles the page lists.
#[tokio::test]
async fn the_summaries_count_the_sessions_waiting_whatever_the_page() {
    let collector = Collector::start().await;
    for (id, hat) in [
        ("blocked", "h"),
        ("asks", "h"),
        ("twice", "g"),
        ("parked", "g"),
        ("quiet", "h"),
        ("withdrawn", "h"),
        ("gone", "h"),
    ] {
        in_hat(&collector, id, hat);
    }
    // Blocked, with no question of its own open.
    block(&collector, "blocked");
    // Open questions, none setting `blocked` (asked outside a turn).
    question(&collector, "asks", "p1", "asks");
    question(&collector, "twice", "p2", "twice");
    question(&collector, "twice", "p3", "twice");
    question(&collector, "parked", "p4", "parked");
    question(&collector, "withdrawn", "p5", "withdrawn");
    block(&collector, "gone");
    collector.other_owner("theirs");
    block(&collector, "theirs");
    let conn = collector.conn();
    conn.execute_batch(
        "UPDATE sessions SET lifecycle = 'parked' WHERE id = 'parked';
         UPDATE pending SET state = 'cancelled' WHERE pending_id = 'p5';
         UPDATE sessions SET lifecycle = 'deleted' WHERE id = 'gone';",
    )
    .unwrap();

    let all = collector.summaries("").await;
    assert_eq!(all.waiting, 4);
    // The same as the list says of each session.
    let listed = all
        .sessions
        .iter()
        .filter(|s| s.session.activity.as_deref() == Some("blocked") || s.question_waits)
        .count();
    assert_eq!(listed, 4);
    let cursor = collector.summaries("?limit=1").await.next_cursor.unwrap();
    for query in [
        "?limit=1".to_string(),
        format!("?limit=1&cursor={cursor}"),
        "?q=quiet".into(),
        "?lifecycle=closed".into(),
        "?lifecycle=active".into(),
    ] {
        assert_eq!(collector.summaries(&query).await.waiting, 4, "{query}");
    }
    for (query, waiting) in [
        ("?hat=h", 2),
        ("?hat=g", 2),
        ("?hat=none", 0),
        ("?hat=h&lifecycle=parked&q=quiet&limit=1", 2),
    ] {
        assert_eq!(collector.summaries(query).await.waiting, waiting, "{query}");
    }
}

/// 4c's request: the list stream sends the count within its `?hat=` after
/// its resume, then whenever it changes, as its burst's last message; a
/// change outside the hat sends its upsert and no count.
#[tokio::test]
async fn the_list_stream_counts_the_sessions_waiting_within_its_hat() {
    let collector = Collector::start().await;
    in_hat(&collector, "a", "h");
    in_hat(&collector, "b", "g");
    in_hat(&collector, "c", "h");
    let page = collector.summaries("?hat=h").await;
    let at = |event: &EventDto| Some(format!("{}:{}", page.epoch, event.event_id));
    let mut stream = collector
        .stream(&format!("{LIST}?hat=h&after={}:{}", page.epoch, page.revision), None)
        .await;
    assert_eq!(
        stream.counted().await,
        (0, Some(format!("{}:{}", page.epoch, page.revision)))
    );
    // Outside the hat: the upsert ends the burst.
    block(&collector, "b");
    let event = collector.event("b", "acp_update", &chunk("1"));
    let message = stream.message().await;
    assert_eq!(
        (message.summary().session.session_id, message.id),
        ("b".to_string(), at(&event))
    );
    // Within it: the upsert, then the count.
    block(&collector, "a");
    let event = collector.event("a", "acp_update", &chunk("1"));
    let message = stream.message().await;
    assert_eq!(
        (message.summary().session.session_id.as_str(), message.id.as_deref()),
        ("a", None)
    );
    assert_eq!(stream.counted().await, (1, at(&event)));
    // An open question counts too.
    question(&collector, "c", "p1", "c");
    let event = collector.event("c", "acp_update", &chunk("1"));
    let summary = stream.message().await.summary();
    assert_eq!(
        (summary.session.session_id.as_str(), summary.question_waits),
        ("c", true)
    );
    assert_eq!(stream.counted().await, (2, at(&event)));
    // Moved out of the hat, it is no longer counted.
    collector
        .conn()
        .execute("UPDATE sessions SET hat_id = 'g' WHERE id = 'a'", [])
        .unwrap();
    let event = collector.event("a", "acp_update", &chunk("2"));
    assert_eq!(stream.message().await.summary().session.hat_id, "g");
    assert_eq!(stream.counted().await, (1, at(&event)));
    // Deleted while it waits: the removal and the count, at once.
    collector
        .conn()
        .execute("UPDATE sessions SET lifecycle = 'closed' WHERE id = 'c'", [])
        .unwrap();
    let Deletion::Done { event: deleted, .. } = collector.state.store.delete_session("c", None).unwrap() else {
        panic!("not deleted");
    };
    let rung = Instant::now();
    collector.state.hub.publish(deleted.clone());
    let removed = stream.message().await;
    assert_eq!(
        (removed.event.as_str(), removed.data.as_str(), removed.id.as_deref()),
        ("session_removed", r#"{"session_id":"c"}"#, None)
    );
    assert_eq!(stream.counted().await, (0, at(&deleted)));
    assert!(rung.elapsed() < LIST_COALESCE, "sent after {:?}", rung.elapsed());
}

/// The list's rule (6b): an empty `hat=` names no hat, and is refused,
/// on the list stream as on the page.
#[tokio::test]
async fn the_list_stream_refuses_an_empty_hat() {
    let collector = Collector::start().await;
    let page = collector.summaries("").await;
    let after = format!("{}:{}", page.epoch, page.revision);
    let resp = collector
        .client()
        .get(collector.url(&format!("{LIST}?hat=&after={after}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(resp.json::<Value>().await.unwrap()["code"], "invalid");
    // The control: a hat named.
    let mut stream = collector.stream(&format!("{LIST}?hat=h&after={after}"), None).await;
    assert_eq!(stream.counted().await, (0, Some(after)));
}

/// 4c's request: every id the list stream sends resumes it, the id a
/// `waiting_changed` carries included (a resume's, or a change's): a
/// reconnect with it is not resynced; its resume ends with the count.
#[tokio::test]
async fn a_waiting_changed_id_resumes_the_list_stream() {
    let collector = Collector::start().await;
    in_hat(&collector, "a", "h");
    let page = collector.summaries("").await;
    let mut stream = collector
        .stream(&format!("{LIST}?after={}:{}", page.epoch, page.revision), None)
        .await;
    let (_, resumed) = stream.counted().await;
    block(&collector, "a");
    collector.event("a", "acp_update", &chunk("1"));
    stream.message().await.summary();
    let (count, changed) = stream.counted().await;
    assert_eq!(count, 1);
    let (resumed, changed) = (resumed.unwrap(), changed.unwrap());
    // From the resume's id: what changed since, then the count.
    let mut again = collector.stream(LIST, Some(&resumed)).await;
    assert_eq!(again.message().await.summary().session.session_id, "a");
    assert_eq!(again.counted().await, (1, Some(changed.clone())));
    // From the change's: nothing to replay, the count, then what comes.
    let mut again = collector.stream(LIST, Some(&changed)).await;
    assert_eq!(again.counted().await, (1, Some(changed)));
    let event = collector.event("a", "acp_update", &chunk("2"));
    let message = again.message().await;
    assert_eq!(message.summary().session.session_id, "a");
    assert_eq!(message.id, Some(format!("{}:{}", page.epoch, event.event_id)));
}

/// The re-confirmation's ask: the count follows the store's own writes,
/// with nothing rung but what ingest stores: a question asked in a turn
/// blocks the session (1), and its verdict lets it run again (0).
#[tokio::test]
async fn the_count_follows_a_question_asked_and_resolved_in_a_turn() {
    use hennery_proto::frames::{Indexed, PendingExtract, PendingKind, PendingResolution, SessionBody};
    let collector = Collector::start().await;
    let store = &collector.state.store;
    let ring = |events: Vec<EventDto>| {
        for event in events {
            collector.state.hub.publish(event);
        }
    };
    assert!(store.create_session("s", "host-1", "fake", "/tmp", "h", None).unwrap());
    ring(store.ingest("s", 1, &SessionBody::session_started("r0", "a1")).unwrap());
    assert!(
        store
            .open_turn("s", "t1", &[json!({"type": "text", "text": "go"})])
            .unwrap()
    );
    let started = SessionBody::TurnStarted {
        request_id: "r1".into(),
        turn_id: "t1".into(),
    };
    ring(store.ingest("s", 2, &started).unwrap());
    let page = collector.summaries("").await;
    let mut stream = collector
        .stream(&format!("{LIST}?after={}:{}", page.epoch, page.revision), None)
        .await;
    assert_eq!(stream.counted().await.0, 0);

    let asked = SessionBody::PendingOpened {
        pending_id: "p1".into(),
        indexed: Indexed {
            turn_id: Some("t1".into()),
            pending: Some(Box::new(PendingExtract {
                id: "p1".into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into()]),
                title: None,
            })),
            ..Indexed::default()
        },
        payload: json!({"toolCall": {"toolCallId": "call-1"}}),
    };
    ring(store.ingest("s", 3, &asked).unwrap());
    let summary = stream.message().await.summary();
    assert_eq!(summary.session.activity.as_deref(), Some("blocked"));
    assert_eq!(stream.counted().await.0, 1);

    let resolved = SessionBody::PendingResolved {
        pending_id: "p1".into(),
        resolution: PendingResolution::Cancelled,
        reason: None,
    };
    ring(store.ingest("s", 4, &resolved).unwrap());
    let summary = stream.message().await.summary();
    assert_eq!(
        (summary.session.activity.as_deref(), summary.question_waits),
        (Some("running"), false)
    );
    assert_eq!(stream.counted().await.0, 0);
}
