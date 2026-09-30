//! The host's connection to the collector (ACP core §2, §5).
//!
//! Adapters belong to the host process, not to this connection: a dropped
//! socket only ends `connect_once`; session actors keep running and keep
//! writing to the outbox, which is resent on the next connection.

use crate::outbox::Outbox;
use crate::session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
use crate::uplink::Uplink;
use anyhow::{Context, Result, bail};
use futures::{SinkExt, StreamExt};
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone)]
pub struct HostConfig {
    /// e.g. `ws://127.0.0.1:7117/api/hosts/ws`
    pub collector_url: String,
    pub host_id: String,
    /// Walking skeleton: shared development token (ACP core §3.5 replaces it).
    pub token: String,
    pub data_dir: PathBuf,
    pub agents: HashMap<String, AgentCommand>,
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
}

impl HostConfig {
    pub fn new(
        collector_url: impl Into<String>,
        host_id: impl Into<String>,
        token: impl Into<String>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            collector_url: collector_url.into(),
            host_id: host_id.into(),
            token: token.into(),
            data_dir,
            agents: HashMap::new(),
            reconnect_min: Duration::from_millis(500),
            reconnect_max: Duration::from_secs(30),
            ping_interval: Duration::from_secs(15),
            read_timeout: Duration::from_secs(45),
            idle_timeout: session::IDLE_TIMEOUT,
            connect_timeout: Duration::from_secs(10),
            healthy_after: Duration::from_secs(60),
        }
    }

    /// Options for every session actor this host spawns.
    pub fn session_options(&self) -> SessionOptions {
        SessionOptions {
            idle_timeout: (!self.idle_timeout.is_zero()).then_some(self.idle_timeout),
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

/// Run the host until the process exits. Reconnects with exponential backoff.
pub async fn run(cfg: HostConfig) -> Result<()> {
    run_until(cfg, std::future::pending()).await
}

/// Run the host until `shutdown` resolves.
pub async fn run_until(cfg: HostConfig, shutdown: impl Future<Output = ()>) -> Result<()> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let outbox = Outbox::open(&cfg.data_dir.join("outbox.db"))?;
    let (uplink, mut replies) = Uplink::new(outbox);
    let sessions: Sessions = Arc::new(Mutex::new(SessionMap::default()));
    let serve = async {
        let mut backoff = cfg.reconnect_min;
        loop {
            if let Err(err) = connect_once(&cfg, &uplink, &sessions, &mut replies, &mut backoff).await {
                tracing::warn!(error = %err, "collector connection ended");
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(cfg.reconnect_max);
        }
    };
    tokio::select! {
        _ = serve => unreachable!("the connection loop never ends"),
        _ = shutdown => {}
    }
    shut_down(&sessions, cfg.session_options().kill_grace + Duration::from_secs(1)).await;
    Ok(())
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
    replies: &mut mpsc::UnboundedReceiver<HostFrame>,
    backoff: &mut Duration,
) -> Result<()> {
    let (ws, _) = tokio::time::timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async(&cfg.collector_url),
    )
    .await
    .map_err(|_| anyhow::anyhow!("no WebSocket handshake within {:?}", cfg.connect_timeout))?
    .context("connect to collector")?;
    let (mut sink, mut stream) = ws.split();

    let attached = attached_sessions(uplink, sessions)?;
    send(
        &mut sink,
        &HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: env!("CARGO_PKG_VERSION").into(),
            host_id: cfg.host_id.clone(),
            token: cfg.token.clone(),
            // Every hennery host can park. `projects` and `images` come
            // with the probes and with image prompts.
            capabilities: Capabilities(vec![Capability::Park]),
            attached_sessions: attached,
        },
    )
    .await?;

    match tokio::time::timeout(cfg.read_timeout, stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<CollectorFrame>(&text)? {
            CollectorFrame::HelloAck { committed, .. } => {
                for (session_id, seq) in committed {
                    uplink.fast_forward(&session_id, seq)?;
                }
            }
            CollectorFrame::HelloError { code, message } => bail!("hello rejected: {code}: {message}"),
            other => bail!("expected hello_ack, got {other:?}"),
        },
        other => bail!("no hello_ack: {other:?}"),
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
                            handle(cfg, uplink, sessions, frame)?
                        }
                        Err(err) => tracing::warn!(error = %err, "ignoring unknown or invalid frame"),
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
    // Before anything can be enqueued for this session: continue from the
    // larger of this host's counter and the collector's (ACP core §5.1), so
    // a session resumed after the outbox was lost never reuses a seq.
    uplink.fast_forward(&req.session_id, req.committed_seq)?;
    let options = cfg.session_options();
    spawn_or_restart(uplink, sessions, req, command, options);
    Ok(())
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
            spawn_or_restart(&uplink, &sessions, req, command, options);
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
    };
    map.handles
        .insert(req.session_id, session::launch(uplink.clone(), launch, options));
}

fn handle(cfg: &HostConfig, uplink: &Uplink, sessions: &Sessions, frame: CollectorFrame) -> Result<()> {
    match frame {
        CollectorFrame::StartSession {
            request_id,
            session_id,
            committed_seq,
            agent,
            cwd,
            config,
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
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
        CollectorFrame::HelloAck { .. } | CollectorFrame::HelloError { .. } => {}
    }
    Ok(())
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
