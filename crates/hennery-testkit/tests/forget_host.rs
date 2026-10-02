//! `forget_session` on a real host, played frame by frame by a fake
//! collector (plan 9d decisions 8, 13, B1, B7): the checks a forget passes
//! before anything is removed.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_host::{AgentCommand, HostConfig};
use hennery_proto::frames::{
    AgentHome, Capability, CollectorFrame, ForgetKind, ForgetOutcome, ForgetReason, HostFrame, SessionBody,
};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_testkit::{FakeScript, SCRIPT_ENV};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

const AGENT_SESSION: &str = "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

#[allow(clippy::result_large_err)]
fn with_nonce(
    _: &tokio_tungstenite::tungstenite::handshake::server::Request,
    mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    response
        .headers_mut()
        .insert(HELLO_NONCE_HEADER, hex::encode([5u8; 32]).parse().unwrap());
    Ok(response)
}

/// The fake collector's end of one host connection.
struct Collector {
    ws: Ws,
}

impl Collector {
    /// Accept the host, answer its `hello`, and read up to its
    /// `resend_complete`: the capabilities it announced.
    async fn accept(listener: &TcpListener) -> (Self, Vec<Capability>) {
        let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let ws = tokio_tungstenite::accept_hdr_async(tcp, with_nonce).await.unwrap();
        let mut collector = Self { ws };
        let HostFrame::Hello { capabilities, .. } = collector.next().await else {
            panic!("expected hello");
        };
        collector
            .send(&CollectorFrame::HelloAck {
                protocol_version: PROTOCOL_VERSION.into(),
                collector_version: "test".into(),
                committed: BTreeMap::new(),
            })
            .await;
        loop {
            if let HostFrame::ResendComplete = collector.next().await {
                break;
            }
        }
        (collector, capabilities.0)
    }

    async fn send(&mut self, frame: &CollectorFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

    async fn next(&mut self) -> HostFrame {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                    Some(Ok(_)) => {}
                    other => panic!("host connection ended: {other:?}"),
                }
            }
        })
        .await
        .expect("a host frame within 20s")
    }

    /// The next session fact, acked.
    async fn fact(&mut self) -> SessionBody {
        loop {
            if let HostFrame::Session { session_id, seq, body } = self.next().await {
                self.send(&CollectorFrame::Ack {
                    session_id,
                    ack_seq: seq,
                })
                .await;
                return body;
            }
        }
    }

    /// The answer to the forget `request_id`: a `session_forgotten`, or an
    /// `error` (as its code).
    async fn forget(&mut self, request_id: &str, agent_session_id: &str, root: &Path) -> Result<HostFrame, String> {
        self.send(&CollectorFrame::ForgetSession {
            request_id: request_id.into(),
            agent: "claude".into(),
            agent_session_id: agent_session_id.into(),
            agent_home: home(root),
        })
        .await;
        loop {
            match self.next().await {
                frame @ HostFrame::SessionForgotten { .. } if frame.probe_request_id() == Some(request_id) => {
                    return Ok(frame);
                }
                HostFrame::Error {
                    request_id: r, code, ..
                } if r == request_id => return Err(code),
                HostFrame::Session { session_id, seq, .. } => {
                    self.send(&CollectorFrame::Ack {
                        session_id,
                        ack_seq: seq,
                    })
                    .await
                }
                _ => {}
            }
        }
    }
}

fn home(root: &Path) -> AgentHome {
    AgentHome {
        root: root.to_str().unwrap().into(),
        sqlite_root: None,
    }
}

/// The reasons of a `session_forgotten`'s remaining entries, with their
/// retry flags.
fn reasons(frame: &HostFrame) -> Vec<(ForgetKind, ForgetReason, bool)> {
    match frame {
        HostFrame::SessionForgotten { remaining, .. } => {
            remaining.iter().map(|r| (r.what.kind, r.reason, r.retry)).collect()
        }
        other => panic!("not a session_forgotten: {other:?}"),
    }
}

/// The umask these tests assume: the root and kind directories must not be
/// writable by group or others (B3), and a 002 umask would make every one
/// so. Set for the whole test binary; every test here wants the same.
fn usual_umask() {
    // SAFETY: umask(2) cannot fail.
    unsafe { libc::umask(0o022) };
}

struct Setup {
    _dir: tempfile::TempDir,
    base: PathBuf,
    listener: TcpListener,
}

