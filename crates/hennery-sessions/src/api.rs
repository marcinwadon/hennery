//! Session REST and SSE endpoints (ACP core §9), walking-skeleton subset.

use crate::AppState;
use crate::content::{self, Refusal};
use crate::hub::{RequestError, Undo};
use crate::resolve::{NotResolved, OnHost, resolve_on_host};
use crate::store::{
    AnswerSubmission, Cursor, Deletion, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT, ListQuery, McpContext, Reassign,
    ResumeRequest, SessionRow, Store, Unattached,
};
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::handler::Handler;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
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
/// before the request gives up (ACP core §3.4). Public for tests that wait
/// on a prompt as long as the collector would.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
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
        // Step-up is layered on `delete` alone (`Handler::layer`, as in
        // hats.rs), so GET and PATCH are as they were; PATCH checks it
        // itself when it names a hat (plan 5d decision 2).
        .route(
            "/api/sessions/{id}",
            get(session_detail)
                .patch(update_session)
                .delete(delete_session.layer(middleware::from_fn(hennery_kernel::auth::require_step_up))),
        )
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
            // The host's words, which can quote what the agent printed
            // (plan 8e decision 11).
            error(status, &code, crate::redact::shown(&message))
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
        // Unreachable until plan 8e sends servers; 8e also fails the
        // session it left `starting`.
        RequestError::McpUndeliverable => error(
            StatusCode::CONFLICT,
            "mcp_isolation_unavailable",
            "the host cannot keep this agent's session to its MCP servers",
        ),
    }
}

