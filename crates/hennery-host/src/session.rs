//! One session actor per attached session (ACP core §2.2), each owning one
//! adapter process (umbrella §6.9).
//!
//! The actor is the only emitter of the session's frames: adapter
//! notifications are forwarded to it in arrival order and it stamps them
//! into the outbox, interleaved with its own facts (turn start/end, exit,
//! park, close). The ACP connection runs in a task of its own, so a dead
//! adapter never takes the actor's teardown down with it.

use crate::adapter::{Adapter, ExitInfo, KILL_GRACE};
pub use crate::adapter::{AgentCommand, NESTING_VARS};
use crate::uplink::Uplink;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, PromptResponse, SessionId,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, UntypedMessage};
use hennery_proto::frames::{HostFrame, Indexed, ParkReason, SessionBody, TurnOutcome};
use serde_json::Value;
use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// How long `start` waits for spawn → `initialize` → `session/new` →
/// `session_started` before giving up. Kept below the collector's 90s start
/// timeout (ACP core §3.4) so the host's `start_failed` always beats it.
pub const START_TIMEOUT: Duration = Duration::from_secs(75);

/// A prompt that fails because the adapter died can resolve before the exit
/// watcher reaps the process; wait this long for the exit before calling
/// the turn `failed` rather than `interrupted`.
const EXIT_SETTLE: Duration = Duration::from_millis(500);

/// After an exit, adapter output still in the pipe is forwarded until the
/// notification stream has been quiet this long.
const DRAIN_QUIET: Duration = Duration::from_millis(100);

/// Messages from the connection task to a session actor.
#[derive(Debug)]
pub enum SessionCmd {
    Prompt {
        request_id: String,
        turn_id: String,
        content: Vec<Value>,
    },
    /// A repeated `start_session` for an attached session: re-emit
    /// `session_started` with the new request id, never a second adapter
    /// (ACP core §2.2).
    Restart { request_id: String },
    /// Operator park: end any turn, kill the group, `session_parked{operator}`.
    Park { request_id: String },
    /// Operator close: end any turn, kill the group, `session_closed`.
    Close { request_id: String },
}

/// Tunables of one session actor.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub start_timeout: Duration,
    /// SIGTERM → SIGKILL grace when the actor kills its adapter.
    pub kill_grace: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            start_timeout: START_TIMEOUT,
            kill_grace: KILL_GRACE,
        }
    }
}

/// The connection task's handle on a session actor.
#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::UnboundedSender<SessionCmd>,
    open_turn: Arc<Mutex<Option<String>>>,
}

impl SessionHandle {
    /// Queue a command. `false` if the actor has ended.
    pub fn send(&self, cmd: SessionCmd) -> bool {
        self.commands.send(cmd).is_ok()
    }

    /// The actor has ended (its last fact is already in the outbox).
    pub fn is_ended(&self) -> bool {
        self.commands.is_closed()
    }

    /// The turn in flight, for `hello.attached_sessions` (ACP core §5.1).
    /// Set before `turn_started` is emitted and cleared after `turn_ended`,
    /// so `None` means every emitted turn has also been ended.
    pub fn open_turn_id(&self) -> Option<String> {
        self.open_turn.lock().expect("open turn lock").clone()
    }
}

/// Spawn a session actor with default options.
pub fn start(
    uplink: Uplink,
    request_id: String,
    session_id: String,
    agent: AgentCommand,
    cwd: PathBuf,
) -> SessionHandle {
    spawn(uplink, request_id, session_id, agent, cwd, SessionOptions::default())
}

/// Spawn a session actor. Returns its handle; the actor ends (and the handle
/// reports `is_ended`) after start failure, adapter exit, park, close, or
/// when every handle has been dropped (host shutdown).
pub fn spawn(
    uplink: Uplink,
    request_id: String,
    session_id: String,
    agent: AgentCommand,
    cwd: PathBuf,
    options: SessionOptions,
) -> SessionHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let open_turn = Arc::new(Mutex::new(None));
    let actor = Actor {
        uplink,
        session_id,
        open_turn: open_turn.clone(),
        options,
    };
    tokio::spawn(actor.run(request_id, agent, cwd, rx));
    SessionHandle {
        commands: tx,
        open_turn,
    }
}

type Reply = Pin<Box<dyn Future<Output = agent_client_protocol::Result<PromptResponse>> + Send>>;

struct Turn {
    id: String,
    reply: Reply,
}

/// Aborts the ACP connection task when the actor ends.
struct AcpTask(tokio::task::JoinHandle<()>);

impl Drop for AcpTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Actor {
    uplink: Uplink,
    session_id: String,
    open_turn: Arc<Mutex<Option<String>>>,
    options: SessionOptions,
}

impl Actor {
    fn emit(&self, body: SessionBody) {
        if let Err(err) = self.uplink.emit(&self.session_id, body) {
            tracing::error!(session_id = %self.session_id, error = %err, "failed to persist a session frame");
        }
    }