impl Setup {
    async fn new() -> Self {
        usual_umask();
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        for sub in ["claude", "host", "work"] {
            std::fs::create_dir(base.join(sub)).unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self {
            _dir: dir,
            base,
            listener,
        }
    }

    fn root(&self) -> PathBuf {
        self.base.join("claude")
    }

    fn data_dir(&self) -> PathBuf {
        self.base.join("host")
    }

    fn start_host(&self) {
        self.start_host_with(FakeScript {
            session_id: Some(AGENT_SESSION.into()),
            ..FakeScript::default()
        });
    }

    fn start_host_with(&self, script: FakeScript) {
        let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
        fake.env
            .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
        fake.env
            .push(("CLAUDE_CONFIG_DIR".into(), self.root().to_string_lossy().into_owned()));
        let mut cfg = HostConfig::new(
            format!("ws://{}/api/hosts/ws", self.listener.local_addr().unwrap()),
            "host-1",
            HostKey::from_seed([1; 32]),
            self.data_dir(),
        );
        cfg.agents.insert("claude".into(), fake);
        tokio::spawn(async move {
            let _ = hennery_host::run(cfg).await;
        });
    }

    fn start(&self) -> CollectorFrame {
        CollectorFrame::StartSession {
            request_id: "r1".into(),
            session_id: "s1".into(),
            committed_seq: 0,
            agent: "claude".into(),
            cwd: self.base.join("work").to_str().unwrap().into(),
            config: Default::default(),
        }
    }
}

/// Decision 13, B7, B1, decision 8: a forget is refused while a live actor
/// has the agent's session, for a root the host never registered, and for
/// an id the agent never writes; once the session is closed it runs.
#[tokio::test]
async fn a_forget_runs_only_for_a_registered_home_with_no_live_actor() {
    let setup = Setup::new().await;
    setup.start_host();
    let (mut collector, capabilities) = Collector::accept(&setup.listener).await;
    assert!(capabilities.contains(&Capability::ForgetSession));
    collector.send(&setup.start()).await;
    let SessionBody::SessionStarted { agent_home, .. } = collector.fact().await else {
        panic!("expected session_started");
    };
    assert_eq!(agent_home, Some(home(&setup.root())));

    let attached = collector.forget("f1", AGENT_SESSION, &setup.root()).await.unwrap();
    assert_eq!(
        reasons(&attached),
        [(ForgetKind::Session, ForgetReason::Attached, true)]
    );

    collector
        .send(&CollectorFrame::CloseSession {
            request_id: "c1".into(),
            session_id: "s1".into(),
        })
        .await;
    loop {
        if let SessionBody::SessionClosed = collector.fact().await {
            break;
        }
    }
    let unknown = collector
        .forget("f2", AGENT_SESSION, &setup.base.join("elsewhere"))
        .await
        .unwrap();
    assert_eq!(
        reasons(&unknown),
        [(ForgetKind::Session, ForgetReason::UnknownToHost, false)]
    );
    let other_id = "1b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";
    let unknown = collector.forget("f3", other_id, &setup.root()).await.unwrap();
    assert_eq!(
        reasons(&unknown),
        [(ForgetKind::Session, ForgetReason::UnknownToHost, false)]
    );
    assert_eq!(
        collector.forget("f4", "../../etc", &setup.root()).await,
        Err("invalid".into())
    );
    let ran = collector.forget("f5", AGENT_SESSION, &setup.root()).await.unwrap();
    let HostFrame::SessionForgotten { outcome, .. } = &ran else {
        unreachable!()
    };
    // Nothing of the session is under the root: complete.
    assert_eq!(*outcome, ForgetOutcome::Complete);
    assert_eq!(reasons(&ran), []);
}

/// Decision 1, B1: a home the host cannot register is never reported, so
/// no forget can name it.
#[tokio::test]
async fn a_home_the_host_could_not_register_is_not_reported() {
    let setup = Setup::new().await;
    // A registry whose every write fails.
    rusqlite::Connection::open(setup.data_dir().join(hennery_host::agent_home::FILE))
        .unwrap()
        .execute_batch(
            "CREATE TABLE agent_homes (agent TEXT, agent_session_id TEXT, root TEXT, sqlite_root TEXT,
                 recorded_at INTEGER, CHECK (0));",
        )
        .unwrap();
    setup.start_host();
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    collector.send(&setup.start()).await;
    let SessionBody::SessionStarted { agent_home, .. } = collector.fact().await else {
        panic!("expected session_started");
    };
    assert_eq!(agent_home, None);
}

