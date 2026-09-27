//! One session actor per attached session (ACP core §2.2), each owning one
//! adapter process (umbrella §6.9).

use crate::uplink::Uplink;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, SessionId};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, UntypedMessage};
use hennery_proto::frames::{HostFrame, Indexed, SessionBody, TurnOutcome};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Environment variables that make an agent refuse to start or double-report
/// when hennery itself runs inside an agent session (ACP core §2.3).
pub const NESTING_VARS: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"];

/// How long `start` waits for spawn → `initialize` → `session/new` →
/// `session_started` before giving up. Kept below the collector's 90s start
/// timeout (ACP core §3.4) so the host's `start_failed` always beats it.
pub const START_TIMEOUT: Duration = Duration::from_secs(75);

/// How to launch an agent's ACP adapter.
#[derive(Debug, Clone)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl AgentCommand {
    /// Parse `"program arg1 arg2"` (whitespace-separated, no quoting).
    pub fn parse(command: &str) -> Option<Self> {
        let mut parts = command.split_whitespace().map(str::to_string);
        let program = parts.next()?;
        Some(Self {
            program,
            args: parts.collect(),
            env: Vec::new(),
        })
    }
}

/// Messages from the connection task to a session actor.
#[derive(Debug)]
pub enum SessionCmd {
    Prompt {
        request_id: String,
        turn_id: String,
        content: Vec<Value>,
    },
}

/// Spawn a session actor. Returns the actor's mailbox.
pub fn start(
    uplink: Uplink,
    request_id: String,
    session_id: String,
    agent: AgentCommand,
    cwd: PathBuf,
) -> mpsc::UnboundedSender<SessionCmd> {
    start_with_timeout(uplink, request_id, session_id, agent, cwd, START_TIMEOUT)
}

/// Like `start`, but with an explicit start timeout. `start` uses
/// `START_TIMEOUT`; tests use a short one so they don't have to wait out the
/// production value.
pub fn start_with_timeout(
    uplink: Uplink,
    request_id: String,
    session_id: String,
    agent: AgentCommand,
    cwd: PathBuf,
    start_timeout: Duration,
) -> mpsc::UnboundedSender<SessionCmd> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let started = Arc::new(AtomicBool::new(false));
        let result = run(
            SessionStart {
                uplink: uplink.clone(),
                request_id: request_id.clone(),
                session_id: session_id.clone(),
                agent,
                cwd,
                started: started.clone(),
                start_timeout,
            },
            rx,
        )
        .await;
        if let Err(err) = result {
            tracing::warn!(%session_id, error = %err, "session actor ended with an error");
            // Before `session_started`, the failure is the start's outcome and
            // must reach the collector durably (ACP core §3.2).
            if !started.load(Ordering::SeqCst) {
                let body = SessionBody::StartFailed {
                    request_id,
                    code: "start_failed".into(),
                    message: err.to_string(),
                };
                if let Err(e) = uplink.emit(&session_id, body) {
                    tracing::error!(error = %e, "failed to persist start_failed");
                }
            }
        }
    });
    tx
}

/// Bundles `run`'s parameters (which otherwise trip clippy's
/// `too_many_arguments`) into one value.
struct SessionStart {
    uplink: Uplink,
    request_id: String,
    session_id: String,
    agent: AgentCommand,
    cwd: PathBuf,
    started: Arc<AtomicBool>,
    start_timeout: Duration,
}

