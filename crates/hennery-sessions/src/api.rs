//! Session REST and SSE endpoints (ACP core §9), walking-skeleton subset.

use crate::AppState;
use crate::content::{self, Refusal};
use crate::hub::{RequestError, Undo};
use crate::store::{AnswerSubmission, ResumeRequest, Store};
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{self, Stream, StreamExt};
use hennery_kernel::json::ApiJson;
use hennery_kernel::operator::Authenticated;
use hennery_proto::frames::{Capability, CollectorFrame, Indexed, SessionBody};
use hennery_proto::rest::{
    AnswerRequest, AnswerResponse, ApiError, CancelResponse, ConfigRequest, EventDto, LifecycleResponse, OpenTurn,
    PendingItem, PromptRequest, PromptResponse, SessionCatalog, SessionDetail, StartSessionRequest,
    StartSessionResponse,
};
use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

const START_TIMEOUT: Duration = Duration::from_secs(90);
/// At least the WebSocket read deadline, so a half-open socket is detected
/// before the request gives up (ACP core §3.4).
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
/// `park_session` / `close_session` (ACP core §3.4).
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(60);
/// `cancel_turn` (ACP core §3.4). The host stops an adapter that ignores the
/// cancel well before this (`hennery_host::session::CANCEL_GRACE`).
const CANCEL_TIMEOUT: Duration = Duration::from_secs(60);
/// `set_config` (ACP core §3.4). The host answers a switch the adapter
/// leaves hanging well before this (`hennery_host::session::CONFIG_TIMEOUT`).
const CONFIG_TIMEOUT: Duration = Duration::from_secs(60);

// ACP core §3.4: every state-changing request waits at least the read
// deadline, so on a live connection its fact or rejection arrives first and
// a half-open socket is dropped before the request gives up. `Duration`'s
// `>` is not const.
const _: () = assert!(
    START_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && PROMPT_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && TEARDOWN_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && CANCEL_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && CONFIG_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis(),
    "every request timeout must exceed the host connection's read deadline"
);

// Plan 6a: a prompt the route accepts fits in one frame to its host, with
// room for the frame's own fields.
const _: () = assert!(
    content::PROMPT_BODY_LIMIT + (1 << 20) <= crate::ws::MAX_FRAME,
    "a prompt request's body must fit in a host frame"
);

/// Every route here is an operator's (kernel spec §3.3).
pub fn router(state: AppState) -> Router {
    let routes = Router::new()
        .route("/api/sessions", post(start_session))
        .route("/api/sessions/{id}", get(session_detail))
        .route("/api/sessions/{id}/resume", post(resume))
        // The one route that reads more than axum's default 2 MB (plan 6a).
        .route(
            "/api/sessions/{id}/prompt",
            post(prompt).layer(DefaultBodyLimit::max(content::PROMPT_BODY_LIMIT)),
        )
        .route("/api/sessions/{id}/cancel", post(cancel))
        .route("/api/sessions/{id}/park", post(park))
        .route("/api/sessions/{id}/close", post(close))
        .route("/api/sessions/{id}/catalog", get(catalog))
        .route("/api/sessions/{id}/config", post(set_config))
        .route("/api/sessions/{id}/pending/{pending_id}/answer", post(answer))
        .route("/api/sessions/{id}/events", get(events))
        .route("/api/stream/sessions/{id}", get(stream_session))
        .route("/api/attachments/{sha256}", get(attachment))
        .route("/api/settings/attachments", get(attachment_usage));
    hennery_kernel::auth::operator_only(routes, state.operator.clone()).with_state(state)
}

pub(crate) fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiError {
            code: code.into(),
            message: message.into(),
            session_id: None,
        }),
    )
        .into_response()
}

/// Like `error`, but with a `session_id` the caller can otherwise have no
/// way to learn (e.g. a session start whose delivery is unknown: the
/// session was created and may still start, but a 202 never arrived).
fn error_with_session(status: StatusCode, code: &str, message: impl Into<String>, session_id: String) -> Response {
    (
        status,
        Json(ApiError {
            code: code.into(),
            message: message.into(),
            session_id: Some(session_id),
        }),
    )
        .into_response()
}

