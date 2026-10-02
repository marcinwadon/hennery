//! Plan 8c (the lane's L3): MCP servers go out only on a connection that
//! announced `mcp_servers`, and only for an agent it isolates unless the
//! collector waived that. Checked by the hub against the connection the
//! frame would go out on; nothing is sent otherwise.

use hennery_proto::frames::{
    AgentIsolation, Capabilities, Capability, CollectorFrame, McpDelivery, McpIsolation, McpServer, NameValue,
    SessionBody,
};
use hennery_sessions::hub::{Hub, RequestError};
use std::time::Duration;
use tokio::sync::mpsc;

fn start(agent: &str, servers: bool, waived: bool) -> CollectorFrame {
    CollectorFrame::StartSession {
        request_id: "r1".into(),
        session_id: "s1".into(),
        committed_seq: 0,
        agent: agent.into(),
        cwd: "/tmp".into(),
        config: Default::default(),
        hat_id: "hat-1".into(),
        mcp: McpDelivery {
            mcp_servers: if servers {
                vec![McpServer::Http {
                    name: "hennery-notes".into(),
                    url: "https://hennery.example/mcp/notes".into(),
                    headers: vec![NameValue::new("Authorization", "Bearer hst_0123456789abcdef")],
                }]
            } else {
                vec![]
            },
            isolation_waived: waived,
        },
    }
}

fn connect(hub: &Hub, capabilities: Capabilities) -> mpsc::UnboundedReceiver<CollectorFrame> {
    let (tx, rx) = mpsc::unbounded_channel();
    let isolation = AgentIsolation(
        [
            ("claude".to_string(), McpIsolation::ClaudeStrict),
            ("codex".to_string(), McpIsolation::None),
        ]
        .into_iter()
        .collect(),
    );
    let registration = hub.register("h", tx, capabilities, isolation).unwrap();
    hub.mark_ready("h", registration.conn_id);
    rx
}

/// Send `frame`; `Ok` once it went out (resolved by the test), or the error.
async fn send(
    hub: &Hub,
    rx: &mut mpsc::UnboundedReceiver<CollectorFrame>,
    frame: CollectorFrame,
) -> Result<(), RequestError> {
    let request = hub.request("h", "r1", frame, Duration::from_secs(5));
    tokio::pin!(request);
    tokio::select! {
        result = &mut request => result.map(|_| ()),
        Some(_) = rx.recv() => {
            hub.resolve("r1", SessionBody::session_started("r1", "a1"));
            request.await.map(|_| ())
        }
    }
}

#[tokio::test]
async fn servers_go_only_where_they_are_announced_and_isolated_or_waived() {
    let with = Capabilities(vec![Capability::McpServers]);
    let cases = [
        // (capabilities, agent, servers, waived, delivered)
        (with.clone(), "claude", true, false, true),
        (with.clone(), "codex", true, false, false),
        (with.clone(), "unknown", true, false, false),
        (with.clone(), "codex", true, true, true),
        (Capabilities::default(), "claude", true, false, false),
        (Capabilities::default(), "claude", true, true, false),
        // No servers: every host takes the frame, as before plan 8c.
        (Capabilities::default(), "codex", false, false, true),
    ];
    for (capabilities, agent, servers, waived, delivered) in cases {
        let hub = Hub::new();
        let mut rx = connect(&hub, capabilities.clone());
        let result = send(&hub, &mut rx, start(agent, servers, waived)).await;
        let case = format!("{capabilities:?} {agent} servers={servers} waived={waived}");
        if delivered {
            assert_eq!(result, Ok(()), "{case}");
        } else {
            assert_eq!(result, Err(RequestError::McpUndeliverable), "{case}");
            assert!(rx.try_recv().is_err(), "sent anyway: {case}");
        }
    }
}

