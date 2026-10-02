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
use agent_client_protocol::schema::v1::{
    BooleanConfigOptionCapabilities, CancelNotification, ClientCapabilities, ClientSessionCapabilities, ContentBlock,
    ElicitationCapabilities, ElicitationFormCapabilities, InitializeRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, PromptResponse, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigOptionValue, SessionConfigOptionsCapabilities, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
};
use agent_client_protocol::schema::{MaybeUndefined, ProtocolVersion};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode, Responder, UntypedMessage};
use hennery_proto::frames::{
    ConfigValue, ElicitationAction, HostFrame, Indexed, ParkReason, PendingExtract, PendingKind, PendingReason,
    PendingResolution, SessionBody, SessionConfig, TurnOutcome,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

#[cfg(feature = "test-hooks")]
pub mod test_hooks;

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

/// At most this many adapter updates are emitted in a row before the actor's
/// other arms (commands, the prompt's reply, the cancel grace) get a turn:
/// an adapter streaming faster than the outbox writes must not delay a
/// cancel until its turn is over.
const UPDATE_BURST: usize = 64;

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
    /// The operator's answer to one of the adapter's questions: answered by
    /// `answer_result` (ACP core §4.6).
    Answer {
        request_id: String,
        pending_id: String,
        answer: Answer,
    },
}

/// An operator's answer to a pending request, as the connection hands it on.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// One of a permission request's options.
    Permission { option_id: String },
    /// An elicitation's action; `content` only when it accepts.
    Elicitation {
        action: ElicitationAction,
        content: Option<Value>,
    },
}

impl Answer {
    fn kind(&self) -> PendingKind {
        match self {
            Answer::Permission { .. } => PendingKind::Permission,
            Answer::Elicitation { .. } => PendingKind::Elicitation,
        }
    }

    /// The ACP response to the adapter's request.
    fn response(&self) -> Value {
        match self {
            Answer::Permission { option_id } => {
                serde_json::json!({"outcome": {"outcome": "selected", "optionId": option_id}})
            }
            Answer::Elicitation { action, content } => {
                let mut response = serde_json::json!({ "action": action });
                if let (ElicitationAction::Accept, Some(content)) = (action, content) {
                    response["content"] = content.clone();
                }
                response
            }
        }
    }
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
    /// `git` for the git probe (ACP core §7), found once when the host
    /// starts (`git::find_git`); `None`: no probe, no `git_state`.
    pub git: Option<PathBuf>,
    /// Test seams (see `test_hooks`).
    #[cfg(feature = "test-hooks")]
    pub test_hooks: Option<test_hooks::TestHooks>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            start_timeout: START_TIMEOUT,
            kill_grace: KILL_GRACE,
            idle_timeout: Some(IDLE_TIMEOUT),
            cancel_grace: CANCEL_GRACE,
            config_timeout: CONFIG_TIMEOUT,
            git: None,
            #[cfg(feature = "test-hooks")]
            test_hooks: None,
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
    /// This actor will serve no further start or resume: a park or close
    /// has been queued (even before the actor has read it), or the actor
    /// has begun ending by itself.
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

    /// The actor is ending: a park or close has been queued through `send`,
    /// or the actor began ending by itself (an idle reap, an adapter exit, an
    /// adapter stopped for ignoring a cancel) and may still be killing its
    /// adapter. A `Restart` sent now would be answered `not_attached` once
    /// the actor gets to it, so a start or resume waits for `finished` and
    /// attaches a fresh adapter instead.
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
    let ending = Arc::new(AtomicBool::new(false));
    let actor = Actor {
        uplink,
        session_id: launch.session_id.clone(),
        open_turn: open_turn.clone(),
        options,
        catalogue: Mutex::new(Catalogue::default()),
        questions: Mutex::new(Questions::default()),
        inbound: OnceLock::new(),
        probe: Mutex::new(None),
        base_probe: Mutex::new(None),
        ending: ending.clone(),
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
        ending,
    }
}

type Reply = Pin<Box<dyn Future<Output = agent_client_protocol::Result<PromptResponse>> + Send>>;

/// One message from the ACP connection task to the actor: a live adapter
/// notification (forwarded verbatim), or a config switch's answer. Both are
/// delivered into the same channel, from the same connection-managed
/// dispatch loop, so `updates.recv()` yields them in true wire order (fix
/// round 2, F2). A switch's answer is routed here — via an ordered
/// `on_receiving_result` callback, never `block_task` (used everywhere
/// else in this file) — precisely because `block_task` does not hold up
/// the dispatch loop, so it gives no ordering guarantee relative to
/// notifications the adapter sends right before or after answering.
enum Inbound {
    Update(Value),
    SwitchAnswer {
        token: u64,
        result: agent_client_protocol::Result<SetSessionConfigOptionResponse>,
    },
    /// A question for the operator, in wire order with the notifications
    /// around it: it follows the tool call it asks about.
    Question(Box<Question>),
    /// The adapter withdrew question `pending_id` (`$/cancel_request`).
    QuestionWithdrawn {
        pending_id: String,
    },
    /// What a git probe found (ACP core §7); `base`: the first after a new
    /// session's start, so `head` is the commit it started from.
    Git {
        state: crate::git::GitState,
        base: bool,
    },
}

/// A `session/request_permission` or `elicitation/create` from the adapter
/// (ACP core §2.5). Its responder must be kept until the question is
/// answered or cancelled: a dropped responder sends nothing, and the adapter
/// would wait for good.
struct Question {
    kind: PendingKind,
    params: Value,
    responder: Responder<Value>,
}

/// The kind of question an adapter request is, if it is one.
fn question_kind(method: &str) -> Option<PendingKind> {
    match method {
        "session/request_permission" => Some(PendingKind::Permission),
        "elicitation/create" => Some(PendingKind::Elicitation),
        _ => None,
    }
}

/// A permission request's option ids, read from its raw params (ACP core
/// §3.2): every string `options[i].optionId`, skipping an entry without
/// one. A typed parse would fail on a single option of a kind this build
/// does not know (`PermissionOptionKind` has no catch-all), and then no
/// answer could be validated. `None` only if `options` is missing or not
/// an array.
fn option_ids(params: &Value) -> Option<Vec<String>> {
    let options = params.get("options")?.as_array()?;
    Some(
        options
            .iter()
            .filter_map(|option| option.get("optionId")?.as_str().map(str::to_string))
            .collect(),
    )
}

/// The most a question's title carries to the collector, in characters.
const MAX_QUESTION_TITLE: usize = 200;

/// What a question is about, as the agent put it (plan 10b): a permission's
/// tool call title, an elicitation's message, cut to
/// `MAX_QUESTION_TITLE` characters. Read from the raw request, as its
/// option ids are; the collector puts it on one line.
fn question_title(kind: PendingKind, params: &Value) -> Option<String> {
    let title = match kind {
        PendingKind::Permission => params.pointer("/toolCall/title"),
        PendingKind::Elicitation => params.get("message"),
    }?
    .as_str()?;
    let title: String = title.chars().take(MAX_QUESTION_TITLE).collect();
    (!title.trim().is_empty()).then_some(title)
}

/// The adapter's questions waiting for the operator (ACP core §4.6), oldest
/// first. No timeout: each waits until it is answered or cancelled.
#[derive(Default)]
struct Questions {
    open: Vec<OpenQuestion>,
    /// The turn a `session/cancel` went out for: a question it asks from
    /// then on is cancelled as soon as it opens.
    cancelled_turn: Option<String>,
}