pub(crate) fn internal(err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "internal error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

fn request_failed(err: RequestError) -> Response {
    match err {
        RequestError::NotConnected => error(StatusCode::CONFLICT, "host_offline", "the host is not connected"),
        RequestError::Rejected { code, message } => {
            let status = match code.as_str() {
                "not_attached" | "turn_in_progress" | "not_running" | "unknown_option" | "images_unsupported" => {
                    StatusCode::CONFLICT
                }
                "unknown_agent" | "start_failed" => StatusCode::BAD_GATEWAY,
                "invalid" => StatusCode::BAD_REQUEST,
                _ => StatusCode::BAD_GATEWAY,
            };
            error(status, &code, message)
        }
        RequestError::DeliveryUnknown => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "delivery_unknown",
            "host disconnected; delivery unknown",
        ),
        // Probes only (`Hub::probe`); a session request never meets them.
        RequestError::Unsupported => error(StatusCode::CONFLICT, "unsupported", "the host does not support this"),
        RequestError::Busy => error(StatusCode::SERVICE_UNAVAILABLE, "busy", "the host is busy; try again"),
    }
}

/// A resume answers like a start (decision 3): whatever code the host
/// rejects it with, the session is marked `failed` with that code and the
/// answer is 502 with it, so the answer and the session agree. An offline
/// host and an unknown delivery answer as for any other request.
fn resume_failed(err: RequestError) -> Response {
    match err {
        RequestError::Rejected { code, message } => error(StatusCode::BAD_GATEWAY, &code, message),
        other => request_failed(other),
    }
}

async fn start_session(State(state): State<AppState>, ApiJson(req): ApiJson<StartSessionRequest>) -> Response {
    let session_id = uuid::Uuid::now_v7().to_string();
    if let Err(err) = state
        .store
        .create_session(&session_id, &req.host_id, &req.agent, &req.cwd)
    {
        return internal(err);
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::StartSession {
        request_id: request_id.clone(),
        session_id: session_id.clone(),
        // A session minted just now has nothing committed.
        committed_seq: 0,
        agent: req.agent,
        cwd: req.cwd,
        config: req.config,
    };
    let undo = Undo::Start {
        session_id: session_id.clone(),
    };
    match state
        .hub
        .request_with_undo(&req.host_id, &request_id, frame, START_TIMEOUT, undo)
        .await
    {
        Ok(_) => (StatusCode::ACCEPTED, Json(StartSessionResponse { session_id })).into_response(),
        // The session was created and may still start; without its id here,
        // the caller would have no way to look it up (ACP core §3.4).
        Err(RequestError::DeliveryUnknown) => error_with_session(
            StatusCode::SERVICE_UNAVAILABLE,
            "delivery_unknown",
            "host disconnected; delivery unknown",
            session_id,
        ),
        // Never sent.
        Err(RequestError::NotConnected) => {
            if let Err(e) = state.store.mark_failed(&session_id, "host_offline") {
                return internal(e);
            }
            request_failed(RequestError::NotConnected)
        }
        // The socket task has already failed the session with the host's
        // code (`Undo::Start`).
        Err(err) => request_failed(err),
    }
}

/// Session detail (ACP core §9): the list item plus the open turn.
async fn session_detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    let open_turn = match session.open_turn_id {
        Some(turn_id) => match state.store.turn_state(&turn_id) {
            Ok(Some(turn_state)) => Some(OpenTurn {
                turn_id,
                state: turn_state,
            }),
            Ok(None) => None,
            Err(err) => return internal(err),
        },
        None => None,
    };
    let pending = match state.store.open_pending(&id) {
        Ok(pending) => pending,
        Err(err) => return internal(err),
    };
    Json(SessionDetail {
        session_id: session.id,
        host_id: session.host_id,
        agent: session.agent,
        cwd: session.cwd,
        lifecycle: session.lifecycle,
        activity: session.activity,
        failure_reason: session.failure_reason,
        presumed_parked: session.presumed_parked,
        open_turn,
        pending,
    })
    .into_response()
}

