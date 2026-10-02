//! Session REST and SSE endpoints (ACP core §9), walking-skeleton subset.

use crate::AppState;
use crate::content::{self, Refusal};
use crate::hub::{RequestError, Undo};
use crate::resolve::{NotResolved, OnHost, resolve_on_host};
use crate::store::{
    AnswerSubmission, Cursor, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT, ListQuery, Reassign, ResumeRequest, Store,
};
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{self, Stream, StreamExt};
use hennery_kernel::hats::{Resolution, SessionHat};
use hennery_kernel::json::ApiJson;
use hennery_kernel::operator::Authenticated;
use hennery_kernel::secret::unix_now;
use hennery_proto::frames::{Capability, CollectorFrame, Indexed, SessionBody};
use hennery_proto::rest::{
    AGENT_MAX_JSON_BYTES, AnswerRequest, AnswerResponse, ApiError, CancelResponse, ConfigRequest, EventDto,
    LifecycleResponse, OpenTurn, PendingItem, PromptRequest, PromptResponse, SessionDetail, StartSessionRequest,
    StartSessionResponse, UpdateSessionRequest, json_width,
};
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
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
        .route("/api/sessions", post(start_session).get(list_sessions))
        .route("/api/sessions/{id}", get(session_detail).patch(update_session))
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
        // Probes only (`Hub::probe`), which the probe routes answer
        // themselves (`projects::probe_failed`): unreachable here.
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

/// Why a session cannot start or resume where it would, and the answer
/// each gives.
pub(crate) enum Unplaceable {
    /// Its cwd did not resolve on its host.
    NotResolved(NotResolved),
    /// 400 `invalid_cwd`: the canonical cwd is not a directory there.
    NotADirectory(String),
    /// 409 `hat_ambiguous`: a rule (this prefix) that misses only by case makes its hat
    /// doubtful (plan 5c decision 3).
    Ambiguous(String),
    /// 404: no such host.
    UnknownHost,
    Internal(anyhow::Error),
}

impl IntoResponse for Unplaceable {
    fn into_response(self) -> Response {
        match self {
            Self::NotResolved(why) => why.into_response(),
            Self::NotADirectory(cwd) => error(
                StatusCode::BAD_REQUEST,
                "invalid_cwd",
                format!("{cwd:?} is not a directory on that host"),
            ),
            Self::Ambiguous(prefix) => error(
                StatusCode::CONFLICT,
                "hat_ambiguous",
                format!(
                    "the rule for {prefix:?} matches this path only in another case (the directory was made or renamed since the rule was saved): save the host's rules again with the path as it is now"
                ),
            ),
            Self::UnknownHost => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
            Self::Internal(err) => internal(err),
        }
    }
}

/// The hat a session whose canonical cwd is `cwd` gets on `host_id`
/// (umbrella §8.2), unless a rule that misses only by case makes it doubtful.
fn session_hat(state: &AppState, host_id: &str, cwd: &str) -> Result<Resolution, Unplaceable> {
    match state.hosts.session_hat(host_id, cwd) {
        Ok(Some(SessionHat::Decided(resolution))) => Ok(resolution),
        Ok(Some(SessionHat::Ambiguous(rule))) => Err(Unplaceable::Ambiguous(rule.prefix)),
        Ok(None) => Err(Unplaceable::UnknownHost),
        Err(err) => Err(Unplaceable::Internal(err)),
    }
}

/// A cwd as its host resolved it, if it is a directory there.
async fn session_cwd(state: &AppState, host_id: &str, cwd: &str) -> Result<OnHost, Unplaceable> {
    let on_host = resolve_on_host(state, host_id, cwd)
        .await
        .map_err(Unplaceable::NotResolved)?;
    if !on_host.is_dir {
        return Err(Unplaceable::NotADirectory(on_host.canonical));
    }
    Ok(on_host)
}

