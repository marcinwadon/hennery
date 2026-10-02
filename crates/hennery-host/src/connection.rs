//! The host's connection to the collector (ACP core §2, §5).
//!
//! Adapters belong to the host process, not to this connection: a dropped
//! socket only ends `connect_once`; session actors keep running and keep
//! writing to the outbox, which is resent on the next connection.

use crate::identity::HostKey;
use crate::outbox::Outbox;
use crate::profile::Profile;
use crate::projects::Probes;
use crate::session::{self, AgentCommand, Answer, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
use crate::uplink::Uplink;
use anyhow::{Context, Result, bail};
use futures::{SinkExt, StreamExt};
use hennery_proto::frames::{
    AgentIsolation, AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, McpDelivery, SessionConfig,
};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// The largest frame or message the host reads (ACP core §11), as the
/// collector does: a prompt with 16 MiB of images is about 21.4 MiB of
/// JSON, past tungstenite's default 16 MiB frame.
const MAX_FRAME: usize = 32 << 20;

#[derive(Debug, Clone)]
pub struct HostConfig {
    /// e.g. `ws://127.0.0.1:7117/api/hosts/ws`
    pub collector_url: String,
    pub host_id: String,
    /// The key this host paired with; it signs every `hello` (ACP core §3.5).
    pub key: HostKey,
    pub data_dir: PathBuf,
    pub agents: HashMap<String, AgentCommand>,
    /// Each agent's profile (ACP core §6), from where it came: the
    /// installed set's `claude` is `Claude` (`ClaudeOwnCli` with
    /// `--use-cli`); an agent not listed, a `--agent` command, is `Generic`.
    pub profiles: HashMap<String, Profile>,
    pub reconnect_min: Duration,
    pub reconnect_max: Duration,
    pub ping_interval: Duration,
    pub read_timeout: Duration,
    /// Idle reaper window (ACP core §4.7); zero turns the reaper off.
    pub idle_timeout: Duration,
    /// Bound on the TCP + WebSocket handshake of one connection attempt.
    pub connect_timeout: Duration,
    /// A connection that stays up this long resets the reconnect backoff,
    /// even if nothing was acked (an idle host sends nothing to ack).
    pub healthy_after: Duration,
    /// Where projects are enumerated, and browsing is allowed (ACP core §7):
    /// absolute, as configured (`projects::workspace_roots`). Reported in
    /// `hello`.
    pub workspace_roots: Vec<PathBuf>,
    /// The host user's home directory (`projects::home_dir`): browsing is
    /// allowed under it too. None unless set, so no test reads the real one;
    /// `hennery host run` sets it.
    pub home: Option<PathBuf>,
    /// `git` for the sessions' git probe (ACP core §7), found on `PATH` once,
    /// when the host starts; `None`: no `git_state`.
    pub git: Option<PathBuf>,
}

impl HostConfig {
    pub fn new(collector_url: impl Into<String>, host_id: impl Into<String>, key: HostKey, data_dir: PathBuf) -> Self {
        Self {
            collector_url: collector_url.into(),
            host_id: host_id.into(),
            key,
            data_dir,
            agents: HashMap::new(),
            profiles: HashMap::new(),
            reconnect_min: Duration::from_millis(500),
            reconnect_max: Duration::from_secs(30),
            ping_interval: Duration::from_secs(15),
            read_timeout: Duration::from_secs(45),
            idle_timeout: session::IDLE_TIMEOUT,
            connect_timeout: Duration::from_secs(10),
            healthy_after: Duration::from_secs(60),
            workspace_roots: Vec::new(),
            home: None,
            git: crate::git::find_git(),
        }
    }

    /// `hello.workspace_roots`: the roots as configured.
    pub fn reported_roots(&self) -> Vec<String> {
        self.workspace_roots
            .iter()
            .filter_map(|root| root.to_str().map(str::to_string))
            .collect()
    }

    /// The profile of `agent` (`Generic` unless listed).
    pub fn profile(&self, agent: &str) -> Profile {
        self.profiles.get(agent).copied().unwrap_or_default()
    }

    /// `hello.mcp_isolation`: every configured agent, isolated or not.
    pub fn mcp_isolation(&self) -> AgentIsolation {
        AgentIsolation(
            self.agents
                .keys()
                .map(|agent| (agent.clone(), self.profile(agent).mcp_isolation()))
                .collect(),
        )
    }

    /// Options for every session actor this host spawns.
    pub fn session_options(&self) -> SessionOptions {
        SessionOptions {
            idle_timeout: (!self.idle_timeout.is_zero()).then_some(self.idle_timeout),
            git: self.git.clone(),
            ..SessionOptions::default()
        }
    }
}

/// The host's session actors, by session id.
#[derive(Default)]
struct SessionMap {
    handles: HashMap<String, SessionHandle>,
    /// Set by host shutdown when it takes `handles`: from then on no actor
    /// is spawned. A start or resume that was waiting behind an ending actor
    /// would otherwise attach an adapter that shutdown never sees, and that
    /// the runtime then SIGKILLs without its grace.
    closing: bool,
}

type Sessions = Arc<Mutex<SessionMap>>;

/// Run the host forever, reconnecting with exponential backoff — except that
/// this returns `Err` the moment the collector says this host is revoked,
/// after stopping every adapter it runs.
pub async fn run(cfg: HostConfig) -> Result<()> {
    run_until(cfg, std::future::pending()).await
}

/// Run the host until `shutdown` resolves.
pub async fn run_until(cfg: HostConfig, shutdown: impl Future<Output = ()>) -> Result<()> {
    // Before the first adapter (smoke test #1, F3); a no-op once made.
    crate::adapter::prepare_death_pipe().context("the adapters' death pipe")?;
    std::fs::create_dir_all(&cfg.data_dir)?;
    let outbox = Outbox::open(&cfg.data_dir.join(crate::outbox::FILE))?;
    let (uplink, mut replies) = Uplink::new(outbox);
    let sessions: Sessions = Arc::new(Mutex::new(SessionMap::default()));
    // Outlives each connection, so its bounds hold across reconnects.
    let probes = Probes::default();
    // Ends only when the collector says this host is revoked.
    let serve = async {
        let mut backoff = cfg.reconnect_min;
        loop {
            if let Err(err) = connect_once(&cfg, &uplink, &sessions, &probes, &mut replies, &mut backoff).await {
                if revoked(&err) {
                    return err;
                }
                if err
                    .downcast_ref::<HelloRejected>()
                    .is_some_and(|r| r.code == "bad_proof")
                {
                    tracing::warn!(
                        error = %err,
                        data_dir = %cfg.data_dir.display(),
                        "the collector does not know this host's key; if it should be paired anew, remove host.key and host.toml from the data directory and run `hennery host join`"
                    );
                } else {
                    tracing::warn!(error = %err, "collector connection ended");
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(cfg.reconnect_max);
        }
    };
    let revocation = tokio::select! {
        err = serve => Some(err),
        _ = shutdown => None,
    };
    // A revoked host stops every adapter it runs (ACP core §3.5), the same
    // way a shutdown does.
    shut_down(&sessions, cfg.session_options().kill_grace + Duration::from_secs(1)).await;
    match revocation {
        Some(err) => Err(err.context("this host was revoked; pair it again with `hennery host join`")),
        None => Ok(()),
    }
}

/// A `hello_error` (ACP core §3.3): the collector refused this host's
/// `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloRejected {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for HelloRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "hello rejected: {}: {}", self.code, self.message)
    }
}

impl std::error::Error for HelloRejected {}

/// Whether `err` is the collector saying this host is revoked.
pub fn revoked(err: &anyhow::Error) -> bool {
    err.downcast_ref::<HelloRejected>().is_some_and(|r| r.code == "revoked")
}

/// What the collector makes of a stored pairing (`hennery host join`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// It accepts the key.
    Accepted,
    /// It accepts the key, and another connection with it is live: the host
    /// itself, or a copy of its data directory (`already_connected`).
    Connected,
    Revoked,
    /// It does not know this host, or not with this key.
    Unknown,
}

