//! Source-policy attach tests are separate from session_mcp's thread-local tracing capture.
use hennery_host::{
    AgentCommand,
    outbox::Outbox,
    profile::Profile,
    session::{self, Attach, Launch, SessionCmd, SessionOptions},
    uplink::Uplink,
};
use hennery_proto::frames::{HostFrame, McpServer, SessionBody};
use hennery_testkit::{FakeScript, SCRIPT_ENV};
use serde_json::{Value, json};
use std::time::Duration;
fn strict() -> Value {
    json!({"claudeCode":{"options":{"extraArgs":{"strict-mcp-config":""}}}})
}
fn fake(script: &FakeScript) -> AgentCommand {
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    fake
}

async fn attach_source(
    script: &FakeScript,
    attach: Attach,
    profile: Profile,
    mcp_servers: Vec<McpServer>,
    source: Option<hennery_proto::mcp_source::McpSourceSnapshot>,
    generation: Option<u64>,
) -> Uplink {
    if profile == Profile::Claude {
        hennery_testkit::first_exec(env!("CARGO_BIN_EXE_hennery-fake-acp"));
    }
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let launch = Launch {
        mcp_source_policy: source,
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach,
        config: Default::default(),
        agent: fake(script),
        cwd: std::env::temp_dir().canonicalize().unwrap(),
        profile,
        mcp_servers,
    };
    let handle = match generation {
        Some(generation) => session::launch_delegated(uplink.clone(), launch, SessionOptions::default(), generation),
        None => session::launch(uplink.clone(), launch, SessionOptions::default()),
    };
    wait_for(&uplink, |body| {
        matches!(
            body,
            SessionBody::SessionStarted { .. } | SessionBody::StartFailed { .. }
        )
    })
    .await;
    // Closed so the adapter is gone before the next launch reads the log
    // (a failed start has ended already).
    let _ = handle.send(SessionCmd::Close {
        request_id: "r-close".into(),
    });
    handle.finished().await;
    uplink
}

#[tokio::test]
async fn explicit_sources_are_applied_and_echoed_on_new_and_load_including_empty_servers() {
    use hennery_proto::mcp_source::{McpSourcePolicy, McpSourceSnapshot};
    for policy in [McpSourcePolicy::ManagedOnly, McpSourcePolicy::ManagedPlusNative] {
        for how in [
            Attach::New,
            Attach::Load {
                agent_session_id: "saved".into(),
            },
        ] {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("session.jsonl");
            let script = FakeScript {
                session_log: Some(log.to_string_lossy().into_owned()),
                ..Default::default()
            };
            let source = McpSourceSnapshot {
                policy,
                revision: 3,
                request_id: "r0".into(),
            };
            let uplink = attach_source(&script, how, Profile::Claude, vec![], Some(source.clone()), None).await;
            assert!(
                uplink
                    .pending()
                    .unwrap()
                    .iter()
                    .any(|frame| matches!(frame,HostFrame::Session{
                body:SessionBody::SessionStarted{indexed,..},..
            } if indexed.mcp_source_policy.as_ref()==Some(&source)))
            );
            let rows = std::fs::read_to_string(log).unwrap();
            let row: Value = serde_json::from_str(rows.lines().next().unwrap()).unwrap();
            assert_eq!(row["params"]["mcpServers"], json!([]));
            if policy == McpSourcePolicy::ManagedOnly {
                assert_eq!(row["params"]["_meta"], strict());
            } else {
                assert_eq!(
                    row["params"]["_meta"],
                    json!({"claudeCode":{"options":{"strictMcpConfig":false}}})
                );
            }
        }
    }
}

#[tokio::test]
async fn delegated_new_and_load_echo_the_policy_with_the_existing_attach_generation() {
    use hennery_proto::mcp_source::{McpSourcePolicy, McpSourceSnapshot};
    for how in [
        Attach::New,
        Attach::Load {
            agent_session_id: "saved".into(),
        },
    ] {
        let source = McpSourceSnapshot {
            policy: McpSourcePolicy::ManagedPlusNative,
            revision: 7,
            request_id: "r0".into(),
        };
        let uplink = attach_source(
            &FakeScript::default(),
            how,
            Profile::Claude,
            vec![],
            Some(source.clone()),
            Some(4),
        )
        .await;
        assert!(
            uplink
                .pending()
                .unwrap()
                .iter()
                .any(|frame| matches!(frame,HostFrame::Session {
            body:SessionBody::SessionStarted{indexed,..},..
        } if indexed.mcp_source_policy.as_ref()==Some(&source)
            && indexed.config_evidence.as_ref().is_some_and(|e|e.attach_generation==4)))
        );
    }
}

#[tokio::test]
async fn explicit_source_policy_on_an_unverified_profile_fails_before_spawning() {
    use hennery_proto::mcp_source::{McpSourcePolicy, McpSourceSnapshot};
    for profile in [Profile::Generic, Profile::Codex, Profile::ClaudeOwnCli] {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("must-not-spawn.jsonl");
        let script = FakeScript {
            session_log: Some(log.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let source = McpSourceSnapshot {
            policy: McpSourcePolicy::ManagedOnly,
            revision: 0,
            request_id: "r0".into(),
        };
        let uplink = attach_source(&script, Attach::New, profile, vec![], Some(source), None).await;
        assert!(!log.exists());
        assert!(
            uplink
                .pending()
                .unwrap()
                .iter()
                .any(|frame| matches!(frame,HostFrame::Session{
            body:SessionBody::StartFailed{code,..},..
        } if code=="mcp_source_policy_unsupported"))
        );
    }
}

async fn wait_for(uplink: &Uplink, pred: impl Fn(&SessionBody) -> bool) -> Vec<HostFrame> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frames = uplink.pending().unwrap();
        if frames
            .iter()
            .any(|f| matches!(f, HostFrame::Session { body, .. } if pred(body)))
        {
            return frames;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out: {frames:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
