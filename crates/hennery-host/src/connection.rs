//! The host's connection to the collector (ACP core §2, §5).
//!
//! Adapters belong to the host process, not to this connection: a dropped
//! socket only ends `connect_once`; session actors keep running and keep
//! writing to the outbox, which is resent on the next connection.

use crate::outbox::Outbox;
use crate::session::{self, AgentCommand, SessionCmd};
use crate::uplink::Uplink;
use anyhow::{Context, Result, bail};
use futures::{SinkExt, StreamExt};
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{AttachedSession, CollectorFrame, HostFrame};
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
        }
    }
}

type Sessions = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<SessionCmd>>>>;

/// Run the host until the process exits. Reconnects with exponential backoff.
pub async fn run(cfg: HostConfig) -> Result<()> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let outbox = Outbox::open(&cfg.data_dir.join("outbox.db"))?;
    let (uplink, mut replies) = Uplink::new(outbox);
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let mut backoff = cfg.reconnect_min;
    loop {
        if let Err(err) = connect_once(&cfg, &uplink, &sessions, &mut replies, &mut backoff).await {
            tracing::warn!(error = %err, "collector connection ended");
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(cfg.reconnect_max);
    }
}

async fn connect_once(
    cfg: &HostConfig,
    uplink: &Uplink,
    sessions: &Sessions,
    replies: &mut mpsc::UnboundedReceiver<HostFrame>,
    backoff: &mut Duration,
) -> Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&cfg.collector_url)
        .await
        .context("connect to collector")?;
    let (mut sink, mut stream) = ws.split();

    let attached = {
        let ids: Vec<String> = sessions.lock().expect("sessions lock").keys().cloned().collect();
        let mut out = Vec::new();
        for session_id in ids {
            let last_seq = uplink.last_seq(&session_id)?;
            out.push(AttachedSession {
                session_id,
                last_seq,
                open_turn_id: None,
            });
        }
        out
    };
    send(
        &mut sink,
        &HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: env!("CARGO_PKG_VERSION").into(),
            host_id: cfg.host_id.clone(),
            token: cfg.token.clone(),
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
    // instead it is reset below, only once the first real `ack` lands.
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
    loop {
        tokio::select! {
            _ = uplink.changed() => send_pending(&mut sink, uplink, &mut sent).await?,
            Some(frame) = replies.recv() => send(&mut sink, &frame).await?,
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

fn handle(cfg: &HostConfig, uplink: &Uplink, sessions: &Sessions, frame: CollectorFrame) -> Result<()> {
    match frame {
        CollectorFrame::StartSession {
            request_id,
            session_id,
            agent,
            cwd,
        } => {
            let Some(command) = cfg.agents.get(&agent).cloned() else {
                uplink.reply(HostFrame::Error {
                    request_id,
                    code: "unknown_agent".into(),
                    message: format!("agent {agent} is not configured on this host"),
                });
                return Ok(());
            };
            let mut map = sessions.lock().expect("sessions lock");
            if map.contains_key(&session_id) {
                // Idempotent: a retried start for an attached session is a no-op;
                // its session_started fact is already in the outbox.
                return Ok(());
            }
            let tx = session::start(
                uplink.clone(),
                request_id,
                session_id.clone(),
                command,
                PathBuf::from(cwd),
            );
            map.insert(session_id, tx);
        }
        CollectorFrame::Prompt {
            request_id,
            session_id,
            turn_id,
            content,
        } => {
            let tx = sessions.lock().expect("sessions lock").get(&session_id).cloned();
            match tx {
                Some(tx)
                    if tx
                        .send(SessionCmd::Prompt {
                            request_id: request_id.clone(),
                            turn_id,
                            content,
                        })
                        .is_ok() => {}
                _ => uplink.reply(HostFrame::Error {
                    request_id,
                    code: "not_attached".into(),
                    message: "session is not attached on this host".into(),
                }),
            }
        }
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

async fn send<S>(sink: &mut S, frame: &HostFrame) -> Result<()>
where
    S: futures::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    sink.send(Message::text(serde_json::to_string(frame)?)).await?;
    Ok(())
}