/// Bound on each step of a probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Ask the collector whether it still accepts this host's key: one `hello`
/// with nothing attached, then the connection is closed.
///
/// A successful probe still passes the collector's proof check like any
/// other `hello`, so for the moment the socket is open it briefly registers
/// this host id as connected (ACP core §4.3) before the probe closes it —
/// the real host reconnecting at the same moment sees `already_connected`
/// rather than being displaced by a probe that never sends `resend_complete`
/// and holds nothing open.
pub async fn probe(collector_url: &str, host_id: &str, key: &HostKey) -> Result<Standing> {
    // No roots: the collector stores them only from a reconciled
    // connection, which a probe never becomes (decision 7).
    let (mut sink, _stream, answer) = handshake(
        collector_url,
        host_id,
        key,
        Announce::default(),
        || Ok(Vec::new()),
        PROBE_TIMEOUT,
        PROBE_TIMEOUT,
    )
    .await?;
    let _ = sink.close().await;
    Ok(match answer {
        CollectorFrame::HelloAck { .. } => Standing::Accepted,
        // Refused only after the proof checked out (ACP core §3.5).
        CollectorFrame::HelloError { code, .. } if code == "already_connected" => Standing::Connected,
        CollectorFrame::HelloError { code, .. } if code == "revoked" => Standing::Revoked,
        CollectorFrame::HelloError { code, .. } if code == "bad_proof" => Standing::Unknown,
        CollectorFrame::HelloError { code, message } => return Err(HelloRejected { code, message }.into()),
        other => bail!("expected hello_ack, got {other:?}"),
    })
}