/// The ACP answer that tells the adapter a question is off: a permission's
/// `cancelled` outcome, an elicitation's `cancel` action.
fn cancelled_response(kind: PendingKind) -> Value {
    match kind {
        PendingKind::Permission => serde_json::json!({"outcome": {"outcome": "cancelled"}}),
        PendingKind::Elicitation => serde_json::json!({"action": "cancel"}),
    }
}

struct OpenQuestion {
    pending_id: String,
    kind: PendingKind,
    responder: Responder<Value>,
    /// Watches for the adapter withdrawing the question; stops with it.
    _withdrawal: Option<Watcher>,
}

/// A task that is aborted when its owner is dropped.
struct Watcher(Option<tokio::task::JoinHandle<()>>);

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

/// A `set_config` received and not sent yet.
struct QueuedSwitch {
    request_id: String,
    config_id: String,
    value: ConfigValue,
    /// Receipt plus `config_timeout`: past it, the switch is answered
    /// `config_failed`, sent or not.
    deadline: Instant,
}

/// A switch sent to the adapter, awaiting its answer. The answer itself is
/// never held here (fix round 2): it arrives as an `Inbound::SwitchAnswer`
/// matched to this switch by `token`.
struct OutSwitch {
    request_id: String,
    token: u64,
    /// When it was sent: an orphan (below) is given up on relative to this,
    /// not to `deadline` (which is relative to receipt, decision 6).
    sent_at: Instant,
    /// Receipt plus `config_timeout`: past it while still out, the switch
    /// becomes an orphan (below) — unless its answer has already been
    /// flagged (`PendingConfigs::answered_token`), in which case
    /// `out_deadline` disarms it (sets this to `None`) instead: the answer
    /// is already in flight on the wire-ordered channel, so no drain is
    /// needed to find it, and a cleared deadline can never re-fire in a
    /// busy loop the way an unchanged, already-past one would.
    deadline: Option<Instant>,
}

/// A timed-out switch may still answer, and decision 6 forbids ever having
/// two requests out at once (the real adapters handle them concurrently, so
/// a late one could land after, and clamp, one sent behind it): fix round 1
/// (F1) tracks it by its `token` instead of sending anything else behind
/// it. Its own requester has already been answered `config_failed`; a late
/// answer (matched by `token`, fix round 2) with a non-empty read-back
/// updates the catalogue and is announced as a `config_applied` under its
/// original `request_id` (final review I2), so the collector stores what
/// the agent now runs with — it has no waiter left for that id, so this is
/// never a second answer. Given up on, unanswered, `config_timeout *
/// ORPHAN_GRACE` after it was sent, in case the adapter never answers at
/// all — the catalogue then simply stays not current, as it already was
/// made at the timeout — unless its answer is already flagged
/// (`PendingConfigs::answered_token`), in which case the grace is disarmed
/// instead, exactly like `OutSwitch::deadline`: the answer is already on
/// the wire-ordered channel and is handled (and announced) in order. A
/// start switch that got no answer in time is an
/// orphan too (final review I1), with no `request_id` and `NO_TOKEN`: its
/// answer was dropped with the start, so only its grace clears it.
struct Orphan {
    token: u64,
    request_id: Option<String>,
    /// `None` once disarmed (its answer is already queued): a cleared grace
    /// can never re-fire in a busy loop.
    drop_after: Option<Instant>,
}

/// How many `config_timeout`s an orphaned switch is still tracked for.
const ORPHAN_GRACE: u32 = 4;

/// The actor's `set_config` switches: at most one is ever out (`out`) or
/// orphaned (`orphan`) at a time, never both, and nothing in `queued` is
/// sent while either is set — the rest wait, in the order they came.
/// Everything still queued or out when the actor ends is answered
/// `not_attached` then, after its last fact: the collector would otherwise
/// wait out its timeout and drop the whole host connection. An orphan is
/// not answered again on drop: its requester already has its answer.
struct PendingConfigs {
    uplink: Uplink,
    queued: VecDeque<QueuedSwitch>,
    out: Option<OutSwitch>,
    orphan: Option<Orphan>,
    /// The token the next switch sent will carry; `Inbound::SwitchAnswer`
    /// is matched against `out`/`orphan` by this, never by arrival order
    /// (fix round 2).
    next_token: u64,
    /// The token of the switch whose answer has begun arriving: set (fix
    /// round 2) inside `send_next_switch`'s `on_receiving_result` callback,
    /// *before* the answer is pushed onto the wire-ordered `updates`
    /// channel — so once `out_deadline` observes a match, the answer is
    /// already either in that channel or about to be, and no drain is
    /// needed to find it (a flooding adapter's own backlog must never hold
    /// up `out_deadline` itself, or it becomes another way to hold off a
    /// cancel). Set from the connection task, possibly a different worker
    /// thread, hence atomic. `NO_TOKEN` (never issued) until the first
    /// answer starts arriving.
    answered_token: Arc<AtomicU64>,
}

/// Sentinel for `PendingConfigs::answered_token`: no switch's answer has
/// begun arriving yet. Tokens are issued from 1 upward, so this can never
/// collide with a real one, and it is below every real one, so the flag can
/// only ever be raised (`fetch_max`, final review M1).
const NO_TOKEN: u64 = 0;

