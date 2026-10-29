//! Request IDs are a test oracle here, never an authorization credential.
//! Both enrolled hosts use real authenticated collector WebSockets.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::{
    hosts::{Enrollment, Hosts},
    operator::Operator,
};
use hennery_proto::{
    HELLO_NONCE_HEADER, PROTOCOL_VERSION,
    frames::{
        AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, ParkReason, SessionBody, TurnOutcome,
    },
};
use hennery_sessions::{
    AppState,
    hub::{RequestError, Undo},
    store::{Deletion, Store, Unattached},
};
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};
use tokio::task::JoinHandle;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

const DEADLINE: Duration = Duration::from_secs(30);
type Outcome = JoinHandle<Result<SessionBody, RequestError>>;

struct Collector {
    state: AppState,
    addr: SocketAddr,
    server: JoinHandle<std::io::Result<()>>,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let hosts = Hosts::open_in_memory().unwrap();
        for (id, seed) in [("a", 1), ("b", 2)] {
            hosts
                .register(
                    id,
                    &Enrollment {
                        public_key: HostKey::from_seed([seed; 32]).public_key_hex(),
                        name: id.into(),
                        host_version: "test".into(),
                        platform: "test".into(),
                    },
                    0,
                )
                .unwrap();
        }
        let state = AppState::new(
            Store::open(&dir.path().join("test.db")).unwrap(),
            hosts,
            Operator::open_in_memory().unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            state,
            addr,
            server,
            _dir: dir,
        }
    }

    fn session(&self, id: &str, host: &str, active: bool) {
        self.state
            .store
            .create_session(id, host, "fake", "/tmp", "hat-1", None)
            .unwrap();
        if active {
            self.state
                .store
                .ingest(id, 1, &SessionBody::session_started("initial", "adapter"))
                .unwrap();
        }
    }

    fn request(&self, id: &str, frame: CollectorFrame, undo: Option<Undo>, timeout: Duration) -> Outcome {
        let hub = self.state.hub.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            match undo {
                Some(undo) => hub.request_with_undo("b", &id, frame, timeout, undo).await,
                None => hub.request("b", &id, frame, timeout).await,
            }
        })
    }

    async fn stop(self) {
        self.state.shutdown.cancel();
        self.server.await.unwrap().unwrap();
    }
}

struct Host {
    ws: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
}