/// Decision 1, B8: a load of an agent session that was registered under
/// another root registers this one too, and says so in a `host_note`.
#[tokio::test]
async fn a_load_under_another_root_registers_both_and_says_so() {
    let setup = Setup::new().await;
    let registry =
        hennery_host::agent_home::Registry::open(&setup.data_dir().join(hennery_host::agent_home::FILE)).unwrap();
    registry
        .record("claude", AGENT_SESSION, &home(&setup.base.join("work")))
        .unwrap();
    setup.start_host();
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    collector
        .send(&CollectorFrame::ResumeSession {
            request_id: "r1".into(),
            session_id: "s1".into(),
            committed_seq: 0,
            agent: "claude".into(),
            cwd: setup.base.join("work").to_str().unwrap().into(),
            agent_session_id: AGENT_SESSION.into(),
            config: Default::default(),
        })
        .await;
    let SessionBody::SessionStarted { agent_home, .. } = collector.fact().await else {
        panic!("expected session_started");
    };
    assert_eq!(agent_home, Some(home(&setup.root())));
    let SessionBody::HostNote { note, text } = collector.fact().await else {
        panic!("expected host_note");
    };
    assert_eq!(note, "agent_home_moved");
    assert!(!text.contains(setup.base.to_str().unwrap()), "{text}");
    assert!(
        registry
            .contains("claude", AGENT_SESSION, &home(&setup.root()))
            .unwrap()
    );
    assert!(
        registry
            .contains("claude", AGENT_SESSION, &home(&setup.base.join("work")))
            .unwrap()
    );
}

/// B7: while a forget runs, the agent session it removes is not attached:
/// a resume of it is refused `forgetting`, and attaches once it is over.
#[tokio::test]
async fn an_attach_waits_out_a_forget_of_the_same_agent_session() {
    let setup = Setup::new().await;
    let (log, gate) = (setup.base.join("delete.log"), setup.base.join("gate"));
    setup.start_host_with(FakeScript {
        session_id: Some(AGENT_SESSION.into()),
        delete_log: Some(log.to_str().unwrap().into()),
        delete_waits_for_file: Some(gate.to_str().unwrap().into()),
        ..FakeScript::default()
    });
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    collector.send(&setup.start()).await;
    let SessionBody::SessionStarted { .. } = collector.fact().await else {
        panic!("expected session_started");
    };
    collector
        .send(&CollectorFrame::CloseSession {
            request_id: "c1".into(),
            session_id: "s1".into(),
        })
        .await;
    loop {
        if let SessionBody::SessionClosed = collector.fact().await {
            break;
        }
    }
    collector
        .send(&CollectorFrame::ForgetSession {
            request_id: "f1".into(),
            agent: "claude".into(),
            agent_session_id: AGENT_SESSION.into(),
            agent_home: home(&setup.root()),
        })
        .await;
    // The forget's adapter has its `session/delete`, and holds it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !log.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the forget's adapter never got session/delete"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // A second forget of the same agent session meanwhile: refused, in
    // progress, retryable (the review's item 3).
    let second = collector.forget("f2", AGENT_SESSION, &setup.root()).await.unwrap();
    assert_eq!(
        reasons(&second),
        [(ForgetKind::Session, ForgetReason::InProgress, true)]
    );
    let resume = |request_id: &str| CollectorFrame::ResumeSession {
        request_id: request_id.into(),
        session_id: "s1".into(),
        committed_seq: 0,
        agent: "claude".into(),
        cwd: setup.base.join("work").to_str().unwrap().into(),
        agent_session_id: AGENT_SESSION.into(),
        config: Default::default(),
    };
    collector.send(&resume("r2")).await;
    loop {
        match collector.next().await {
            HostFrame::Error { request_id, code, .. } if request_id == "r2" => {
                assert_eq!(code, "forgetting");
                break;
            }
            HostFrame::Session { body, .. } => panic!("attached during the forget: {body:?}"),
            _ => {}
        }
    }
    std::fs::write(&gate, "").unwrap();
    loop {
        if let frame @ HostFrame::SessionForgotten { .. } = collector.next().await {
            assert_eq!(frame.probe_request_id(), Some("f1"));
            break;
        }
    }
    collector.send(&resume("r3")).await;
    let SessionBody::SessionStarted { request_id, .. } = collector.fact().await else {
        panic!("expected session_started");
    };
    assert_eq!(request_id, "r3");
}