type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type WsStream = futures::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Connect, send a `hello` signed over this connection's nonce (ACP core
/// §3.5), and read the collector's answer. `attached` is read once the
/// socket is up, right before the `hello` goes out.
/// What a `hello` reports of the host's configuration: nothing for a
/// probe.
#[derive(Default)]
struct Announce {
    workspace_roots: Vec<String>,
    mcp_isolation: AgentIsolation,
}

async fn handshake(
    collector_url: &str,
    host_id: &str,
    key: &HostKey,
    announce: Announce,
    attached: impl FnOnce() -> Result<Vec<AttachedSession>>,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<(WsSink, WsStream, CollectorFrame)> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let connect = tokio_tungstenite::connect_async_with_config(collector_url, Some(config), false);
    let (ws, response) = tokio::time::timeout(connect_timeout, connect)
        .await
        .map_err(|_| anyhow::anyhow!("no WebSocket handshake within {connect_timeout:?}"))?
        .context("connect to collector")?;
    // The proof is over this connection's nonce, so it cannot be replayed on
    // another one. Without one there is nothing to sign.
    let nonce = response
        .headers()
        .get(HELLO_NONCE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| hex::decode(v).ok())
        .filter(|n| n.len() == 32)
        .context("the collector sent no hello nonce")?;
    let (mut sink, mut stream) = ws.split();
    send(
        &mut sink,
        &HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: env!("CARGO_PKG_VERSION").into(),
            host_id: host_id.to_string(),
            proof: key.sign_hello(&nonce, host_id, PROTOCOL_VERSION),
            // Every hennery host can park, take images (a session whose
            // agent offers none refuses them, plan 6a decision 2) and serve
            // the project picker (browsing under home works without roots).
            // `mcp_servers`: it passes a session's servers, isolated as
            // `mcp_isolation` says, and refuses those it cannot isolate
            // (plan 8c).
            capabilities: Capabilities(vec![
                Capability::Park,
                Capability::Images,
                Capability::Projects,
                Capability::ResolvePath,
                Capability::McpServers,
            ]),
            mcp_isolation: announce.mcp_isolation,
            workspace_roots: announce.workspace_roots,
            attached_sessions: attached()?,
        },
    )
    .await?;
    let answer = match tokio::time::timeout(read_timeout, stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<CollectorFrame>(&text)?,
        other => bail!("no hello_ack: {other:?}"),
    };
    Ok((sink, stream, answer))
}

