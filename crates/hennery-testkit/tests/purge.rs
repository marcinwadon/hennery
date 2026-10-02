//! A hat's purge over HTTP (kernel spec §5.5; plan 9c decisions 10 to 13,
//! A2, A3, A7, A12, A13, A15), against a scripted host over a real
//! WebSocket: the preview, the purge of every session of the hat in every
//! lifecycle, with its rules, recents and row, and `forget_hat` at the next
//! handshake; what refuses a purge, and leaves the hat as it was; what a
//! frozen hat refuses; and a purge that stopped after the freeze, resumed.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hats::{HatChange, NewRule, PurgeStart, RulesChange};
use hennery_kernel::hosts::{Enrollment, Hosts, Revoke};
use hennery_kernel::lifecycle::LifecycleHooks;
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
use hennery_proto::rest::{ApiError, HatItem, PurgePreview, PurgeResult};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::hats::SessionsRunning;
use hennery_sessions::store::Store;
use hennery_sessions::{AppState, store::Reassign};
use rusqlite::Connection;
use serde_json::json;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const HOST: &str = "host-1";
const NOW: i64 = 4_000_000_000;

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
    db: PathBuf,
    _dir: tempfile::TempDir,
}

impl Collector {
    /// A collector on one database file, with `HOST` paired and not
    /// connected, and two hats: Acme, which is purged, and Other.
    async fn start() -> (Self, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        state
            .hosts
            .register(HOST, &enrollment(host_key().public_key_hex()), 0)
            .unwrap();
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        let collector = Self {
            addr,
            state,
            db,
            _dir: dir,
        };
        let acme = collector.hat("Acme");
        let other = collector.hat("Other");
        (collector, acme, other)
    }