#[tokio::test]
async fn the_hub_reports_each_agents_isolation_from_the_live_connection() {
    let hub = Hub::new();
    assert_eq!(hub.mcp_isolation("h", "claude"), None);
    let _rx = connect(&hub, Capabilities(vec![Capability::McpServers]));
    assert_eq!(
        hub.mcp_isolation("h", "claude"),
        Some((true, McpIsolation::ClaudeStrict))
    );
    assert_eq!(hub.mcp_isolation("h", "codex"), Some((true, McpIsolation::None)));
    assert_eq!(hub.mcp_isolation("h", "absent"), Some((true, McpIsolation::None)));
}

/// A host that reconnects on an older build (no `mcp_servers`) loses what
/// its last connection announced at once: nothing with servers goes out on
/// the new one, and the hub reports no isolation for it.
#[tokio::test]
async fn a_reconnect_without_the_capability_takes_no_servers() {
    let hub = Hub::new();
    let old = connect(&hub, Capabilities(vec![Capability::McpServers]));
    drop(old); // the old socket's writer is gone
    let mut rx = connect(&hub, Capabilities::default());
    let result = send(&hub, &mut rx, start("claude", true, false)).await;
    assert_eq!(result, Err(RequestError::McpUndeliverable));
    assert!(rx.try_recv().is_err(), "sent anyway");
    assert_eq!(hub.mcp_isolation("h", "claude"), Some((false, McpIsolation::None)));
}

/// A frame nobody waits for (`Hub::notify`) is checked too.
#[test]
fn notify_sends_no_servers_a_connection_cannot_take() {
    let hub = Hub::new();
    let mut rx = connect(&hub, Capabilities::default());
    assert!(!hub.notify("h", start("claude", true, false)));
    assert!(rx.try_recv().is_err(), "sent anyway");
    assert!(hub.notify("h", start("claude", false, false)));
}

/// `start(agent, true, waived)` as a resume.
fn resume(agent: &str, waived: bool) -> CollectorFrame {
    let CollectorFrame::StartSession {
        request_id,
        session_id,
        committed_seq,
        agent,
        cwd,
        config,
        hat_id,
        mcp,
    } = start(agent, true, waived)
    else {
        unreachable!()
    };
    CollectorFrame::ResumeSession {
        request_id,
        session_id,
        committed_seq,
        agent,
        cwd,
        agent_session_id: "a1".into(),
        config,
        hat_id,
        mcp,
    }
}

/// A resume carries servers as a start does (an agent keeps none across
/// `session/load`), and the hub checks it the same way: by the agent it
/// names, on the connection it would go out on.
#[tokio::test]
async fn a_resume_with_servers_is_checked_as_a_start_is() {
    let with = Capabilities(vec![Capability::McpServers]);
    for (capabilities, agent, waived, delivered) in [
        // (capabilities, agent, waived, delivered)
        (with.clone(), "claude", false, true),
        (with.clone(), "codex", false, false),
        (with, "codex", true, true),
        (Capabilities::default(), "claude", true, false),
    ] {
        let hub = Hub::new();
        let mut rx = connect(&hub, capabilities.clone());
        let result = send(&hub, &mut rx, resume(agent, waived)).await;
        let case = format!("{capabilities:?} {agent} waived={waived}");
        if delivered {
            assert_eq!(result, Ok(()), "{case}");
        } else {
            assert_eq!(result, Err(RequestError::McpUndeliverable), "{case}");
            assert!(rx.try_recv().is_err(), "sent anyway: {case}");
        }
    }
}

/// A frame that carries no delivery (a prompt) is not the guard's to
/// refuse: it goes out on any connection, one that announced nothing too.
#[tokio::test]
async fn a_frame_without_a_delivery_goes_out_on_any_connection() {
    let hub = Hub::new();
    let mut rx = connect(&hub, Capabilities::default());
    let prompt = CollectorFrame::Prompt {
        request_id: "r1".into(),
        session_id: "s1".into(),
        turn_id: "t1".into(),
        content: vec![],
    };
    assert_eq!(send(&hub, &mut rx, prompt).await, Ok(()));
}