/// Host shutdown (ACP core §2.3): the connection loop is already stopped;
/// dropping every handle closes each actor's command channel, so each takes
/// its graceful path — SIGTERM its adapter's group, SIGKILL after the grace.
/// Returning without waiting would let the runtime drop the actors instead,
/// and `Adapter::drop` SIGKILLs at once. Bounded: an actor still starting
/// does not read its commands and is left to that drop.
async fn shut_down(sessions: &Sessions, bound: Duration) {
    let handles = {
        let mut map = sessions.lock().expect("sessions lock");
        map.closing = true;
        std::mem::take(&mut map.handles)
    };
    let actors: Vec<_> = handles.into_values().map(|handle| handle.finished()).collect();
    if tokio::time::timeout(bound, futures::future::join_all(actors))
        .await
        .is_err()
    {
        tracing::warn!(?bound, "session actors did not stop in time; killing their adapters");
    }
}

async fn connect_once(
    cfg: &HostConfig,
    uplink: &Uplink,
    sessions: &Sessions,
    probes: &Probes,
    replies: &mut mpsc::UnboundedReceiver<HostFrame>,
    backoff: &mut Duration,
) -> Result<()> {
    let (mut sink, mut stream, answer) = handshake(
        &cfg.collector_url,
        &cfg.host_id,
        &cfg.key,
        Announce {
            workspace_roots: cfg.reported_roots(),
            mcp_isolation: cfg.mcp_isolation(),
        },
        || attached_sessions(uplink, sessions),
        cfg.connect_timeout,
        cfg.read_timeout,
    )
    .await?;
    match answer {
        CollectorFrame::HelloAck { committed, .. } => {
            for (session_id, seq) in committed {
                uplink.fast_forward(&session_id, seq)?;
            }
        }
        CollectorFrame::HelloError { code, message } => return Err(HelloRejected { code, message }.into()),
        other => bail!("expected hello_ack, got {other:?}"),
    }
    // A bare handshake is not proof the collector is actually committing
    // anything: a persistently failing collector (e.g. disk full) can still
    // ack `hello` and then drop every subsequent frame without acking it
    // (ws.rs never acks past a failed ingest). Resetting backoff here would
    // make the host hammer such a collector at `reconnect_min` forever;
    // instead it is reset below, once the first real `ack` lands, or —
    // failing that — once the connection has simply stayed up for
    // `healthy_after` (an idle host has nothing to ack). So a collector that
    // never acks anything now costs at most one reconnect per
    // `healthy_after`, not an ever-growing backoff.
    tracing::info!(collector = %cfg.collector_url, "connected to collector");

    // Resend everything unacked, then tell the collector we are done.
    let mut sent: HashMap<String, u64> = HashMap::new();
    send_pending(&mut sink, uplink, &mut sent).await?;
    send(&mut sink, &HostFrame::ResendComplete).await?;

    let mut ping = tokio::time::interval(cfg.ping_interval);
    ping.tick().await;
    // Tracks the deadline independently of `select!`'s per-iteration futures:
    // rebuilding `timeout(stream.next())` fresh every loop (as the earlier
    // version did) restarts its clock on every unrelated arm (a ping tick, an
    // outbox wakeup, a reply), so the "no frame in read_timeout" branch could
    // never actually fire. `deadline` only moves when a frame arrives.
    let mut deadline = Instant::now() + cfg.read_timeout;
    // An idle host has nothing to ack, so a connection that simply stays up
    // is proof enough too (the ack-based reset below covers busy hosts).
    let healthy = tokio::time::sleep(cfg.healthy_after);
    tokio::pin!(healthy);
    let mut proven = false;
    loop {
        tokio::select! {
            _ = &mut healthy, if !proven => {
                proven = true;
                *backoff = cfg.reconnect_min;
            }
            _ = uplink.changed() => send_pending(&mut sink, uplink, &mut sent).await?,
            Some(frame) = replies.recv() => send_reply(&mut sink, uplink, &mut sent, &frame).await?,
            _ = ping.tick() => sink.send(Message::Ping(Default::default())).await?,
            _ = tokio::time::sleep_until(deadline) => bail!("no frame from collector within {:?}", cfg.read_timeout),
            msg = stream.next() => {
                deadline = Instant::now() + cfg.read_timeout;
                match msg {
                    None => bail!("collector closed the connection"),
                    Some(Err(err)) => return Err(err.into()),
                    Some(Ok(Message::Text(text))) => match serde_json::from_str::<CollectorFrame>(&text) {
                        Ok(frame) => {
                            if matches!(frame, CollectorFrame::Ack { .. }) {
                                // Proof the collector is actually committing
                                // frames, not just accepting the handshake:
                                // only now is it safe to forget the backoff
                                // accumulated from earlier failed attempts.
                                *backoff = cfg.reconnect_min;
                            }
                            handle(cfg, uplink, sessions, probes, frame)?
                        }
                        // By the error's kind and place only: its text can
                        // quote a string of the frame, a header value say.
                        Err(err) => tracing::warn!(
                            kind = ?err.classify(),
                            line = err.line(),
                            column = err.column(),
                            "ignoring unknown or invalid frame"
                        ),
                    },
                    Some(Ok(Message::Close(_))) => bail!("collector closed the connection"),
                    Some(Ok(_)) => {} // ping/pong/binary: liveness only
                }
            },
        }
    }
}

