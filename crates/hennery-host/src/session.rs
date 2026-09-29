//! One session actor per attached session (ACP core §2.2), each owning one
//! adapter process (umbrella §6.9).
//!
//! The actor is the only emitter of the session's frames: adapter
//! notifications are forwarded to it in arrival order and it stamps them
//! into the outbox, interleaved with its own facts (turn start/end, exit,
//! park, close). The ACP connection runs in a task of its own, so a dead
//! adapter never takes the actor's teardown down with it.

use crate::adapter::{Adapter, ExitInfo, KILL_GRACE, scrub};
pub use crate::adapter::{AgentCommand, NESTING_VARS};
use crate::uplink::Uplink;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    BooleanConfigOptionCapabilities, CancelNotification, ClientCapabilities, ClientSessionCapabilities, ContentBlock,
    InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, PromptResponse, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigOptionsCapabilities,
    SessionId, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    StopReason,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode, UntypedMessage};
use hennery_proto::frames::{ConfigValue, HostFrame, Indexed, ParkReason, SessionBody, SessionConfig, TurnOutcome};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

/// How long `start` waits for spawn → `initialize` → `session/new` (or
/// `session/load`) → `session_started` before giving up. Kept below the
/// collector's 90s start timeout (ACP core §3.4) so the host's
/// `start_failed` always beats it.
pub const START_TIMEOUT: Duration = Duration::from_secs(75);

/// A prompt that fails because the adapter died can resolve before the exit
/// watcher reaps the process; wait this long for the exit before calling
/// the turn `failed` rather than `interrupted`.
const EXIT_SETTLE: Duration = Duration::from_millis(500);

/// After an exit, adapter output still in the pipe is forwarded until the
/// notification stream has been quiet this long.
const DRAIN_QUIET: Duration = Duration::from_millis(100);

/// Default idle window before the reaper parks a session (ACP core §4.7).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long an adapter gets to end a turn after `session/cancel` before the
/// host stops it. Well below the collector's 60 s cancel timeout (ACP core
/// §3.4), so the collector hears the turn end, not a timeout that would
/// drop the whole host connection.
pub const CANCEL_GRACE: Duration = Duration::from_secs(20);

/// How long one `session/set_config_option` may take. A start's or a
/// resume's switch that takes longer is reported, not fatal (ACP core
/// §4.3); a `set_config` that takes longer is answered `config_failed`,
/// well before the collector's 60 s timeout (§3.4).
pub const CONFIG_TIMEOUT: Duration = Duration::from_secs(15);

/// Update kinds a `session/load` replays as history: dropped while the load
/// is outstanding (ACP core §4.5).
const HISTORY_KINDS: &[&str] = &[
    "user_message_chunk",
    "agent_message_chunk",
    "agent_thought_chunk",
    "tool_call",
    "tool_call_update",
    "plan",
];

/// Update kinds that describe the adapter's current state: passed through
/// during a load (ACP core §4.5).
const STATE_KINDS: &[&str] = &[
    "available_commands_update",
    "config_option_update",
    "current_mode_update",
    "usage_update",
    "session_info_update",
];

/// Messages from the connection task to a session actor.
#[derive(Debug)]
pub enum SessionCmd {
    Prompt {
        request_id: String,
        turn_id: String,
        content: Vec<Value>,
    },
    /// A repeated `start_session` or `resume_session` for an attached
    /// session: re-emit `session_started` with the new request id, never a
    /// second adapter (ACP core §2.2).
    Restart { request_id: String },
    /// Operator park: end any turn, kill the group, `session_parked{operator}`.
    Park { request_id: String },
    /// Operator close: end any turn, kill the group, `session_closed`.
    Close { request_id: String },
    /// Operator cancel of the turn in flight: `session/cancel` to the
    /// adapter; the turn's `turn_ended` completes it (ACP core §4.4).
    Cancel { request_id: String, turn_id: String },
    /// Switch one config option (`session/set_config_option`): answered by
    /// `config_applied`, or an error (ACP core §3.3).
    SetConfig {
        request_id: String,
        config_id: String,
        value: ConfigValue,
    },
}

/// How the actor creates its adapter session.
#[derive(Debug, Clone, PartialEq)]
pub enum Attach {
    /// `session/new`: a fresh agent session.
    New,
    /// `session/load` of the agent's own session id, with replay
    /// suppression (ACP core §4.3, §4.5).
    Load { agent_session_id: String },
}

/// Tunables of one session actor.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub start_timeout: Duration,
    /// SIGTERM → SIGKILL grace when the actor kills its adapter.
    pub kill_grace: Duration,
    /// Park the session after this long with no turn in flight
    /// (`session_parked{idle}`); `None` disables the reaper.
    pub idle_timeout: Option<Duration>,
    /// How long a cancelled turn may run on before the adapter is stopped.
    pub cancel_grace: Duration,
    /// How long one config switch may take.
    pub config_timeout: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            start_timeout: START_TIMEOUT,
            kill_grace: KILL_GRACE,
            idle_timeout: Some(IDLE_TIMEOUT),
            cancel_grace: CANCEL_GRACE,
            config_timeout: CONFIG_TIMEOUT,
        }
    }
}

/// Everything a session actor needs to attach its session.
#[derive(Debug, Clone)]
pub struct Launch {
    pub request_id: String,
    pub session_id: String,
    pub attach: Attach,
    /// Applied once the adapter session exists (ACP core §4.3).
    pub config: SessionConfig,
    pub agent: AgentCommand,
    pub cwd: PathBuf,
}

/// The connection task's handle on a session actor.
#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::UnboundedSender<SessionCmd>,
    open_turn: Arc<Mutex<Option<String>>>,
    /// Cancelled when the actor's task has finished (its adapter is gone).
    done: CancellationToken,
    /// A park or close has been queued: this actor will serve no further
    /// start or resume, even before it has read that command.
    ending: Arc<AtomicBool>,
}

