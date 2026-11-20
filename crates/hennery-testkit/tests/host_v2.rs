use hennery_host::{
    outbox::Outbox,
    session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionOptions},
    uplink::Uplink,
};
use hennery_proto::frames::{AcpProtocolPreference, HostFrame, SessionBody, SessionConfig};
use serde_json::json;
use std::time::Duration;

async fn wait(uplink: &Uplink, predicate: impl Fn(&[HostFrame]) -> bool) -> Vec<HostFrame> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frames = uplink.pending().unwrap();
            if predicate(&frames) {
                return frames;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture actor deadline")
}
fn started(frames: &[HostFrame]) -> bool {
    frames.iter().any(|frame| {
        matches!(
            frame,
            HostFrame::Session {
                body: SessionBody::SessionStarted { .. },
                ..
            }
        )
    })
}
fn ended(frames: &[HostFrame]) -> bool {
    frames.iter().any(|frame| {
        matches!(
            frame,
            HostFrame::Session {
                body: SessionBody::TurnEnded { .. },
                ..
            }
        )
    })
}
fn fixture(protocol: AcpProtocolPreference, selected: u16, idle: Option<Duration>) -> (Uplink, session::SessionHandle) {
    let (uplink, _) = Uplink::new(Outbox::open_in_memory().unwrap());
    let mut agent = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-v2")).unwrap();
    agent.env.push(("FAKE_ACP_SELECTED".into(), selected.to_string()));
    let launch = Launch {
        hooks: None,
        request_id: "start".into(),
        session_id: "s".into(),
        attach: Attach::New,
        config: SessionConfig {
            protocol,
            ..Default::default()
        },
        agent,
        cwd: std::env::temp_dir(),
        profile: Default::default(),
        mcp_servers: vec![],
        mcp_source_policy: None,
    };
    let handle = session::launch(
        uplink.clone(),
        launch,
        SessionOptions {
            idle_timeout: idle,
            ..Default::default()
        },
    );
    (uplink, handle)
}

#[tokio::test]
async fn v2_ack_waits_for_idle_and_background_prevents_idle_reaping() {
    let (uplink, handle) = fixture(AcpProtocolPreference::V2, 2, Some(Duration::from_millis(150)));
    wait(&uplink, started).await;
    assert!(handle.send(SessionCmd::Prompt {
        request_id: "p".into(),
        turn_id: "turn".into(),
        content: vec![json!({"type":"text","text":"hi"})],
        plugin_context: None
    }));
    wait(&uplink,|frames|frames.iter().any(|f|matches!(f,HostFrame::Session{body:SessionBody::AcpUpdate{payload,..},..} if payload["update"]["state"]=="running"))).await;
    assert!(
        !ended(&uplink.pending().unwrap()),
        "prompt acknowledgement cannot end the turn"
    );
    wait(&uplink, ended).await;
    tokio::time::sleep(Duration::from_millis(220)).await;
    assert!(!handle.is_ended(), "background task must inhibit the idle reaper");
    let frames=wait(&uplink,|frames|frames.iter().any(|f|matches!(f,HostFrame::Session{body:SessionBody::AcpUpdate{payload,..},..} if payload["update"].get("exitStatus").is_some()))).await;
    assert_eq!(
        frames
            .iter()
            .filter(|f| matches!(
                f,
                HostFrame::Session {
                    body: SessionBody::TurnEnded { .. },
                    ..
                }
            ))
            .count(),
        1
    );
    wait(&uplink, |frames| {
        frames.iter().any(|f| {
            matches!(
                f,
                HostFrame::Session {
                    body: SessionBody::SessionParked { .. },
                    ..
                }
            )
        })
    })
    .await;
}

#[tokio::test]
async fn automatic_negotiates_v1_but_strict_v2_refuses_before_session_creation() {
    for (preference, success) in [(AcpProtocolPreference::V2V1, true), (AcpProtocolPreference::V2, false)] {
        let (uplink, handle) = fixture(preference, 1, None);
        let frames = wait(&uplink, |f| {
            started(f)
                || f.iter().any(|f| {
                    matches!(
                        f,
                        HostFrame::Session {
                            body: SessionBody::StartFailed { .. },
                            ..
                        }
                    )
                })
        })
        .await;
        assert_eq!(started(&frames), success);
        if success {
            assert!(frames.iter().any(|f|matches!(f,HostFrame::Session{body:SessionBody::SessionStarted{indexed,..},..} if indexed.acp_protocol==Some(1)&&indexed.acp_fallback.is_some())));
            handle.send(SessionCmd::Close {
                request_id: "close".into(),
            });
            wait(&uplink, |f| {
                f.iter().any(|f| {
                    matches!(
                        f,
                        HostFrame::Session {
                            body: SessionBody::SessionClosed,
                            ..
                        }
                    )
                })
            })
            .await;
        }
    }
}