/// `hello.attached_sessions`: every session whose actor is still running,
/// with its open turn (ACP core §5.1). Ended actors are pruned here.
fn attached_sessions(uplink: &Uplink, sessions: &Sessions) -> Result<Vec<AttachedSession>> {
    let live: Vec<(String, SessionHandle)> = {
        let mut map = sessions.lock().expect("sessions lock");
        map.handles.retain(|_, handle| !handle.is_ended());
        map.handles.iter().map(|(id, h)| (id.clone(), h.clone())).collect()
    };
    let mut out = Vec::new();
    for (session_id, handle) in live {
        out.push(AttachedSession {
            last_seq: uplink.last_seq(&session_id)?,
            open_turn_id: handle.open_turn_id(),
            session_id,
        });
    }
    out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Ok(out)
}

/// The running actor for `session_id`, if any.
fn live_session(sessions: &Sessions, session_id: &str) -> Option<SessionHandle> {
    sessions
        .lock()
        .expect("sessions lock")
        .handles
        .get(session_id)
        .filter(|h| !h.is_ended())
        .cloned()
}

fn not_attached(uplink: &Uplink, request_id: String) {
    uplink.reply(HostFrame::Error {
        request_id,
        code: "not_attached".into(),
        message: "session is not attached on this host".into(),
    });
}

/// A `start_session` or `resume_session`.
struct AttachRequest {
    request_id: String,
    session_id: String,
    committed_seq: u64,
    agent: String,
    cwd: String,
    attach: Attach,
    config: SessionConfig,
    mcp: McpDelivery,
}

