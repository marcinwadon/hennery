//! Session REST and SSE endpoints (ACP core §9), walking-skeleton subset.

use crate::AppState;
use crate::hub::RequestError;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use futures::stream::{self, Stream, StreamExt};
use hennery_proto::frames::CollectorFrame;
use hennery_proto::rest::{
    ApiError, EventDto, PromptRequest, PromptResponse, StartSessionRequest, StartSessionResponse,
};
use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

const START_TIMEOUT: Duration = Duration::from_secs(90);
/// At least the WebSocket read deadline, so a half-open socket is detected
/// before the request gives up (ACP core §3.4).
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/hosts", get(list_hosts))
        .route("/api/sessions", post(start_session))
        .route("/api/sessions/{id}/prompt", post(prompt))
        .route("/api/sessions/{id}/events", get(events))
        .route("/api/stream/sessions/{id}", get(stream_session))
        .layer(middleware::from_fn_with_state(
            state.token.clone(),
            hennery_kernel::auth::require_bearer,
        ))
        .with_state(state)
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiError {
            code: code.into(),
            message: message.into(),
        }),
    )
        .into_response()
}

fn internal(err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "internal error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

fn request_failed(err: RequestError) -> Response {
    match err {
        RequestError::NotConnected => error(StatusCode::CONFLICT, "host_offline", "the host is not connected"),
        RequestError::Rejected { code, message } => {
            let status = match code.as_str() {
                "not_attached" => StatusCode::CONFLICT,
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
    }
}

async fn list_hosts(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.hub.connected_hosts())
}

async fn start_session(State(state): State<AppState>, Json(req): Json<StartSessionRequest>) -> Response {
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
        agent: req.agent,
        cwd: req.cwd,
    };
    match state.hub.request(&req.host_id, &request_id, frame, START_TIMEOUT).await {
        Ok(_) => (StatusCode::ACCEPTED, Json(StartSessionResponse { session_id })).into_response(),
        Err(RequestError::DeliveryUnknown) => request_failed(RequestError::DeliveryUnknown),
        Err(err) => {
            let reason = match &err {
                RequestError::Rejected { code, .. } => code.clone(),
                _ => "host_offline".into(),
            };
            if let Err(e) = state.store.mark_failed(&session_id, &reason) {
                return internal(e);
            }
            request_failed(err)
        }
    }
}

async fn prompt(State(state): State<AppState>, Path(id): Path<String>, Json(req): Json<PromptRequest>) -> Response {
    if req.content.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "empty_prompt",
            "a prompt needs text or an image",
        );
    }
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    if session.lifecycle != "active" {
        return error(StatusCode::CONFLICT, "not_attached", "resume the session first");
    }
    let turn_id = uuid::Uuid::now_v7().to_string();
    match state.store.open_turn(&id, &turn_id, &req.content) {
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
        content: req.content,
    };
    match state
        .hub
        .request(&session.host_id, &request_id, frame, PROMPT_TIMEOUT)
        .await
    {
        Ok(_) => (StatusCode::ACCEPTED, Json(PromptResponse { turn_id })).into_response(),
        // Unknown delivery keeps the turn open; the outbox resolves it.
        Err(RequestError::DeliveryUnknown) => request_failed(RequestError::DeliveryUnknown),
        Err(err) => {
            if let Err(e) = state.store.abandon_turn(&id, &turn_id) {
                return internal(e);
            }
            request_failed(err)
        }
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

/// Session stream: replays from `Last-Event-ID`, then follows live events.
async fn stream_session(
    State(state): State<AppState>,
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
    let replay = stream::iter(backlog.into_iter().map(|e| Ok(sse_event(&e))));
    let session = id.clone();
    let follow = live.filter_map(move |item| {
        let session = session.clone();
        async move {
            match item {
                Ok(e) if e.session_id == session && e.event_id > last => Some(Ok(sse_event(&e))),
                Ok(_) => None,
                // Lagged: tell the client to refetch instead of skipping silently.
                Err(_) => Some(Ok(Event::default().event("resync_required").data("{}"))),
            }
        }
    });
    let stream = replay
        .chain(follow)
        .take_until(state.shutdown.clone().cancelled_owned());
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
