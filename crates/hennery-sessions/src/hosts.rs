//! Host pairing endpoints (kernel spec §4.1, §8): minting pairing codes and
//! enrollment. They live beside the session API because the collector's
//! only HTTP router is here for now; the registry itself is the kernel's
//! (`hennery_kernel::hosts`).

use crate::AppState;
use crate::api::{error, internal};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router, middleware};
use hennery_kernel::hosts::{EnrollOutcome, Enrollment};
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{EnrollRequest, EnrollResponse, PairingCodeResponse};
use std::net::SocketAddr;
use std::time::Instant;

/// Routes that need an operator (the development bearer until operator
/// auth replaces it), and enrollment, which is authenticated by its code
/// alone and so sits outside that layer (kernel spec §3.3).
pub fn router(state: AppState) -> Router {
    let operator = Router::new()
        .route("/api/hosts/pairing-codes", post(mint_pairing_code))
        .layer(middleware::from_fn_with_state(
            state.token.clone(),
            hennery_kernel::auth::require_bearer,
        ));
    let code_authenticated = Router::new().route("/api/hosts/enroll", post(enroll));
    operator.merge(code_authenticated).with_state(state)
}

/// RFC 3339 for a kernel timestamp (seconds since the epoch).
pub(crate) fn rfc3339(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_default()
}

/// `POST /api/hosts/pairing-codes`: 201 with a fresh code.
async fn mint_pairing_code(State(state): State<AppState>) -> Response {
    match state.hosts.mint_pairing_code(unix_now()) {
        Ok(code) => (
            StatusCode::CREATED,
            Json(PairingCodeResponse {
                code: code.code,
                expires_at: rfc3339(code.expires_at),
            }),
        )
            .into_response(),
        Err(err) => internal(err),
    }
}

/// `POST /api/hosts/enroll`: 201 `{host_id}`. Every attempt counts against
/// the client's address until it pairs a host (kernel spec §4.1); once that
/// address is locked out, attempts are answered 429 without the code being
/// looked at.
async fn enroll(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(req): Json<EnrollRequest>,
) -> Response {
    let now = Instant::now();
    if let Err(retry_after) = state.enroll_limiter.attempt(peer.ip(), now) {
        let mut response = error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many wrong pairing codes from this address; try again later",
        );
        let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        return response;
    }
    let enrollment = Enrollment {
        public_key: req.public_key,
        name: req.name,
        host_version: req.host_version,
        platform: req.platform,
    };
    match state.hosts.enroll(&req.code, &enrollment, unix_now()) {
        Ok(EnrollOutcome::Enrolled { host_id }) => {
            state.enroll_limiter.succeeded(peer.ip());
            tracing::info!(%host_id, name = %enrollment.name, "host paired");
            (StatusCode::CREATED, Json(EnrollResponse { host_id })).into_response()
        }
        Ok(EnrollOutcome::InvalidCode) => error(
            StatusCode::UNAUTHORIZED,
            "invalid_code",
            "the pairing code is unknown, used or expired",
        ),
        Ok(EnrollOutcome::AlreadyPaired { .. }) => error(
            StatusCode::CONFLICT,
            "already_paired",
            "a host with this key is paired already",
        ),
        Ok(EnrollOutcome::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}