/// Resume a parked, closed or failed session (ACP core §4.3): 202 with the
/// lifecycle once the host's `session_started` is ingested.
async fn resume(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    let busy = |lifecycle: &str| {
        error(
            StatusCode::CONFLICT,
            lifecycle,
            format!("the session is already {lifecycle}"),
        )
    };
    if matches!(session.lifecycle.as_str(), "starting" | "active") {
        return busy(&session.lifecycle);
    }
    // Checked before any state changes, so an offline host leaves the
    // session exactly as it was.
    if !state.hub.is_ready(&session.host_id) {
        return request_failed(RequestError::NotConnected);
    }
    let (agent_session_id, committed_seq, config) = match state.store.request_resume(&id) {
        Ok(ResumeRequest::Starting {
            events,
            agent_session_id,
            committed_seq,
            config,
        }) => {
            for event in events {
                state.hub.publish(event);
            }
            (agent_session_id, committed_seq, config)
        }
        // A concurrent resume got there first (ACP core §12 scenario 11).
        Ok(ResumeRequest::Busy(lifecycle)) => return busy(&lifecycle),
        Ok(ResumeRequest::NoRecord) => {
            return error(
                StatusCode::CONFLICT,
                "agent_has_no_record",
                "the agent never created this session; start a new one in the same project",
            );
        }
        Ok(ResumeRequest::NotFound) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ResumeSession {
        request_id: request_id.clone(),
        session_id: id.clone(),
        committed_seq,
        agent: session.agent,
        cwd: session.cwd,
        agent_session_id,
        // Re-applied after the load (ACP core §4.3).
        config,
    };
    let undo = Undo::Start { session_id: id.clone() };
    match state
        .hub
        .request_with_undo(&session.host_id, &request_id, frame, START_TIMEOUT, undo)
        .await
    {
        Ok(_) => lifecycle_response(&state, &id),
        // Still `starting`: the next handshake reconciles it (ACP core §3.4).
        Err(RequestError::DeliveryUnknown) => request_failed(RequestError::DeliveryUnknown),
        // Never sent: the host went away since the check above.
        Err(RequestError::NotConnected) => {
            if let Err(e) = state.store.mark_failed_if_starting(&id, "host_offline") {
                return internal(e);
            }
            resume_failed(RequestError::NotConnected)
        }
        // The socket task has already failed the session with the host's
        // code (`Undo::Start`).
        Err(err) => resume_failed(err),
    }
}

/// Send a prompt (ACP core §4.4, §9). Everything that can refuse it is
/// checked before anything is written (plan 6a): the content, the session,
/// and for images the host's `images` capability. Its images are then
/// saved, and the turn opened with references to them; only the frame to
/// the host carries their bytes, in the blocks as checked (the review's
/// A1).
async fn prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<PromptRequest>,
) -> Response {
    let mut checked = match content::check(req.content) {
        Ok(checked) => checked,
        Err(Refusal::Empty) => {
            return error(
                StatusCode::BAD_REQUEST,
                "empty_prompt",
                "a prompt needs text or an image",
            );
        }
        Err(Refusal::Invalid(why)) => return error(StatusCode::BAD_REQUEST, "invalid_content", why),
        Err(Refusal::TooLarge(why)) => return error(StatusCode::PAYLOAD_TOO_LARGE, "content_too_large", why),
    };
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    if session.lifecycle != "active" {
        return error(StatusCode::CONFLICT, "not_attached", "resume the session first");
    }
    if !checked.images.is_empty() {
        // A host that is gone has no capabilities: it is offline, not one
        // without images.
        if !state.hub.is_ready(&session.host_id) {
            return request_failed(RequestError::NotConnected);
        }
        // Never an image to a host that did not announce it (ACP core §3.3).
        if !state.hub.has_capability(&session.host_id, Capability::Images) {
            return error(
                StatusCode::CONFLICT,
                "images_unsupported",
                "this host cannot take images; send text only",
            );
        }
        // Not to write files for a prompt `open_prompt` would refuse anyway;
        // it still decides.
        if session.open_turn_id.is_some() {
            return error(
                StatusCode::CONFLICT,
                "turn_in_progress",
                "wait for the current turn to end",
            );
        }
        if let Err(err) = state.store.save_images(&checked.images) {
            return internal(err);
        }
    }
    let turn_id = uuid::Uuid::now_v7().to_string();
    match state.store.open_prompt(&id, &turn_id, &checked) {
        Ok(true) => {}
        Ok(false) => {
            return error(
                StatusCode::CONFLICT,
                "turn_in_progress",
                "wait for the current turn to end",
            );
        }
        Err(err) => return internal(err),
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::Prompt {
        request_id: request_id.clone(),
        session_id: id.clone(),
        turn_id: turn_id.clone(),
        content: std::mem::take(&mut checked.sent),
    };
    let undo = Undo::Prompt {
        session_id: id.clone(),
        turn_id: turn_id.clone(),
    };
    match state
        .hub
        .request_with_undo(&session.host_id, &request_id, frame, PROMPT_TIMEOUT, undo)
        .await
    {
        Ok(_) => (StatusCode::ACCEPTED, Json(PromptResponse { turn_id })).into_response(),
        // Unknown delivery keeps the turn open; the outbox resolves it.
        Err(RequestError::DeliveryUnknown) => request_failed(RequestError::DeliveryUnknown),
        // Never sent.
        Err(RequestError::NotConnected) => {
            if let Err(e) = state.store.abandon_turn(&id, &turn_id) {
                return internal(e);
            }
            request_failed(RequestError::NotConnected)
        }
        // The socket task has already removed the turn (`Undo::Prompt`).
        Err(err) => request_failed(err),
    }
}

/// One of the owner's images (plan 6a), by its hash. Served as the type it
/// was stored as, never sniffed, and never as a document: no script, no
/// style, no frame. Its name is its hash, so it never changes: cached for a
/// year, privately, since it needs the cookie.
async fn attachment(State(state): State<AppState>, Path(sha256): Path<String>) -> Response {
    let found = match state.store.attachment(&sha256) {
        Ok(Some(found)) => found,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such attachment"),
        Err(err) => return internal(err),
    };
    let Ok(mime) = HeaderValue::from_str(&found.mime) else {
        return internal(anyhow::anyhow!("attachment {sha256} has a type no header can carry"));
    };
    let headers = [
        (header::CONTENT_TYPE, mime),
        (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'"),
        ),
        (
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=31536000, immutable"),
        ),
        // The review's O8: no other site may embed it.
        (
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("same-origin"),
        ),
    ];
    (headers, found.bytes).into_response()
}

/// The owner's attachment store, for Settings (ACP core §15, plan 6a).
async fn attachment_usage(State(state): State<AppState>) -> Response {
    match state.store.attachment_usage() {
        Ok(usage) => Json(usage).into_response(),
        Err(err) => internal(err),
    }
}

/// Cancel the open turn (ACP core §9): 202 `CancelResponse` once that
/// turn's `turn_ended` is ingested, with the outcome it really had: a turn
/// that finished before the cancel reached the agent is not `cancelled`.
/// That includes a turn whose end was ingested between reading the open
/// turn and sending the cancel: the host then answers `not_running`, and
/// the stored outcome is the answer.
async fn cancel(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    if session.lifecycle != "active" || !state.hub.is_ready(&session.host_id) {
        return error(StatusCode::CONFLICT, "not_attached", "the session is not attached");
    }
    let Some(turn_id) = session.open_turn_id else {
        return error(StatusCode::CONFLICT, "no_open_turn", "no turn is in flight");
    };
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::CancelTurn {
        request_id: request_id.clone(),
        session_id: id.clone(),
        turn_id: turn_id.clone(),
    };
    match state
        .hub
        .request_for_turn(&session.host_id, &request_id, &id, &turn_id, frame, CANCEL_TIMEOUT)
        .await
    {
        Ok(SessionBody::TurnEnded { turn_id, outcome, .. }) => {
            (StatusCode::ACCEPTED, Json(CancelResponse { turn_id, outcome })).into_response()
        }
        Ok(other) => internal(anyhow::anyhow!("cancel completed by {other:?}")),
        Err(RequestError::Rejected { code, message }) if code == "not_running" => {
            match state.store.ended_turn_outcome(&turn_id) {
                Ok(Some(outcome)) => (StatusCode::ACCEPTED, Json(CancelResponse { turn_id, outcome })).into_response(),
                // The host has no such turn in flight, and it has not ended.
                Ok(None) => request_failed(RequestError::Rejected { code, message }),
                Err(err) => internal(err),
            }
        }
        Err(err) => request_failed(err),
    }
}

/// The session's config catalogue and current values (ACP core §9).
async fn catalog(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    catalog_response(&state, &id, StatusCode::OK)
}

fn catalog_response(state: &AppState, id: &str, status: StatusCode) -> Response {
    match state.store.catalog(id) {
        Ok(Some(catalog)) => (status, Json(catalog)).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => internal(err),
    }
}

/// Switch one config option of an attached session (ACP core §9): 202
/// with the session's catalogue once the host's `config_applied` is
/// ingested. Every viewer sees the change as SSE `catalog_changed`.
async fn set_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<ConfigRequest>,
) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    if session.lifecycle != "active" || !state.hub.is_ready(&session.host_id) {
        return error(StatusCode::CONFLICT, "not_attached", "resume the session first");
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::SetConfig {
        request_id: request_id.clone(),
        session_id: id.clone(),
        config_id: req.config_id,
        value: req.value,
    };
    match state
        .hub
        .request(&session.host_id, &request_id, frame, CONFIG_TIMEOUT)
        .await
    {
        Ok(_) => catalog_response(&state, &id, StatusCode::ACCEPTED),
        Err(err) => request_failed(err),
    }
}

