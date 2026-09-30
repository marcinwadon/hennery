//! The host WebSocket endpoint (ACP core §3, §5).

use crate::AppState;
use crate::hub::Undo;
use crate::store::Store;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderValue;
use axum::response::Response;
use axum::routing::get;
use futures::{SinkExt, StreamExt};
use hennery_kernel::hosts::HelloCheck;
use hennery_kernel::lifecycle::LifecycleHooks;
use hennery_kernel::secret::{random_bytes, unix_now};
use hennery_proto::frames::{CollectorFrame, HostFrame, SessionBody};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION, protocol_major};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use tokio::sync::mpsc;

const MAX_FRAME: usize = 32 << 20;
const PING_INTERVAL: Duration = Duration::from_secs(15);
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(45);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

pub fn router(state: AppState) -> Router {
    Router::new().route("/api/hosts/ws", get(upgrade)).with_state(state)
}

/// The upgrade response carries this connection's nonce: the host signs it
/// in `hello` (ACP core §3.5), so a proof is good for one connection only.
async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let nonce = random_bytes::<32>();
    let mut response = ws
        .max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| serve(socket, state, nonce));
    response.headers_mut().insert(
        HELLO_NONCE_HEADER,
        HeaderValue::from_str(&hex::encode(nonce)).expect("hex is a valid header value"),
    );
    response
}

fn text(frame: &CollectorFrame) -> Message {
    Message::Text(serde_json::to_string(frame).expect("frame serializes").into())
}