/// Start or resume a session (ACP core §4.3), idempotently (§2.2).
fn attach(cfg: &HostConfig, uplink: &Uplink, sessions: &Sessions, req: AttachRequest) -> Result<()> {
    let Some(command) = cfg.agents.get(&req.agent).cloned() else {
        uplink.reply(HostFrame::Error {
            request_id: req.request_id,
            code: "unknown_agent".into(),
            message: format!("agent {} is not configured on this host", req.agent),
        });
        return Ok(());
    };
    // The adapter starts in exactly the directory whose hat the collector
    // decided (plan 5c decision 5): a cwd that no longer resolves to itself
    // (a symlink swapped in since, or one sent from before hats) is refused.
    if let Some(problem) = cwd_problem(&req.cwd) {
        uplink.reply(HostFrame::Error {
            request_id: req.request_id,
            code: "cwd_not_canonical".into(),
            message: problem,
        });
        return Ok(());
    }
    // Servers this host cannot keep the agent to, unless the collector
    // waived that, are refused before anything is spawned: never dropped,
    // never passed (plan 8c, the lane's L3).
    let profile = cfg.profile(&req.agent);
    if let Some(problem) =
        crate::profile::mcp_refusal(&req.agent, profile, &req.mcp.mcp_servers, req.mcp.isolation_waived)
    {
        uplink.reply(HostFrame::Error {
            request_id: req.request_id,
            code: "mcp_isolation_unavailable".into(),
            message: problem,
        });
        return Ok(());
    }
    // Before anything can be enqueued for this session: continue from the
    // larger of this host's counter and the collector's (ACP core §5.1), so
    // a session resumed after the outbox was lost never reuses a seq.
    uplink.fast_forward(&req.session_id, req.committed_seq)?;
    let options = cfg.session_options();
    spawn_or_restart(uplink, sessions, req, command, profile, options);
    Ok(())
}

/// Why `cwd` cannot be a session's directory here, if it cannot: it must be
/// a directory and in its canonical form (kernel spec §5.2).
fn cwd_problem(cwd: &str) -> Option<String> {
    match crate::paths::resolve(cwd, None) {
        Ok(resolved) if resolved.is_dir && resolved.canonical == cwd => None,
        Ok(resolved) if resolved.is_dir => Some(format!("{cwd:?} resolves to {:?} on this host", resolved.canonical)),
        Ok(_) => Some(format!("{cwd:?} is not a directory on this host")),
        Err(why) => Some(format!("{cwd:?}: {why}")),
    }
}

/// Re-emit `session_started` from a live actor (never a second adapter), or
/// spawn a fresh one. Restart, wait and launch are all decided from one
/// locked read of the map: deciding "is it ending" and "then launch" as two
/// separate locked reads (the old `attach`, separately from this function)
/// let a self-ending actor (task 9: idle reap, adapter exit, a stopped
/// cancel, or a start-failure kill grace) flip `is_ending` in the gap
/// between them, so the first read's "not ending yet" and the second read's
/// "not restartable any more" both looked safe to fall through to a launch —
/// running a second adapter alongside the first while it was still tearing
/// down (fix round 1). This closes that launch gap; `begin_ending` itself
/// still does not take the sessions lock, so it can still land between the
/// check below and `handle.send` — but that only ever answers a `Restart`
/// `not_attached` (retryable), never launches a second adapter.
fn spawn_or_restart(
    uplink: &Uplink,
    sessions: &Sessions,
    req: AttachRequest,
    command: AgentCommand,
    profile: Profile,
    options: SessionOptions,
) {
    let mut map = sessions.lock().expect("sessions lock");
    if map.closing {
        // Host shutdown: the collector sees this connection end and
        // reconciles the start after the next handshake (ACP core §5.1).
        tracing::info!(session_id = %req.session_id, "host shutting down; not attaching");
        return;
    }
    // The old actor is ending — a park or close is queued ahead of this
    // request, or it began ending by itself — and may still be killing its
    // adapter: a `Restart` sent now would be answered `not_attached`, and
    // launching a fresh adapter now would run it alongside the old one.
    // Wait for it to finish, then re-decide from scratch under a fresh lock:
    // by then the entry reads `is_ended` (the common case), or another
    // attach already replaced it (handled the same way, recursively).
    if let Some(old) = map
        .handles
        .get(&req.session_id)
        .filter(|h| h.is_ending() && !h.is_ended())
        .cloned()
    {
        drop(map);
        let (uplink, sessions) = (uplink.clone(), sessions.clone());
        tokio::spawn(async move {
            old.finished().await;
            spawn_or_restart(&uplink, &sessions, req, command, profile, options);
        });
        return;
    }
    if let Some(handle) = map.handles.get(&req.session_id).filter(|h| !h.is_ended())
        && handle.send(SessionCmd::Restart {
            request_id: req.request_id.clone(),
        })
    {
        return;
    }
    let launch = Launch {
        request_id: req.request_id,
        session_id: req.session_id.clone(),
        attach: req.attach,
        config: req.config,
        agent: command,
        cwd: PathBuf::from(req.cwd),
        profile,
        mcp_servers: req.mcp.mcp_servers,
    };
    map.handles
        .insert(req.session_id, session::launch(uplink.clone(), launch, options));
}