#[tokio::test]
async fn managed_adapter_feature_gap_falls_back_before_session_creation_or_refuses_strict_v2() {
    for (preference, automatic) in [(AcpProtocolPreference::V2V1, true), (AcpProtocolPreference::V2, false)] {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("wire-methods");
        let (uplink, _) = Uplink::new(Outbox::open_in_memory().unwrap());
        let mut agent = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-v2")).unwrap();
        agent
            .env
            .push(("FAKE_ACP_REQUEST_LOG".into(), log.to_string_lossy().into_owned()));
        let launch = Launch {
            hooks: None,
            request_id: "start".into(),
            session_id: "s".into(),
            attach: Attach::New,
            config: SessionConfig {
                protocol: preference,
                ..Default::default()
            },
            agent,
            cwd: directory.path().to_path_buf(),
            profile: hennery_host::profile::Profile::Claude,
            mcp_servers: vec![],
            mcp_source_policy: None,
        };
        let handle = session::launch(
            uplink.clone(),
            launch,
            SessionOptions {
                idle_timeout: None,
                ..Default::default()
            },
        );
        let frames = wait(&uplink, |frames| {
            started(frames)
                || frames.iter().any(|f| {
                    matches!(
                        f,
                        HostFrame::Session {
                            body: SessionBody::StartFailed { .. },
                            ..
                        }
                    )
                })
        })
        .await;
        let methods = std::fs::read_to_string(&log).unwrap();
        assert_eq!(methods.lines().next(), Some("initialize:2"));
        if automatic {
            assert!(started(&frames));
            assert_eq!(
                methods.lines().filter(|line| line.starts_with("session/new")).count(),
                1
            );
            assert!(methods.contains("initialize:1"));
            assert!(frames.iter().any(|frame| matches!(frame, HostFrame::Session {
                body: SessionBody::SessionStarted { indexed, .. }, ..
            } if indexed.acp_protocol == Some(1) && indexed.acp_fallback.as_ref().is_some_and(|reason| reason.contains("steering")))));
            handle.send(SessionCmd::Close {
                request_id: "close".into(),
            });
            wait(&uplink, |frames| {
                frames.iter().any(|frame| {
                    matches!(
                        frame,
                        HostFrame::Session {
                            body: SessionBody::SessionClosed,
                            ..
                        }
                    )
                })
            })
            .await;
        } else {
            assert!(!started(&frames));
            assert!(!methods.contains("session/new"));
            assert!(frames.iter().any(|frame| matches!(frame, HostFrame::Session {
                body: SessionBody::StartFailed { code, message, .. }, ..
            } if code == "acp_v2_feature_gap" && message.contains("steering"))));
        }
    }
}

#[tokio::test]
async fn unsupported_version_error_restarts_process_before_v1_initialization() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("methods");
    let processes = directory.path().join("processes");
    let (uplink, _) = Uplink::new(Outbox::open_in_memory().unwrap());
    let mut agent = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-v2")).unwrap();
    agent.env.extend([
        ("FAKE_ACP_REQUEST_LOG".into(), log.to_string_lossy().into_owned()),
        ("FAKE_ACP_PROCESS_LOG".into(), processes.to_string_lossy().into_owned()),
        ("FAKE_ACP_REJECT_V2".into(), "1".into()),
    ]);
    let handle = session::launch(
        uplink.clone(),
        Launch {
            hooks: None,
            request_id: "start".into(),
            session_id: "s".into(),
            attach: Attach::New,
            config: SessionConfig {
                protocol: AcpProtocolPreference::V2V1,
                ..Default::default()
            },
            agent,
            cwd: directory.path().to_path_buf(),
            profile: Default::default(),
            mcp_servers: vec![],
            mcp_source_policy: None,
        },
        SessionOptions {
            idle_timeout: None,
            ..Default::default()
        },
    );
    let frames = wait(&uplink, started).await;
    let processes = std::fs::read_to_string(processes).unwrap();
    let attempts: Vec<_> = processes.lines().map(|line| line.split_once(':').unwrap()).collect();
    assert_eq!(attempts.len(), 2);
    assert_eq!((attempts[0].1, attempts[1].1), ("2", "1"));
    assert_ne!(
        attempts[0].0, attempts[1].0,
        "fallback must create a fresh adapter process"
    );
    let methods = std::fs::read_to_string(log).unwrap();
    assert_eq!(
        methods.lines().filter(|line| line.starts_with("session/new:")).count(),
        1
    );
    assert!(frames.iter().any(|frame|matches!(frame, HostFrame::Session {
        body: SessionBody::SessionStarted { indexed, .. }, ..
    } if indexed.acp_protocol==Some(1)&&indexed.acp_fallback.as_ref().is_some_and(|reason|reason.contains("supports ACP v1")))));
    handle.send(SessionCmd::Close {
        request_id: "close".into(),
    });
    wait(&uplink, |frames| {
        frames.iter().any(|frame| {
            matches!(
                frame,
                HostFrame::Session {
                    body: SessionBody::SessionClosed,
                    ..
                }
            )
        })
    })
    .await;
}