async fn serve(socket: WebSocket, state: AppState, nonce: [u8; 32]) {
    let (mut sink, mut stream) = socket.split();

    // 1. hello: first frame, authenticated.
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str::<HostFrame>(&t).ok(),
        _ => None,
    };
    let Some(HostFrame::Hello {
        protocol_version,
        host_version,
        host_id,
        proof,
        capabilities,
        attached_sessions,
    }) = hello
    else {
        return;
    };
    let reject = |code: &str, message: &str| CollectorFrame::HelloError {
        code: code.into(),
        message: message.into(),
    };
    if protocol_major(&protocol_version) != protocol_major(PROTOCOL_VERSION) {
        let _ = sink
            .send(text(&reject("incompatible", "unsupported protocol major")))
            .await;
        return;
    }
    // The host id is never taken on its word (ACP core §3.5). Until the
    // proof checks out it is logged `Debug`-escaped (`?host_id`): an
    // unauthenticated peer chose it.
    match state.hosts.check_hello(&host_id, &nonce, &protocol_version, &proof) {
        Ok(HelloCheck::Accepted) => {}
        Ok(HelloCheck::Revoked) => {
            tracing::warn!(%host_id, "revoked host tried to connect");
            let _ = sink
                .send(text(&reject(
                    "revoked",
                    "this host was revoked; pair it again with `hennery host join`",
                )))
                .await;
            return;
        }
        Ok(HelloCheck::BadProof) => {
            tracing::warn!(?host_id, "hello with an unknown host id or an invalid proof");
            let _ = sink
                .send(text(&reject("bad_proof", "unknown host or invalid proof")))
                .await;
            return;
        }
        Err(err) => {
            tracing::error!(?host_id, error = %err, "checking a hello failed");
            return;
        }
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<CollectorFrame>();
    let Some(registration) = state.hub.register(&host_id, tx.clone(), capabilities.clone()) else {
        let _ = sink
            .send(text(&reject(
                "already_connected",
                "another connection for this host is live",
            )))
            .await;
        return;
    };
    // A revoke that landed after the proof was checked but before this
    // registration found no connection to close: this one must not live
    // on to reconcile what the revoke parked (kernel spec §4.3).
    if !matches!(state.hosts.is_revoked(&host_id), Ok(false)) {
        state.hub.unregister(&host_id, registration.conn_id);
        let _ = sink
            .send(text(&reject(
                "revoked",
                "this host was revoked; pair it again with `hennery host join`",
            )))
            .await;
        return;
    }

    let mut committed = BTreeMap::new();
    for a in &attached_sessions {
        committed.insert(
            a.session_id.clone(),
            state.store.committed_seq(&a.session_id).unwrap_or(0),
        );
    }
    let ack = CollectorFrame::HelloAck {
        protocol_version: PROTOCOL_VERSION.into(),
        collector_version: env!("CARGO_PKG_VERSION").into(),
        committed,
    };
    let conn_id = registration.conn_id;
    if sink.send(text(&ack)).await.is_err() {
        state.hub.unregister(&host_id, conn_id);
        crate::offline::after_disconnect(&state, host_id, conn_id);
        return;
    }
    if let Err(err) = state
        .hosts
        .record_hello(&host_id, &host_version, &capabilities, unix_now())
    {
        tracing::warn!(%host_id, error = %err, "recording the host's hello failed");
    }
    tracing::info!(%host_id, "host connected");

    // 2. writer: frames for this host plus keepalive pings.
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.tick().await;
        loop {
            tokio::select! {
                frame = rx.recv() => match frame {
                    Some(frame) => if sink.send(text(&frame)).await.is_err() { break },
                    None => break,
                },
                _ = ping.tick() => if sink.send(Message::Ping(Default::default())).await.is_err() { break },
            }
        }
    });

    // 3. reader. Requests reach this host only once its resend is complete
    // and reconciled (`mark_ready`).
    let mut reconciled = false;
    // `close_session` frames the reconciliation loop re-sends by itself
    // (below), outside `Hub::request_for_session`: no waiter is registered
    // for them, so their `request_id` is tracked here instead. A `not_attached`
    // rejection for one still has to close the session collector-side
    // (decision 7): the host no longer has it attached, but the store would
    // otherwise keep it `active` with `close_requested = 1` (until the host's
    // next reconnect reconciles it). Only while the session is still what
    // reconciliation asked to close: the answer can arrive after the operator
    // has resumed it, and must not close that fresh start (final review F1).
    let mut reconcile_closes: HashMap<String, String> = HashMap::new();
    loop {
        // Biased: a kicked connection (a timed-out request, a revoke) reads
        // no further frame, however many are waiting.
        let next = tokio::select! {
            biased;
            _ = state.shutdown.cancelled() => break,
            _ = registration.kicked.cancelled() => break,
            next = tokio::time::timeout(READ_TIMEOUT, stream.next()) => next,
        };
        let msg = match next {
            Ok(Some(Ok(msg))) => msg,
            _ => break,
        };
        let Message::Text(t) = msg else {
            if matches!(msg, Message::Close(_)) {
                break;
            }
            continue;
        };
        let frame = match serde_json::from_str::<HostFrame>(&t) {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(%host_id, error = %err, "ignoring unknown or invalid frame");
                continue;
            }
        };
        match frame {
            HostFrame::Session { session_id, seq, body } => {
                // A host may only write to its own sessions. Acks are
                // cumulative and the host prunes its outbox on ack, so any
                // store error here (lookup or ingest) must drop the
                // connection rather than silently skip the frame: acking a
                // later frame would tell the host this one is safe to
                // discard forever (ACP core §3.3, §5).
                match state.store.session(&session_id) {
                    Ok(Some(row)) if row.host_id == host_id => {}
                    Ok(_) => {
                        tracing::warn!(%host_id, %session_id, "frame for a session this host does not own");
                        continue;
                    }
                    Err(err) => {
                        tracing::error!(%host_id, %session_id, error = %err, "store lookup failed; dropping connection without acking");
                        break;
                    }
                }
                match state.store.ingest(&session_id, seq, &body) {
                    Ok(created) => {
                        for event in created {
                            state.hub.publish(event);
                        }
                        match &body {
                            SessionBody::SessionStarted { request_id, .. }
                            | SessionBody::TurnStarted { request_id, .. }
                            | SessionBody::ConfigApplied { request_id, .. } => {
                                state.hub.resolve(request_id, body.clone());
                            }
                            SessionBody::StartFailed {
                                request_id,
                                code,
                                message,
                            } => {
                                state.hub.reject(request_id, code.clone(), message.clone());
                            }
                            // Park and close are completed by facts that
                            // name only the session (ACP core §3.2).
                            SessionBody::SessionParked { .. } | SessionBody::SessionClosed => {
                                state.hub.resolve_session(&session_id, body.clone());
                            }
                            // `cancel_turn` is completed by its turn's end,
                            // scoped to the session it belongs to (final
                            // review M1): this host already owns
                            // `session_id` (checked above), but that must
                            // not let it complete another session's waiter
                            // by naming that session's turn_id here.
                            SessionBody::TurnEnded { turn_id, .. } => {
                                state.hub.resolve_turn(&session_id, turn_id, body.clone());
                            }
                            _ => {}
                        }
                        let _ = tx.send(CollectorFrame::Ack {
                            session_id,
                            ack_seq: seq,
                        });
                    }
                    Err(err) => {
                        tracing::error!(%host_id, error = %err, "ingest failed; dropping connection without acking");
                        break;
                    }
                }
            }
            HostFrame::Error {
                request_id,
                code,
                message,
            } => {
                if let Some(session_id) = reconcile_closes.remove(&request_id) {
                    if code == "not_attached" {
                        match state.store.close_after_rejected_reconcile_close(&session_id) {
                            Ok(events) => {
                                for event in events {
                                    state.hub.publish(event);
                                }
                            }
                            Err(err) => {
                                tracing::error!(%host_id, %session_id, error = %err, "closing after a rejected reconcile close failed");
                            }
                        }
                    } else {
                        tracing::warn!(%host_id, %session_id, %code, %message, "reconcile close_session rejected");
                    }
                } else {
                    // Take the waiter first, so no timeout can answer it any
                    // more; undo what the request changed; then answer. The
                    // HTTP answer and the store agree, and the store is
                    // right even when no handler waits (decision 6). If the
                    // undo itself fails, the store no longer matches what
                    // the rejection says: answer delivery-unknown instead of
                    // `Rejected`, and drop the connection so the next
                    // handshake's reconciliation settles it (final review
                    // F1) — never answer `Rejected` and then break.
                    if let Some(rejection) = state.hub.take_rejected(&request_id) {
                        let undo_failed = match rejection.undo() {
                            Some(undo) => undo_rejected(&state.store, undo, &code)
                                .err()
                                .map(|err| (undo.clone(), err)),
                            None => None,
                        };
                        match undo_failed {
                            None => rejection.answer(code, message),
                            Some((undo, err)) => {
                                tracing::error!(%host_id, ?undo, error = %err, "undoing a rejected request failed; dropping connection");
                                rejection.delivery_unknown();
                                break;
                            }
                        }
                    } else {
                        // No waiter: an answer (its verdict comes from
                        // `answer_result` or its question's resolution),
                        // or a request whose caller already gave up.
                        tracing::warn!(%host_id, %request_id, %code, %message, "host refused a request nobody waits for");
                    }
                }
            }
            HostFrame::ResendComplete if !reconciled => {
                // A revoke whose wait for this very connection timed out:
                // it is still live and about to reconcile, but nothing it
                // reports must ever reattach what the revoke already
                // parked, or mark it ready (kernel spec §4.3, fix round 1
                // F1 defence in depth). Fail closed if the check errors.
                if !matches!(state.hosts.is_revoked(&host_id), Ok(false)) {
                    tracing::warn!(%host_id, "revoked host reached resend_complete; closing without reconciling");
                    break;
                }
                // Everything the host had in its outbox is ingested: only now
                // is anything still unresolved known to be lost (ACP core
                // §5.1 step 4).
                match state.store.reconcile_host(&host_id, &attached_sessions) {
                    Ok(done) => {
                        for event in done.events {
                            state.hub.publish(event);
                        }
                        for session_id in done.close {
                            let request_id = uuid::Uuid::now_v7().to_string();
                            reconcile_closes.insert(request_id.clone(), session_id.clone());
                            let _ = tx.send(CollectorFrame::CloseSession { request_id, session_id });
                        }
                        reconciled = true;
                        state.hub.mark_ready(&host_id, conn_id);
                        // The answer queue (ACP core §5.1 step 4), after the
                        // reconciliation cancelled what a restart lost, and
                        // after `mark_ready`: an answer submitted meanwhile
                        // is either read here or sent by its own handler
                        // (or both; the host dedupes by pending id).
                        match state.store.answers_to_send(&host_id) {
                            Ok(answers) => {
                                for frame in answers {
                                    let _ = tx.send(frame);
                                }
                            }
                            Err(err) => {
                                tracing::error!(%host_id, error = %err, "reading the answer queue failed; dropping connection");
                                break;
                            }
                        }
                        tracing::info!(%host_id, "host reconciled");
                    }
                    Err(err) => {
                        tracing::error!(%host_id, error = %err, "reconciliation failed; dropping connection");
                        break;
                    }
                }
            }
            HostFrame::ResendComplete => tracing::warn!(%host_id, "ignoring repeated resend_complete"),
            HostFrame::Hello { .. } => tracing::warn!(%host_id, "ignoring repeated hello"),
        }
    }

    writer.abort();
    state.hub.unregister(&host_id, conn_id);
    tracing::info!(%host_id, "host disconnected");
    // A revoke that gave up waiting for this connection parked the sessions
    // while it could still apply frames. Parking again now that it is gone
    // converges them (the hook is idempotent).
    if matches!(state.hosts.is_revoked(&host_id), Ok(true))
        && let Err(err) = state.on_host_revoked(&host_id)
    {
        tracing::error!(%host_id, error = %err, "parking a revoked host's sessions failed");
    }
    crate::offline::after_disconnect(&state, host_id, conn_id);
}

/// Revert what a request the host rejected changed in the store: a start or
/// resume fails with the host's code, a prompt's turn is removed.
fn undo_rejected(store: &Store, undo: &Undo, code: &str) -> anyhow::Result<()> {
    match undo {
        Undo::Start { session_id } => store.mark_failed_if_starting(session_id, code),
        Undo::Prompt { session_id, turn_id } => store.abandon_turn(session_id, turn_id),
    }
}