fn handle(
    cfg: &HostConfig,
    uplink: &Uplink,
    sessions: &Sessions,
    probes: &Probes,
    frame: CollectorFrame,
) -> Result<()> {
    match frame {
        CollectorFrame::StartSession {
            request_id,
            session_id,
            committed_seq,
            agent,
            cwd,
            config,
            // Carried for plan 8h's composed `CODEX_HOME`; unused here.
            hat_id: _,
            mcp,
        } => attach(
            cfg,
            uplink,
            sessions,
            AttachRequest {
                request_id,
                session_id,
                committed_seq,
                agent,
                cwd,
                attach: Attach::New,
                config,
                mcp,
            },
        )?,
        CollectorFrame::ResumeSession {
            request_id,
            session_id,
            committed_seq,
            agent,
            cwd,
            agent_session_id,
            config,
            hat_id: _,
            mcp,
        } => attach(
            cfg,
            uplink,
            sessions,
            AttachRequest {
                request_id,
                session_id,
                committed_seq,
                agent,
                cwd,
                attach: Attach::Load { agent_session_id },
                config,
                mcp,
            },
        )?,
        CollectorFrame::Prompt {
            request_id,
            session_id,
            turn_id,
            content,
        } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::Prompt {
                    request_id: request_id.clone(),
                    turn_id,
                    content,
                }) => {}
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::ParkSession { request_id, session_id } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::Park {
                    request_id: request_id.clone(),
                }) => {}
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::CloseSession { request_id, session_id } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::Close {
                    request_id: request_id.clone(),
                }) => {}
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::CancelTurn {
            request_id,
            session_id,
            turn_id,
        } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::Cancel {
                    request_id: request_id.clone(),
                    turn_id,
                }) => {}
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::SetConfig {
            request_id,
            session_id,
            config_id,
            value,
        } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::SetConfig {
                    request_id: request_id.clone(),
                    config_id,
                    value,
                }) => {}
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::AnswerPermission {
            request_id,
            session_id,
            pending_id,
            option_id,
        } => answer(
            uplink,
            sessions,
            request_id,
            &session_id,
            pending_id,
            Answer::Permission { option_id },
        ),
        CollectorFrame::AnswerElicitation {
            request_id,
            session_id,
            pending_id,
            action,
            content,
        } => answer(
            uplink,
            sessions,
            request_id,
            &session_id,
            pending_id,
            Answer::Elicitation { action, content },
        ),
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
        // Probes (ACP core §3.3, §7), answered from blocking threads.
        CollectorFrame::ListProjects { request_id } => {
            probes.list(uplink, request_id, cfg.workspace_roots.clone(), cfg.home.clone())
        }
        CollectorFrame::BrowseDirectory { request_id, path } => {
            probes.browse(uplink, request_id, path, cfg.workspace_roots.clone(), cfg.home.clone())
        }
        CollectorFrame::ResolvePath { request_id, path } => probes.resolve(uplink, request_id, path, cfg.home.clone()),
        // A purged hat (plan 9c decision 12): nothing of it is kept here
        // yet. Plan 8 deletes the hat's composed agent home here, after
        // checking that the id is `hat-<hex>` before building a path from
        // it (A7).
        // A purged hat (kernel spec §5.5). Plan 8 deletes the hat's
        // composed agent home here, and is bound by plan 9c's review: only
        // for an id that is `hat-<hex>`, never building a path from
        // anything else, and only once no adapter of that hat runs; until
        // then, deferred. Idempotent: it comes after every handshake.
        CollectorFrame::ForgetHat { hat_id } => tracing::info!(%hat_id, "the collector purged a hat"),
        CollectorFrame::HelloAck { .. } | CollectorFrame::HelloError { .. } => {}
    }
    Ok(())
}

