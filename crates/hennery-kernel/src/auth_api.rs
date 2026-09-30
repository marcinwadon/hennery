//! Operator auth over HTTP (kernel spec §3, §8): the one-time setup, and
//! login and logout.

use crate::operator::{Operator, PublicUrl, SetupOutcome, cleared_cookie, session_cookie, session_token};
use crate::secret::unix_now;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use hennery_proto::rest::{ApiError, LoginRequest, SetupRequest, SetupResponse};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The largest request body the auth routes read: a password is at most
/// 1024 bytes (`MAX_PASSWORD_BYTES`), so anything much larger is refused
/// with 413 before it is parsed.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// The operator auth routes. Setup has its own `Origin` rule; the rest are
/// browser routes (kernel spec §3.3). Serve with `ConnectInfo<SocketAddr>`:
/// login rate-limits on the peer address.
pub fn router(operator: Arc<Operator>) -> Router {
    let browser = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .layer(middleware::from_fn_with_state(
            operator.clone(),
            crate::origin::browser_rules,
        ));
    let private = || middleware::map_response(crate::setup_page::private_headers);
    Router::new()
        .route("/api/setup", post(setup).layer(private()))
        .route("/setup", get(crate::setup_page::page).layer(private()))
        .route("/setup.js", get(crate::setup_page::script).layer(private()))
        .merge(browser)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
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

/// 429 with `Retry-After`, in whole seconds rounded up.
pub(crate) fn rate_limited(retry_after: Duration, message: &str) -> Response {
    let mut response = error(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message);
    let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(secs));
    response
}

/// Whether the session cookie is `Secure`: as `public_url` says, and
/// `Secure` when there is none yet.
pub(crate) fn secure_cookies(operator: &Operator) -> bool {
    operator.public_url().is_none_or(|u| u.is_https())
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

/// `POST /api/auth/login`: 204 and a new session cookie. Every attempt
/// counts against the client's address until one succeeds; an attempt
/// that is not refused for that runs exactly one password check, right or
/// wrong, so the answer takes as long either way (kernel spec §3.2).
async fn login(
    State(operator): State<Arc<Operator>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    if let Err(retry_after) = operator.login_limiter.attempt(peer.ip(), Instant::now()) {
        return rate_limited(
            retry_after,
            "too many wrong passwords from this address; try again later",
        );
    }
    match operator.check_password(req.password).await {
        Ok(true) => {}
        Ok(false) => return error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
        Err(err) => return internal(err),
    }
    operator.login_limiter.succeeded(peer.ip());
    match operator.open_session(&user_agent(&headers), unix_now()) {
        Ok(Some(token)) => with_cookie(
            StatusCode::NO_CONTENT.into_response(),
            &session_cookie(&token, secure_cookies(&operator)),
        ),
        Ok(None) => error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
        Err(err) => internal(err),
    }
}

/// `POST /api/auth/logout`: 204, the request's session (if any) ended and
/// its cookie cleared.
async fn logout(State(operator): State<Arc<Operator>>, headers: HeaderMap) -> Response {
    if let Some(token) = session_token(&headers) {
        match operator.authenticate(token, unix_now()) {
            Ok(Some(session)) => {
                if let Err(err) = operator.revoke_session(&session.session_id) {
                    return internal(err);
                }
            }
            Ok(None) => {}
            Err(err) => return internal(err),
        }
    }
    with_cookie(
        StatusCode::NO_CONTENT.into_response(),
        &cleared_cookie(secure_cookies(&operator)),
    )
}