    fn set_open_turn(&self, turn_id: Option<String>) {
        *self.open_turn.lock().expect("open turn lock") = turn_id;
    }

    fn start_failed(&self, request_id: String, message: String) {
        tracing::warn!(session_id = %self.session_id, %message, "session start failed");
        self.emit(SessionBody::StartFailed {
            request_id,
            code: "start_failed".into(),
            message,
        });
    }

    async fn run(
        self,
        request_id: String,
        agent: AgentCommand,
        cwd: PathBuf,
        mut commands: mpsc::UnboundedReceiver<SessionCmd>,
    ) {
        self.drive(request_id, agent, cwd, &mut commands).await;
        // The session's last frame is in the outbox. Commands sent while the
        // actor was ending (killing its adapter can take the whole grace)
        // are answered, never dropped: the collector would otherwise wait
        // out its timeout. `close` first, so nothing slips in after the
        // last `try_recv`.
        commands.close();
        while let Ok(cmd) = commands.try_recv() {
            match cmd {
                SessionCmd::Prompt { request_id, .. }
                | SessionCmd::Park { request_id }
                | SessionCmd::Close { request_id } => {
                    self.reject(request_id, "not_attached", "the session has ended on this host".into());
                }
                SessionCmd::Restart { request_id } => {
                    tracing::info!(session_id = %self.session_id, %request_id, "ignoring a start for an ended session");
                }
            }
        }
    }