/// Start a session (ACP core §4.3): its cwd is resolved on its host, its
/// hat decided by that host's rules, and both stored before the host is
/// asked to start anything (umbrella §8.2). The client never names a hat.
/// A host that is not connected and reconciled gets no session at all
/// (plan 5c decision 2).
async fn start_session(State(state): State<AppState>, ApiJson(req): ApiJson<StartSessionRequest>) -> Response {
    // What a list item shows must be bounded (plan 6b, the review's A1): a
    // paired host's id, and an agent's name within its cap. Refused before
    // any session exists.
    if json_width(&req.agent) > AGENT_MAX_JSON_BYTES {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("an agent's name is at most {AGENT_MAX_JSON_BYTES} bytes"),
        );
    }
    match state.hosts.host(&req.host_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "unknown_host",
                "no host is paired with that id",
            );
        }
        Err(err) => return internal(err),
    }
    let cwd = match session_cwd(&state, &req.host_id, &req.cwd).await {
        Ok(on_host) => on_host.canonical,
        Err(why) => return why.into_response(),
    };
    let hat = match session_hat(&state, &req.host_id, &cwd) {
        Ok(hat) => hat,
        // Unpaired since the check above: the same answer as that check.
        Err(Unplaceable::UnknownHost) => {
            return error(
                StatusCode::BAD_REQUEST,
                "unknown_host",
                "no host is paired with that id",
            );
        }
        Err(why) => return why.into_response(),
    };
    let session_id = uuid::Uuid::now_v7().to_string();
    if let Err(err) = state.store.create_session(
        &session_id,
        &req.host_id,
        &req.agent,
        &cwd,
        &hat.hat_id,
        hat.rule_id.as_deref(),
    ) {
        return internal(err);
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::StartSession {
        request_id: request_id.clone(),
        session_id: session_id.clone(),
        // A session minted just now has nothing committed.
        committed_seq: 0,
        agent: req.agent,
        cwd,
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

/// The longest search the list takes, in characters (plan 6b decision 8).
const SEARCH_MAX_CHARS: usize = 200;

/// `GET /api/sessions`' query (ACP core §9). Every value is read as text,
/// so a malformed one gets an `ApiError`.
#[derive(Deserialize)]
struct ListParams {
    cursor: Option<String>,
    limit: Option<String>,
    q: Option<String>,
    hat: Option<String>,
    lifecycle: Option<String>,
}

/// The session list (ACP core §9; plan 6b decision 8): newest
/// `last_event_at` first, in pages of `limit` (50 by default, clamped to
/// 1..=200) after `cursor`; only the comma-separated `lifecycle`s, unless
/// `q` searches the title, cwd, branch and id of every session; and only
/// the sessions of `hat`, with or without `q` (frontend §5; plan 5c). The
/// hat is matched as given: an empty one lists the sessions from before
/// hats that got none.
async fn list_sessions(State(state): State<AppState>, Query(params): Query<ListParams>) -> Response {
    let limit = match params.limit.as_deref().map(str::parse::<u32>) {
        None => LIST_DEFAULT_LIMIT,
        Some(Ok(limit)) => limit.clamp(1, LIST_MAX_LIMIT),
        Some(Err(_)) => return error(StatusCode::BAD_REQUEST, "invalid", "limit must be a whole number"),
    };
    let cursor = match params.cursor.as_deref().map(Cursor::decode) {
        None => None,
        Some(Some(cursor)) => Some(cursor),
        Some(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_cursor",
                "not a cursor this list gave out",
            );
        }
    };
    let search = params.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    // A control character would cut the pattern short (a NUL ends it: the
    // second review's P4).
    if search.is_some_and(|q| q.chars().count() > SEARCH_MAX_CHARS || q.chars().any(char::is_control)) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("a search is at most {SEARCH_MAX_CHARS} characters, with no control characters"),
        );
    }
    let mut lifecycles: Vec<&str> = Vec::new();
    for name in params
        .lifecycle
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
    {
        if name.is_empty() || lifecycles.contains(&name) {
            continue;
        }
        if !LIFECYCLES.contains(&name) {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!("lifecycle is a list of {}", LIFECYCLES.join(", ")),
            );
        }
        lifecycles.push(name);
    }
    // An empty hat names none: refused, so that "no hat chosen" sent as
    // `hat=` is not answered with an empty list (6b: a filter it cannot
    // honour is refused, never ignored).
    if params.hat.as_deref() == Some("") {
        return error(StatusCode::BAD_REQUEST, "invalid", "hat names no hat");
    }
    let query = ListQuery {
        after: cursor.as_ref(),
        limit,
        lifecycles: (!lifecycles.is_empty()).then_some(lifecycles.as_slice()),
        search,
        hat: params.hat.as_deref(),
    };
    match state.store.list(&query) {
        Ok(page) => Json(page).into_response(),
        Err(err) => internal(err),
    }
}