    fn hat(&self, name: &str) -> String {
        match self.state.hosts.create_hat(name, None, NOW).unwrap() {
            HatChange::Done(hat) => hat.id,
            other => panic!("expected a hat, got {other:?}"),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// The owner's client, stepped up.
    fn client(&self) -> reqwest::Client {
        hennery_testkit::operator_client(&self.state.operator)
    }

    async fn preview(&self, hat: &str) -> PurgePreview {
        let resp = self
            .client()
            .get(self.url(&format!("/api/hats/{hat}/purge")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        resp.json().await.unwrap()
    }

    async fn purge(&self, hat: &str) -> reqwest::Response {
        self.client()
            .post(self.url(&format!("/api/hats/{hat}/purge")))
            .send()
            .await
            .unwrap()
    }

    async fn purged(&self, hat: &str) -> PurgeResult {
        let resp = self.purge(hat).await;
        assert_eq!(resp.status(), 200);
        resp.json().await.unwrap()
    }

    /// `id` on `host` in `hat`, `active`: its host's `session_started`,
    /// stored as if it came in.
    fn active(&self, id: &str, host: &str, hat: &str) {
        assert!(
            self.state
                .store
                .create_session(id, host, "fake", "/p/acme", hat, None)
                .unwrap()
        );
        self.state
            .store
            .ingest(id, 1, &SessionBody::session_started("r0", "a0"))
            .unwrap();
    }

    fn parked(&self, id: &str, host: &str, hat: &str) {
        self.active(id, host, hat);
        self.state
            .store
            .ingest(
                id,
                2,
                &SessionBody::SessionParked {
                    reason: hennery_proto::frames::ParkReason::Idle,
                },
            )
            .unwrap();
    }

    fn lifecycle(&self, id: &str) -> Option<String> {
        self.state.store.find_session(id).unwrap().map(|s| s.lifecycle)
    }

    fn raw(&self, id: &str) -> (String, String) {
        Connection::open(&self.db)
            .unwrap()
            .query_row("SELECT lifecycle, hat_id FROM sessions WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap()
    }
}

fn enrollment(public_key: String) -> Enrollment {
    Enrollment {
        public_key,
        name: "test".into(),
        host_version: "test".into(),
        platform: "test".into(),
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String, String) {
    let status = resp.status().as_u16();
    let error: ApiError = resp.json().await.unwrap();
    (status, error.code, error.message)
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The test playing `HOST`.
struct ScriptedHost {
    ws: Ws,
}

impl ScriptedHost {
    /// `hello` with `attached`, then `resend_complete`, and wait until the
    /// collector lists the host as reconciled.
    async fn connect(collector: &Collector, attached: Vec<AttachedSession>) -> Self {
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities: Capabilities(vec![Capability::Park, Capability::ResolvePath]),
            workspace_roots: vec![],
            attached_sessions: attached,
        })
        .await;
        let ack = host.next().await;
        assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
        host.send(&HostFrame::ResendComplete).await;
        wait_for("host ready", || collector.state.hub.is_ready(HOST)).await;
        host
    }

    async fn send(&mut self, frame: &HostFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

    /// The next collector frame, whatever it is.
    async fn next(&mut self) -> CollectorFrame {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                    Some(Ok(_)) => {}
                    other => panic!("collector connection ended: {other:?}"),
                }
            }
        })
        .await
        .expect("a collector frame within 10s")
    }

    /// The `forget_hat`s that reach the host: the first, however long it
    /// takes (up to `next`'s 10s), then any more within `within` of it,
    /// whatever else comes in between (acks, closes, answers).
    async fn forgotten(&mut self, within: Duration) -> Vec<String> {
        let mut hats = loop {
            if let CollectorFrame::ForgetHat { hat_id } = self.next().await {
                break vec![hat_id];
            }
        };
        let _ = tokio::time::timeout(within, async {
            loop {
                if let CollectorFrame::ForgetHat { hat_id } = self.next().await {
                    hats.push(hat_id);
                }
            }
        })
        .await;
        hats
    }

    /// Answer every `resolve_path` with the path itself, as a host with no
    /// symlinks would (plan 5c), until `request` is answered.
    async fn serve_while(&mut self, request: tokio::task::JoinHandle<reqwest::Response>) -> reqwest::Response {
        tokio::pin!(request);
        loop {
            tokio::select! {
                done = &mut request => return done.unwrap(),
                frame = self.next() => {
                    if let CollectorFrame::ResolvePath { request_id, path } = frame {
                        self.send(&HostFrame::ResolvedPath {
                            request_id,
                            canonical: path,
                            exists: true,
                            is_dir: true,
                        })
                        .await;
                    }
                }
            }
        }
    }

    /// Drop the connection and wait until the collector has noticed.
    async fn drop_connection(self, collector: &Collector) {
        drop(self.ws);
        wait_for("host gone", || !collector.state.hub.is_ready(HOST)).await;
    }
}

async fn wait_for(what: &str, mut probe: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !probe() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn attached(session_id: &str, last_seq: u64) -> AttachedSession {
    AttachedSession {
        session_id: session_id.into(),
        last_seq,
        open_turn_id: None,
    }
}

/// Decisions 10, 11 and 13, the 5c and 5d hand-ons, A7 and A13: every
/// session of the hat goes, of every lifecycle and host, presumed parked
/// and re-assigned in included, each leaving a tombstone that keeps the
/// hat's id; so do its rules, its recents and its row. Its `purged_hats`
/// row stays, and the host is told to forget the hat at its next
/// handshake. Another hat's sessions and the sessions of no hat stay; the
/// latter are listed in the preview.
#[tokio::test]
async fn a_purge_deletes_every_session_rule_and_recent_of_the_hat() {
    let (c, acme, other) = Collector::start().await;
    let rules = c.state.hosts.replace_path_rules(
        HOST,
        &[NewRule {
            prefix: "/p/acme".into(),
            hat_id: acme.clone(),
            verified: true,
        }],
    );
    assert!(matches!(rules.unwrap(), RulesChange::Done(_)));
    assert!(c.state.hosts.remember(HOST, &acme, "/p/acme", NOW).unwrap());

    // HOST is away: nothing of it is running where the collector reaches.
    c.parked("parked", HOST, &acme);
    c.parked("closed", HOST, &acme);
    c.state.store.close_now("closed").unwrap();
    assert!(
        c.state
            .store
            .create_session("failed", HOST, "fake", "/p/acme", &acme, None)
            .unwrap()
    );
    c.state.store.mark_failed("failed", "spawn").unwrap();
    assert!(
        c.state
            .store
            .create_session("starting", HOST, "fake", "/p/acme", &acme, None)
            .unwrap()
    );
    c.active("active", HOST, &acme);
    c.active("presumed", "host-gone", &acme);
    c.state.store.presume_parked("host-gone").unwrap();
    c.parked("moved-in", HOST, &other);
    assert!(matches!(
        c.state.store.reassign_hat("moved-in", &acme).unwrap(),
        Reassign::Done(_)
    ));
    c.parked("kept", HOST, &other);
    assert!(
        c.state
            .store
            .create_session("no-hat", HOST, "fake", "/p", "", None)
            .unwrap()
    );
    let purged = [
        "active", "closed", "failed", "moved-in", "parked", "presumed", "starting",
    ];

    let preview = c.preview(&acme).await;
    assert_eq!(
        (
            preview.hat_id.as_str(),
            preview.purging,
            preview.sessions,
            preview.running.len(),
            preview.rules,
            preview.recents
        ),
        (acme.as_str(), false, 7, 0, 1, 1)
    );
    let unassigned: Vec<&str> = preview.unassigned.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!((unassigned, preview.unassigned_count), (vec!["no-hat"], 1));

    let mut events = c.state.hub.subscribe();
    let result = c.purged(&acme).await;
    assert_eq!((result.sessions, result.rules), (7, 1));
    // Closed here while their host may still run them (A13).
    let mut unconfirmed = result.unconfirmed.clone();
    unconfirmed.sort();
    assert_eq!(unconfirmed, ["active", "presumed", "starting"]);
    // Each deletion reached the session's streams (A10).
    let mut deleted = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.kind == "session_deleted" {
            deleted.push(event.session_id);
        }
    }
    deleted.sort();
    assert_eq!(deleted, purged);

    for id in purged {
        assert_eq!(c.lifecycle(id), None, "{id}");
        assert_eq!(c.raw(id), ("deleted".to_string(), acme.clone()), "{id}");
    }
    assert_eq!(c.lifecycle("kept").as_deref(), Some("parked"));
    assert_eq!(c.lifecycle("no-hat").as_deref(), Some("starting"));
    assert_eq!(c.state.hosts.hat(&acme).unwrap(), None);
    assert_eq!(c.state.hosts.purge_counts(&acme).unwrap(), (0, 0));
    assert_eq!(c.state.hosts.path_rules(HOST).unwrap().unwrap(), []);
    assert_eq!(c.state.hosts.purged_hats().unwrap(), [acme.as_str()]);
    let hats: Vec<HatItem> = c
        .client()
        .get(c.url("/api/hats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(hats.iter().all(|hat| hat.id != acme && !hat.purging), "{hats:?}");
    // Done: the hat is unknown.
    let resp = c
        .client()
        .get(c.url(&format!("/api/hats/{acme}/purge")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(code_of(c.purge(&acme).await).await.0, 404);

    // The host forgets it at its next handshake, and at every one after.
    let mut host = ScriptedHost::connect(&c, vec![]).await;
    assert_eq!(host.forgotten(Duration::from_millis(500)).await, [acme.as_str()]);
    host.drop_connection(&c).await;
    let mut host = ScriptedHost::connect(&c, vec![]).await;
    assert_eq!(host.forgotten(Duration::from_millis(500)).await, [acme.as_str()]);
}

/// Decision 10a and b, A2: a default hat, by the setting, a host's, or a
/// revoked host's, is not purged; nor is a hat with a session that may run
/// where the collector reaches it, until that session is closed. Each
/// leaves the hat as it was: not frozen, its rules and sessions there.
#[tokio::test]
async fn a_default_hat_or_a_running_session_refuses_the_purge() {
    let (c, acme, other) = Collector::start().await;
    let personal = c.state.hosts.default_hat_for_new_hosts().unwrap();
    let old = c.hat("Old");
    let key = HostKey::from_seed([2; 32]).public_key_hex();
    c.state.hosts.register("host-2", &enrollment(key), 0).unwrap();
    c.state.hosts.update_host(HOST, None, Some(&other)).unwrap();
    c.state.hosts.update_host("host-2", None, Some(&old)).unwrap();
    assert_eq!(c.state.hosts.revoke("host-2", NOW).unwrap(), Revoke::Revoked);
    for hat in [&personal, &other, &old] {
        let (status, code, _) = code_of(c.purge(hat).await).await;
        assert_eq!((status, code.as_str()), (409, "hat_is_default"), "{hat}");
        assert!(!c.state.hosts.is_frozen(hat).unwrap(), "{hat}");
    }
    assert_eq!(code_of(c.purge("hat-nope").await).await.0, 404);

    c.active("running", HOST, &acme);
    c.parked("parked", HOST, &acme);
    c.active("running-other", HOST, &other);
    let mut host = ScriptedHost::connect(&c, vec![attached("running", 1), attached("running-other", 1)]).await;
    assert_eq!(c.lifecycle("running").as_deref(), Some("active"));
    // A default hat is refused as one, running sessions or not (a before b).
    let (status, code, _) = code_of(c.purge(&other).await).await;
    assert_eq!((status, code.as_str()), (409, "hat_is_default"));
    let preview = c.preview(&acme).await;
    assert_eq!((preview.sessions, preview.running), (2, vec!["running".to_string()]));
    let (status, code, message) = code_of(c.purge(&acme).await).await;
    assert_eq!((status, code.as_str()), (409, "sessions_running"));
    assert!(message.contains("running"), "{message}");
    assert!(!c.state.hosts.is_frozen(&acme).unwrap());
    assert!(!c.state.hosts.hat(&acme).unwrap().unwrap().purging);
    assert_eq!(c.lifecycle("running").as_deref(), Some("active"));
    assert_eq!(c.lifecycle("parked").as_deref(), Some("parked"));

    // Closed by its host: the purge goes through.
    host.send(&HostFrame::Session {
        session_id: "running".into(),
        seq: 2,
        body: SessionBody::SessionClosed,
    })
    .await;
    wait_for("closed", || c.lifecycle("running").as_deref() == Some("closed")).await;
    let result = c.purged(&acme).await;
    assert_eq!((result.sessions, result.unconfirmed.len()), (2, 0));
}

/// Decision 10c, A2, A3, A12: once frozen, the hat is listed `purging`, and
/// nothing resumes or moves in or out of it, it becomes no default, and no
/// rule names it; each refusal changes nothing.
#[tokio::test]
async fn a_frozen_hat_refuses_a_resume_a_move_a_default_and_a_rule() {
    let (c, acme, other) = Collector::start().await;
    c.parked("in-acme", HOST, &acme);
    c.parked("in-other", HOST, &other);
    let mut host = ScriptedHost::connect(&c, vec![]).await;
    assert!(matches!(
        c.state.hosts.begin_purge(&acme, NOW).unwrap(),
        PurgeStart::Frozen { .. }
    ));
    let hats: Vec<HatItem> = c
        .client()
        .get(c.url("/api/hats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed = hats.iter().find(|hat| hat.id == acme).unwrap();
    assert!(listed.purging);
    assert!(c.preview(&acme).await.purging);

    let client = c.client();
    let resume = client.post(c.url("/api/sessions/in-acme/resume"));
    let resume = tokio::spawn(async move { resume.send().await.unwrap() });
    let (status, code, _) = code_of(host.serve_while(resume).await).await;
    assert_eq!((status, code.as_str()), (409, "hat_purging"));
    assert_eq!(c.lifecycle("in-acme").as_deref(), Some("parked"));

    for (session, hat) in [("in-acme", &other), ("in-other", &acme)] {
        let resp = client
            .patch(c.url(&format!("/api/sessions/{session}")))
            .json(&json!({ "hat_id": hat }))
            .send()
            .await
            .unwrap();
        let (status, code, _) = code_of(resp).await;
        assert_eq!((status, code.as_str()), (409, "hat_purging"), "{session}");
    }
    assert_eq!(c.raw("in-acme").1, acme);
    assert_eq!(c.raw("in-other").1, other);

    let resp = client
        .patch(c.url(&format!("/api/hats/{acme}")))
        .json(&json!({ "default_for_new_hosts": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await.1, "hat_purging");
    assert_ne!(c.state.hosts.default_hat_for_new_hosts().unwrap(), acme);
    let resp = client
        .patch(c.url(&format!("/api/hosts/{HOST}")))
        .json(&json!({ "default_hat_id": acme }))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await.0, 400);
    assert_ne!(c.state.hosts.host(HOST).unwrap().unwrap().default_hat_id, acme);
    let put = client
        .put(c.url(&format!("/api/hosts/{HOST}/path-rules")))
        .json(&json!({ "rules": [{ "prefix": "/p/acme", "hat_id": acme }] }));
    let resp = host
        .serve_while(tokio::spawn(async move { put.send().await.unwrap() }))
        .await;
    assert_eq!(code_of(resp).await.0, 400);
    assert_eq!(c.state.hosts.path_rules(HOST).unwrap().unwrap(), []);
}

/// Decision 10's crash safety, A12: a purge that stopped after the freeze
/// leaves the hat frozen and present, and a purge again completes it, from
/// the freeze alone, or after its sessions' part failed on a session that
/// was running (a race the route's own check cannot see).
#[tokio::test]
async fn a_purge_that_stopped_after_the_freeze_completes_when_posted_again() {
    let (c, acme, other) = Collector::start().await;
    // The freeze alone, as a crash right after it leaves it.
    c.parked("a1", HOST, &acme);
    assert!(matches!(
        c.state.hosts.begin_purge(&acme, NOW).unwrap(),
        PurgeStart::Frozen { .. }
    ));
    let result = c.purged(&acme).await;
    assert_eq!((result.sessions, result.rules), (1, 0));
    assert_eq!(c.state.hosts.hat(&acme).unwrap(), None);

    // The freeze, then the sessions' part, which finds one running.
    let third = c.hat("Third");
    c.parked("t-parked", HOST, &third);
    c.active("t-running", HOST, &third);
    let host = ScriptedHost::connect(&c, vec![attached("t-running", 1)]).await;
    assert!(matches!(
        c.state.hosts.begin_purge(&third, NOW).unwrap(),
        PurgeStart::Frozen { .. }
    ));
    let err = c.state.on_hat_purged(&third).unwrap_err();
    assert_eq!(
        err.downcast_ref::<SessionsRunning>(),
        Some(&SessionsRunning(vec!["t-running".into()]))
    );
    assert!(c.state.hosts.is_frozen(&third).unwrap());
    assert!(c.state.hosts.hat(&third).unwrap().unwrap().purging);
    // It stopped before deleting anything.
    assert_eq!(c.lifecycle("t-parked").as_deref(), Some("parked"));
    let (status, code, _) = code_of(c.purge(&third).await).await;
    assert_eq!((status, code.as_str()), (409, "sessions_running"));
    assert!(c.state.hosts.hat(&third).unwrap().unwrap().purging);

    // Its host goes away: it can no longer be reached, and is closed here.
    host.drop_connection(&c).await;
    let result = c.purged(&third).await;
    assert_eq!(
        (result.sessions, result.unconfirmed),
        (2, vec!["t-running".to_string()])
    );
    assert_eq!(c.state.hosts.hat(&third).unwrap(), None);
    assert_eq!(c.lifecycle("t-parked"), None);
    // Idempotent: run again on a hat done, the hook deletes nothing.
    c.state.on_hat_purged(&third).unwrap();
    assert!(c.state.hosts.hat(&other).unwrap().is_some());
}

/// A8: once purged, nothing of the hat's is left in the database's files,
/// the WAL included: its name was in its row, which the purge deleted
/// (`secure_delete`), and the WAL is checkpointed after it.
#[tokio::test]
async fn a_purge_leaves_no_trace_of_the_hat_in_the_database_files() {
    let (c, _, _) = Collector::start().await;
    let name = "Zebracorn Unlimited";
    let hat = c.hat(name);
    c.parked("s1", HOST, &hat);
    c.purged(&hat).await;
    let dir = c.db.parent().unwrap();
    for file in ["hennery.db", "hennery.db-wal"] {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
        assert!(
            !bytes.windows(name.len()).any(|w| w == name.as_bytes()),
            "{file} still holds the hat's name"
        );
    }
}

/// A8, for a purge: its deletes share one checkpoint, after the last, so a
/// reader that holds the WAL across the purge holds it up once, not once
/// per session (a held-up checkpoint takes about 3.5 s here). Once the reader is
/// gone, the checkpoint owed is retried and the hat's name leaves the WAL.
#[tokio::test]
async fn a_reader_holding_the_wal_holds_a_purge_up_once_not_once_per_session() {
    const SESSIONS: usize = 10;
    let (c, _, _) = Collector::start().await;
    let name = "Quagga Holdings";
    let hat = c.hat(name);
    for n in 0..SESSIONS {
        c.parked(&format!("s{n}"), HOST, &hat);
    }
    // A read transaction on another connection, open across the purge.
    let reader = Connection::open(&c.db).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    let started = std::time::Instant::now();
    let result = c.purged(&hat).await;
    let took = started.elapsed();
    assert_eq!(result.sessions, SESSIONS as u64);
    // Measured: about 3.5 s with one checkpoint, 33 s with one per session.
    assert!(took < Duration::from_secs(15), "the purge took {took:?}");
    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    let wal = c.db.with_extension("db-wal");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::fs::read(&wal)
        .unwrap_or_default()
        .windows(name.len())
        .any(|w| w == name.as_bytes())
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the checkpoint owed was not retried"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