    /// The actor's life: start, then serve until it parks, closes or ends.
    /// Every return leaves the session's final frame in the outbox.
    async fn drive(
        &self,
        request_id: String,
        agent: AgentCommand,
        cwd: PathBuf,
        commands: &mut mpsc::UnboundedReceiver<SessionCmd>,
    ) {
        let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
            Ok(spawned) => spawned,
            Err(err) => return self.start_failed(request_id, format!("spawn {}: {err}", agent.program)),
        };
        let (updates_tx, mut updates) = mpsc::unbounded_channel::<Value>();
        let (conn_tx, conn_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (_stop_tx, stop_rx) = oneshot::channel::<()>();
        let transport = ByteStreams::new(io.stdin.compat_write(), io.stdout.compat());
        let session_id = self.session_id.clone();
        let _acp = AcpTask(tokio::spawn(async move {
            let result = Client
                .builder()
                .name("hennery-host")
                // Raw handler: payloads are forwarded verbatim, including
                // update kinds this build does not know (ACP core §2.4).
                .on_receive_notification(
                    async move |msg: UntypedMessage, _cx| {
                        if msg.method == "session/update" {
                            let _ = updates_tx.send(msg.params);
                        }
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(transport, async move |conn: ConnectionTo<Agent>| {
                    let _ = conn_tx.send(conn);
                    // Keep the connection up until the actor ends.
                    let _ = stop_rx.await;
                    Ok(())
                })
                .await;
            if let Err(err) = result {
                tracing::debug!(%session_id, error = %err, "ACP connection ended");
            }
        }));

        let Ok(conn) = conn_rx.await else {
            adapter.terminate(self.options.kill_grace).await;
            return self.start_failed(request_id, "the ACP connection could not be set up".into());
        };
        let negotiated = tokio::select! {
            result = tokio::time::timeout(self.options.start_timeout, negotiate(&conn, cwd)) => match result {
                Ok(Ok(agent_session)) => Ok(agent_session),
                Ok(Err(err)) => Err(err.to_string()),
                Err(_) => Err(format!("adapter did not start within {}s", self.options.start_timeout.as_secs())),
            },
            info = adapter.exited() => {
                let tail = adapter.stderr_tail().await;
                Err(format!("adapter exited during start ({}): {}", describe(info), last_lines(&tail, 5)))
            }
        };
        let agent_session = match negotiated {
            Ok(agent_session) => agent_session,
            Err(message) => {
                adapter.terminate(self.options.kill_grace).await;
                return self.start_failed(request_id, message);
            }
        };
        self.emit(SessionBody::SessionStarted {
            request_id,
            agent_session_id: agent_session.to_string(),
        });

        // Prompts are deduplicated by turn_id: a retried delivery after a
        // lost acknowledgement must never run the same turn twice. Only a
        // turn that actually started is recorded, so a corrected retry of a
        // rejected (invalid) prompt with the same turn_id still runs.
        let mut seen_turns = HashSet::new();
        let mut turn: Option<Turn> = None;
        loop {
            tokio::select! {
                // Biased: adapter output already received is emitted before
                // the prompt reply it preceded on the wire.
                biased;
                Some(payload) = updates.recv() => self.emit(update(payload)),
                info = adapter.exited() => {
                    return self.adapter_exited(info, &mut adapter, &mut updates, turn.take()).await;
                }
                cmd = commands.recv() => match cmd {
                    // Every handle dropped: the host is shutting down.
                    None => {
                        adapter.terminate(self.options.kill_grace).await;
                        return;
                    }
                    Some(SessionCmd::Prompt { request_id, turn_id, content }) => {
                        let blocks = match parse_prompt(content) {
                            Ok(blocks) => blocks,
                            Err(message) => {
                                self.reject(request_id, "invalid", message);
                                continue;
                            }
                        };
                        if seen_turns.contains(&turn_id) {
                            tracing::info!(%turn_id, "ignoring duplicate prompt delivery");
                            continue;
                        }
                        if turn.is_some() {
                            self.reject(request_id, "turn_in_progress", "a turn is already running".into());
                            continue;
                        }
                        seen_turns.insert(turn_id.clone());
                        self.set_open_turn(Some(turn_id.clone()));
                        self.emit(SessionBody::TurnStarted { request_id, turn_id: turn_id.clone() });
                        let reply = conn.send_request(PromptRequest::new(agent_session.clone(), blocks)).block_task();
                        turn = Some(Turn { id: turn_id, reply: Box::pin(reply) });
                    }
                    Some(SessionCmd::Restart { request_id }) => self.emit(SessionBody::SessionStarted {
                        request_id,
                        agent_session_id: agent_session.to_string(),
                    }),
                    Some(SessionCmd::Park { .. }) => {
                        self.teardown(&mut adapter, turn.take()).await;
                        return self.emit(SessionBody::SessionParked { reason: ParkReason::Operator });
                    }
                    Some(SessionCmd::Close { .. }) => {
                        self.teardown(&mut adapter, turn.take()).await;
                        return self.emit(SessionBody::SessionClosed);
                    }
                },
                result = next_reply(&mut turn) => {
                    let ended = turn.take().expect("a reply implies a turn");
                    match result {
                        Ok(response) => self.end_turn(ended.id, TurnOutcome::Completed, stop_reason(&response), None),
                        Err(err) => {
                            if let Some(info) = adapter.exited_within(EXIT_SETTLE).await {
                                return self.adapter_exited(info, &mut adapter, &mut updates, Some(ended)).await;
                            }
                            self.end_turn(ended.id, TurnOutcome::Failed, None, Some(err.to_string()));
                        }
                    }
                }
            }
        }
    }

    fn reject(&self, request_id: String, code: &str, message: String) {
        self.uplink.reply(HostFrame::Error {
            request_id,
            code: code.into(),
            message,
        });
    }

    fn end_turn(&self, turn_id: String, outcome: TurnOutcome, stop_reason: Option<String>, error: Option<String>) {
        self.emit(SessionBody::TurnEnded {
            turn_id,
            outcome,
            stop_reason,
            error,
        });
        self.set_open_turn(None);
    }

    /// Park or close: end the turn as interrupted, then kill the group.
    async fn teardown(&self, adapter: &mut Adapter, turn: Option<Turn>) {
        if let Some(turn) = turn {
            self.end_turn(turn.id, TurnOutcome::Interrupted, None, None);
        }
        adapter.terminate(self.options.kill_grace).await;
    }

    /// The exit watcher's steps (ACP core §2.3): outstanding calls fail (the
    /// reply future is dropped), the turn ends `interrupted`, then
    /// `adapter_exited` and `session_parked{adapter_exited}`.
    async fn adapter_exited(
        &self,
        info: ExitInfo,
        adapter: &mut Adapter,
        updates: &mut mpsc::UnboundedReceiver<Value>,
        turn: Option<Turn>,
    ) {
        tracing::warn!(session_id = %self.session_id, exit = %describe(info), "adapter exited");
        // Output the adapter wrote before dying is still in the pipe.
        while let Ok(Some(payload)) = tokio::time::timeout(DRAIN_QUIET, updates.recv()).await {
            self.emit(update(payload));
        }
        if let Some(turn) = turn {
            self.end_turn(
                turn.id,
                TurnOutcome::Interrupted,
                None,
                Some("the adapter exited".into()),
            );
        }
        let stderr_tail = adapter.stderr_tail().await;
        adapter.kill_group();
        self.emit(SessionBody::AdapterExited {
            code: info.code,
            signal: info.signal,
            stderr_tail,
        });
        self.emit(SessionBody::SessionParked {
            reason: ParkReason::AdapterExited,
        });
    }
}

/// The in-flight prompt's reply, or never if no turn is running.
async fn next_reply(turn: &mut Option<Turn>) -> agent_client_protocol::Result<PromptResponse> {
    match turn {
        Some(turn) => (&mut turn.reply).await,
        None => std::future::pending().await,
    }
}

/// `initialize` then `session/new`.
async fn negotiate(conn: &ConnectionTo<Agent>, cwd: PathBuf) -> agent_client_protocol::Result<SessionId> {
    conn.send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task()
        .await?;
    let created = conn.send_request(NewSessionRequest::new(cwd)).block_task().await?;
    Ok(created.session_id)
}

fn update(payload: Value) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed::default(),
        payload,
    }
}

fn stop_reason(response: &PromptResponse) -> Option<String> {
    serde_json::to_value(response.stop_reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
}

fn describe(info: ExitInfo) -> String {
    match (info.code, info.signal) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "unknown status".into(),
    }
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().rev().take(n).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
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