/// An operator's answer goes to the session's live actor, which reports
/// `answer_result`. With no live actor there is no question to answer: the
/// rejection is correlated, like a prompt's, and the collector records the
/// answer as not delivered.
fn answer(
    uplink: &Uplink,
    sessions: &Sessions,
    request_id: String,
    session_id: &str,
    pending_id: String,
    answer: Answer,
) {
    match live_session(sessions, session_id) {
        Some(handle)
            if handle.send(SessionCmd::Answer {
                request_id: request_id.clone(),
                pending_id,
                answer,
            }) => {}
        _ => not_attached(uplink, request_id),
    }
}

async fn send_pending<S>(sink: &mut S, uplink: &Uplink, sent: &mut HashMap<String, u64>) -> Result<()>
where
    S: futures::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    for frame in uplink.pending()? {
        if let HostFrame::Session { session_id, seq, .. } = &frame {
            if sent.get(session_id).is_some_and(|last| seq <= last) {
                continue;
            }
            sent.insert(session_id.clone(), *seq);
        }
        send(sink, &frame).await?;
    }
    Ok(())
}

/// A reply, after every fact already in the outbox: the `select!` above is
/// unbiased, so without this a `not_attached` rejection could overtake the
/// `session_parked`/`session_closed` its actor emitted just before it.
async fn send_reply<S>(sink: &mut S, uplink: &Uplink, sent: &mut HashMap<String, u64>, frame: &HostFrame) -> Result<()>
where
    S: futures::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    send_pending(sink, uplink, sent).await?;
    send(sink, frame).await
}

async fn send<S>(sink: &mut S, frame: &HostFrame) -> Result<()>
where
    S: futures::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    sink.send(Message::text(serde_json::to_string(frame)?)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_proto::frames::{ParkReason, SessionBody};

    #[tokio::test]
    async fn a_reply_goes_out_after_the_facts_already_in_the_outbox() {
        let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
        uplink
            .emit(
                "s1",
                SessionBody::SessionParked {
                    reason: ParkReason::Operator,
                },
            )
            .unwrap();
        let reply = HostFrame::Error {
            request_id: "r1".into(),
            code: "not_attached".into(),
            message: "the session has ended on this host".into(),
        };
        let (mut sink, mut wire) = futures::channel::mpsc::unbounded::<Message>();
        let mut sent = HashMap::new();
        send_reply(&mut sink, &uplink, &mut sent, &reply).await.unwrap();
        // A fact already on the wire is not sent twice.
        send_reply(&mut sink, &uplink, &mut sent, &reply).await.unwrap();
        drop(sink);
        let mut kinds = Vec::new();
        while let Some(Message::Text(text)) = wire.next().await {
            let frame: HostFrame = serde_json::from_str(&text).unwrap();
            kinds.push(match frame {
                HostFrame::Session { seq, .. } => format!("session#{seq}"),
                HostFrame::Error { code, .. } => code,
                other => format!("{other:?}"),
            });
        }
        assert_eq!(kinds, ["session#1", "not_attached", "not_attached"]);
    }
}