impl Drop for PendingConfigs {
    fn drop(&mut self) {
        let out = self.out.take().map(|out| out.request_id);
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

/// What the adapter sent before `session_started`, in wire order: emitted
/// right after it.
enum Early {
    Update(Value),
    /// Opened once the session is announced, never before (the collector
    /// has no session to attach it to yet).
    Question(Box<Question>),
}

/// What a `session/load` replayed: state updates to pass through, questions
/// to open, and the unknown kinds that were dropped (ACP core §4.5).
#[derive(Default)]
struct Replay {
    kept: Vec<Early>,
    unknown: BTreeMap<String, usize>,
}

impl Replay {
    fn observe(&mut self, payload: Value) {
        let kind = payload["update"]["sessionUpdate"]
            .as_str()
            .unwrap_or("<none>")
            .to_string();
        if STATE_KINDS.contains(&kind.as_str()) {
            self.kept.push(Early::Update(payload));
        } else if !HISTORY_KINDS.contains(&kind.as_str()) {
            *self.unknown.entry(kind).or_default() += 1;
        }
    }

    /// Whatever the adapter sent while the session was being attached: an
    /// update is replay (`observe`), a question is live and kept.
    fn inbound(&mut self, inbound: Inbound) {
        match inbound {
            Inbound::Update(payload) => self.observe(payload),
            Inbound::Question(question) => self.kept.push(Early::Question(question)),
            // No switch is sent, no question is open and no probe runs
            // before the actor's main loop starts.
            Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. } | Inbound::Git { .. } => {}
        }
    }

    /// The updates kept, in order.
    fn updates(&self) -> impl DoubleEndedIterator<Item = &Value> {
        self.kept.iter().filter_map(|early| match early {
            Early::Update(payload) => Some(payload),
            Early::Question(_) => None,
        })
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
    /// The adapter's questions waiting for the operator.
    questions: Mutex<Questions>,
    /// The inbound channel, for the questions' withdrawal watchers and the
    /// git probe.
    inbound: OnceLock<mpsc::UnboundedSender<Inbound>>,
    /// The git probe of the last turn's end, if it still runs: a newer one
    /// replaces it, which aborts it, and so does the actor's end.
    probe: Mutex<Option<Watcher>>,
    /// The probe after a new session's start, which records the base
    /// commit, with what says it is over (`true`, or the sender gone). A
    /// turn's probe waits for it rather than aborting it (the second
    /// review's B1), so the states stay in order; the actor's end aborts it.
    base_probe: Mutex<Option<(Watcher, watch::Receiver<bool>)>>,
    /// Shared with the handle (`SessionHandle::is_ending`).
    ending: Arc<AtomicBool>,
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

    /// This actor is about to end by itself: from now on a start or resume
    /// waits for it to finish instead of being routed to it.
    fn begin_ending(&self) {
        self.ending.store(true, Ordering::SeqCst);
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
                | SessionCmd::SetConfig { request_id, .. }
                | SessionCmd::Answer { request_id, .. } => {
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
        // For the git probe: `cwd` goes to the adapter's start.
        let probe_cwd = cwd.clone();
        let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
            Ok(spawned) => spawned,
            Err(err) => {
                return self.start_failed(request_id, StartError::other(format!("spawn {}: {err}", agent.program)));
            }
        };
        let (updates_tx, mut updates) = mpsc::unbounded_channel::<Inbound>();
        // Kept for `send_next_switch`'s own `on_receiving_result` callbacks
        // (fix round 2): the notification handler below moves its own clone
        // into the connection task.
        let switch_tx = updates_tx.clone();
        let questions_tx = updates_tx.clone();
        let _ = self.inbound.set(updates_tx.clone());
        let (conn_tx, conn_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (_stop_tx, stop_rx) = oneshot::channel::<()>();
        let transport = ByteStreams::new(io.stdin.compat_write(), io.stdout.compat());
        let session_id = self.session_id.clone();
        #[cfg(feature = "test-hooks")]
        let hooks = self.options.test_hooks.clone();
        let _acp = AcpTask(tokio::spawn(async move {
            let result = Client
                .builder()
                .name("hennery-host")
                // Raw handler: payloads are forwarded verbatim, including
                // update kinds this build does not know (ACP core §2.4).
                .on_receive_notification(
                    async move |msg: UntypedMessage, _cx| {
                        if msg.method == "session/update" {
                            let _sent = updates_tx.send(Inbound::Update(msg.params));
                            #[cfg(feature = "test-hooks")]
                            if _sent.is_ok()
                                && let Some(hooks) = &hooks
                            {
                                hooks.update_queued();
                            }
                        }
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                // Questions for the operator join the same ordered channel.
                // Every other request is refused `-32601 Method not found`
                // here (ACP core §2.5): left unhandled, the crate would hold
                // any request that names a session (`fs/*`, `terminal/*`)
                // for a session handler that never comes, and the adapter
                // would wait for good.
                .on_receive_request(
                    async move |msg: UntypedMessage, responder: Responder<Value>, _cx| match question_kind(&msg.method)
                    {
                        Some(kind) => {
                            let question = Question {
                                kind,
                                params: msg.params,
                                responder,
                            };
                            let _ = questions_tx.send(Inbound::Question(Box::new(question)));
                            Ok(())
                        }
                        None => responder
                            .respond_with_error(agent_client_protocol::Error::method_not_found().data(msg.method)),
                    },
                    agent_client_protocol::on_receive_request!(),
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
            self.begin_ending();
            adapter.terminate(self.options.kill_grace).await;
            return self.start_failed(
                request_id,
                StartError::other("the ACP connection could not be set up".into()),
            );
        };
        // One deadline for the whole start: the switches share it, so a slow
        // one cannot push the answer past the collector's start timeout.
        let deadline = Instant::now() + self.options.start_timeout;
        // What the adapter sends before `session_started`, emitted after it.
        // Outside the start's future, so a start that runs out of time can
        // still say which questions it was holding back.
        let mut replay = Replay::default();
        let started = tokio::select! {
            result = async {
                let (session, catalogue, images) =
                    match tokio::time::timeout_at(deadline, negotiate(&conn, cwd, &attach, &mut updates, &mut replay)).await {
                        Ok(result) => result?,
                        Err(_) => {
                            return Err(StartError::other(format!(
                                "adapter did not start within {}s{}",
                                self.options.start_timeout.as_secs(),
                                held_questions(&replay, &mut updates)
                            )));
                        }
                    };
                let applied = apply_config(&conn, &session, catalogue, &config, self.options.config_timeout, deadline).await;
                Ok((session, applied, images))
            } => result,
            info = adapter.exited() => {
                let tail = adapter.stderr_tail().await;
                Err(StartError::other(format!("adapter exited during start ({}): {}", describe(info), last_lines(&tail, 5))))
            }
        };
        let (agent_session, applied, images) = match started {
            Ok(started) => started,
            Err(error) => {
                self.begin_ending();
                adapter.terminate(self.options.kill_grace).await;
                return self.start_failed(request_id, error);
            }
        };
        let applied_hung = applied.hung;
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
        // catalogue extracts (P-13). Questions asked meanwhile open here, in
        // wire order. Then the note about what the load dropped (ACP core
        // §4.5), then the one about switches that did not take.
        let mut early = std::mem::take(&mut replay.kept);
        // Only what is queued now: a flooding adapter must not hold up the
        // start.
        for _ in 0..updates.len() {
            // A `SwitchAnswer` here is impossible: `send_next_switch` is
            // only ever called from the main loop below, which has not
            // started yet.
            match updates.try_recv() {
                Ok(Inbound::Update(payload)) => early.push(Early::Update(payload)),
                Ok(Inbound::Question(question)) => early.push(Early::Question(question)),
                Ok(Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. } | Inbound::Git { .. }) => {}
                Err(_) => break,
            }
        }
        for early in early {
            match early {
                Early::Update(payload) => self.emit(early_update(payload)),
                Early::Question(question) => self.open_question(question, None),
            }
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
        // The git state after the start; a new session's names the commit
        // it started from (the review's O3: a resume's would name a later
        // one).
        self.probe_git(&probe_cwd, matches!(attach, Attach::New));

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
            // A start switch that got no answer in time may still land, and
            // a late model switch would clamp a mode switched after it
            // (decision 6): nothing is sent until its grace passes (final
            // review I1). Its answer was dropped with the start's future,
            // so `NO_TOKEN` matches nothing: only the grace clears it.
            orphan: applied_hung.map(|sent_at| Orphan {
                token: NO_TOKEN,
                request_id: None,
                drop_after: Some(sent_at + self.options.config_timeout * ORPHAN_GRACE),
            }),
            next_token: NO_TOKEN + 1,
            answered_token: Arc::new(AtomicU64::new(NO_TOKEN)),
        };
        // Updates handled since the burst last reset: reset only once a
        // full pass over the other arms (below) finds none of them ready.
        let mut burst = 0;
        loop {
            let cancel_at = turn.as_ref().and_then(|t| t.cancel_deadline);
            let out_at = configs.out.as_ref().and_then(|out| out.deadline);
            let orphan_at = configs.orphan.as_ref().and_then(|orphan| orphan.drop_after);
            tokio::select! {
                // Biased: adapter output already received is emitted before
                // the prompt reply it preceded on the wire (every arm that
                // acts on the adapter drains what is queued first).
                biased;
                Some(inbound) = updates.recv(), if burst < UPDATE_BURST => {
                    // Live updates and switch answers arrive on the same,
                    // wire-ordered channel (fix round 2, F2): handling
                    // exactly one item per poll, in receipt order, is
                    // correct for every interleaving — no peeking or
                    // reordering needed.
                    burst += 1;
                    let is_answer = matches!(inbound, Inbound::SwitchAnswer { .. });
                    if self.handle_inbound(inbound, turn.as_ref().map(|t| t.id.as_str()), &mut configs) {
                        idle_since = Instant::now();
                    }
                    if is_answer {
                        self.send_next_switch(&conn, &agent_session, &switch_tx, &mut configs);
                    }
                }
                info = adapter.exited() => {
                    return self.adapter_exited(info, &mut adapter, &mut updates, turn.take(), &mut configs).await;
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
                        // ACP: images only to an agent whose `initialize`
                        // offered them (plan 6a, decision 2).
                        if !images && blocks.iter().any(|block| matches!(block, ContentBlock::Image(_))) {
                            self.reject(request_id, "images_unsupported", "this agent does not take images".into());
                            continue;
                        }
                        if seen_turns.contains(&turn_id) {
                            tracing::info!(%turn_id, "ignoring duplicate prompt delivery");
                            continue;
                        }
                        if turn.is_some() {
                            self.reject(request_id, "turn_in_progress", "a turn is already running".into());
                            continue;
                        }
                        // Updates from before this turn are not part of it.
                        self.drain_updates(&mut updates, None, &mut configs);
                        // The drain above may have answered a switch that
                        // was out, freeing whatever waits behind it: sent
                        // now, not left for this new turn's own reply to
                        // discover (same pattern as the reply arm below).
                        self.send_next_switch(&conn, &agent_session, &switch_tx, &mut configs);
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
                                #[cfg(feature = "test-hooks")]
                                if let Some(hooks) = &self.options.test_hooks {
                                    hooks.cancel_read();
                                }
                                if let Err(err) = conn.send_notification(CancelNotification::new(agent_session.clone())) {
                                    tracing::warn!(session_id = %self.session_id, error = %err, "session/cancel not sent");
                                }
                                running.cancel_deadline = Some(Instant::now() + self.options.cancel_grace);
                                // ACP: after `session/cancel`, every pending
                                // request is answered cancelled.
                                self.questions.lock().expect("questions lock").cancelled_turn = Some(turn_id);
                                self.cancel_questions(PendingReason::TurnCancelled);
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
                    Some(SessionCmd::SetConfig { request_id, config_id, value }) => {
                        idle_since = Instant::now();
                        if configs.orphan.is_some() {
                            // Nothing is sent while an orphan's fate is
                            // unknown: it may still answer, and decision 6
                            // forbids two switches ever being (possibly)
                            // out at once (fix round 1, F1).
                            self.reject(request_id, "config_failed", "an earlier switch is still out".into());
                        } else {
                            let deadline = Instant::now() + self.options.config_timeout;
                            configs.queued.push_back(QueuedSwitch { request_id, config_id, value, deadline });
                            self.send_next_switch(&conn, &agent_session, &switch_tx, &mut configs);
                            #[cfg(feature = "test-hooks")]
                            if configs.out.is_some()
                                && let Some(hooks) = &self.options.test_hooks
                            {
                                hooks.hold_if_armed(test_hooks::HoldAt::SwitchSent).await;
                            }
                        }
                    }
                    Some(SessionCmd::Answer { request_id, pending_id, answer }) => {
                        idle_since = Instant::now();
                        self.answer(request_id, pending_id, answer);
                    }
                    Some(SessionCmd::Park { .. }) => {
                        let reason = PendingReason::SessionParked;
                        self.teardown(&mut adapter, &mut updates, turn.take(), &mut configs, reason).await;
                        return self.emit(SessionBody::SessionParked { reason: ParkReason::Operator });
                    }
                    Some(SessionCmd::Close { .. }) => {
                        let reason = PendingReason::SessionClosed;
                        self.teardown(&mut adapter, &mut updates, turn.take(), &mut configs, reason).await;
                        return self.emit(SessionBody::SessionClosed);
                    }
                },
                result = next_reply(&mut turn) => {
                    let ended = turn.take().expect("a reply implies a turn");
                    let cancelling = ended.cancel_deadline.is_some();
                    idle_since = Instant::now();
                    // Closes the race described on `drain_updates`: a late
                    // update (or switch answer) that arrived just as the
                    // reply resolved must be handled before this turn's
                    // `turn_ended`.
                    self.drain_updates(&mut updates, Some(&ended.id), &mut configs);
                    self.send_next_switch(&conn, &agent_session, &switch_tx, &mut configs);
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
                            self.probe_git(&probe_cwd, false);
                        }
                        Err(err) => {
                            #[cfg(feature = "test-hooks")]
                            if let Some(hooks) = &self.options.test_hooks {
                                hooks.hold_if_armed(test_hooks::HoldAt::PromptErrored).await;
                            }
                            let exited = adapter.exited_within(EXIT_SETTLE).await;
                            // Updates that arrived during the wait above are
                            // also ahead of this turn's end.
                            self.drain_updates(&mut updates, Some(&ended.id), &mut configs);
                            self.send_next_switch(&conn, &agent_session, &switch_tx, &mut configs);
                            if let Some(info) = exited {
                                return self.adapter_exited(info, &mut adapter, &mut updates, Some(ended), &mut configs).await;
                            }
                            // An agent may answer an aborted prompt with an
                            // error instead of `cancelled`.
                            let outcome = if cancelling { TurnOutcome::Cancelled } else { TurnOutcome::Failed };
                            self.end_turn(ended.id, outcome, None, Some(err.to_string()));
                            self.probe_git(&probe_cwd, false);
                        }
                    }
                }
                _ = out_deadline(out_at) => {
                    #[cfg(feature = "test-hooks")]
                    if let Some(hooks) = &self.options.test_hooks {
                        hooks.hold_if_armed(test_hooks::HoldAt::SwitchDeadline).await;
                    }
                    // Never drains: an adapter's own flood backlog must
                    // never hold up this arm either, or it becomes just
                    // another way to hold off a cancel sitting behind it in
                    // `commands` (fix round 2). Whether the answer is
                    // already in flight is decided from the flag alone
                    // (set, ordered, before the answer is pushed onto the
                    // channel — see `send_next_switch`), never by looking.
                    let out = configs.out.as_ref().expect("a deadline implies an out switch");
                    if configs.answered_token.load(Ordering::SeqCst) == out.token {
                        // Disarm rather than orphan: the answer is already
                        // in flight (or sitting in `updates`) and will be
                        // handled in wire order by the ordinary arm above,
                        // whenever the actor gets to it. Cleared, not left
                        // at its old (now past) instant, so this arm can
                        // never re-fire for it in a busy loop.
                        configs.out.as_mut().expect("just matched").deadline = None;
                    } else {
                        let out = configs.out.take().expect("just matched");
                        self.orphan_switch(out, &mut configs);
                        #[cfg(feature = "test-hooks")]
                        if let Some(hooks) = &self.options.test_hooks {
                            hooks.hold_if_armed(test_hooks::HoldAt::Orphaned).await;
                        }
                    }
                }
                _ = orphan_deadline(orphan_at) => {
                    // Never drains, like `out_deadline`: an orphan whose
                    // answer is already flagged (so on the channel, or about
                    // to be) keeps waiting for it, grace disarmed, and the
                    // ordinary arm handles it in wire order. A start's
                    // orphan (`NO_TOKEN`) has no answer to wait for.
                    let orphan = configs.orphan.as_mut().expect("a grace implies an orphan");
                    if orphan.token != NO_TOKEN && configs.answered_token.load(Ordering::SeqCst) == orphan.token {
                        orphan.drop_after = None;
                    } else {
                        configs.orphan = None;
                        idle_since = Instant::now();
                    }
                }
                _ = cancel_deadline(cancel_at) => {
                    let unanswered = turn.take().expect("a deadline implies a turn");
                    return self.stop_after_unanswered_cancel(&mut adapter, &mut updates, unanswered, &mut configs).await;
                }
                // Never with a question open, in a turn or not: it has no
                // timeout (ACP core §4.6).
                _ = idle_deadline(self.options.idle_timeout, idle_since),
                    if turn.is_none() && configs.out.is_none() && configs.orphan.is_none() && !self.has_questions() =>
                {
                    tracing::info!(session_id = %self.session_id, "reaping idle session");
                    self.begin_ending();
                    self.teardown(&mut adapter, &mut updates, None, &mut configs, PendingReason::SessionParked).await;
                    return self.emit(SessionBody::SessionParked { reason: ParkReason::Idle });
                }
                // A burst is over and nothing else was ready: back to the
                // updates, but only after giving this worker up once. What
                // the arms above send the adapter (`session/cancel` above
                // all) is only queued for the ACP task, and an idle ACP task
                // woken from here waits in this worker's LIFO slot, which no
                // other worker can steal from, until the actor's poll ends.
                // Every update is a synchronous outbox commit: without this
                // yield that poll runs on for over a burst of them, and on a
                // slow disk a cancel read promptly still went out seconds
                // later.
                _ = tokio::task::yield_now(), if burst >= UPDATE_BURST => burst = 0,
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

    /// Send the oldest waiting switch, unless one is already out or an
    /// earlier one is orphaned (fix round 1, F1): decision 6 allows at most
    /// one switch in flight, and an orphan's fate is unknown until it
    /// answers or its grace passes. A switch whose deadline passed while it
    /// waited is answered `config_failed` without being sent. Checked
    /// against the catalogue only now, when it is actually about to go out
    /// (fix round 1, F3): checking it at arrival would judge a queued
    /// switch against options a switch ahead of it in the queue is about to
    /// replace, and could answer a refusal before an earlier, still-pending
    /// switch's own answer — out of the order the operator sent them in.
    fn send_next_switch(
        &self,
        conn: &ConnectionTo<Agent>,
        session: &SessionId,
        updates_tx: &mpsc::UnboundedSender<Inbound>,
        configs: &mut PendingConfigs,
    ) {
        while configs.out.is_none()
            && configs.orphan.is_none()
            && let Some(next) = configs.queued.pop_front()
        {
            if Instant::now() >= next.deadline {
                self.reject(
                    next.request_id,
                    "config_failed",
                    "an earlier switch is still out".into(),
                );
                continue;
            }
            if let Err((code, message)) = self.check_switch(&next.config_id, &next.value) {
                self.reject(next.request_id, code, message);
                continue;
            }
            let token = configs.next_token;
            configs.next_token += 1;
            let request = SetSessionConfigOptionRequest::new(session.clone(), next.config_id, acp_value(&next.value));
            let tx = updates_tx.clone();
            let answered_token = configs.answered_token.clone();
            #[cfg(feature = "test-hooks")]
            let hooks = self.options.test_hooks.clone();
            // Ordered (`on_receiving_result`, not `block_task`): the
            // dispatch loop holds any later notification until this
            // callback returns, so the answer lands in `updates` in true
            // wire order relative to it (fix round 2, F2). The callback
            // itself must never fail (an `Err` here would shut the whole
            // ACP connection down) — it only ever forwards the result.
            if let Err(err) = conn.send_request(request).on_receiving_result(move |result| {
                // Flagged *before* the answer is pushed onto the channel
                // (fix round 2): `out_deadline`, on a different task, can
                // only ever observe this after the answer is there (or
                // about to be), never before — so it is safe to disarm on
                // the strength of this flag alone, with no drain to confirm.
                // Raised, never overwritten (final review M1): tokens only
                // increase, so a late answer for an orphan already given up
                // on cannot lower a newer switch's flag.
                answered_token.fetch_max(token, Ordering::SeqCst);
                let _ = tx.send(Inbound::SwitchAnswer { token, result });
                #[cfg(feature = "test-hooks")]
                if let Some(hooks) = &hooks {
                    hooks.answer_queued();
                }
                std::future::ready(Ok(()))
            }) {
                // Never sent (the connection is shutting down), so nothing
                // can answer it: answered now, and not left out to block
                // every switch behind it until its deadline (final review
                // M2).
                tracing::warn!(session_id = %self.session_id, error = %err, "set_config not sent");
                self.reject(
                    next.request_id,
                    "config_failed",
                    format!("not sent to the adapter: {err}"),
                );
                continue;
            }
            configs.out = Some(OutSwitch {
                request_id: next.request_id,
                token,
                sent_at: Instant::now(),
                deadline: Some(next.deadline),
            });
        }
    }

    /// A switch's read-back becomes the catalogue if it is non-empty; an
    /// empty one means the current values are unknown, not that the adapter
    /// has no options (decision 3).
    fn apply_read_back(&self, response: SetSessionConfigOptionResponse) {
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

    /// Answer a `set_config` from the adapter's genuine, timely answer:
    /// `config_applied` with the catalogue it answered with (none if it
    /// answered without one), or `config_failed` if it refused. A switch
    /// that instead ran out its deadline is handled by `orphan_switch`
    /// below, never here.
    fn config_answered(
        &self,
        request_id: String,
        result: agent_client_protocol::Result<SetSessionConfigOptionResponse>,
    ) {
        match result {
            Ok(response) => {
                self.apply_read_back(response);
                self.emit(SessionBody::ConfigApplied {
                    request_id,
                    indexed: self.catalogue_extracts(),
                });
            }
            Err(err) => self.reject(request_id, "config_failed", err.to_string()),
        }
    }

    /// A switch whose deadline passed while it was still out (fix round 1,
    /// F1). Its requester is answered now, but the real adapters handle
    /// requests concurrently, so it may still answer late — with a fresher
    /// read-back than anything sent behind it, which is why nothing is sent
    /// behind an orphan either. Everything already queued behind it is
    /// answered the same way, in order, rather than each waiting out its
    /// own deadline (which would still leave two requests briefly out at
    /// once, the moment the next one's turn came).
    fn orphan_switch(&self, out: OutSwitch, configs: &mut PendingConfigs) {
        self.catalogue.lock().expect("catalogue lock").current = false;
        let timeout = self.options.config_timeout;
        self.reject(
            out.request_id.clone(),
            "config_failed",
            format!("no answer within {timeout:?} of the request"),
        );
        for queued in configs.queued.drain(..) {
            self.reject(
                queued.request_id,
                "config_failed",
                "an earlier switch is still out".into(),
            );
        }
        configs.orphan = Some(Orphan {
            token: out.token,
            request_id: Some(out.request_id),
            drop_after: Some(out.sent_at + self.options.config_timeout * ORPHAN_GRACE),
        });
    }

    /// An orphaned switch's late answer (fix round 1, F1): its requester
    /// already has its `config_failed`, but a non-empty read-back is what
    /// the agent now runs with, so it is announced as a `config_applied`
    /// under the orphan's own request id (final review I2). The collector
    /// stores it while the session is attached; with no waiter left for
    /// that id, it answers no one. An empty read-back or a refusal is not
    /// announced: neither tells the collector anything new.
    fn orphan_answered(
        &self,
        request_id: Option<String>,
        result: agent_client_protocol::Result<SetSessionConfigOptionResponse>,
    ) {
        let Ok(response) = result else { return };
        let announced = !response.config_options.is_empty();
        self.apply_read_back(response);
        if announced && let Some(request_id) = request_id {
            self.emit(SessionBody::ConfigApplied {
                request_id,
                indexed: self.catalogue_extracts(),
            });
        }
    }

    /// One item off the inbound channel (fix round 2): a live update,
    /// emitted (`live_update` carries a `config_option_update`'s catalogue
    /// extracts), or a switch's answer, routed by `route_switch_answer`.
    /// Returns whether the idle reaper's clock should restart: an orphan
    /// cleared, or the adapter withdrew one of its own questions (ACP core
    /// §4.7) — either is activity, not just an orphan clearing.
    fn handle_inbound(&self, inbound: Inbound, turn: Option<&str>, configs: &mut PendingConfigs) -> bool {
        #[cfg(feature = "test-hooks")]
        if let Some(hooks) = &self.options.test_hooks {
            hooks.inbound_handled();
        }
        match inbound {
            Inbound::Update(payload) => {
                self.emit(self.live_update(payload, turn));
                false
            }
            Inbound::SwitchAnswer { token, result } => self.route_switch_answer(token, result, configs),
            Inbound::Question(question) => {
                self.open_question(question, turn);
                false
            }
            // The session may be idle now: the reaper's clock restarts.
            Inbound::QuestionWithdrawn { pending_id } => {
                self.withdraw_question(pending_id);
                true
            }
            Inbound::Git { state, base } => {
                self.emit(SessionBody::GitState {
                    base_commit: state.head.clone().filter(|_| base),
                    branch: state.branch,
                    dirty: state.dirty,
                    worktree: state.worktree,
                    head: state.head,
                });
                false
            }
        }
    }

    /// Probe `cwd`'s git state on a task of its own (ACP core §7; plan
    /// 6b-ii decision 11): it never holds the actor, so it never delays or
    /// reorders a turn's end. Its result, if any, comes back on the ordered
    /// inbound channel as `Inbound::Git`. A newer probe aborts this one, and
    /// so does the actor's end; either kills git's process group. The probe
    /// that records the base commit is never replaced (the second review's
    /// B1): it has a slot of its own, and a turn's probe first waits for it
    /// to be over (it is bounded too), so a quick first turn cannot cost the
    /// base and the states stay in order. Turns' probes replace each other.
    fn probe_git(&self, cwd: &std::path::Path, base: bool) {
        let (Some(git), Some(inbound)) = (self.options.git.clone(), self.inbound.get().cloned()) else {
            return;
        };
        let cwd = cwd.to_path_buf();
        if base {
            let (over, over_rx) = watch::channel(false);
            let task = tokio::spawn(async move {
                if let Some(state) = crate::git::probe(&git, &cwd).await {
                    let _ = inbound.send(Inbound::Git { state, base: true });
                }
                let _ = over.send(true);
            });
            *self.base_probe.lock().expect("probe lock") = Some((Watcher(Some(task)), over_rx));
            return;
        }
        let base_over = self
            .base_probe
            .lock()
            .expect("probe lock")
            .as_ref()
            .map(|(_, over)| over.clone());
        let task = tokio::spawn(async move {
            if let Some(mut over) = base_over {
                // Done, or aborted (its sender gone): either way, over.
                while !*over.borrow_and_update() {
                    if over.changed().await.is_err() {
                        break;
                    }
                }
            }
            if let Some(state) = crate::git::probe(&git, &cwd).await {
                let _ = inbound.send(Inbound::Git { state, base: false });
            }
        });
        *self.probe.lock().expect("probe lock") = Some(Watcher(Some(task)));
    }

    /// Announce an adapter's question as `pending_opened` and keep its
    /// responder until the operator answers (ACP core §4.6). `turn` is the
    /// turn it was asked in. A permission's option ids are read from the
    /// raw request (`option_ids`).
    fn open_question(&self, question: Box<Question>, turn: Option<&str>) {
        let Question {
            kind,
            params,
            responder,
        } = *question;
        let pending_id = uuid::Uuid::now_v7().to_string();
        let option_ids = match kind {
            PendingKind::Permission => option_ids(&params),
            PendingKind::Elicitation => None,
        };
        let title = question_title(kind, &params);
        self.emit(SessionBody::PendingOpened {
            pending_id: pending_id.clone(),
            indexed: Indexed {
                turn_id: turn.map(str::to_string),
                pending: Some(Box::new(PendingExtract {
                    id: pending_id.clone(),
                    kind,
                    option_ids,
                    title,
                })),
                ..Indexed::default()
            },
            payload: params,
        });
        // The adapter may withdraw its question (`$/cancel_request`): a
        // watcher reports that through the ordered channel, and stops when
        // the question is answered or cancelled.
        let cancellation = responder.cancellation();
        let withdrawal = self.inbound.get().cloned().map(|inbound| {
            let pending_id = pending_id.clone();
            Watcher(Some(tokio::spawn(async move {
                cancellation.cancelled().await;
                let _ = inbound.send(Inbound::QuestionWithdrawn { pending_id });
            })))
        });
        let question = OpenQuestion {
            pending_id,
            kind,
            responder,
            _withdrawal: withdrawal,
        };
        let mut questions = self.questions.lock().expect("questions lock");
        if turn.is_some() && questions.cancelled_turn.as_deref() == turn {
            // Asked in a turn the operator already stopped.
            drop(questions);
            self.resolve_cancelled(question, PendingReason::TurnCancelled);
        } else {
            questions.open.push(question);
        }
    }

    fn has_questions(&self) -> bool {
        !self.questions.lock().expect("questions lock").open.is_empty()
    }

    /// Tell the adapter every open question is off, and the collector why
    /// (ACP core §4.6): `pending_resolved{cancelled, reason}` each, oldest
    /// first.
    fn cancel_questions(&self, reason: PendingReason) {
        let open = std::mem::take(&mut self.questions.lock().expect("questions lock").open);
        for question in open {
            self.resolve_cancelled(question, reason);
        }
    }

    fn resolve_cancelled(&self, question: OpenQuestion, reason: PendingReason) {
        // An adapter that is gone cannot hear it; the collector still must.
        if let Err(err) = question.responder.respond(cancelled_response(question.kind)) {
            tracing::debug!(session_id = %self.session_id, error = %err, "cancellation not sent to the adapter");
        }
        self.emit(SessionBody::PendingResolved {
            pending_id: question.pending_id,
            resolution: PendingResolution::Cancelled,
            reason: Some(reason),
        });
    }

    /// The adapter withdrew a question (`$/cancel_request`): nobody waits
    /// for its answer any more, so the operator can no longer give one
    /// (`pending_resolved{cancelled, agent_withdrew}`). The request is
    /// answered with the standard cancellation error, as JSON-RPC expects.
    fn withdraw_question(&self, pending_id: String) {
        let question = {
            let mut questions = self.questions.lock().expect("questions lock");
            let at = questions.open.iter().position(|q| q.pending_id == pending_id);
            at.map(|at| questions.open.remove(at))
        };
        // Already answered or cancelled: nothing is left to withdraw.
        let Some(question) = question else {
            return;
        };
        let cancelled = agent_client_protocol::Error::request_cancelled();
        if let Err(err) = question.responder.respond_with_error(cancelled) {
            tracing::debug!(session_id = %self.session_id, error = %err, "withdrawal not acknowledged to the adapter");
        }
        self.emit(SessionBody::PendingResolved {
            pending_id,
            resolution: PendingResolution::Cancelled,
            reason: Some(PendingReason::AgentWithdrew),
        });
    }

    /// Deliver an operator's answer if its question is still open here
    /// (ACP core §4.6): `answer_result{delivered: true}`, then
    /// `pending_resolved{delivered}`. An answer nobody waits for (already
    /// answered, cancelled, asked of an earlier adapter, or of another kind)
    /// is `answer_result{delivered: false}` and changes nothing.
    fn answer(&self, request_id: String, pending_id: String, answer: Answer) {
        let question = {
            let mut questions = self.questions.lock().expect("questions lock");
            let at = questions
                .open
                .iter()
                .position(|q| q.pending_id == pending_id && q.kind == answer.kind());
            at.map(|at| questions.open.remove(at))
        };
        let Some(question) = question else {
            return self.emit(SessionBody::AnswerResult {
                pending_id,
                request_id,
                delivered: false,
            });
        };
        let sent = question.responder.respond(answer.response());
        self.emit(SessionBody::AnswerResult {
            pending_id: pending_id.clone(),
            request_id,
            delivered: sent.is_ok(),
        });
        let (resolution, reason) = match sent {
            Ok(()) => (PendingResolution::Delivered, None),
            // The connection to the adapter is gone: so is the question.
            Err(err) => {
                tracing::warn!(session_id = %self.session_id, error = %err, "answer not sent to the adapter");
                (PendingResolution::Cancelled, Some(PendingReason::AdapterLost))
            }
        };
        self.emit(SessionBody::PendingResolved {
            pending_id,
            resolution,
            reason,
        });
    }

    /// Apply a switch's answer if its `token` matches the switch that is out
    /// or orphaned; a stray answer for anything else (e.g. an orphan already
    /// given up on past its grace) matches neither and is discarded. Returns
    /// whether this cleared the orphan.
    fn route_switch_answer(
        &self,
        token: u64,
        result: agent_client_protocol::Result<SetSessionConfigOptionResponse>,
        configs: &mut PendingConfigs,
    ) -> bool {
        if configs.out.as_ref().is_some_and(|out| out.token == token) {
            let out = configs.out.take().expect("just matched");
            self.config_answered(out.request_id, result);
            false
        } else if let Some(orphan) = configs.orphan.take_if(|orphan| orphan.token == token) {
            self.orphan_answered(orphan.request_id, result);
            true
        } else {
            false
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

    /// Handle whatever is already queued on the inbound channel, without
    /// waiting, as part of `turn` (if any): a live update is emitted; a
    /// switch's answer is routed by `route_switch_answer`. Both arrive on
    /// the same wire-ordered channel (fix round 2, F2), so handling them one
    /// at a time, in receipt order, right before acting on a reply (or
    /// before ending a torn-down turn) is correct — no reordering needed.
    /// Returns whether the idle reaper's clock should restart: an orphan
    /// cleared, or a question was withdrawn, during the drain (ACP core
    /// §4.7); callers that are ending the actor regardless (teardown, an
    /// unanswered cancel, an adapter exit) can ignore it.
    fn drain_updates(
        &self,
        updates: &mut mpsc::UnboundedReceiver<Inbound>,
        turn: Option<&str>,
        configs: &mut PendingConfigs,
    ) -> bool {
        let mut restart_idle_clock = false;
        // Only what is queued now: an adapter that keeps streaming cannot
        // hold the actor here.
        for _ in 0..updates.len() {
            match updates.try_recv() {
                Ok(inbound) => restart_idle_clock |= self.handle_inbound(inbound, turn, configs),
                Err(_) => break,
            }
        }
        restart_idle_clock
    }

    /// Park or close: forward any output already queued, end the turn as
    /// interrupted, cancel the open questions for `reason` (ACP core §4.8),
    /// then kill the group.
    async fn teardown(
        &self,
        adapter: &mut Adapter,
        updates: &mut mpsc::UnboundedReceiver<Inbound>,
        turn: Option<Turn>,
        configs: &mut PendingConfigs,
        reason: PendingReason,
    ) {
        self.drain_updates(updates, turn.as_ref().map(|t| t.id.as_str()), configs);
        if let Some(turn) = turn {
            self.end_turn(turn.id, TurnOutcome::Interrupted, None, None);
        }
        self.cancel_questions(reason);
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
        updates: &mut mpsc::UnboundedReceiver<Inbound>,
        turn: Turn,
        configs: &mut PendingConfigs,
    ) {
        let grace = self.options.cancel_grace;
        tracing::warn!(session_id = %self.session_id, ?grace, "adapter ignored session/cancel; stopping it");
        self.begin_ending();
        self.drain_updates(updates, Some(&turn.id), configs);
        let message = format!("the adapter did not stop within {grace:?} of session/cancel");
        self.end_turn(turn.id, TurnOutcome::Cancelled, None, Some(message.clone()));
        // The cancel already answered the turn's questions; any asked
        // outside it go the same way.
        self.cancel_questions(PendingReason::TurnCancelled);
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
    /// reply future is dropped), the turn ends `interrupted`, the open
    /// questions are cancelled `adapter_lost`, then `adapter_exited` and
    /// `session_parked{adapter_exited}`.
    async fn adapter_exited(
        &self,
        info: ExitInfo,
        adapter: &mut Adapter,
        updates: &mut mpsc::UnboundedReceiver<Inbound>,
        turn: Option<Turn>,
        configs: &mut PendingConfigs,
    ) {
        tracing::warn!(session_id = %self.session_id, exit = %describe(info), "adapter exited");
        self.begin_ending();
        // Output the adapter wrote before dying is still in the pipe.
        while let Ok(Some(inbound)) = tokio::time::timeout(DRAIN_QUIET, updates.recv()).await {
            self.handle_inbound(inbound, turn.as_ref().map(|t| t.id.as_str()), configs);
        }
        if let Some(turn) = turn {
            self.end_turn(
                turn.id,
                TurnOutcome::Interrupted,
                None,
                Some("the adapter exited".into()),
            );
        }
        self.cancel_questions(PendingReason::AdapterLost);
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

/// Resolves once the switch that is out has waited past its deadline; never
/// if none is out.
async fn out_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Resolves once an orphaned switch's grace has passed; never if none is
/// orphaned.
async fn orphan_deadline(drop_after: Option<Instant>) {
    match drop_after {
        Some(drop_after) => tokio::time::sleep_until(drop_after).await,
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
/// none, or they did not parse), and whether its `initialize` offered
/// images in prompts (plan 6a). What the adapter sends meanwhile goes into
/// `replay`.
async fn negotiate(
    conn: &ConnectionTo<Agent>,
    cwd: PathBuf,
    attach: &Attach,
    updates: &mut mpsc::UnboundedReceiver<Inbound>,
    replay: &mut Replay,
) -> Result<(SessionId, Announced, bool), StartError> {
    let init = conn
        .send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(client_capabilities()))
        .block_task()
        .await
        .map_err(|err| StartError::acp(err, false))?;
    let images = init.agent_capabilities.prompt_capabilities.image;
    let agent_session_id = match attach {
        Attach::New => {
            let created = conn
                .send_request(NewSessionRequest::new(cwd))
                .block_task()
                .await
                .map_err(|err| StartError::acp(err, false))?;
            // What the adapter sent before its answer (see
            // `Actor::drain_updates` for the ordering argument) follows the
            // start, like a load's kept updates. A `SwitchAnswer` here is
            // impossible: no switch is sent before this actor's main loop
            // starts, well after this function returns.
            for _ in 0..updates.len() {
                match updates.try_recv() {
                    Ok(Inbound::Update(payload)) => replay.kept.push(Early::Update(payload)),
                    Ok(Inbound::Question(question)) => replay.kept.push(Early::Question(question)),
                    Ok(Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. } | Inbound::Git { .. }) => {}
                    Err(_) => break,
                }
            }
            let catalogue = announced_options(created.config_options, replay.updates());
            return Ok((created.session_id, catalogue, images));
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
    loop {
        tokio::select! {
            biased;
            Some(inbound) = updates.recv() => replay.inbound(inbound),
            result = &mut load => {
                // Everything the adapter sent before its answer is replay,
                // even if this select saw the answer first (see
                // `Actor::drain_updates` for the ordering argument). The
                // drain cannot tell "before" from "just after", though: a
                // live notification the adapter sends microseconds after
                // its load answer, already queued when this drain runs, is
                // classified as replay too — dropped if a history kind,
                // passed through if a state kind.
                while let Ok(inbound) = updates.try_recv() {
                    replay.inbound(inbound);
                }
                let loaded = result.map_err(|err| StartError::acp(err, true))?;
                let catalogue = announced_options(loaded.config_options, replay.updates());
                return Ok((id, catalogue, images));
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
fn announced_options<'a>(
    answered: Option<Vec<SessionConfigOption>>,
    before: impl DoubleEndedIterator<Item = &'a Value>,
) -> Announced {
    match answered {
        Some(options) if !options.is_empty() => Announced {
            options,
            authoritative: true,
        },
        _ => Announced {
            options: before.rev().find_map(config_update).unwrap_or_default(),
            authoritative: false,
        },
    }
}

/// What the host tells the adapter it can do (ACP core §2.5, §6): boolean
/// config options, so agents offer them as booleans rather than on/off
/// selects, and form elicitation as `{"form": {}}`, never a boolean (P-19).
fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .session(
            ClientSessionCapabilities::new().config_options(
                SessionConfigOptionsCapabilities::new().boolean(BooleanConfigOptionCapabilities::new()),
            ),
        )
        .elicitation(ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()))
}

/// What a start that ran out of time was holding back (decision 2): the
/// questions the agent asked before the session existed, which nobody could
/// see or answer. Empty if there were none.
fn held_questions(replay: &Replay, updates: &mut mpsc::UnboundedReceiver<Inbound>) -> String {
    let mut kinds: Vec<PendingKind> = replay
        .kept
        .iter()
        .filter_map(|early| match early {
            Early::Question(question) => Some(question.kind),
            Early::Update(_) => None,
        })
        .collect();
    while let Ok(inbound) = updates.try_recv() {
        if let Inbound::Question(question) = inbound {
            kinds.push(question.kind);
        }
    }
    if kinds.is_empty() {
        return String::new();
    }
    let mut names: Vec<&str> = Vec::new();
    for kind in &kinds {
        let name = match kind {
            PendingKind::Permission => "permission",
            PendingKind::Elicitation => "elicitation",
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    format!(
        "; the agent asked {} question(s) during start-up ({}) that hennery cannot show before the session exists",
        kinds.len(),
        names.join("/")
    )
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
    /// When the switch that got no answer in time was sent, if one did
    /// not. Its request is still live at the adapter, which may apply it
    /// late, so the main loop starts with it as an orphan (final review I1).
    hung: Option<Instant>,
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
/// switch could clamp a mode sent after it (`Applied::hung` carries it
/// into the main loop, which sends no `set_config` behind it either).
/// Finally every requested value is
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
        hung: None,
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
                applied.hung = Some(now);
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
///
/// A `session_info_update`'s title and an `available_commands_update`'s
/// commands are extracted here, wherever the update comes from: live, sent
/// before the start was announced, or replayed by `session/load`. Unlike
/// the catalogue (P-13), they are the adapter's current state, and they are
/// emitted in wire order, so the latest wins (plan 6b decision 2).
fn update(payload: Value, turn: Option<&str>) -> SessionBody {
    let (title, commands) = state_extracts(&payload);
    SessionBody::AcpUpdate {
        indexed: Indexed {
            turn_id: turn.map(str::to_string),
            title,
            commands,
            ..Indexed::default()
        },
        payload,
    }
}

/// An update the adapter sent before `session_started`, emitted after it:
/// replayed by a load, or sent while the start ran. Marked `early`, so the
/// collector lets its title only fill an empty one (plan 6b decision 2).
fn early_update(payload: Value) -> SessionBody {
    let mut body = update(payload, None);
    if let SessionBody::AcpUpdate { indexed, .. } = &mut body {
        indexed.early = true;
    }
    body
}

/// The `title` and `commands` extracts (ACP core §3.2) of a
/// `session_info_update` or an `available_commands_update`. A title of
/// `null` clears it, and is sent as an empty title; an update without one
/// changes nothing. Commands are extracted only from a list the adapter
/// sent: the crate reads a missing or malformed one, and one whose every
/// entry it skips, as an empty list, which would wipe the stored commands.
fn state_extracts(payload: &Value) -> (Option<String>, Option<Vec<Value>>) {
    if !matches!(
        payload["update"]["sessionUpdate"].as_str(),
        Some("session_info_update" | "available_commands_update")
    ) {
        return (None, None);
    }
    let Ok(notification) = serde_json::from_value::<SessionNotification>(payload.clone()) else {
        return (None, None);
    };
    match notification.update {
        SessionUpdate::SessionInfoUpdate(info) => match info.title {
            MaybeUndefined::Value(title) => (Some(title), None),
            MaybeUndefined::Null => (Some(String::new()), None),
            MaybeUndefined::Undefined => (None, None),
        },
        SessionUpdate::AvailableCommandsUpdate(update) => {
            let Some(sent) = payload["update"]["availableCommands"].as_array() else {
                return (None, None);
            };
            let commands: Vec<Value> = update
                .available_commands
                .iter()
                .filter_map(|command| serde_json::to_value(command).ok())
                .collect();
            if commands.is_empty() && !sent.is_empty() {
                return (None, None);
            }
            (None, Some(commands))
        }
        _ => (None, None),
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

    /// Plan 10b: what a question is about, for a push under `details`.
    #[test]
    fn a_questions_title_is_its_tool_call_title_or_its_message() {
        let permission = serde_json::json!({"toolCall": {"toolCallId": "c1", "title": "Run cargo test"}});
        assert_eq!(
            question_title(PendingKind::Permission, &permission).as_deref(),
            Some("Run cargo test")
        );
        let elicitation = serde_json::json!({"message": "Which branch?", "requestedSchema": {}});
        assert_eq!(
            question_title(PendingKind::Elicitation, &elicitation).as_deref(),
            Some("Which branch?")
        );
        // None where the agent gave none, or gave only blanks.
        assert_eq!(
            question_title(PendingKind::Permission, &serde_json::json!({"toolCall": {}})),
            None
        );
        assert_eq!(
            question_title(PendingKind::Elicitation, &serde_json::json!({"message": "  "})),
            None
        );
        assert_eq!(
            question_title(PendingKind::Elicitation, &serde_json::json!({"message": 7})),
            None
        );
        // Cut to MAX_QUESTION_TITLE characters, on a character boundary.
        let long = serde_json::json!({"message": "\u{e9}".repeat(MAX_QUESTION_TITLE + 5)});
        let cut = question_title(PendingKind::Elicitation, &long).unwrap();
        assert_eq!(cut.chars().count(), MAX_QUESTION_TITLE);
    }

    /// A boolean would be discarded by the adapter's schema validator,
    /// which looks exactly like not advertising it (P-19).
    #[test]
    fn form_elicitation_is_advertised_as_an_object() {
        let advertised = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(advertised["elicitation"], serde_json::json!({"form": {}}));
    }

    /// Read raw: one option of a kind this build does not know must not
    /// cost the whole list.
    #[test]
    fn option_ids_are_read_from_the_raw_request() {
        let options = serde_json::json!({"options": [
            {"optionId": "a", "kind": "from_the_future"}, {"name": "no id"}, {"optionId": 3}, {"optionId": "b"}
        ]});
        assert_eq!(option_ids(&options), Some(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(option_ids(&serde_json::json!({})), None);
        assert_eq!(option_ids(&serde_json::json!({"options": {"optionId": "a"}})), None);
    }

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