async fn run(config: SessionStart, mut commands: mpsc::UnboundedReceiver<SessionCmd>) -> anyhow::Result<()> {
    let SessionStart {
        uplink,
        request_id,
        session_id,
        agent,
        cwd,
        started,
        start_timeout,
    } = config;
    let mut command = tokio::process::Command::new(&agent.program);
    command
        .args(&agent.args)
        .envs(agent.env.iter().cloned())
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .process_group(0)
        .kill_on_drop(true);
    for var in NESTING_VARS {
        command.env_remove(var);
    }
    let mut child = command.spawn()?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());

    let updates_uplink = uplink.clone();
    let updates_session = session_id.clone();

    Client
        .builder()
        .name("hennery-host")
        // Raw handler: ACP payloads are forwarded verbatim, including update
        // kinds this build does not know (ACP core §2.4). Emitting inside the
        // handler keeps frames in arrival order.
        .on_receive_notification(
            async move |msg: UntypedMessage, _cx| {
                if msg.method == "session/update" {
                    let body = SessionBody::AcpUpdate {
                        indexed: Indexed::default(),
                        payload: msg.params,
                    };
                    if let Err(err) = updates_uplink.emit(&updates_session, body) {
                        tracing::error!(error = %err, "failed to persist a session update");
                    }
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(transport, async move |conn: ConnectionTo<Agent>| {
            let agent_session = match tokio::time::timeout(
                start_timeout,
                negotiate_session(&conn, &uplink, &session_id, &request_id, cwd.clone()),
            )
            .await
            {
                Ok(result) => result?,
                Err(_elapsed) => {
                    // Not durable via `started`: `start`'s wrapper emits
                    // `start_failed` from this error, since `started` is
                    // still false at this point.
                    return Err(agent_client_protocol::Error::new(
                        i32::from(agent_client_protocol::ErrorCode::InternalError),
                        format!("adapter did not start within {}s", start_timeout.as_secs()),
                    ));
                }
            };
            started.store(true, Ordering::SeqCst);

            // Prompts are deduplicated by turn_id: a retried delivery after a
            // lost acknowledgement must never run the same turn twice. Only a
            // turn that actually started is recorded here, so a corrected
            // retry of a rejected (invalid) prompt with the same turn_id is
            // not silently dropped.
            let mut seen_turns = std::collections::HashSet::new();
            while let Some(cmd) = commands.recv().await {
                match cmd {
                    SessionCmd::Prompt {
                        request_id,
                        turn_id,
                        content,
                    } => {
                        let blocks = match parse_prompt(content) {
                            Ok(blocks) => blocks,
                            Err(message) => {
                                uplink.reply(HostFrame::Error {
                                    request_id,
                                    code: "invalid".into(),
                                    message,
                                });
                                continue;
                            }
                        };
                        if !seen_turns.insert(turn_id.clone()) {
                            tracing::info!(%turn_id, "ignoring duplicate prompt delivery");
                            continue;
                        }
                        run_turn(&conn, &uplink, &session_id, &agent_session, request_id, turn_id, blocks).await;
                    }
                }
            }
            Ok(())
        })
        .await?;
    Ok(())
}

/// `initialize` then `session/new`, then durably emit `session_started`.
/// Split out from `run` so the whole sequence can be raced against a timeout
/// without also bounding the (potentially long-lived) prompt loop that
/// follows it.
async fn negotiate_session(
    conn: &ConnectionTo<Agent>,
    uplink: &Uplink,
    session_id: &str,
    request_id: &str,
    cwd: PathBuf,
) -> agent_client_protocol::Result<SessionId> {
    conn.send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task()
        .await?;
    let created = conn.send_request(NewSessionRequest::new(cwd)).block_task().await?;
    let agent_session: SessionId = created.session_id;
    uplink
        .emit(
            session_id,
            SessionBody::SessionStarted {
                request_id: request_id.to_string(),
                agent_session_id: agent_session.to_string(),
            },
        )
        .map_err(|e| agent_client_protocol::Error::into_internal_error(&*e))?;
    Ok(agent_session)
}

/// Validate raw prompt content into ACP content blocks. `Err` carries the
/// user-facing rejection reason for `error{code: invalid}`.
fn parse_prompt(content: Vec<Value>) -> Result<Vec<ContentBlock>, String> {
    let blocks: Result<Vec<ContentBlock>, _> = content.into_iter().map(serde_json::from_value).collect();
    match blocks {
        Ok(blocks) if !blocks.is_empty() => Ok(blocks),
        Ok(_) => Err("empty prompt".to_string()),
        Err(err) => Err(err.to_string()),
    }
}

async fn run_turn(
    conn: &ConnectionTo<Agent>,
    uplink: &Uplink,
    session_id: &str,
    agent_session: &SessionId,
    request_id: String,
    turn_id: String,
    blocks: Vec<ContentBlock>,
) {
    if let Err(err) = uplink.emit(
        session_id,
        SessionBody::TurnStarted {
            request_id,
            turn_id: turn_id.clone(),
        },
    ) {
        tracing::error!(error = %err, "failed to persist turn_started");
        return;
    }
    let result = conn
        .send_request(PromptRequest::new(agent_session.clone(), blocks))
        .block_task()
        .await;
    let body = match result {
        Ok(response) => SessionBody::TurnEnded {
            turn_id,
            outcome: TurnOutcome::Completed,
            stop_reason: serde_json::to_value(response.stop_reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string)),
            error: None,
        },
        Err(err) => SessionBody::TurnEnded {
            turn_id,
            outcome: TurnOutcome::Failed,
            stop_reason: None,
            error: Some(err.to_string()),
        },
    };
    if let Err(err) = uplink.emit(session_id, body) {
        tracing::error!(error = %err, "failed to persist turn_ended");
    }
}