/// Session detail (ACP core §9): the list item, as stored, plus the open
/// turn and the open questions.
async fn session_detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let (session, item) = match (state.store.find_session(&id), state.store.find_session_item(&id)) {
        (Ok(Some(s)), Ok(Some(item))) => (s, item),
        (Ok(None), _) | (_, Ok(None)) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        (Err(err), _) | (_, Err(err)) => return internal(err),
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
        session: item,
        open_turn,
        pending,
    })
    .into_response()
}

/// `hat_id` for a message: by its name, by its id when it has none, or "no
/// hat" for a session from before hats that got none.
fn hat_name(state: &AppState, hat_id: &str) -> String {
    if hat_id.is_empty() {
        return "no hat".to_string();
    }
    match state.hosts.hat(hat_id) {
        Ok(Some(hat)) => format!("hat {:?}", hat.name),
        _ => format!("hat {hat_id}"),
    }
}

/// `PATCH /api/sessions/{id}` (ACP core §9): re-assign the session to another
/// hat (ACP core §4.9), 200 with its detail. Only with no running adapter,
/// and only from a session stepped up within five minutes (plan 5d decision
/// 2): it moves the session's history into another hat's reach.
async fn update_session(
    State(state): State<AppState>,
    Extension(operator_session): Extension<Authenticated>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<UpdateSessionRequest>,
) -> Response {
    let Some(hat_id) = req.hat_id else {
        return session_detail(State(state), Path(id)).await;
    };
    if !operator_session.stepped_up(unix_now()) {
        return hennery_kernel::auth::step_up_required();
    }
    match state.store.reassign_hat(&id, &hat_id) {
        Ok(Reassign::Done(event)) => {
            tracing::info!(session_id = %id, %hat_id, "session re-assigned");
            state.hub.publish(event);
        }
        Ok(Reassign::Unchanged) => {}
        Ok(Reassign::Attached(lifecycle)) => {
            return error(
                StatusCode::CONFLICT,
                &lifecycle,
                match lifecycle.as_str() {
                    "presumed_parked" => {
                        "its host has been away and may still run it: close the session first".to_string()
                    }
                    other => format!("the session is {other}: park or close it first"),
                },
            );
        }
        Ok(Reassign::UnknownHat) => return error(StatusCode::BAD_REQUEST, "invalid", "no such hat"),
        Ok(Reassign::NotFound) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    }
    session_detail(State(state), Path(id)).await
}