/// Answer a pending request (ACP core §4.6, §9): 202 once the answer is
/// queued durably, whatever the host's state. It goes out now if the host
/// is connected and reconciled, else after its next handshake; the verdict
/// follows as SSE `pending_changed`.
async fn answer(
    State(state): State<AppState>,
    Path((id, pending_id)): Path<(String, String)>,
    ApiJson(req): ApiJson<AnswerRequest>,
) -> Response {
    match state.store.submit_answer(&id, &pending_id, &req) {
        Ok(AnswerSubmission::Queued(queued)) => {
            state.hub.publish(queued.event);
            state.hub.notify(&queued.host_id, queued.frame);
            let body = AnswerResponse {
                pending_id,
                request_id: queued.request_id,
            };
            (StatusCode::ACCEPTED, Json(body)).into_response()
        }
        Ok(AnswerSubmission::NotFound) => error(StatusCode::NOT_FOUND, "not_found", "no such pending request"),
        Ok(AnswerSubmission::NotOpen) => error(
            StatusCode::CONFLICT,
            "not_open",
            "the request was answered or cancelled already",
        ),
        Ok(AnswerSubmission::AlreadyAnswered) => error(
            StatusCode::CONFLICT,
            "already_answered",
            "an answer is already on its way",
        ),
        Ok(AnswerSubmission::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}

fn lifecycle_response(state: &AppState, id: &str) -> Response {
    match state.store.session(id) {
        Ok(Some(s)) => (
            StatusCode::ACCEPTED,
            Json(LifecycleResponse {
                session_id: s.id,
                lifecycle: s.lifecycle,
            }),
        )
            .into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => internal(err),
    }
}

/// Close collector-side: the session has no adapter the collector can
/// reach (ACP core §4.8).
fn close_unattached(state: &AppState, id: &str) -> Response {
    match state.store.close_now(id) {
        Ok(events) => {
            for event in events {
                state.hub.publish(event);
            }
            lifecycle_response(state, id)
        }
        Err(err) => internal(err),
    }
}

/// Explicit park of an attached session: 202 with the lifecycle once the
/// host's `session_parked` is ingested.
async fn park(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    if session.lifecycle != "active" || !state.hub.is_ready(&session.host_id) {
        return error(StatusCode::CONFLICT, "not_attached", "the session is not attached");
    }
    // Only to hosts that announced it (ACP core §3.3).
    if !state.hub.has_capability(&session.host_id, Capability::Park) {
        return error(
            StatusCode::CONFLICT,
            "park_unsupported",
            "this host cannot park sessions; close the session instead",
        );
    }
    match state.store.record_park_request(&id) {
        Ok(event) => state.hub.publish(event),
        Err(err) => return internal(err),
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ParkSession {
        request_id: request_id.clone(),
        session_id: id.clone(),
    };
    match state
        .hub
        .request_for_session(&session.host_id, &request_id, &id, frame, TEARDOWN_TIMEOUT)
        .await
    {
        Ok(_) => lifecycle_response(&state, &id),
        Err(err) => request_failed(err),
    }
}

/// Close: attached sessions are closed by their host (`session_closed`);
/// anything else is closed immediately. A close whose delivery is unknown
/// stays requested and is re-sent after the host's next handshake.
async fn close(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    let reachable = state.hub.is_ready(&session.host_id);
    match session.lifecycle.as_str() {
        "closed" => return lifecycle_response(&state, &id),
        "starting" if reachable => {
            return error(
                StatusCode::CONFLICT,
                "starting",
                "the session is starting; close it once the start settles",
            );
        }
        "active" if reachable => {}
        _ => return close_unattached(&state, &id),
    }
    match state.store.record_close_request(&id) {
        Ok(event) => state.hub.publish(event),
        Err(err) => return internal(err),
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::CloseSession {
        request_id: request_id.clone(),
        session_id: id.clone(),
    };
    match state
        .hub
        .request_for_session(&session.host_id, &request_id, &id, frame, TEARDOWN_TIMEOUT)
        .await
    {
        // `session_closed`, or a `session_parked` that overtook the close
        // (ingest turns that into `closed`, since a close was requested).
        Ok(_) => lifecycle_response(&state, &id),
        // The host no longer has it (or went away): nothing left to stop.
        Err(RequestError::Rejected { code, .. }) if code == "not_attached" => close_unattached(&state, &id),
        Err(RequestError::NotConnected) => close_unattached(&state, &id),
        Err(err) => request_failed(err),
    }
}

#[derive(Deserialize)]
struct EventsQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    500
}

async fn events(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<EventsQuery>) -> Response {
    match state.store.events(&id, q.after, q.limit.min(5000)) {
        Ok(list) => Json(list).into_response(),
        Err(err) => internal(err),
    }
}

fn sse_event(e: &EventDto) -> Event {
    Event::default()
        .id(e.event_id.to_string())
        .event("event")
        .data(serde_json::to_string(e).expect("event serializes"))
}

/// The SSE messages for one stored event: the event, then `catalog_changed`
/// with the same id if it carries a catalogue snapshot, and
/// `pending_changed` with the same id if it concerns a pending request
/// (ACP core §9). A listed event with a snapshot is one that changed the
/// stored catalogue, and both come from the stored row, so a replay from
/// `Last-Event-ID` sends them too. `pending_changed` carries the request as
/// it stands when the message is sent.
fn sse_messages(store: &Store, e: &EventDto) -> Vec<Result<Event, Infallible>> {
    let mut out = vec![Ok(sse_event(e))];
    if let Some(catalog) = catalog_in(e) {
        out.push(Ok(Event::default()
            .id(e.event_id.to_string())
            .event("catalog_changed")
            .data(serde_json::to_string(&catalog).expect("catalog serializes"))));
    }
    if let Some(pending) = pending_in(store, e) {
        out.push(Ok(Event::default()
            .id(e.event_id.to_string())
            .event("pending_changed")
            .data(serde_json::to_string(&pending).expect("pending request serializes"))));
    }
    out
}

/// The pending request a stored event concerns, as it stands now.
fn pending_in(store: &Store, e: &EventDto) -> Option<PendingItem> {
    if !matches!(
        e.kind.as_str(),
        "pending_opened" | "pending_resolved" | "pending_cancelled" | "answer_submitted" | "answer_result"
    ) {
        return None;
    }
    let pending_id = e.body.get("pending_id")?.as_str()?;
    store.pending_item(pending_id).ok().flatten()
}

/// The catalogue snapshot a stored host fact carries in its extracts.
fn catalog_in(e: &EventDto) -> Option<SessionCatalog> {
    if !matches!(e.kind.as_str(), "session_started" | "config_applied" | "acp_update") {
        return None;
    }
    let indexed: Indexed = serde_json::from_value(e.body.get("indexed")?.clone()).ok()?;
    SessionCatalog::from_indexed(&e.session_id, &indexed)
}

/// The session's events as SSE: replays from `Last-Event-ID`, then follows
/// live events, until the collector shuts down or the operator's session
/// that opened it ends (3b decision 7).
async fn stream_session(
    State(state): State<AppState>,
    Extension(operator_session): Extension<Authenticated>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let after: i64 = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // Subscribe before reading the backlog so nothing falls between them.
    let live = BroadcastStream::new(state.hub.subscribe());
    let backlog = state.store.events(&id, after, u32::MAX).unwrap_or_default();
    let last = backlog.last().map(|e| e.event_id).unwrap_or(after);
    let replay = stream::iter(
        backlog
            .iter()
            .flat_map(|e| sse_messages(&state.store, e))
            .collect::<Vec<_>>(),
    );
    let session = id.clone();
    let store = state.store.clone();
    let follow = live
        .filter_map(move |item| {
            let session = session.clone();
            let store = store.clone();
            async move {
                match item {
                    Ok(e) if e.session_id == session && e.event_id > last => Some(sse_messages(&store, &e)),
                    Ok(_) => None,
                    // Lagged: tell the client to refetch instead of skipping silently.
                    Err(_) => Some(vec![Ok(Event::default().event("resync_required").data("{}"))]),
                }
            }
        })
        .flat_map(stream::iter);
    let stream = replay
        .chain(follow)
        .take_until(state.shutdown.clone().cancelled_owned())
        .take_until(hennery_kernel::auth::session_ended(
            state.operator.clone(),
            operator_session,
        ));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