impl SessionHandle {
    /// Queue a command. `false` if the actor has ended.
    pub fn send(&self, cmd: SessionCmd) -> bool {
        let ends = matches!(cmd, SessionCmd::Park { .. } | SessionCmd::Close { .. });
        let sent = self.commands.send(cmd).is_ok();
        if sent && ends {
            self.ending.store(true, Ordering::SeqCst);
        }
        sent
    }

    /// A park or close has been queued through `send` (set only there; an
    /// actor that ends by itself — an idle reap, an adapter exit — never
    /// sets it, and shows only as `is_ended` once it is done): a `Restart`
    /// sent now would be answered `not_attached` once the actor gets to it.
    pub fn is_ending(&self) -> bool {
        self.ending.load(Ordering::SeqCst)
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

    /// Resolves once the actor's task has finished — after its adapter has
    /// been terminated. Owned, so it can be awaited after every handle is
    /// dropped (host shutdown).
    pub fn finished(&self) -> WaitForCancellationFutureOwned {
        self.done.clone().cancelled_owned()
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

/// Spawn a session actor for a new agent session (`session/new`). Returns
/// its handle; the actor ends (and the handle reports `is_ended`) after
/// start failure, adapter exit, park, close, or when every handle has been
/// dropped (host shutdown).
pub fn spawn(
    uplink: Uplink,
    request_id: String,
    session_id: String,
    agent: AgentCommand,
    cwd: PathBuf,
    options: SessionOptions,
) -> SessionHandle {
    let launch = Launch {
        request_id,
        session_id,
        attach: Attach::New,
        config: SessionConfig::default(),
        agent,
        cwd,
    };
    self::launch(uplink, launch, options)
}

/// Spawn a session actor that resumes the agent's session
/// `agent_session_id` (`session/load`, ACP core §4.3). Otherwise like
/// [`spawn`].
pub fn resume(
    uplink: Uplink,
    request_id: String,
    session_id: String,
    agent_session_id: String,
    agent: AgentCommand,
    cwd: PathBuf,
    options: SessionOptions,
) -> SessionHandle {
    let launch = Launch {
        request_id,
        session_id,
        attach: Attach::Load { agent_session_id },
        config: SessionConfig::default(),
        agent,
        cwd,
    };
    self::launch(uplink, launch, options)
}

/// Spawn a session actor for `launch`: `session/new` or `session/load`,
/// then its config. Otherwise like [`spawn`].
pub fn launch(uplink: Uplink, launch: Launch, options: SessionOptions) -> SessionHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let open_turn = Arc::new(Mutex::new(None));
    let actor = Actor {
        uplink,
        session_id: launch.session_id.clone(),
        open_turn: open_turn.clone(),
        options,
        catalogue: Mutex::new(Catalogue::default()),
    };
    let done = CancellationToken::new();
    let finished = done.clone().drop_guard();
    tokio::spawn(async move {
        let _finished = finished;
        actor.run(launch, rx).await;
    });
    SessionHandle {
        commands: tx,
        open_turn,
        done,
        ending: Arc::new(AtomicBool::new(false)),
    }
}

type Reply = Pin<Box<dyn Future<Output = agent_client_protocol::Result<PromptResponse>> + Send>>;

type ConfigReply = Pin<
    Box<
        dyn Future<
                Output = Result<
                    agent_client_protocol::Result<SetSessionConfigOptionResponse>,
                    tokio::time::error::Elapsed,
                >,
            > + Send,
    >,
>;

/// A `set_config` received and not sent yet.
struct QueuedSwitch {
    request_id: String,
    config_id: String,
    value: ConfigValue,
    /// Receipt plus `config_timeout`: past it, the switch is answered
    /// `config_failed`, sent or not.
    deadline: Instant,
}

/// The actor's `set_config` switches. At most one is out at a time: the
/// real adapters handle requests concurrently, so two switches in flight
/// could land in either order (a model switch clamping a mode set after
/// it). The rest wait, in the order they came. Those still waiting or out
/// when the actor ends are answered `not_attached` then, after its last
/// fact: the collector would otherwise wait out its timeout and drop the
/// whole host connection.
struct PendingConfigs {
    uplink: Uplink,
    queued: VecDeque<QueuedSwitch>,
    out: Option<(String, ConfigReply)>,
}

impl Drop for PendingConfigs {
    fn drop(&mut self) {
        let out = self.out.take().map(|(request_id, _)| request_id);
        let queued = self.queued.drain(..).map(|q| q.request_id);
        for request_id in out.into_iter().chain(queued) {
            self.uplink.reply(HostFrame::Error {
                request_id,
                code: "not_attached".into(),
                message: "the session has ended on this host".into(),
            });
        }
    }
}

struct Turn {
    id: String,
    reply: Reply,
    /// Set once `session/cancel` went out: the adapter must end the turn by
    /// then, or it is stopped.
    cancel_deadline: Option<Instant>,
}

/// Why a start or resume failed: the `start_failed` code and message.
struct StartError {
    code: &'static str,
    message: String,
}

impl StartError {
    fn other(message: String) -> Self {
        Self {
            code: "start_failed",
            message,
        }
    }

    /// Known adapter failures get their own codes (ACP core §4.3): the agent
    /// has no record of a loaded session, or its CLI is not logged in.
    fn acp(err: agent_client_protocol::Error, loading: bool) -> Self {
        let code = match err.code {
            ErrorCode::ResourceNotFound if loading => "agent_has_no_record",
            ErrorCode::AuthRequired => "agent_not_logged_in",
            _ => "start_failed",
        };
        Self {
            code,
            message: err.to_string(),
        }
    }
}

/// What a `session/load` replayed: state updates to pass through, and the
/// unknown kinds that were dropped (ACP core §4.5).
#[derive(Default)]
struct Replay {
    kept: Vec<Value>,
    unknown: BTreeMap<String, usize>,
}

impl Replay {
    fn observe(&mut self, payload: Value) {
        let kind = payload["update"]["sessionUpdate"]
            .as_str()
            .unwrap_or("<none>")
            .to_string();
        if STATE_KINDS.contains(&kind.as_str()) {
            self.kept.push(payload);
        } else if !HISTORY_KINDS.contains(&kind.as_str()) {
            *self.unknown.entry(kind).or_default() += 1;
        }
    }

    /// The `host_note` for dropped unknown kinds, if any were dropped.
    fn note(&self) -> Option<SessionBody> {
        if self.unknown.is_empty() {
            return None;
        }
        let total: usize = self.unknown.values().sum();
        let kinds: Vec<String> = self.unknown.iter().map(|(k, n)| format!("{k} ×{n}")).collect();
        Some(SessionBody::HostNote {
            note: "replay_unknown_dropped".into(),
            text: scrub(&format!(
                "dropped {total} update(s) of unknown kind during session/load: {}",
                kinds.join(", ")
            )),
        })
    }
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
    /// The adapter's config options as last reported.
    catalogue: Mutex<Catalogue>,
}

/// What the actor knows of its adapter's config options.
#[derive(Default)]
struct Catalogue {
    options: Vec<SessionConfigOption>,
    /// `false` once a switch went unanswered or was answered without a
    /// catalogue: the options may be stale, so they are not announced
    /// until the adapter reports them again.
    current: bool,
}

impl Actor {
    /// The catalogue extracts to announce: none while the options may be
    /// stale.
    fn catalogue_extracts(&self) -> Indexed {
        let catalogue = self.catalogue.lock().expect("catalogue lock");
        if catalogue.current {
            catalogue_extracts(&catalogue.options)
        } else {
            Indexed::default()
        }
    }

    fn emit(&self, body: SessionBody) {
        if let Err(err) = self.uplink.emit(&self.session_id, body) {
            tracing::error!(session_id = %self.session_id, error = %err, "failed to persist a session frame");
        }
    }

    fn set_open_turn(&self, turn_id: Option<String>) {
        *self.open_turn.lock().expect("open turn lock") = turn_id;
    }

    fn start_failed(&self, request_id: String, error: StartError) {
        tracing::warn!(session_id = %self.session_id, code = error.code, message = %error.message, "session start failed");
        self.emit(SessionBody::StartFailed {
            request_id,
            code: error.code.into(),
            message: error.message,
        });
    }

    async fn run(self, launch: Launch, mut commands: mpsc::UnboundedReceiver<SessionCmd>) {
        self.drive(launch, &mut commands).await;
        // The session's last frame is in the outbox. Commands sent while the
        // actor was ending (killing its adapter can take the whole grace) are
        // answered, never dropped: the collector would otherwise wait out its
        // timeout. `close()` stops new sends from succeeding but does not
        // make a `try_recv` drain safe: `UnboundedSender::send` reserves its
        // slot (so the caller sees `Ok`/`true`) before it actually pushes the
        // value, so a send that reserved its slot just before `close()` can
        // still push after `close()` returns, while the queue looks empty to
        // `try_recv` in between — losing that command after its sender was
        // told it was queued. `recv().await` instead waits for the channel to
        // truly go empty (every already-permitted send observed) before
        // yielding `None`, so nothing sent before `close()` is lost.
        commands.close();
        while let Some(cmd) = commands.recv().await {
            match cmd {
                SessionCmd::Prompt { request_id, .. }
                | SessionCmd::Restart { request_id }
                | SessionCmd::Park { request_id }
                | SessionCmd::Close { request_id }
                | SessionCmd::Cancel { request_id, .. }
                | SessionCmd::SetConfig { request_id, .. } => {
                    // Also covers a start that reached this actor while it was
                    // tearing down (park/close/reap/adapter exit): the
                    // connection routed it to `Restart` because the handle
                    // was not yet `is_ended()`, but by the time this drain
                    // sees it the session really has ended. Answered here
                    // too, or the collector's start waiter (up to 90s) would
                    // eventually drop the whole host connection.
                    self.reject(request_id, "not_attached", "the session has ended on this host".into());
                }
            }
        }
    }

    /// The actor's life: start, then serve until it parks, closes or ends.
    /// Every return leaves the session's final frame in the outbox.
    async fn drive(&self, launch: Launch, commands: &mut mpsc::UnboundedReceiver<SessionCmd>) {
        let Launch {
            request_id,
            attach,
            config,
            agent,
            cwd,
            ..
        } = launch;
        let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
            Ok(spawned) => spawned,
            Err(err) => {
                return self.start_failed(request_id, StartError::other(format!("spawn {}: {err}", agent.program)));
            }
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
            return self.start_failed(
                request_id,
                StartError::other("the ACP connection could not be set up".into()),
            );
        };
        // One deadline for the whole start: the switches share it, so a slow
        // one cannot push the answer past the collector's start timeout.
        let deadline = Instant::now() + self.options.start_timeout;
        let started = tokio::select! {
            result = async {
                let (session, replay, catalogue) =
                    match tokio::time::timeout_at(deadline, negotiate(&conn, cwd, &attach, &mut updates)).await {
                        Ok(result) => result?,
                        Err(_) => {
                            return Err(StartError::other(format!(
                                "adapter did not start within {}s",
                                self.options.start_timeout.as_secs()
                            )));
                        }
                    };
                let applied = apply_config(&conn, &session, catalogue, &config, self.options.config_timeout, deadline).await;
                Ok((session, replay, applied))
            } => result,
            info = adapter.exited() => {
                let tail = adapter.stderr_tail().await;
                Err(StartError::other(format!("adapter exited during start ({}): {}", describe(info), last_lines(&tail, 5))))
            }
        };
        let (agent_session, replay, applied) = match started {
            Ok(started) => started,
            Err(error) => {
                adapter.terminate(self.options.kill_grace).await;
                return self.start_failed(request_id, error);
            }
        };
        *self.catalogue.lock().expect("catalogue lock") = Catalogue {
            options: applied.options,
            current: applied.current,
        };
        // The catalogue after the switches, never the one before (P-13).
        self.emit(SessionBody::SessionStarted {
            request_id,
            agent_session_id: agent_session.to_string(),
            indexed: self.catalogue_extracts(),
        });
        // A load's state updates follow the start they belong to, and so do
        // updates the adapter sent while the start's switches ran. They are
        // older than the catalogue just announced, so they carry no
        // catalogue extracts (P-13). Then the note about what the load
        // dropped (ACP core §4.5), then the one about switches that did not
        // take.
        let mut early = replay.kept.clone();
        while let Ok(payload) = updates.try_recv() {
            early.push(payload);
        }
        for payload in early {
            self.emit(update(payload, None));
        }
        if let Some(note) = replay.note() {
            self.emit(note);
        }
        if !applied.failures.is_empty() {
            let (note, what) = match attach {
                Attach::New => ("config_failed", "could not apply"),
                Attach::Load { .. } => ("reapply_failed", "could not re-apply"),
            };
            self.emit(SessionBody::HostNote {
                note: note.into(),
                text: scrub(&format!("{what}: {}", applied.failures.join("; "))),
            });
        }

        // Prompts are deduplicated by turn_id: a retried delivery after a
        // lost acknowledgement must never run the same turn twice. Only a
        // turn that actually started is recorded, so a corrected retry of a
        // rejected (invalid) prompt with the same turn_id still runs.
        let mut seen_turns = HashSet::new();
        let mut turn: Option<Turn> = None;
        // The reaper's clock: restarted when the session starts and when a
        // turn ends; it never runs while a turn (or a pending question
        // inside one) is in flight.
        let mut idle_since = Instant::now();
        let mut configs = PendingConfigs {
            uplink: self.uplink.clone(),
            queued: VecDeque::new(),
            out: None,
        };
        loop {
            let cancel_at = turn.as_ref().and_then(|t| t.cancel_deadline);
            tokio::select! {
                // Biased: adapter output already received is emitted before
                // the prompt reply it preceded on the wire.
                biased;
                Some(payload) = updates.recv() => {
                    self.emit(self.live_update(payload, turn.as_ref().map(|t| t.id.as_str())));
                }
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
                        turn = Some(Turn { id: turn_id, reply: Box::pin(reply), cancel_deadline: None });
                    }
                    Some(SessionCmd::Cancel { request_id, turn_id }) => match turn.as_mut() {
                        Some(running) if running.id == turn_id => {
                            // A repeated cancel changes nothing: the one
                            // already sent is still being honoured.
                            if running.cancel_deadline.is_none() {
                                if let Err(err) = conn.send_notification(CancelNotification::new(agent_session.clone())) {
                                    tracing::warn!(session_id = %self.session_id, error = %err, "session/cancel not sent");
                                }
                                running.cancel_deadline = Some(Instant::now() + self.options.cancel_grace);
                            }
                        }
                        // Not started here, or already ended: its end (if
                        // any) is in the outbox ahead of this answer.
                        _ => self.reject(request_id, "not_running", "that turn is not running".into()),
                    },
                    Some(SessionCmd::Restart { request_id }) => self.emit(SessionBody::SessionStarted {
                        request_id,
                        agent_session_id: agent_session.to_string(),
                        indexed: self.catalogue_extracts(),
                    }),
                    // A switch may run during a turn; it is answered in order.
                    Some(SessionCmd::SetConfig { request_id, config_id, value }) => match self.check_switch(&config_id, &value) {
                        Err((code, message)) => self.reject(request_id, code, message),
                        Ok(()) => {
                            let deadline = Instant::now() + self.options.config_timeout;
                            configs.queued.push_back(QueuedSwitch { request_id, config_id, value, deadline });
                            self.send_next_switch(&conn, &agent_session, &mut configs);
                        }
                    },
                    Some(SessionCmd::Park { .. }) => {
                        self.teardown(&mut adapter, &mut updates, turn.take()).await;
                        return self.emit(SessionBody::SessionParked { reason: ParkReason::Operator });
                    }
                    Some(SessionCmd::Close { .. }) => {
                        self.teardown(&mut adapter, &mut updates, turn.take()).await;
                        return self.emit(SessionBody::SessionClosed);
                    }
                },
                result = next_reply(&mut turn) => {
                    let ended = turn.take().expect("a reply implies a turn");
                    let cancelling = ended.cancel_deadline.is_some();
                    idle_since = Instant::now();
                    // Closes the race described on `drain_updates`: a late
                    // update that arrived just as the reply resolved must be
                    // emitted before this turn's `turn_ended`.
                    self.drain_updates(&mut updates, Some(&ended.id));
                    match result {
                        // The agent reports a cancelled turn by its stop
                        // reason (ACP): anything else finished first.
                        Ok(response) => {
                            let outcome = if response.stop_reason == StopReason::Cancelled {
                                TurnOutcome::Cancelled
                            } else {
                                TurnOutcome::Completed
                            };
                            self.end_turn(ended.id, outcome, stop_reason(&response), None);
                        }
                        Err(err) => {
                            let exited = adapter.exited_within(EXIT_SETTLE).await;
                            // Updates that arrived during the wait above are
                            // also ahead of this turn's end.
                            self.drain_updates(&mut updates, Some(&ended.id));
                            if let Some(info) = exited {
                                return self.adapter_exited(info, &mut adapter, &mut updates, Some(ended)).await;
                            }
                            // An agent may answer an aborted prompt with an
                            // error instead of `cancelled`.
                            let outcome = if cancelling { TurnOutcome::Cancelled } else { TurnOutcome::Failed };
                            self.end_turn(ended.id, outcome, None, Some(err.to_string()));
                        }
                    }
                }
                result = next_config(&mut configs) => {
                    let (request_id, _) = configs.out.take().expect("an answer implies a switch");
                    // Updates the adapter sent before its answer come first.
                    self.drain_updates(&mut updates, turn.as_ref().map(|t| t.id.as_str()));
                    self.config_answered(request_id, result);
                    self.send_next_switch(&conn, &agent_session, &mut configs);
                }
                _ = cancel_deadline(cancel_at) => {
                    let unanswered = turn.take().expect("a deadline implies a turn");
                    return self.stop_after_unanswered_cancel(&mut adapter, &mut updates, unanswered).await;
                }
                _ = idle_deadline(self.options.idle_timeout, idle_since), if turn.is_none() => {
                    tracing::info!(session_id = %self.session_id, "reaping idle session");
                    self.teardown(&mut adapter, &mut updates, None).await;
                    return self.emit(SessionBody::SessionParked { reason: ParkReason::Idle });
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

    /// Refuse a `set_config` the adapter could not take: an option it does
    /// not offer (`unknown_option`), or a value of the wrong kind for it
    /// (`invalid`). Whether a select offers the value is the adapter's call.
    fn check_switch(&self, config_id: &str, value: &ConfigValue) -> Result<(), (&'static str, String)> {
        let catalogue = self.catalogue.lock().expect("catalogue lock");
        let Some(option) = catalogue.options.iter().find(|o| &*o.id.0 == config_id) else {
            return Err(("unknown_option", format!("the adapter offers no option {config_id}")));
        };
        match (&option.kind, value) {
            (SessionConfigKind::Select(_), ConfigValue::Id(_))
            | (SessionConfigKind::Boolean(_), ConfigValue::Bool(_)) => Ok(()),
            _ => Err(("invalid", format!("{config_id} takes a different kind of value"))),
        }
    }

    /// Answer a `set_config` from the adapter's answer: `config_applied`
    /// with the catalogue it answered with (none if it answered without
    /// one), or `config_failed`.
    /// Send the oldest waiting switch, unless one is out. A switch whose
    /// deadline passed while it waited is answered `config_failed`.
    fn send_next_switch(&self, conn: &ConnectionTo<Agent>, session: &SessionId, configs: &mut PendingConfigs) {
        while configs.out.is_none()
            && let Some(next) = configs.queued.pop_front()
        {
            if Instant::now() >= next.deadline {
                let message = "an earlier switch is still out".to_string();
                self.reject(next.request_id, "config_failed", message);
                continue;
            }
            let request = SetSessionConfigOptionRequest::new(session.clone(), next.config_id, acp_value(&next.value));
            let reply = tokio::time::timeout_at(next.deadline, conn.send_request(request).block_task());
            configs.out = Some((next.request_id, Box::pin(reply)));
        }
    }

    fn config_answered(
        &self,
        request_id: String,
        result: Result<agent_client_protocol::Result<SetSessionConfigOptionResponse>, tokio::time::error::Elapsed>,
    ) {
        match result {
            Ok(Ok(response)) => {
                {
                    let mut catalogue = self.catalogue.lock().expect("catalogue lock");
                    if response.config_options.is_empty() {
                        catalogue.current = false;
                    } else {
                        *catalogue = Catalogue {
                            options: response.config_options,
                            current: true,
                        };
                    }
                }
                self.emit(SessionBody::ConfigApplied {
                    request_id,
                    indexed: self.catalogue_extracts(),
                });
            }
            Ok(Err(err)) => self.reject(request_id, "config_failed", err.to_string()),
            Err(_) => {
                self.catalogue.lock().expect("catalogue lock").current = false;
                let timeout = self.options.config_timeout;
                let message = format!("no answer within {timeout:?} of the request");
                self.reject(request_id, "config_failed", message);
            }
        }
    }

    /// A live adapter notification as a session frame. A
    /// `config_option_update` (the agent changed its own config, e.g. left
    /// plan mode) replaces the catalogue and carries its extracts.
    fn live_update(&self, payload: Value, turn: Option<&str>) -> SessionBody {
        let mut body = update(payload, turn);
        if let SessionBody::AcpUpdate { indexed, payload } = &mut body
            && let Some(options) = config_update(payload)
        {
            *self.catalogue.lock().expect("catalogue lock") = Catalogue { options, current: true };
            let catalogue = self.catalogue_extracts();
            indexed.config_options = catalogue.config_options;
            indexed.current_model = catalogue.current_model;
            indexed.current_mode = catalogue.current_mode;
            indexed.current_axes = catalogue.current_axes;
        }
        body
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

    /// Emit whatever updates are already queued, without waiting, as part of
    /// `turn` (if any). The ACP connection task and this actor run on
    /// different worker threads: it can push a notification and then resolve
    /// the matching reply in quick succession, and this actor's `select!` can
    /// observe the reply as ready before it happens to observe the
    /// notification, in the same poll. ACP delivers messages in order, so the
    /// notification's send always completes-before the reply resolves — a
    /// non-blocking drain right before acting on a reply (or before ending a
    /// torn-down turn) is therefore guaranteed to see it, closing that
    /// ordering gap.
    fn drain_updates(&self, updates: &mut mpsc::UnboundedReceiver<Value>, turn: Option<&str>) {
        while let Ok(payload) = updates.try_recv() {
            self.emit(self.live_update(payload, turn));
        }
    }

    /// Park or close: forward any output already queued, end the turn as
    /// interrupted, then kill the group.
    async fn teardown(&self, adapter: &mut Adapter, updates: &mut mpsc::UnboundedReceiver<Value>, turn: Option<Turn>) {
        self.drain_updates(updates, turn.as_ref().map(|t| t.id.as_str()));
        if let Some(turn) = turn {
            self.end_turn(turn.id, TurnOutcome::Interrupted, None, None);
        }
        adapter.terminate(self.options.kill_grace).await;
    }

    /// The adapter kept running a turn past `cancel_grace` after
    /// `session/cancel`. It cannot take another prompt while that one runs
    /// (one turn at a time, ACP core §4.4), so it is stopped: the turn ends
    /// `cancelled` (what the operator asked for), a `host_note` says why,
    /// and the session parks.
    async fn stop_after_unanswered_cancel(
        &self,
        adapter: &mut Adapter,
        updates: &mut mpsc::UnboundedReceiver<Value>,
        turn: Turn,
    ) {
        let grace = self.options.cancel_grace;
        tracing::warn!(session_id = %self.session_id, ?grace, "adapter ignored session/cancel; stopping it");
        self.drain_updates(updates, Some(&turn.id));
        let message = format!("the adapter did not stop within {grace:?} of session/cancel");
        self.end_turn(turn.id, TurnOutcome::Cancelled, None, Some(message.clone()));
        adapter.terminate(self.options.kill_grace).await;
        // No `adapter_exited` follows a stop the host asked for, so the
        // note keeps the adapter's last words (already scrubbed, bounded).
        let tail = last_lines(&adapter.stderr_tail().await, 5);
        let text = if tail.is_empty() {
            format!("{message}; it was stopped")
        } else {
            format!("{message}; it was stopped. Last stderr:\n{tail}")
        };
        self.emit(SessionBody::HostNote {
            note: "cancel_unanswered".into(),
            text: scrub(&text),
        });
        self.emit(SessionBody::SessionParked {
            reason: ParkReason::Operator,
        });
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
            self.emit(self.live_update(payload, turn.as_ref().map(|t| t.id.as_str())));
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

/// Resolves when the idle window since `since` has passed; never if the
/// reaper is off.
async fn idle_deadline(window: Option<Duration>, since: Instant) {
    match window {
        Some(window) => tokio::time::sleep_until(since + window).await,
        None => std::future::pending().await,
    }
}

/// Resolves when a cancelled turn's grace is up; never if no cancel is out.
async fn cancel_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// The answer to the `set_config` that is out, or never if none is.
async fn next_config(
    configs: &mut PendingConfigs,
) -> Result<agent_client_protocol::Result<SetSessionConfigOptionResponse>, tokio::time::error::Elapsed> {
    match configs.out.as_mut() {
        Some((_, reply)) => reply.await,
        None => std::future::pending().await,
    }
}

/// The in-flight prompt's reply, or never if no turn is running.
async fn next_reply(turn: &mut Option<Turn>) -> agent_client_protocol::Result<PromptResponse> {
    match turn {
        Some(turn) => (&mut turn.reply).await,
        None => std::future::pending().await,
    }
}

/// `initialize`, then `session/new` or `session/load`. While a load is
/// outstanding its replay is classified, never emitted (ACP core §4.5).
/// Returns the config options the adapter announced (none if it announced
/// none, or they did not parse).
async fn negotiate(
    conn: &ConnectionTo<Agent>,
    cwd: PathBuf,
    attach: &Attach,
    updates: &mut mpsc::UnboundedReceiver<Value>,
) -> Result<(SessionId, Replay, Announced), StartError> {
    // Advertised so that agents offer boolean options as booleans, not as
    // on/off selects (ACP `session.configOptions.boolean`).
    let capabilities = ClientCapabilities::new().session(
        ClientSessionCapabilities::new()
            .config_options(SessionConfigOptionsCapabilities::new().boolean(BooleanConfigOptionCapabilities::new())),
    );
    let init = conn
        .send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(capabilities))
        .block_task()
        .await
        .map_err(|err| StartError::acp(err, false))?;
    let agent_session_id = match attach {
        Attach::New => {
            let created = conn
                .send_request(NewSessionRequest::new(cwd))
                .block_task()
                .await
                .map_err(|err| StartError::acp(err, false))?;
            // What the adapter sent before its answer (see
            // `Actor::drain_updates` for the ordering argument) follows the
            // start, like a load's kept updates.
            let mut replay = Replay::default();
            for _ in 0..updates.len() {
                match updates.try_recv() {
                    Ok(payload) => replay.kept.push(payload),
                    Err(_) => break,
                }
            }
            let catalogue = announced_options(created.config_options, &replay.kept);
            return Ok((created.session_id, replay, catalogue));
        }
        Attach::Load { agent_session_id } => agent_session_id,
    };
    if !init.agent_capabilities.load_session {
        return Err(StartError {
            code: "load_unsupported",
            message: "the adapter does not support session/load".into(),
        });
    }
    let id = SessionId::new(agent_session_id.clone());
    let load = conn.send_request(LoadSessionRequest::new(id.clone(), cwd)).block_task();
    tokio::pin!(load);
    let mut replay = Replay::default();
    loop {
        tokio::select! {
            biased;
            Some(payload) = updates.recv() => replay.observe(payload),
            result = &mut load => {
                // Everything the adapter sent before its answer is replay,
                // even if this select saw the answer first (see
                // `Actor::drain_updates` for the ordering argument). The
                // drain cannot tell "before" from "just after", though: a
                // live notification the adapter sends microseconds after
                // its load answer, already queued when this drain runs, is
                // classified as replay too — dropped if a history kind,
                // passed through if a state kind.
                while let Ok(payload) = updates.try_recv() {
                    replay.observe(payload);
                }
                let loaded = result.map_err(|err| StartError::acp(err, true))?;
                let catalogue = announced_options(loaded.config_options, &replay.kept);
                return Ok((id, replay, catalogue));
            }
        }
    }
}

/// The config options a new or loaded session starts with, and whether they
/// are trustworthy for judging a switch's effect.
struct Announced {
    options: Vec<SessionConfigOption>,
    /// `true` only for the adapter's own `session/new` | `session/load`
    /// answer. `false` for a catalogue seeded from a pre-answer
    /// `config_option_update` (decision 4, amended) or for no catalogue at
    /// all: it may be stale or incomplete, so it must not gate whether a
    /// switch is skipped or judged to not exist — only send can tell.
    authoritative: bool,
}

/// The config options a new or loaded session starts with: the answer's,
/// or, if it had none, those of the last `config_option_update` the adapter
/// sent before it. That update still carries no extracts: it is older than
/// the catalogue `session_started` announces.
fn announced_options(answered: Option<Vec<SessionConfigOption>>, before: &[Value]) -> Announced {
    match answered {
        Some(options) if !options.is_empty() => Announced {
            options,
            authoritative: true,
        },
        _ => Announced {
            options: before.iter().rev().find_map(config_update).unwrap_or_default(),
            authoritative: false,
        },
    }
}

/// The options of a non-empty `config_option_update` notification.
fn config_update(payload: &Value) -> Option<Vec<SessionConfigOption>> {
    let notification = serde_json::from_value::<SessionNotification>(payload.clone()).ok()?;
    match notification.update {
        SessionUpdate::ConfigOptionUpdate(update) if !update.config_options.is_empty() => Some(update.config_options),
        _ => None,
    }
}

/// What a start's config switches left behind.
struct Applied {
    /// The options the last switch answered with (or the announced ones).
    options: Vec<SessionConfigOption>,
    /// Whether `options` is trustworthy right now: `false` to start with a
    /// catalogue seeded from a `config_option_update` (decision 4) rather
    /// than an adapter answer, and whenever a switch went unanswered or was
    /// answered without a catalogue; `true` again after any switch answers
    /// with a fresh, non-empty one. Gates the "already current" skip and
    /// the "no such option" judgement (never a false "does not need
    /// sending"), and the final read-back mismatch check and announcement.
    current: bool,
    /// One line per requested value that did not take.
    failures: Vec<String>,
}

/// Apply `wanted` to a new or loaded session (ACP core §4.3): the model
/// first, then the other axes, then the mode, each with
/// `session/set_config_option`, so a model that clamps the mode cannot undo
/// the requested mode. A value already current in an authoritative
/// catalogue is not sent — a stale or seeded one (`catalogue.authoritative`
/// false) never gates that decision, so its switch is sent regardless (a
/// redundant switch is harmless; a wrongly skipped one lies). Each switch
/// has `timeout`, and none runs past `deadline`.
///
/// Once a switch goes unanswered (or the deadline has passed), nothing more
/// is sent: the adapter may still apply the late switch, and a late model
/// switch could clamp a mode sent after it. Finally every requested value is
/// checked against the read-back. Whatever did not take is reported in
/// `failures`; the start goes on.
async fn apply_config(
    conn: &ConnectionTo<Agent>,
    session: &SessionId,
    catalogue: Announced,
    wanted: &SessionConfig,
    timeout: Duration,
    deadline: Instant,
) -> Applied {
    let mut applied = Applied {
        options: catalogue.options,
        current: catalogue.authoritative,
        failures: Vec::new(),
    };
    let mut switches: Vec<(String, ConfigValue)> = Vec::new();
    if let Some(model) = &wanted.model {
        match axis_id(&applied.options, &SessionConfigOptionCategory::Model, "model") {
            Some(id) => switches.push((id, ConfigValue::Id(model.clone()))),
            None => applied
                .failures
                .push(format!("model={model}: the adapter offers no model option")),
        }
    }
    switches.extend(wanted.axes.iter().map(|(id, value)| (id.clone(), value.clone())));
    // Resolved now, and applied last: after the model and the other axes.
    let mode = wanted.mode.as_ref().map(|mode| {
        (
            axis_id(&applied.options, &SessionConfigOptionCategory::Mode, "mode"),
            mode,
        )
    });
    match mode {
        Some((Some(id), mode)) => switches.push((id, ConfigValue::Id(mode.clone()))),
        Some((None, mode)) => applied
            .failures
            .push(format!("mode={mode}: the adapter offers no mode option")),
        None => {}
    }
    // Requested values that already have their own line in `failures`.
    let mut reported: HashSet<String> = HashSet::new();
    let mut unanswered = false;
    for (id, value) in &switches {
        // Only an authoritative catalogue may decide "already current" or
        // "no such option": a stale or seeded one is not proof of either, so
        // its switch is sent regardless.
        if applied.current {
            match current_value(&applied.options, id) {
                None => {
                    applied
                        .failures
                        .push(format!("{id}: the adapter offers no such option"));
                    reported.insert(id.clone());
                    continue;
                }
                Some(current) if current == *value => continue,
                Some(_) => {}
            }
        }
        let now = Instant::now();
        if unanswered || now >= deadline {
            let why = if unanswered {
                "an earlier switch did not answer"
            } else {
                "the start deadline passed"
            };
            applied.failures.push(format!("{id}={}: not sent: {why}", shown(value)));
            reported.insert(id.clone());
            continue;
        }
        let request = SetSessionConfigOptionRequest::new(session.clone(), id.clone(), acp_value(value));
        let cut_short = now + timeout > deadline;
        let until = (now + timeout).min(deadline);
        match tokio::time::timeout_at(until, conn.send_request(request).block_task()).await {
            // The catalogue of the last successful switch is the one kept,
            // and it is a genuine, fresh read-back: authoritative again for
            // whatever runs after it, even if an earlier switch in this same
            // start was not (a seeded catalogue, or an earlier empty answer).
            Ok(Ok(response)) if !response.config_options.is_empty() => {
                applied.options = response.config_options;
                applied.current = true;
            }
            // An empty answer is no read-back, not an adapter without options.
            Ok(Ok(_)) => applied.current = false,
            Ok(Err(err)) => {
                applied.failures.push(format!("{id}={}: {err}", shown(value)));
                reported.insert(id.clone());
            }
            Err(_) => {
                applied.current = false;
                unanswered = true;
                let wait = if cut_short {
                    "no answer before the start deadline".to_string()
                } else {
                    format!("no answer within {timeout:?}")
                };
                applied.failures.push(format!("{id}={}: {wait}", shown(value)));
                reported.insert(id.clone());
            }
        }
    }
    // What the agent reports is what counts: a value it accepted but does
    // not report (or a later switch undid) did not take either.
    if applied.current {
        for (id, value) in switches.iter().filter(|(id, _)| !reported.contains(id)) {
            match current_value(&applied.options, id) {
                Some(current) if current == *value => {}
                Some(current) => applied.failures.push(format!(
                    "{id}: asked {}, agent reports {}",
                    shown(value),
                    shown(&current)
                )),
                None => applied
                    .failures
                    .push(format!("{id}: asked {}, agent no longer offers it", shown(value))),
            }
        }
    }
    applied
}

/// The id of the option for `category`. Categories are only a UX hint in
/// ACP, so an option with the conventional id counts too, but only if it
/// has no category, or a custom (`_`-prefixed) one: an option another
/// category claims is not the model or the mode.
fn axis_id(options: &[SessionConfigOption], category: &SessionConfigOptionCategory, id: &str) -> Option<String> {
    let uncategorized = |o: &&SessionConfigOption| match &o.category {
        None => true,
        Some(SessionConfigOptionCategory::Other(custom)) => custom.starts_with('_'),
        Some(_) => false,
    };
    options
        .iter()
        .find(|o| o.category.as_ref() == Some(category))
        .or_else(|| options.iter().filter(uncategorized).find(|o| &*o.id.0 == id))
        .map(|o| o.id.to_string())
}

/// The current value of option `id`, if the adapter offers it.
fn current_value(options: &[SessionConfigOption], id: &str) -> Option<ConfigValue> {
    let option = options.iter().find(|o| &*o.id.0 == id)?;
    match &option.kind {
        SessionConfigKind::Select(select) => Some(ConfigValue::Id(select.current_value.to_string())),
        SessionConfigKind::Boolean(toggle) => Some(ConfigValue::Bool(toggle.current_value)),
        _ => None,
    }
}

fn acp_value(value: &ConfigValue) -> SessionConfigOptionValue {
    match value {
        ConfigValue::Bool(on) => SessionConfigOptionValue::boolean(*on),
        ConfigValue::Id(id) => SessionConfigOptionValue::value_id(id.clone()),
    }
}

fn shown(value: &ConfigValue) -> String {
    match value {
        ConfigValue::Bool(on) => on.to_string(),
        ConfigValue::Id(id) => id.clone(),
    }
}

/// The catalogue extracts (ACP core §3.2) of `options`: the options
/// themselves, and the current model, mode and other axes. None for an
/// empty list, which is no read-back.
fn catalogue_extracts(options: &[SessionConfigOption]) -> Indexed {
    if options.is_empty() {
        return Indexed::default();
    }
    let model = axis_id(options, &SessionConfigOptionCategory::Model, "model");
    let mode = axis_id(options, &SessionConfigOptionCategory::Mode, "mode");
    let id_of = |id: &Option<String>| id.as_deref().and_then(|id| current_value(options, id));
    let as_id = |value: Option<ConfigValue>| match value {
        Some(ConfigValue::Id(id)) => Some(id),
        _ => None,
    };
    let axes = options
        .iter()
        .map(|o| o.id.to_string())
        .filter(|id| Some(id) != model.as_ref() && Some(id) != mode.as_ref())
        .filter_map(|id| current_value(options, &id).map(|value| (id, value)))
        .collect();
    Indexed {
        config_options: Some(options.iter().filter_map(|o| serde_json::to_value(o).ok()).collect()),
        current_model: as_id(id_of(&model)),
        current_mode: as_id(id_of(&mode)),
        current_axes: Some(axes),
        ..Indexed::default()
    }
}

/// An adapter notification as a session frame; `turn` is the turn in flight
/// when it arrived (the `turn_id` extract, ACP core §3.2).
fn update(payload: Value, turn: Option<&str>) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed {
            turn_id: turn.map(str::to_string),
            ..Indexed::default()
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn options(json: Value) -> Vec<SessionConfigOption> {
        serde_json::from_value(json).unwrap()
    }

    fn select(id: &str, category: Option<&str>) -> Value {
        let mut option = serde_json::json!({
            "id": id, "name": id, "type": "select", "currentValue": "a",
            "options": [{"value": "a", "name": "a"}]
        });
        if let Some(category) = category {
            option["category"] = category.into();
        }
        option
    }

    #[test]
    fn the_model_is_its_category_or_else_an_uncategorized_option_named_model() {
        let model = SessionConfigOptionCategory::Model;
        let pick = |json| axis_id(&options(json), &model, "model");
        let categorized = serde_json::json!([select("model", Some("thought_level")), select("brain", Some("model"))]);
        assert_eq!(pick(categorized).as_deref(), Some("brain"));
        assert_eq!(
            pick(serde_json::json!([select("model", None)])).as_deref(),
            Some("model")
        );
        assert_eq!(
            pick(serde_json::json!([select("model", Some("_mine"))])).as_deref(),
            Some("model")
        );
        // Another category claims it, even one this build does not know.
        assert_eq!(pick(serde_json::json!([select("model", Some("thought_level"))])), None);
        assert_eq!(pick(serde_json::json!([select("model", Some("future"))])), None);
    }
}