impl Host {
    async fn connect(c: &Collector, id: &str, attached: &[&str]) -> (Self, BTreeMap<String, u64>) {
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", c.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws };
        host.send(HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: id.into(),
            proof: HostKey::from_seed([if id == "a" { 1 } else { 2 }; 32]).sign_hello(&nonce, id, PROTOCOL_VERSION),
            capabilities: Capabilities(vec![Capability::Park]),
            workspace_roots: vec![],
            attached_sessions: attached
                .iter()
                .map(|s| AttachedSession {
                    session_id: (*s).into(),
                    last_seq: 0,
                    open_turn_id: None,
                    ending: None,
                })
                .collect(),
            mcp_isolation: Default::default(),
            runtime_profiles: Default::default(),
        })
        .await;
        let CollectorFrame::HelloAck { committed, .. } = host.next().await else {
            panic!("hello_ack")
        };
        (host, committed)
    }

    async fn ready(c: &Collector, id: &str) -> Self {
        let (mut host, _) = Self::connect(c, id, &[]).await;
        host.send(HostFrame::ResendComplete).await;
        tokio::time::timeout(DEADLINE, async {
            while !c.state.hub.is_ready(id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        host
    }

    async fn send(&mut self, frame: HostFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(&frame).unwrap()))
            .await
            .unwrap();
    }

    async fn next(&mut self) -> CollectorFrame {
        tokio::time::timeout(DEADLINE, async {
            loop {
                if let Message::Text(t) = self.ws.next().await.unwrap().unwrap() {
                    return serde_json::from_str(&t).unwrap();
                }
            }
        })
        .await
        .unwrap()
    }

    async fn fact(&mut self, session: &str, seq: u64, body: SessionBody) {
        self.send(HostFrame::Session {
            session_id: session.into(),
            seq,
            body,
        })
        .await;
        assert_eq!(
            self.next().await,
            CollectorFrame::Ack {
                session_id: session.into(),
                ack_seq: seq
            }
        );
    }

    // The Ack proves the socket processed the earlier non-outboxed Error.
    async fn error(&mut self, request: &str, barrier: &str, seq: u64) {
        self.send(HostFrame::Error {
            request_id: request.into(),
            code: "refused".into(),
            message: "synthetic".into(),
        })
        .await;
        self.fact(
            barrier,
            seq,
            SessionBody::HostNote {
                note: "barrier".into(),
                text: String::new(),
            },
        )
        .await;
    }

    async fn disconnect(mut self, c: &Collector, id: &str) {
        self.ws.close(None).await.unwrap();
        tokio::time::timeout(DEADLINE, async {
            while c.state.hub.is_ready(id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

fn start(request: &str, session: &str) -> CollectorFrame {
    CollectorFrame::StartSession {
        request_id: request.into(),
        session_id: session.into(),
        committed_seq: 0,
        agent: "fake".into(),
        cwd: "/tmp".into(),
        config: Default::default(),
        hat_id: "hat-1".into(),
        mcp: Default::default(),
    }
}

fn prompt(request: &str, session: &str) -> CollectorFrame {
    CollectorFrame::Prompt {
        request_id: request.into(),
        session_id: session.into(),
        turn_id: "turn".into(),
        content: vec![],
        plugin_context: None,
    }
}

fn failed(request: &str) -> SessionBody {
    SessionBody::StartFailed {
        request_id: request.into(),
        code: "refused".into(),
        message: "synthetic".into(),
    }
}

fn refused() -> Result<SessionBody, RequestError> {
    Err(RequestError::Rejected {
        code: "refused".into(),
        message: "synthetic".into(),
    })
}

#[tokio::test]
async fn foreign_errors_leave_start_and_prompt_undo_untouched() {
    let c = Collector::start().await;
    let mut a = Host::ready(&c, "a").await;
    let mut b = Host::ready(&c, "b").await;
    c.session("barrier-a", "a", true);
    c.session("barrier-b", "b", true);
    c.session("starting", "b", false);
    c.session("prompting", "b", true);
    assert!(c.state.store.open_turn("prompting", "turn", &[]).unwrap());
    let starting = c.request(
        "start",
        start("start", "starting"),
        Some(Undo::Start {
            session_id: "starting".into(),
        }),
        DEADLINE,
    );
    let prompting = c.request(
        "prompt",
        prompt("prompt", "prompting"),
        Some(Undo::Prompt {
            session_id: "prompting".into(),
            turn_id: "turn".into(),
        }),
        DEADLINE,
    );
    b.next().await;
    b.next().await;
    let before = c.state.store.events("starting", 0, 100).unwrap();
    a.error("start", "barrier-a", 2).await;
    a.error("prompt", "barrier-a", 3).await;
    assert_eq!(c.state.hub.pending_requests(), 2);
    assert!(!starting.is_finished() && !prompting.is_finished());
    assert_eq!(
        c.state.store.find_session("starting").unwrap().unwrap().lifecycle,
        "starting"
    );
    assert_eq!(c.state.store.events("starting", 0, 100).unwrap(), before);
    assert_eq!(c.state.store.turn_state("turn").unwrap().as_deref(), Some("sent"));
    b.error("start", "barrier-b", 2).await;
    assert_eq!(starting.await.unwrap(), refused());
    assert_eq!(
        c.state.store.find_session("starting").unwrap().unwrap().lifecycle,
        "failed"
    );
    assert_eq!(c.state.hub.pending_requests(), 1);
    b.error("prompt", "barrier-b", 3).await;
    assert_eq!(prompting.await.unwrap(), refused());
    assert_eq!(c.state.store.turn_state("turn").unwrap(), None);
    assert_eq!(c.state.hub.pending_requests(), 0);
    c.stop().await;
}

async fn foreign_fact_preserves_start(forged: SessionBody) {
    let c = Collector::start().await;
    let mut a = Host::ready(&c, "a").await;
    let mut b = Host::ready(&c, "b").await;
    c.session("victim", "b", false);
    c.session("foreign", "a", false);
    let pending = c.request("known", start("known", "victim"), None, DEADLINE);
    b.next().await;
    let before = c.state.store.events("victim", 0, 100).unwrap();
    a.fact("foreign", 1, forged).await;
    assert_eq!(c.state.hub.pending_requests(), 1);
    assert!(!pending.is_finished());
    assert_eq!(c.state.store.events("victim", 0, 100).unwrap(), before);
    assert_eq!(
        c.state.store.find_session("victim").unwrap().unwrap().lifecycle,
        "starting"
    );
    let fact = SessionBody::session_started("known", "real");
    b.fact("victim", 1, fact.clone()).await;
    assert_eq!(pending.await.unwrap(), Ok(fact));
    assert_eq!(c.state.hub.pending_requests(), 0);
    c.stop().await;
}

#[tokio::test]
async fn foreign_session_started_preserves_victim_waiter_and_store() {
    foreign_fact_preserves_start(SessionBody::session_started("known", "forged")).await;
}

#[tokio::test]
async fn foreign_turn_started_preserves_victim_waiter_and_store() {
    foreign_fact_preserves_start(SessionBody::TurnStarted {
        request_id: "known".into(),
        turn_id: "forged".into(),
    })
    .await;
}

#[tokio::test]
async fn foreign_config_applied_preserves_victim_waiter_and_store() {
    foreign_fact_preserves_start(SessionBody::ConfigApplied {
        request_id: "known".into(),
        indexed: Default::default(),
    })
    .await;
}

#[tokio::test]
async fn foreign_start_failed_preserves_victim_waiter_and_store() {
    foreign_fact_preserves_start(failed("known")).await;
}

async fn foreign_turn_fact_preserves_victim(ending: bool) {
    let c = Collector::start().await;
    let mut a = Host::ready(&c, "a").await;
    let mut b = Host::ready(&c, "b").await;
    c.session("victim", "b", true);
    c.session("foreign", "a", true);
    assert!(
        c.state
            .store
            .open_turn(
                "victim",
                "turn",
                &[serde_json::json!({"type":"text", "text":"private synthetic prompt"})]
            )
            .unwrap()
    );
    let started = SessionBody::TurnStarted {
        request_id: "known".into(),
        turn_id: "turn".into(),
    };
    let ended = SessionBody::TurnEnded {
        turn_id: "turn".into(),
        outcome: TurnOutcome::Completed,
        stop_reason: None,
        error: None,
    };
    let mut pending = c.request("known", prompt("known", "victim"), None, DEADLINE);
    b.next().await;
    if ending {
        b.fact("victim", 2, started.clone()).await;
        assert_eq!(pending.await.unwrap(), Ok(started.clone()));
        let hub = c.state.hub.clone();
        pending = tokio::spawn(async move {
            hub.request_for_turn(
                "b",
                "cancel",
                "victim",
                "turn",
                CollectorFrame::CancelTurn {
                    request_id: "cancel".into(),
                    session_id: "victim".into(),
                    turn_id: "turn".into(),
                },
                DEADLINE,
            )
            .await
        });
        b.next().await;
    }
    let before = c.state.store.events("victim", 0, 100).unwrap();
    a.fact("foreign", 2, if ending { ended.clone() } else { started.clone() })
        .await;
    assert_eq!(
        c.state.store.turn_state("turn").unwrap().as_deref(),
        Some(if ending { "started" } else { "sent" })
    );
    assert_eq!(c.state.store.events("victim", 0, 100).unwrap(), before);
    assert!(
        !c.state
            .store
            .events("foreign", 0, 100)
            .unwrap()
            .iter()
            .any(|event| event.kind == "user_turn")
    );
    assert_eq!(c.state.hub.pending_requests(), 1);
    let real = if ending { ended } else { started };
    b.fact("victim", if ending { 3 } else { 2 }, real.clone()).await;
    assert_eq!(pending.await.unwrap(), Ok(real));
    c.stop().await;
}

#[tokio::test]
async fn foreign_turn_started_cannot_take_a_victim_turn() {
    foreign_turn_fact_preserves_victim(false).await;
}

#[tokio::test]
async fn foreign_turn_ended_cannot_end_a_victim_turn() {
    foreign_turn_fact_preserves_victim(true).await;
}

#[tokio::test]
async fn same_host_reply_must_match_session_fact_kind_and_prompt_turn() {
    let c = Collector::start().await;
    let _a = Host::ready(&c, "a").await;
    let mut b = Host::ready(&c, "b").await;
    c.session("victim", "b", true);
    c.session("other", "b", false);
    assert!(c.state.store.open_turn("victim", "turn", &[]).unwrap());
    let pending = c.request("known", prompt("known", "victim"), None, DEADLINE);
    b.next().await;
    for (i, fact) in [
        SessionBody::session_started("known", "wrong"),
        failed("known"),
        SessionBody::TurnStarted {
            request_id: "known".into(),
            turn_id: "turn".into(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        b.fact("other", i as u64 + 1, fact).await;
        assert_eq!(c.state.hub.pending_requests(), 1);
    }
    b.fact(
        "victim",
        2,
        SessionBody::ConfigApplied {
            request_id: "known".into(),
            indexed: Default::default(),
        },
    )
    .await;
    assert_eq!(c.state.hub.pending_requests(), 1);
    b.fact(
        "victim",
        3,
        SessionBody::TurnStarted {
            request_id: "known".into(),
            turn_id: "wrong".into(),
        },
    )
    .await;
    assert_eq!(c.state.hub.pending_requests(), 1);
    b.fact("victim", 4, failed("known")).await;
    assert_eq!(c.state.hub.pending_requests(), 1);
    assert_eq!(c.state.store.turn_state("turn").unwrap().as_deref(), Some("sent"));
    b.fact("victim", 5, SessionBody::session_started("known", "wrong-kind"))
        .await;
    assert_eq!(c.state.hub.pending_requests(), 1);
    let fact = SessionBody::TurnStarted {
        request_id: "known".into(),
        turn_id: "turn".into(),
    };
    b.fact("victim", 6, fact.clone()).await;
    assert_eq!(pending.await.unwrap(), Ok(fact));
    c.stop().await;
}

#[tokio::test]
async fn a_park_fact_cannot_complete_a_close_waiter() {
    let c = Collector::start().await;
    let mut a = Host::ready(&c, "a").await;
    let mut b = Host::ready(&c, "b").await;
    c.session("victim", "b", true);
    c.session("foreign", "a", true);
    let hub = c.state.hub.clone();
    let pending = tokio::spawn(async move {
        hub.request_for_session(
            "b",
            "close",
            "victim",
            CollectorFrame::CloseSession {
                request_id: "close".into(),
                session_id: "victim".into(),
            },
            DEADLINE,
        )
        .await
    });
    b.next().await;
    a.fact("foreign", 2, SessionBody::SessionClosed).await;
    assert_eq!(c.state.hub.pending_requests(), 1);
    b.fact(
        "victim",
        2,
        SessionBody::SessionParked {
            reason: ParkReason::Operator,
        },
    )
    .await;
    assert_eq!(c.state.hub.pending_requests(), 1);
    b.fact("victim", 3, SessionBody::SessionClosed).await;
    assert_eq!(pending.await.unwrap(), Ok(SessionBody::SessionClosed));
    c.stop().await;
}

#[tokio::test]
async fn foreign_error_preserves_detached_waiter_watchdog_and_reconnect() {
    let c = Collector::start().await;
    let mut a = Host::ready(&c, "a").await;
    let mut b = Host::ready(&c, "b").await;
    c.session("foreign", "a", true);
    c.session("victim", "b", false);
    let pending = c.request(
        "known",
        start("known", "victim"),
        Some(Undo::Start {
            session_id: "victim".into(),
        }),
        Duration::from_secs(120),
    );
    b.next().await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    a.error("known", "foreign", 2).await;
    assert_eq!(c.state.hub.pending_requests(), 1);
    assert!(c.state.hub.is_ready("b"));
    // Pause only after real socket I/O; unrelated timers must not advance it.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(121)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
    tokio::time::timeout(DEADLINE, async {
        while c.state.hub.is_ready("b") {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(c.state.hub.pending_requests(), 0);
    assert_eq!(
        c.state.store.find_session("victim").unwrap().unwrap().lifecycle,
        "starting"
    );
    drop(b);
    let mut reconnected = Host::ready(&c, "b").await;
    assert_eq!(
        c.state.store.find_session("victim").unwrap().unwrap().lifecycle,
        "failed"
    );
    c.session("next", "b", false);
    let pending = c.request("next", start("next", "next"), None, DEADLINE);
    reconnected.next().await;
    reconnected.fact("next", 1, failed("next")).await;
    assert_eq!(pending.await.unwrap(), refused());
    c.stop().await;
}

#[tokio::test]
async fn hello_committed_sequences_are_bound_to_host_including_tombstones() {
    let c = Collector::start().await;
    let mut a = Host::ready(&c, "a").await;
    let mut b = Host::ready(&c, "b").await;
    for (id, host) in [("own", "a"), ("victim", "b"), ("deleted-a", "a"), ("deleted-b", "b")] {
        c.session(id, host, false);
    }
    a.fact("own", 7, SessionBody::session_started("r", "a")).await;
    b.fact("victim", 73, SessionBody::session_started("r", "b")).await;
    a.fact("deleted-a", 81, failed("r")).await;
    b.fact("deleted-b", 92, failed("r")).await;
    for id in ["deleted-a", "deleted-b"] {
        assert!(matches!(
            c.state
                .store
                .delete_session(
                    id,
                    Some(&Unattached {
                        lifecycle: "failed".into(),
                        presumed_parked: false
                    })
                )
                .unwrap(),
            Deletion::Done { .. }
        ));
    }
    a.disconnect(&c, "a").await;
    let (mut a, committed) = Host::connect(&c, "a", &["own", "victim", "unknown", "deleted-a", "deleted-b"]).await;
    assert_eq!(committed.get("own"), Some(&7));
    for id in ["victim", "unknown", "deleted-a", "deleted-b"] {
        assert_eq!(committed.get(id).copied().unwrap_or(0), 0, "{id}");
    }
    // A tombstone still belongs to its host: discard/ack late facts, never resurrect it.
    a.fact("deleted-a", 100, SessionBody::session_started("late", "late"))
        .await;
    assert!(c.state.store.find_session("deleted-a").unwrap().is_none());
    assert_eq!(c.state.store.committed_seq("deleted-a").unwrap(), 0);
    b.disconnect(&c, "b").await;
    let (_b, committed) = Host::connect(&c, "b", &["victim", "own"]).await;
    assert_eq!(committed.get("victim"), Some(&73));
    assert_eq!(committed.get("own").copied().unwrap_or(0), 0);
    c.stop().await;
}
