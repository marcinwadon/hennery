//! A scripted ACP agent for tests. Speaks ACP over stdio via the
//! `agent-client-protocol` crate, so the host is tested against the same
//! wire format real adapters use.

use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Stdio, UntypedMessage};
use hennery_testkit::{CRASH_EXIT_CODE, FakeScript, SCRIPT_ENV};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> agent_client_protocol::Result<()> {
    let script: FakeScript = std::env::var(SCRIPT_ENV)
        .ok()
        .map(|s| serde_json::from_str(&s).expect("valid fake script JSON"))
        .unwrap_or_default();

    for line in &script.stderr_lines {
        eprintln!("{line}");
    }
    if let Some(path) = &script.grandchild_pid_file {
        // Detached from our stdio so it cannot hold the ACP pipes open; it
        // stays in our process group, like an agent CLI's own subprocess.
        // Never waited on by design: it must outlive us unless the host kills
        // the whole group.
        #[allow(clippy::zombie_processes)]
        let child = std::process::Command::new("sleep")
            .arg("600")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn grandchild");
        std::fs::write(path, child.id().to_string()).expect("write grandchild pid");
    }

    let load_session = !script.no_load_session;
    // `session/cancel` for the prompt in flight: set by the notification,
    // cleared when a prompt starts. Handlers run in arrival order, so a
    // cancel sent right after its prompt is never cleared by that prompt.
    let cancel = Arc::new(watch::channel(false).0);
    Agent
        .builder()
        .name("hennery-fake-acp")
        .on_receive_request(
            async move |req: InitializeRequest, responder, _cx| {
                responder.respond(
                    InitializeResponse::new(req.protocol_version)
                        .agent_capabilities(AgentCapabilities::new().load_session(load_session)),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                async move |_req: NewSessionRequest, responder, _cx| match script.new_session_error {
                    Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
                    None => responder.respond(NewSessionResponse::new("fake-session-1")),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                async move |req: LoadSessionRequest, responder, cx| {
                    // History first, then the answer: an ACP agent replays a
                    // loaded session as `session/update`s before it responds.
                    for update in &script.replay {
                        cx.send_notification(UntypedMessage::new(
                            "session/update",
                            serde_json::json!({ "sessionId": req.session_id, "update": update }),
                        )?)?;
                    }
                    match script.load_error {
                        Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
                        None => responder.respond(LoadSessionResponse::new()),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let cancel = cancel.clone();
                let ignore = script.ignore_cancel;
                async move |_n: CancelNotification, _cx| {
                    if !ignore {
                        cancel.send_replace(true);
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                async move |req: PromptRequest, responder, cx| {
                    let script = script.clone();
                    let cx2 = cx.clone();
                    cancel.send_replace(false);
                    let mut cancelled = cancel.subscribe();
                    cx.spawn(async move {
                        for (sent, chunk) in script.chunks.into_iter().enumerate() {
                            if script.exit_after_chunks == Some(sent) {
                                crash().await;
                            }
                            // A cancelled prompt stops streaming and answers
                            // `cancelled`, as ACP asks of an agent.
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_millis(script.chunk_delay_ms)) => {}
                                _ = cancelled.wait_for(|c| *c) => {
                                    return match script.cancel_error {
                                        Some(code) => responder
                                            .respond_with_error(agent_client_protocol::Error::new(code, "aborted")),
                                        None => responder.respond(PromptResponse::new(StopReason::Cancelled)),
                                    };
                                }
                            }
                            cx2.send_notification(SessionNotification::new(
                                req.session_id.clone(),
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                                    TextContent::new(chunk),
                                ))),
                            ))?;
                        }
                        if script.exit_after_chunks.is_some() {
                            crash().await;
                        }
                        responder.respond(PromptResponse::new(StopReason::EndTurn))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}

/// Exit mid-turn without answering the prompt. The short pause lets the
/// chunks already sent reach stdout first.
async fn crash() -> ! {
    tokio::time::sleep(Duration::from_millis(100)).await;
    eprintln!("fake-acp: crashing mid-turn");
    std::process::exit(CRASH_EXIT_CODE);
}