/// Resume a parked, closed or failed session (ACP core §4.3): 202 with the
/// lifecycle once the host's `session_started` is ingested. Its cwd is
/// resolved on its host again first, and must still be the same directory
/// (409 `cwd_moved`, plan 5c decision 4), and its hat re-resolved: a
/// different one refuses the resume (409 `hat_mismatch`) until the session
/// is re-assigned.
async fn resume(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.find_session(&id) {
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
    // Before the host is asked anything (ACP core §4.3).
    if session.agent_session_id.is_none() {
        return no_record();
    }
    let on_host = match session_cwd(&state, &session.host_id, &session.cwd).await {
        Ok(on_host) => on_host,
        Err(why) => return why.into_response(),
    };
    if on_host.canonical != session.cwd {
        return error(
            StatusCode::CONFLICT,
            "cwd_moved",
            format!(
                "the session's directory {:?} now resolves to {:?} on its host; start a new session there",
                session.cwd, on_host.canonical
            ),
        );
    }
    let hat = match session_hat(&state, &session.host_id, &session.cwd) {
        Ok(hat) => hat,
        Err(why) => return why.into_response(),
    };
    let (agent_session_id, committed_seq, config) = match state.store.request_resume(&id, &hat.hat_id) {
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
        Ok(ResumeRequest::NoRecord) => return no_record(),
        Ok(ResumeRequest::HatMismatch(stored)) => {
            return error(
                StatusCode::CONFLICT,
                "hat_mismatch",
                format!(
                    "the session belongs to {}, but its directory now resolves to {}; re-assign the session, or change the path rules",
                    hat_name(&state, &stored),
                    hat_name(&state, &hat.hat_id)
                ),
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
    let session = match state.store.find_session(&id) {
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
    let session = match state.store.find_session(&id) {
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
    let session = match state.store.find_session(&id) {
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

fn no_record() -> Response {
    error(
        StatusCode::CONFLICT,
        "agent_has_no_record",
        "the agent never created this session; start a new one in the same project",
    )
}

fn lifecycle_response(state: &AppState, id: &str) -> Response {
    match state.store.find_session(id) {
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
    let session = match state.store.find_session(&id) {
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
    let session = match state.store.find_session(&id) {
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
/// with the same id if it changed the catalogue (and `catalog` is asked
/// for), and `pending_changed` with the same id if it concerns a pending
/// request (ACP core §9). Both are derived from the stored row, so a replay
/// from `Last-Event-ID` sends them too, and both carry what they describe
/// as it stands when the message is sent (plan 6b decision 4: the
/// catalogue has parts that change apart, the config and the commands, so
/// one built from the event alone would wipe the part it does not carry).
fn sse_messages(store: &Store, e: &EventDto, catalog: bool) -> Vec<Result<Event, Infallible>> {
    let mut out = vec![Ok(sse_event(e))];
    if catalog
        && changes_catalogue(e)
        && let Ok(Some(catalog)) = store.catalog(&e.session_id)
    {
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

/// Whether a stored host fact changed the catalogue: its extracts carry a
/// config snapshot or the commands. Listed events only reach here, and a
/// listed one with either changed the stored catalogue.
fn changes_catalogue(e: &EventDto) -> bool {
    if !matches!(e.kind.as_str(), "session_started" | "config_applied" | "acp_update") {
        return false;
    }
    let Some(indexed) = e.body.get("indexed") else {
        return false;
    };
    serde_json::from_value::<Indexed>(indexed.clone())
        .is_ok_and(|indexed| indexed.current_config().is_some() || indexed.commands.is_some())
}

/// Events one page of a stream's replay reads (smoke test #1, F2): the
/// replay holds at most one page at a time, and reads the next only once
/// the client has taken the last, so a session's history is never loaded
/// whole.
pub const REPLAY_PAGE: u32 = 500;

/// A replay in pages of `REPLAY_PAGE` events, from after `cursor`, read
/// with `fetch(after, limit)` one page per poll. `cursor` moves past each
/// page as it is read, so it ends at the last event replayed. A short page
/// is the last; an error is sent and ends the replay.
fn replay_pages<E>(
    cursor: Arc<AtomicI64>,
    fetch: impl FnMut(i64, u32) -> Result<Vec<EventDto>, E>,
) -> impl Stream<Item = Result<Vec<EventDto>, E>> {
    stream::unfold(Some(fetch), move |fetch| {
        let cursor = cursor.clone();
        async move {
            let mut fetch = fetch?;
            match fetch(cursor.load(Ordering::SeqCst), REPLAY_PAGE) {
                Ok(page) if page.is_empty() => None,
                Ok(page) => {
                    if let Some(last) = page.last() {
                        cursor.store(last.event_id, Ordering::SeqCst);
                    }
                    let more = page.len() >= REPLAY_PAGE as usize;
                    Some((Ok(page), more.then_some(fetch)))
                }
                Err(err) => Some((Err(err), None)),
            }
        }
    })
}

/// The SSE messages of one replayed page. The catalogue once per page,
/// after the last event of the page that changed it: every
/// `catalog_changed` carries the whole catalogue as it stands, so one per
/// event would only repeat it (the review's A6, P-23); and one held back
/// to a later page would carry an id older than the events before it.
fn page_messages(store: &Store, page: &[EventDto]) -> Vec<Result<Event, Infallible>> {
    let last_catalogue = page.iter().rposition(changes_catalogue);
    page.iter()
        .enumerate()
        .flat_map(|(at, e)| sse_messages(store, e, Some(at) == last_catalogue))
        .collect()
}

fn resync_required() -> Event {
    Event::default().event("resync_required").data("{}")
}

/// The replayed pages as SSE, `render`ed, then `follow`. A failed page
/// sends `resync_required` and ends the stream: `follow` would skip the
/// events after it.
fn replay_then_follow<E: std::fmt::Display>(
    replay: impl Stream<Item = Result<Vec<EventDto>, E>>,
    mut render: impl FnMut(&[EventDto]) -> Vec<Result<Event, Infallible>>,
    follow: impl Stream<Item = Result<Event, Infallible>>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let failed = Arc::new(AtomicBool::new(false));
    let replay = {
        let failed = failed.clone();
        replay.flat_map(move |page| {
            stream::iter(match page {
                Ok(page) => render(&page),
                Err(err) => {
                    tracing::error!(error = %err, "replaying a session stream");
                    failed.store(true, Ordering::SeqCst);
                    vec![Ok(resync_required())]
                }
            })
        })
    };
    // Polled only once the replay is done.
    let follow = stream::once(async move { (!failed.load(Ordering::SeqCst)).then_some(follow) })
        .filter_map(std::future::ready)
        .flatten();
    replay.chain(follow)
}

/// The session's events as SSE: replays from `Last-Event-ID`, then follows
/// live events, until the collector shuts down or the operator's session
/// that opened it ends (3b decision 7). 404 for a session the owner does
/// not have (ACP core §9); the replay reads the events table a page at a
/// time (`REPLAY_PAGE`), and a failed read sends `resync_required` and
/// ends the stream.
async fn stream_session(
    State(state): State<AppState>,
    Extension(operator_session): Extension<Authenticated>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match state.store.session(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    }
    let after: i64 = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // Subscribe before reading the backlog so nothing falls between them.
    let live = BroadcastStream::new(state.hub.subscribe());
    // The last event sent: the replay moves it, and the live events are
    // filtered by it only once the replay is done, so an event both read
    // in a page and published live is sent once. Nothing is lost between
    // them because event ids grow in commit order (`AUTOINCREMENT`, one
    // writing connection), an event is published only once committed, and
    // an event stored unapplied is never applied later nor published.
    let cursor = Arc::new(AtomicI64::new(after));
    let replay = {
        let store = state.store.clone();
        let session = id.clone();
        replay_pages(cursor.clone(), move |after, limit| store.events(&session, after, limit))
    };
    let session = id.clone();
    let store = state.store.clone();
    let follow = {
        let store = store.clone();
        live.filter_map(move |item| {
            let session = session.clone();
            let store = store.clone();
            let last = cursor.load(Ordering::SeqCst);
            async move {
                match item {
                    Ok(e) if e.session_id == session && e.event_id > last => Some(sse_messages(&store, &e, true)),
                    Ok(_) => None,
                    // Lagged: tell the client to refetch instead of skipping silently.
                    Err(_) => Some(vec![Ok(resync_required())]),
                }
            }
        })
        .flat_map(stream::iter)
    };
    let stream = replay_then_follow(replay, move |page| page_messages(&store, page), follow)
        .take_until(state.shutdown.clone().cancelled_owned())
        .take_until(hennery_kernel::auth::session_ended(
            state.operator.clone(),
            operator_session,
        ));
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn event(event_id: i64) -> EventDto {
        EventDto {
            event_id,
            session_id: "s1".into(),
            host_seq: None,
            kind: "acp_update".into(),
            body: serde_json::json!({}),
            ts: String::new(),
        }
    }

    /// Every `(after, limit)` a history was read with.
    type Reads = Arc<Mutex<Vec<(i64, u32)>>>;

    /// A history of `len` events, ids 1 to `len`, and its reads.
    fn history(len: i64) -> (Reads, impl FnMut(i64, u32) -> Result<Vec<EventDto>, ()>) {
        let reads = Arc::new(Mutex::new(Vec::new()));
        let log = reads.clone();
        let fetch = move |after: i64, limit: u32| {
            log.lock().unwrap().push((after, limit));
            Ok((after + 1..=len).take(limit as usize).map(event).collect())
        };
        (reads, fetch)
    }

    /// Smoke test #1, F2: the replay reads one page at a time, and the next
    /// only when the client takes it, never the whole history at once.
    #[tokio::test]
    async fn a_replay_reads_one_bounded_page_per_poll() {
        let page = i64::from(REPLAY_PAGE);
        let (reads, fetch) = history(2 * page + 7);
        let cursor = Arc::new(AtomicI64::new(0));
        let mut pages = Box::pin(replay_pages(cursor.clone(), fetch));
        let first = pages.next().await.unwrap().unwrap();
        assert_eq!(first.len(), REPLAY_PAGE as usize);
        assert_eq!(*reads.lock().unwrap(), [(0, REPLAY_PAGE)]);
        assert_eq!(cursor.load(Ordering::SeqCst), page);
        // Bounded: a replay that never moves on fails here instead of hanging.
        let rest: Vec<_> = pages.take(3).map(Result::unwrap).collect().await;
        assert_eq!(rest.iter().map(Vec::len).collect::<Vec<_>>(), [page as usize, 7]);
        assert_eq!(
            *reads.lock().unwrap(),
            [(0, REPLAY_PAGE), (page, REPLAY_PAGE), (2 * page, REPLAY_PAGE)]
        );
        assert_eq!(cursor.load(Ordering::SeqCst), 2 * page + 7);
    }

    /// A history of exactly whole pages ends on the empty read after them.
    #[tokio::test]
    async fn a_replay_of_whole_pages_ends_on_an_empty_read() {
        let page = i64::from(REPLAY_PAGE);
        let (reads, fetch) = history(page);
        let pages: Vec<_> = replay_pages(Arc::new(AtomicI64::new(0)), fetch).take(3).collect().await;
        assert_eq!(pages.len(), 1);
        assert_eq!(reads.lock().unwrap().len(), 2);
    }

    /// A failed read is sent, and nothing is read after it.
    #[tokio::test]
    async fn a_failed_read_ends_the_replay() {
        let reads = Arc::new(Mutex::new(0));
        let count = reads.clone();
        let fetch = move |_: i64, _: u32| -> Result<Vec<EventDto>, ()> {
            *count.lock().unwrap() += 1;
            Err(())
        };
        let pages: Vec<_> = replay_pages(Arc::new(AtomicI64::new(0)), fetch).collect().await;
        assert_eq!(pages, [Err(())]);
        assert_eq!(*reads.lock().unwrap(), 1);
    }

    fn one_message_each(page: &[EventDto]) -> Vec<Result<Event, Infallible>> {
        page.iter().map(|_| Ok(Event::default())).collect()
    }

    fn one_live() -> impl Stream<Item = Result<Event, Infallible>> {
        stream::iter([Ok(Event::default())])
    }

    /// The replay, then the live events.
    #[tokio::test]
    async fn the_live_events_follow_the_replay() {
        let replay = stream::iter([Ok::<_, &str>(vec![event(1), event(2)])]);
        let sent: Vec<_> = replay_then_follow(replay, one_message_each, one_live()).collect().await;
        assert_eq!(sent.len(), 3);
    }

    /// A failed page sends `resync_required` and ends the stream: the live
    /// events would skip the events after it.
    #[tokio::test]
    async fn a_failed_page_ends_the_stream_without_the_live_events() {
        let replay = stream::iter([Ok(vec![event(1)]), Err("disk")]);
        let sent: Vec<_> = replay_then_follow(replay, one_message_each, one_live()).collect().await;
        assert_eq!(sent.len(), 2);
    }
}
