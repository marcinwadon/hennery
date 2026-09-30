//! Operator auth over HTTP (kernel spec §3, §8): the one-time setup.

use crate::operator::{Operator, PublicUrl, SetupOutcome, session_cookie};
use crate::secret::unix_now;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use hennery_proto::rest::{ApiError, SetupRequest, SetupResponse};
use std::sync::Arc;

/// The operator auth routes, each with its own `Origin` rule (kernel spec
/// §3.3).
pub fn router(operator: Arc<Operator>) -> Router {
    let private = || middleware::map_response(crate::setup_page::private_headers);
    Router::new()
        .route("/api/setup", post(setup).layer(private()))
        .route("/setup", get(crate::setup_page::page).layer(private()))
        .route("/setup.js", get(crate::setup_page::script).layer(private()))
        .with_state(operator)
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

pub(crate) fn internal(err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "internal error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

/// The request's `User-Agent`, for the session list.
pub(crate) fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// A response that sets `cookie`.
pub(crate) fn with_cookie(mut response: Response, cookie: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// `POST /api/setup`: 201, the owner signed in. There is no `public_url`
/// yet to check `Origin` against, so it must be the origin of the
/// `public_url` being stored: the browser that sets hennery up is the one
/// that can use it afterwards. The token is what authenticates.
async fn setup(State(operator): State<Arc<Operator>>, headers: HeaderMap, Json(req): Json<SetupRequest>) -> Response {
    let public_url = match PublicUrl::parse(&req.public_url) {
        Ok(url) => url,
        Err(why) => return error(StatusCode::BAD_REQUEST, "invalid", why),
    };
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin != Some(public_url.origin()) {
        tracing::debug!(expected = %public_url.origin(), received = ?origin, "setup refused: origin_mismatch");
        return error(
            StatusCode::FORBIDDEN,
            "origin_mismatch",
            "Origin must be the public_url being set up",
        );
    }
    let op = operator.clone();
    let outcome =
        tokio::task::spawn_blocking(move || op.set_up(&req.token, &req.password, &req.public_url, unix_now())).await;
    match outcome {
        Ok(Ok(SetupOutcome::Done { owner_id })) => {
            tracing::info!(%owner_id, public_url = %public_url.origin(), "hennery set up");
        }
        Ok(Ok(SetupOutcome::AlreadySetUp)) => {
            return error(StatusCode::CONFLICT, "already_set_up", "hennery is set up already");
        }
        Ok(Ok(SetupOutcome::InvalidToken)) => {
            return error(
                StatusCode::UNAUTHORIZED,
                "invalid_setup_token",
                "the setup link is unknown, used or expired; restart the collector for a new one",
            );
        }
        Ok(Ok(SetupOutcome::Invalid(why))) => return error(StatusCode::BAD_REQUEST, "invalid", why),
        Ok(Err(err)) => return internal(err),
        Err(err) => return internal(err.into()),
    }
    let token = match operator.open_session(&user_agent(&headers), unix_now()) {
        Ok(Some(token)) => token,
        Ok(None) => return internal(anyhow::anyhow!("no owner right after setup")),
        Err(err) => return internal(err),
    };
    let response = (
        StatusCode::CREATED,
        Json(SetupResponse {
            public_url: public_url.origin().to_string(),
        }),
    )
        .into_response();
    with_cookie(response, &session_cookie(&token, public_url.is_https()))
}