/// A resume answers like a start (decision 3): whatever code the host
/// rejects it with, the session is marked `failed` with that code and the
/// answer is 502 with it, so the answer and the session agree. An offline
/// host and an unknown delivery answer as for any other request.
fn resume_failed(err: RequestError) -> Response {
    match err {
        RequestError::Rejected { code, message } => {
            error(StatusCode::BAD_GATEWAY, &code, crate::redact::shown(&message))
        }
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
/// The `host_offline` answer to a start on a host that has not connected
/// since it was paired.
const NEVER_CONNECTED: &str = "the host has not connected since it was paired; on its first start a host installs its \
     agents (a few hundred MB) before it connects, which can take minutes, so try again once the Hosts view shows it \
     connected";

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
        // Never connected, no `hello` on record and not connected now
        // (smoke test #1, F1): a host's first start installs its agents
        // before it connects, so say that rather than only "not connected".
        Ok(Some(host)) if host.last_seen_at.is_none() && !state.hub.is_ready(&req.host_id) => {
            return error(StatusCode::CONFLICT, "host_offline", NEVER_CONNECTED);
        }
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
    let mcp_context = match mcp_context(&state, &req.host_id, &req.agent) {
        Ok(context) => context,
        Err(err) => return internal(err),
    };
    let session_id = uuid::Uuid::now_v7().to_string();
    let mcp = match state.store.create_session_with_mcp(
        &session_id,
        &req.host_id,
        &req.agent,
        &cwd,
        &hat.hat_id,
        hat.rule_id.as_deref(),
        mcp_context,
    ) {
        Ok(Some(mcp)) => mcp,
        // The hat resolved before its purge froze it (plan 9c decision 10c).
        Ok(None) => return hat_purging(&state, &hat.hat_id),
        Err(err) => return internal(err),
    };
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::StartSession {
        request_id: request_id.clone(),
        session_id: session_id.clone(),
        // A session minted just now has nothing committed.
        committed_seq: 0,
        agent: req.agent,
        cwd,
        config: req.config,
        // The hat just stored, and what it was given (plan 8e).
        hat_id: hat.hat_id.clone(),
        mcp: mcp.frame(),
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
        // Never sent: the host's connection withdrew what the decision
        // read (a reconnect in between). Failed with that code, its token
        // revoked (api-8e-8f A2).
        Err(RequestError::McpUndeliverable) => {
            if let Err(e) = state.store.mark_failed(&session_id, MCP_UNDELIVERABLE) {
                return internal(e);
            }
            request_failed(RequestError::McpUndeliverable)
        }
        // The socket task has already failed the session with the host's
        // code (`Undo::Start`).
        Err(err) => request_failed(err),
    }
}

/// The code a start or resume the hub refused for its MCP servers fails
/// with (plan 8c's `mcp_isolation_unavailable`).
const MCP_UNDELIVERABLE: &str = "mcp_isolation_unavailable";

/// What the delivery decision reads of `host_id` outside the store (lane
/// L2): its live connection's capability and `agent`'s isolation, from the
/// hub, and whether its rules name another hat, from the kernel. A host not
/// connected takes nothing.
fn mcp_context(state: &AppState, host_id: &str, agent: &str) -> anyhow::Result<McpContext> {
    let Some((capable, isolation)) = state.hub.mcp_isolation(host_id, agent) else {
        return Ok(McpContext::NONE);
    };
    Ok(McpContext {
        capable,
        isolation,
        rules_name_other_hats: state.hosts.rules_name_other_hats(host_id)?,
    })
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
    let mcp_delivery = match state.store.mcp_delivery(&id) {
        Ok(delivery) => delivery,
        Err(err) => return internal(err),
    };
    Json(SessionDetail {
        session: item,
        open_turn,
        pending,
        mcp_delivery,
    })
    .into_response()
}

/// 409 `hat_purging` (plan 9c decision 10c): the hat is frozen for its
/// purge, so nothing starts, resumes or moves in or out of it.
pub(crate) fn hat_purging(state: &AppState, hat_id: &str) -> Response {
    error(
        StatusCode::CONFLICT,
        "hat_purging",
        format!("{} is being purged", hat_name(state, hat_id)),
    )
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
        Ok(Reassign::HatPurging) => {
            return error(
                StatusCode::CONFLICT,
                "hat_purging",
                "the session's hat or the one it would move to is being purged",
            );
        }
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
    let mcp_context = match mcp_context(&state, &session.host_id, &session.agent) {
        Ok(context) => context,
        Err(err) => return internal(err),
    };
    let (agent_session_id, committed_seq, config, mcp) = match state.store.request_resume_with_mcp(&id, &hat.hat_id, mcp_context)
    {
        Ok(ResumeRequest::Starting {
            events,
            agent_session_id,
            committed_seq,
            config,
            mcp,
        }) => {
            for event in events {
                state.hub.publish(event);
            }
            (agent_session_id, committed_seq, config, mcp)
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
        Ok(ResumeRequest::HatPurging) => return hat_purging(&state, &session.hat_id),
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
        // The hat the resume just re-resolved, equal to the stored one, and
        // what it was given, with a fresh token (plan 8e).
        hat_id: hat.hat_id.clone(),
        mcp: mcp.frame(),
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
        // Never sent, as for a start.
        Err(RequestError::McpUndeliverable) => {
            if let Err(e) = state.store.mark_failed_if_starting(&id, MCP_UNDELIVERABLE) {
                return internal(e);
            }
            resume_failed(RequestError::McpUndeliverable)
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

/// Where closing a session stands (ACP core §4.8), for `close` and
/// `delete` (plan 9a decision 5).
pub(crate) enum Closing {
    /// Closed: by its host just now, or already.
    Closed,
    /// It has no adapter the collector can reach, as read: to be closed
    /// collector-side, while it is still this (A4).
    Unattached(Unattached),
    /// Refused, or the host's answer is not known: this response.
    Answer(Response),
}

/// Close `session` through its host if it is attached there: a start in
/// flight on a reachable host is refused; an active session on a reachable
/// host gets a durable close request and `close_session`, waiting for its
/// end. A host that answers `not_attached` no longer has it: it is closed
/// collector-side here. A close whose delivery is unknown stays requested
/// and is re-sent after the host's next handshake.
pub(crate) async fn close_through_host(state: &AppState, session: &SessionRow) -> Closing {
    let judged = Unattached {
        lifecycle: session.lifecycle.clone(),
        presumed_parked: session.presumed_parked,
    };
    let reachable = state.hub.is_ready(&session.host_id);
    match session.lifecycle.as_str() {
        "closed" => return Closing::Closed,
        "starting" if reachable => {
            return Closing::Answer(error(
                StatusCode::CONFLICT,
                "starting",
                "the session is starting; close it once the start settles",
            ));
        }
        "active" if reachable => {}
        _ => return Closing::Unattached(judged),
    }
    let id = &session.id;
    match state.store.record_close_request(id) {
        Ok(event) => state.hub.publish(event),
        Err(err) => return Closing::Answer(internal(err)),
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::CloseSession {
        request_id: request_id.clone(),
        session_id: id.clone(),
    };
    match state
        .hub
        .request_for_session(&session.host_id, &request_id, id, frame, TEARDOWN_TIMEOUT)
        .await
    {
        // `session_closed`, or a `session_parked` that overtook the close
        // (ingest turns that into `closed`, since a close was requested).
        Ok(_) => Closing::Closed,
        // The host no longer has it (or went away before the close was
        // sent): nothing left to stop, so closed collector-side, while it
        // is still as read (A4; a close request changes neither).
        Err(RequestError::Rejected { code, .. }) if code == "not_attached" => Closing::Unattached(judged),
        Err(RequestError::NotConnected) => Closing::Unattached(judged),
        Err(err) => Closing::Answer(request_failed(err)),
    }
}

/// Close: attached sessions are closed by their host (`session_closed`);
/// anything else is closed immediately.
async fn close(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.find_session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    match close_through_host(&state, &session).await {
        Closing::Closed => lifecycle_response(&state, &id),
        Closing::Unattached(_) => close_unattached(&state, &id),
        Closing::Answer(response) => response,
    }
}

/// Delete (ACP core §4.10; plan 9a decision 5), behind step-up, which the
/// route checks before this reads anything. Closed as `close` closes it,
/// then deleted in one transaction that requires it closed, or closes it
/// there while it is still as judged unattached (A4): 409 with the
/// lifecycle if something moved it on meanwhile (a resume). 204.
///
/// A session closed without its host (`unconfirmed`) is closed by that
/// host when it reconciles next (decision 4). Its host may be back by the
/// time the delete commits: a reconciliation that ran between this read and
/// the commit left a listed active session as it was, so the judgement
/// still held, and the delete went ahead. If the host is ready now, it is
/// sent `close_session` here (`finish_delete`), with nobody waiting for the
/// answer; if the commit came before it was ready, its connection finds the
/// tombstone once it is (`ws::ready`, after `mark_ready`). A `not_attached`
/// answer is only logged: a tombstone has nothing left to close. A
/// connection kicked between the two sends neither; its next reconcile
/// closes the session.
async fn delete_session(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.find_session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    let unattached = match close_through_host(&state, &session).await {
        Closing::Closed => None,
        Closing::Unattached(judged) => Some(judged),
        Closing::Answer(response) => return response,
    };
    finish_delete(&state, &session, unattached.as_ref())
}

/// The delete itself, once `close_through_host` has judged `session`.
pub(crate) fn finish_delete(state: &AppState, session: &SessionRow, unattached: Option<&Unattached>) -> Response {
    let id = &session.id;
    match state.store.delete_session(id, unattached) {
        // Only `session_deleted`: what a collector-side close wrote went
        // with the session, in the same transaction.
        Ok(Deletion::Done { event, unconfirmed }) => {
            tracing::info!(session_id = %id, unconfirmed, "session deleted");
            state.hub.publish(event);
            if unconfirmed {
                close_deleted_on_host(state, &session.host_id, id);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Deletion::Refused(lifecycle)) => error(
            StatusCode::CONFLICT,
            &lifecycle,
            format!("the session is {lifecycle} now; delete it again once that settles"),
        ),
        Ok(Deletion::NotFound) => error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => internal(err),
    }
}

/// A session deleted `unconfirmed` (closed without its host): its host may
/// be ready by now (it reconciled after the judgement), so it is sent
/// `close_session`. Nobody waits for the answer, which changes nothing on a
/// tombstone. `notify` sends only to a host that is ready. A delete's and a
/// purge's (plan 9c decision 10d) both.
pub(crate) fn close_deleted_on_host(state: &AppState, host_id: &str, session_id: &str) {
    state.hub.notify(
        host_id,
        CollectorFrame::CloseSession {
            request_id: uuid::Uuid::now_v7().to_string(),
            session_id: session_id.to_string(),
        },
    );
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

/// The session's timeline; 404 for an unknown or deleted session (plan 9a
/// A10).
async fn events(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<EventsQuery>) -> Response {
    match state.store.find_session(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    }
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
/// events after it. The items are SSE messages, or anything a message
/// converts into (the stream's end mark, plan 9a A10).
fn replay_then_follow<E: std::fmt::Display, M: From<Result<Event, Infallible>>>(
    replay: impl Stream<Item = Result<Vec<EventDto>, E>>,
    mut render: impl FnMut(&[EventDto]) -> Vec<M>,
    follow: impl Stream<Item = M>,
) -> impl Stream<Item = M> {
    let failed = Arc::new(AtomicBool::new(false));
    let replay = {
        let failed = failed.clone();
        replay.flat_map(move |page| {
            stream::iter(match page {
                Ok(page) => render(&page),
                Err(err) => {
                    tracing::error!(error = %err, "replaying a session stream");
                    failed.store(true, Ordering::SeqCst);
                    vec![M::from(Ok(resync_required()))]
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
/// live events, until the collector shuts down, the operator's session
/// that opened it ends (3b decision 7), or it sends the session's
/// `session_deleted` (plan 9a A10). 404 for an unknown or deleted session
/// (ACP core §9); the replay reads the events table a page at a time
/// (`REPLAY_PAGE`), and a failed read sends `resync_required` and ends the
/// stream.
async fn stream_session(
    State(state): State<AppState>,
    Extension(operator_session): Extension<Authenticated>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match state.store.find_session(&id) {
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
                    Ok(e) if e.session_id == session && e.event_id > last => {
                        Some(with_end_mark(sse_messages(&store, &e, true), ends_stream(&e)))
                    }
                    Ok(_) => None,
                    // Lagged: tell the client to refetch instead of skipping silently.
                    Err(_) => Some(vec![Some(Ok(resync_required()))]),
                }
            }
        })
        .flat_map(stream::iter)
    };
    // The replay up to the event that ends the stream, if a page holds it,
    // and an end mark after it, which ends the stream at once, not at the
    // next message.
    let render = move |page: &[EventDto]| {
        let end = page.iter().position(ends_stream);
        let page = end.map_or(page, |end| &page[..=end]);
        with_end_mark(page_messages(&store, page), end.is_some())
    };
    let stream = replay_then_follow(replay, render, follow)
        .take_while(|message| std::future::ready(message.is_some()))
        .filter_map(std::future::ready)
        .take_until(state.shutdown.clone().cancelled_owned())
        .take_until(hennery_kernel::auth::session_ended(
            state.operator.clone(),
            operator_session,
        ));
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

/// The event after which a session's stream ends: its tombstone (plan 9a
/// A10), after which nothing of it is written again.
fn ends_stream(e: &EventDto) -> bool {
    e.kind == "session_deleted"
}

/// `messages` as a stream's items, with the end mark (`None`) after them
/// if `ends`.
fn with_end_mark(messages: Vec<Result<Event, Infallible>>, ends: bool) -> Vec<Option<Result<Event, Infallible>>> {
    messages.into_iter().map(Some).chain(ends.then_some(None)).collect()
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

/// A delete against a host's reconciliation (plan 9a, the whole-branch
/// review's race). Its window, between `reconcile_host`'s commit and
/// `mark_ready`, has no await point, so these tests do not race it: they
/// call the steps the route and the host's connection run, in each order
/// the window allows, over a connection registered with a channel of
/// their own.
#[cfg(test)]
mod delete_race_tests {
    use super::*;
    use crate::ws::{after_reconcile, ready};
    use hennery_kernel::hosts::Hosts;
    use hennery_kernel::operator::Operator;
    use hennery_proto::frames::{AttachedSession, Capabilities, SessionBody};
    use std::collections::{HashMap, HashSet};
    use tokio::sync::mpsc;

    const HOST: &str = "host-1";

    struct Fixture {
        state: AppState,
        rx: mpsc::UnboundedReceiver<CollectorFrame>,
        tx: mpsc::UnboundedSender<CollectorFrame>,
        conn_id: u64,
        _dir: tempfile::TempDir,
    }

    /// A collector's state, and `HOST` connected but not yet reconciled.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let conn_id = state
            .hub
            .register(HOST, tx.clone(), Capabilities::default(), Default::default())
            .unwrap()
            .conn_id;
        Fixture {
            state,
            rx,
            tx,
            conn_id,
            _dir: dir,
        }
    }

    fn session(f: &Fixture, id: &str, started: bool) -> SessionRow {
        f.state
            .store
            .create_session(id, HOST, "fake", "/tmp", "hat-1", None)
            .unwrap();
        if started {
            f.state
                .store
                .ingest(id, 1, &SessionBody::session_started("r0", "a1"))
                .unwrap();
        }
        f.state.store.find_session(id).unwrap().unwrap()
    }

    fn listed(id: &str) -> Vec<AttachedSession> {
        vec![AttachedSession {
            session_id: id.into(),
            last_seq: 1,
            open_turn_id: None,
        }]
    }

    /// The route's judgement while the host is not ready: unattached, as
    /// read.
    async fn judged_unattached(f: &Fixture, session: &SessionRow) -> Unattached {
        match close_through_host(&f.state, session).await {
            Closing::Unattached(judged) => judged,
            _ => panic!("expected the session judged unattached"),
        }
    }

    /// The `close_session` frames sent to the host so far.
    fn closes(f: &mut Fixture) -> Vec<String> {
        let mut ids = Vec::new();
        while let Ok(frame) = f.rx.try_recv() {
            if let CollectorFrame::CloseSession { session_id, .. } = frame {
                ids.push(session_id);
            }
        }
        ids
    }

    /// The delete commits after the reconciliation and before the host is
    /// ready: the route cannot reach the host, so its connection, once
    /// ready, finds the tombstone and closes it.
    #[tokio::test]
    async fn a_delete_committed_before_the_host_is_ready_is_closed_by_its_connection() {
        let mut f = fixture();
        let s = session(&f, "s1", true);
        let attached = listed("s1");
        let mut reconcile_closes = HashMap::new();
        let done = f.state.store.reconcile_host(HOST, &attached).unwrap();
        let closed = after_reconcile(&f.state, HOST, &[], done, &f.tx, &mut reconcile_closes);
        assert!(closed.is_empty());
        let judged = judged_unattached(&f, &s).await;
        let response = finish_delete(&f.state, &s, Some(&judged));
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            closes(&mut f).is_empty(),
            "the host is not ready: the route sends nothing"
        );
        ready(
            &f.state,
            HOST,
            f.conn_id,
            &attached,
            &closed,
            &f.tx,
            &mut reconcile_closes,
        )
        .unwrap();
        assert_eq!(closes(&mut f), ["s1"]);
        // Tracked, so a `not_attached` answer finds its session.
        assert_eq!(reconcile_closes.values().filter(|s| *s == "s1").count(), 1);
    }

    /// The route judged the session while its host was not ready, and the
    /// delete commits once it is: the connection's check came too early,
    /// so the route closes it.
    #[tokio::test]
    async fn a_delete_committed_after_the_host_is_ready_is_closed_by_the_route() {
        let mut f = fixture();
        let s = session(&f, "s1", true);
        let attached = listed("s1");
        let mut reconcile_closes = HashMap::new();
        let done = f.state.store.reconcile_host(HOST, &attached).unwrap();
        let closed = after_reconcile(&f.state, HOST, &[], done, &f.tx, &mut reconcile_closes);
        let judged = judged_unattached(&f, &s).await;
        ready(
            &f.state,
            HOST,
            f.conn_id,
            &attached,
            &closed,
            &f.tx,
            &mut reconcile_closes,
        )
        .unwrap();
        assert!(closes(&mut f).is_empty(), "nothing is deleted yet");
        let response = finish_delete(&f.state, &s, Some(&judged));
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(closes(&mut f), ["s1"]);
    }

    /// The same window for a purge (plan 9c decision 10d): the purge read
    /// the hat's session while its host was not ready, and deletes it once
    /// the host is, so the purge closes it.
    #[tokio::test]
    async fn a_purge_committed_after_the_host_is_ready_closes_the_session_on_it() {
        let mut f = fixture();
        session(&f, "s1", true);
        let attached = listed("s1");
        let mut reconcile_closes = HashMap::new();
        let read = f.state.store.hat_sessions("hat-1").unwrap();
        let done = f.state.store.reconcile_host(HOST, &attached).unwrap();
        let closed = after_reconcile(&f.state, HOST, &[], done, &f.tx, &mut reconcile_closes);
        ready(
            &f.state,
            HOST,
            f.conn_id,
            &attached,
            &closed,
            &f.tx,
            &mut reconcile_closes,
        )
        .unwrap();
        assert!(closes(&mut f).is_empty(), "nothing is deleted yet");
        let mut purged = crate::hats::PurgedSessions::default();
        for session in read {
            crate::hats::delete_as_read(&f.state, session, &mut purged).unwrap();
        }
        assert_eq!(purged.deleted, 1);
        assert_eq!(purged.unconfirmed, ["s1"]);
        assert_eq!(closes(&mut f), ["s1"]);
    }

    /// A tombstone the reconciliation itself closes gets one
    /// `close_session`, not a second from the check after `mark_ready`.
    #[tokio::test]
    async fn a_delete_committed_before_the_reconciliation_is_closed_once() {
        let mut f = fixture();
        let s = session(&f, "s1", true);
        let attached = listed("s1");
        let judged = judged_unattached(&f, &s).await;
        assert_eq!(
            finish_delete(&f.state, &s, Some(&judged)).status(),
            StatusCode::NO_CONTENT
        );
        let mut reconcile_closes = HashMap::new();
        let done = f.state.store.reconcile_host(HOST, &attached).unwrap();
        let closed = after_reconcile(&f.state, HOST, &[], done, &f.tx, &mut reconcile_closes);
        assert_eq!(closed, HashSet::from(["s1".to_string()]));
        ready(
            &f.state,
            HOST,
            f.conn_id,
            &attached,
            &closed,
            &f.tx,
            &mut reconcile_closes,
        )
        .unwrap();
        assert_eq!(closes(&mut f), ["s1"]);
    }

    /// The route's own `starting` arm (plan 9a decision 5): refused before
    /// the store is asked, which would refuse it too.
    #[tokio::test]
    async fn a_starting_session_on_a_ready_host_is_refused_by_the_route() {
        let f = fixture();
        let s = session(&f, "s1", false);
        f.state.hub.mark_ready(HOST, f.conn_id);
        let Closing::Answer(response) = close_through_host(&f.state, &s).await else {
            panic!("expected a refusal");
        };
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "starting", "{body}");
        assert_eq!(f.state.store.find_session("s1").unwrap().unwrap().lifecycle, "starting");
    }
}
