//! Operator auth over HTTP (kernel spec §3, §8): the one-time setup, login
//! and logout, step-up, the signed-in sessions, and passkeys (plan 3c).

use crate::json::ApiJson;
use crate::operator::{Authenticated, Operator, PublicUrl, SetupOutcome, cleared_cookie, session_cookie};
use crate::passkeys::{PasskeyRecord, Refused, Start};
use crate::secret::{rfc3339, unix_now};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router, middleware};
use hennery_proto::rest::{
    ApiError, AuthSessionItem, LoginRequest, PasskeyCeremony, PasskeyFinishRequest, PasskeyItem,
    PasskeyRegisterRequest, SetupRequest, SetupResponse, StepUpRequest,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The largest request body the auth routes read: a password is at most
/// 1024 bytes (`MAX_PASSWORD_BYTES`), and a passkey's answer, with no
/// attestation asked for (plan 3c decision 4), about one, so anything much
/// larger is refused with 413 before it is parsed.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// The operator auth routes. Setup has its own `Origin` rule; the rest are
/// browser routes (kernel spec §3.3). Serve with `ConnectInfo<SocketAddr>`:
/// login rate-limits on the peer address.
pub fn router(operator: Arc<Operator>) -> Router {
    let browser = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/passkeys/login/start", post(passkey_login_start))
        .route("/api/auth/passkeys/login/finish", post(passkey_login_finish))
        .layer(middleware::from_fn_with_state(
            operator.clone(),
            crate::origin::browser_rules,
        ));
    let step_up_first = || middleware::from_fn(crate::auth::require_step_up);
    let signed_in = crate::auth::operator_only(
        Router::new()
            .route("/api/auth/step-up/password", post(step_up))
            .route("/api/auth/step-up/passkey/start", post(passkey_step_up_start))
            .route("/api/auth/step-up/passkey/finish", post(passkey_step_up_finish))
            .route("/api/auth/sessions", get(list_sessions))
            .route(
                "/api/auth/sessions/{id}",
                delete(revoke_session).route_layer(step_up_first()),
            )
            .route("/api/auth/passkeys", get(list_passkeys))
            .route(
                "/api/auth/passkeys/{id}",
                delete(remove_passkey).route_layer(step_up_first()),
            )
            .route(
                "/api/auth/passkeys/register/start",
                post(passkey_register_start).route_layer(step_up_first()),
            )
            .route(
                "/api/auth/passkeys/register/finish",
                post(passkey_register_finish).route_layer(step_up_first()),
            ),
        operator.clone(),
    );
    Router::new()
        .route(
            "/api/setup",
            post(setup).layer(middleware::map_response(private_headers)),
        )
        .merge(browser)
        .merge(signed_in)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(operator)
}

/// Every `POST /api/setup` answer: never cached, and never a `Referer`
/// onwards (kernel spec §3.1). The page itself is the web UI's (`web.rs`).
async fn private_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
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
async fn setup(
    State(operator): State<Arc<Operator>>,
    headers: HeaderMap,
    ApiJson(req): ApiJson<SetupRequest>,
) -> Response {
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
    let outcome = tokio::task::spawn_blocking(move || {
        op.set_up_naming_hat(
            &req.token,
            &req.password,
            &req.public_url,
            req.default_hat_name.as_deref(),
            unix_now(),
        )
    })
    .await;
    let phc = match outcome {
        Ok(Ok(SetupOutcome::Done { owner_id, phc })) => {
            tracing::info!(%owner_id, public_url = %public_url.origin(), "hennery set up");
            phc
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
    };
    let token = match operator.open_session(&user_agent(&headers), &phc, unix_now()) {
        Ok(Some(token)) => token,
        Ok(None) => return internal(anyhow::anyhow!("the password changed right after setup")),
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
    ApiJson(req): ApiJson<LoginRequest>,
) -> Response {
    if let Err(retry_after) = operator.login_limiter.attempt(peer.ip(), Instant::now()) {
        return rate_limited(
            retry_after,
            "too many wrong passwords from this address; try again later",
        );
    }
    let phc = match operator.check_password(req.password).await {
        Ok(Some(phc)) => phc,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
        Err(err) => return internal(err),
    };
    operator.login_limiter.succeeded(peer.ip());
    // Bound to the password just checked: a reset since then wins.
    match operator.open_session(&user_agent(&headers), &phc, unix_now()) {
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
    match operator.authenticate_cookies(&headers, unix_now()) {
        Ok(Some((_, session))) => {
            if let Err(err) = operator.revoke_session(&session.session_id, unix_now()) {
                return internal(err);
            }
        }
        Ok(None) => {}
        Err(err) => return internal(err),
    }
    with_cookie(
        StatusCode::NO_CONTENT.into_response(),
        &cleared_cookie(secure_cookies(&operator)),
    )
}

/// `POST /api/auth/step-up/password`: 204, the session stepped up for five
/// minutes (kernel spec §3.4). Rate limited like login, on a budget of its
/// own (3b decision 10).
async fn step_up(
    State(operator): State<Arc<Operator>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(session): Extension<Authenticated>,
    ApiJson(req): ApiJson<StepUpRequest>,
) -> Response {
    if let Err(retry_after) = operator.step_up_limiter.attempt(peer.ip(), Instant::now()) {
        return rate_limited(
            retry_after,
            "too many wrong passwords from this address; try again later",
        );
    }
    match operator.check_password(req.password).await {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
        Err(err) => return internal(err),
    }
    operator.step_up_limiter.succeeded(peer.ip());
    match operator.step_up(&session.session_id, unix_now()) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first"),
        Err(err) => internal(err),
    }
}

/// `GET /api/auth/sessions`: every signed-in session, most recently used
/// first, the request's own marked `current`.
async fn list_sessions(
    State(operator): State<Arc<Operator>>,
    Extension(session): Extension<Authenticated>,
) -> Response {
    match operator.sessions(unix_now()) {
        Ok(sessions) => {
            let items: Vec<AuthSessionItem> = sessions
                .into_iter()
                .map(|s| AuthSessionItem {
                    current: s.id == session.session_id,
                    id: s.id,
                    user_agent: s.user_agent,
                    created_at: rfc3339(s.created_at),
                    last_seen_at: rfc3339(s.last_seen_at),
                    expires_at: rfc3339(s.expires_at),
                })
                .collect();
            Json(items).into_response()
        }
        Err(err) => internal(err),
    }
}

/// `DELETE /api/auth/sessions/{id}` (step-up): 204, that session signed
/// out; 404 if there is none. Ending the request's own session also clears
/// its cookie.
async fn revoke_session(
    State(operator): State<Arc<Operator>>,
    Extension(session): Extension<Authenticated>,
    Path(id): Path<String>,
) -> Response {
    match operator.revoke_session(&id, unix_now()) {
        Ok(true) if id == session.session_id => with_cookie(
            StatusCode::NO_CONTENT.into_response(),
            &cleared_cookie(secure_cookies(&operator)),
        ),
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => internal(err),
    }
}

/// The answer to a ceremony's start (plan 3c decision 11): 200 and the
/// options, or why it did not begin.
fn started(outcome: anyhow::Result<Start>) -> Response {
    match outcome {
        Ok(Start::Begun { ceremony_id, options }) => Json(PasskeyCeremony { ceremony_id, options }).into_response(),
        Ok(Start::Unavailable) => passkeys_unavailable(),
        Ok(Start::NoPasskeys) => error(StatusCode::CONFLICT, "no_passkeys", "there is no passkey to use"),
        Ok(Start::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}

fn passkeys_unavailable() -> Response {
    error(
        StatusCode::CONFLICT,
        "passkeys_unavailable",
        "passkeys need public_url to name a host, not an IP address",
    )
}

/// The answer to a refused finish (plan 3c decision 11). Nothing changed.
fn refused(why: Refused) -> Response {
    match why {
        Refused::Ceremony => error(
            StatusCode::BAD_REQUEST,
            "invalid_ceremony",
            "the passkey ceremony is unknown, used or expired; start again",
        ),
        Refused::Credential(why) => {
            // `?`, not `%`: the reason can quote the client's input, and
            // `Debug` escapes it (3c review, O4).
            tracing::debug!(?why, "passkey refused: the answer did not verify");
            error(
                StatusCode::UNAUTHORIZED,
                "passkey_refused",
                "the passkey was not accepted",
            )
        }
        // Logged where they were caught: a counter that went back at `warn`
        // with the passkey's id, a passkey changed since it was read at
        // `debug` with its id. A passkey removed since its ceremony began,
        // or not the owner's, has no row to name, and is not logged.
        Refused::Passkey | Refused::CounterWentBack => error(
            StatusCode::UNAUTHORIZED,
            "passkey_refused",
            "the passkey was not accepted",
        ),
        Refused::AlreadyRegistered => error(
            StatusCode::CONFLICT,
            "already_registered",
            "this passkey is registered already",
        ),
        Refused::Unavailable => passkeys_unavailable(),
    }
}

fn passkey_item(record: PasskeyRecord) -> PasskeyItem {
    PasskeyItem {
        id: record.id,
        label: record.label,
        created_at: rfc3339(record.created_at),
        last_used_at: record.last_used_at.map(rfc3339),
    }
}

/// `POST /api/auth/passkeys/login/start`: a login ceremony over the
/// owner's passkeys. Every start counts against the client's address on
/// the passkey budget until a login succeeds (plan 3c decision 8).
async fn passkey_login_start(
    State(operator): State<Arc<Operator>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    if let Err(retry_after) = operator.passkey_limiter.attempt(peer.ip(), Instant::now()) {
        return rate_limited(
            retry_after,
            "too many passkey logins begun from this address; try again later",
        );
    }
    started(operator.start_passkey_login(unix_now()))
}

/// `POST /api/auth/passkeys/login/finish`: 204 and a new session cookie,
/// the session stepped up, as a password login's is.
async fn passkey_login_finish(
    State(operator): State<Arc<Operator>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ApiJson(req): ApiJson<PasskeyFinishRequest>,
) -> Response {
    match operator.finish_passkey_login(&req.ceremony_id, &req.credential, &user_agent(&headers), unix_now()) {
        Ok(Ok(token)) => {
            operator.passkey_limiter.succeeded(peer.ip());
            with_cookie(
                StatusCode::NO_CONTENT.into_response(),
                &session_cookie(&token, secure_cookies(&operator)),
            )
        }
        Ok(Err(why)) => refused(why),
        Err(err) => internal(err),
    }
}

/// `POST /api/auth/step-up/passkey/start`: a step-up ceremony for the
/// request's session (kernel spec §3.4).
async fn passkey_step_up_start(
    State(operator): State<Arc<Operator>>,
    Extension(session): Extension<Authenticated>,
) -> Response {
    started(operator.start_passkey_step_up(&session.session_id, unix_now()))
}

/// `POST /api/auth/step-up/passkey/finish`: 204, the session stepped up
/// for five minutes.
async fn passkey_step_up_finish(
    State(operator): State<Arc<Operator>>,
    Extension(session): Extension<Authenticated>,
    ApiJson(req): ApiJson<PasskeyFinishRequest>,
) -> Response {
    match operator.finish_passkey_step_up(&session.session_id, &req.ceremony_id, &req.credential, unix_now()) {
        Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(false)) => error(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first"),
        Ok(Err(why)) => refused(why),
        Err(err) => internal(err),
    }
}

/// `GET /api/auth/passkeys`: the owner's passkeys, oldest first.
async fn list_passkeys(State(operator): State<Arc<Operator>>) -> Response {
    match operator.passkeys() {
        Ok(records) => Json(records.into_iter().map(passkey_item).collect::<Vec<_>>()).into_response(),
        Err(err) => internal(err),
    }
}

/// `DELETE /api/auth/passkeys/{id}` (step-up): 204, or 404 if there is no
/// such passkey. The password stays (plan 3c decision 2).
async fn remove_passkey(State(operator): State<Arc<Operator>>, Path(id): Path<String>) -> Response {
    match operator.remove_passkey(&id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, "not_found", "no such passkey"),
        Err(err) => internal(err),
    }
}

/// `POST /api/auth/passkeys/register/start` (step-up, plan 3c decision 7):
/// a registration ceremony for the request's session.
async fn passkey_register_start(
    State(operator): State<Arc<Operator>>,
    Extension(session): Extension<Authenticated>,
    ApiJson(req): ApiJson<PasskeyRegisterRequest>,
) -> Response {
    started(operator.start_passkey_registration(&session.session_id, &req.label, unix_now()))
}

/// `POST /api/auth/passkeys/register/finish` (step-up): 201 and the new
/// passkey.
async fn passkey_register_finish(
    State(operator): State<Arc<Operator>>,
    Extension(session): Extension<Authenticated>,
    ApiJson(req): ApiJson<PasskeyFinishRequest>,
) -> Response {
    match operator.finish_passkey_registration(&session.session_id, &req.ceremony_id, &req.credential, unix_now()) {
        Ok(Ok(record)) => (StatusCode::CREATED, Json(passkey_item(record))).into_response(),
        Ok(Err(why)) => refused(why),
        Err(err) => internal(err),
    }
}
