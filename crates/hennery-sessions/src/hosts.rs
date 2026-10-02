//! Host endpoints (kernel spec §4, §8): the host list, minting pairing
//! codes, enrollment, renaming or re-hatting a host, and revoke. They live beside the session API because the collector's
//! only HTTP router is here for now; the registry itself is the kernel's
//! (`hennery_kernel::hosts`).

use crate::AppState;
use crate::api::{error, internal};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router, middleware};
use hennery_kernel::hats::HostChange;
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HostRecord, Revoke, TooManyPairingCodes};
use hennery_kernel::json::ApiJson;
use hennery_kernel::lifecycle::LifecycleHooks;
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::frames::Capability;
use hennery_proto::rest::{
    EnrollRequest, EnrollResponse, HostItem, McpAgentDelivery, PairingCodeResponse, UpdateHostRequest,
};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// How long a revoke waits for the host's socket task to let go.
const REVOKE_DISCONNECT_BOUND: Duration = Duration::from_secs(10);

/// Routes that need the operator's session (minting a code, changing a
/// host and revoking one a fresh step-up too: kernel spec §3.4, plan 5a
/// decision 8), and enrollment, which is authenticated by its code alone
/// and so sits outside that layer (kernel spec §3.3).
pub fn router(state: AppState) -> Router {
    let operator = hennery_kernel::auth::operator_only(
        Router::new()
            .route("/api/hosts", get(list_hosts))
            .route(
                "/api/hosts/pairing-codes",
                post(mint_pairing_code).route_layer(middleware::from_fn(hennery_kernel::auth::require_step_up)),
            )
            .route(
                "/api/hosts/{id}",
                // `.route_layer` covers only the methods chained before it:
                // a method added after it would run without step-up.
                delete(revoke_host)
                    .patch(update_host)
                    .route_layer(middleware::from_fn(hennery_kernel::auth::require_step_up)),
            ),
        state.operator.clone(),
    );
    let code_authenticated = Router::new().route("/api/hosts/enroll", post(enroll));
    operator.merge(code_authenticated).with_state(state)
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
        Err(err) if err.is::<TooManyPairingCodes>() => error(StatusCode::CONFLICT, "too_many_codes", err.to_string()),
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
    ApiJson(req): ApiJson<EnrollRequest>,
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

/// `DELETE /api/hosts/{id}`: revoke a host (kernel spec §4.3), 200 with its
/// entry. In this order: the registry refuses its `hello`s from now on,
/// its live connection is closed and gone, and only then are its sessions
/// parked, so no reconciliation on that connection can bring them back.
/// Repeating it repeats the steps, which heals a revoke cut short.
async fn revoke_host(State(state): State<AppState>, Path(host_id): Path<String>) -> Response {
    match state.hosts.revoke(&host_id, unix_now()) {
        Ok(Revoke::NotFound) => return error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Ok(Revoke::Revoked | Revoke::AlreadyRevoked) => {}
        Err(err) => return internal(err),
    }
    if !state.hub.disconnect_and_wait(&host_id, REVOKE_DISCONNECT_BOUND).await {
        tracing::warn!(%host_id, "the revoked host's connection did not close in time");
    }
    if let Err(err) = state.on_host_revoked(&host_id) {
        return internal(err);
    }
    tracing::info!(%host_id, "host revoked");
    match state.hosts.host(&host_id) {
        Ok(Some(record)) => Json(host_item(&state, record)).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => internal(err),
    }
}

/// `PATCH /api/hosts/{id}`: rename a host or change its default hat
/// (kernel spec §4.3), 200 with its entry. A new default hat applies to the
/// sessions started on the host from now on. Stored sessions keep their hat
/// (umbrella §8.2: changing rules does not re-bucket history), so a parked
/// one that no rule covers is refused at its resume (`hat_mismatch`, ACP
/// core §4.3) until the operator re-assigns it.
async fn update_host(
    State(state): State<AppState>,
    Path(host_id): Path<String>,
    ApiJson(req): ApiJson<UpdateHostRequest>,
) -> Response {
    match state
        .hosts
        .update_host(&host_id, req.name.as_deref(), req.default_hat_id.as_deref())
    {
        Ok(HostChange::Done) => {}
        Ok(HostChange::NotFound) => return error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Ok(HostChange::Invalid(why)) => return error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => return internal(err),
    }
    match state.hosts.host(&host_id) {
        Ok(Some(record)) => Json(host_item(&state, record)).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => internal(err),
    }
}

/// A registry entry as the API shows it, with whether it is connected.
pub(crate) fn host_item(state: &AppState, record: HostRecord) -> HostItem {
    // Plan 8e decision E7: per agent, from the latest accepted `hello`, and
    // only for a host that takes servers at all (one without says so by
    // its capabilities). The one mapping, `McpAgentDelivery::of`.
    let mcp_delivery = record
        .mcp_isolation
        .filter(|_| record.capabilities.has(Capability::McpServers))
        .map(|isolation| {
            isolation
                .0
                .into_iter()
                .map(|(agent, how)| (agent, McpAgentDelivery::of(how)))
                .collect()
        });
    HostItem {
        mcp_delivery,
        connected: state.hub.is_ready(&record.id),
        host_id: record.id,
        name: record.name,
        platform: record.platform,
        host_version: record.host_version,
        capabilities: record.capabilities,
        default_hat_id: record.default_hat_id,
        workspace_roots: record.workspace_roots,
        created_at: rfc3339(record.created_at),
        last_seen_at: record.last_seen_at.map(rfc3339),
        revoked_at: record.revoked_at.map(rfc3339),
    }
}

/// `GET /api/hosts`: every paired host, oldest first.
async fn list_hosts(State(state): State<AppState>) -> Response {
    match state.hosts.list() {
        Ok(records) => {
            let items: Vec<HostItem> = records.into_iter().map(|r| host_item(&state, r)).collect();
            Json(items).into_response()
        }
        Err(err) => internal(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_kernel::operator::Operator;
    use hennery_proto::frames::{AgentIsolation, Capabilities, McpIsolation};

    fn record(capabilities: Capabilities, isolation: Option<AgentIsolation>) -> HostRecord {
        HostRecord {
            id: "h".into(),
            name: "h".into(),
            platform: "linux".into(),
            host_version: "1".into(),
            capabilities,
            default_hat_id: "hat".into(),
            workspace_roots: vec![],
            created_at: 0,
            last_seen_at: None,
            revoked_at: None,
            mcp_isolation: isolation,
        }
    }

    /// Plan 8e decision E7: per agent, through the one mapping, and only for
    /// a host that takes servers at all; none before its first `hello`.
    #[test]
    fn a_hosts_delivery_is_shown_only_when_it_takes_servers() {
        let state = AppState::new(
            crate::store::Store::open_in_memory().unwrap(),
            hennery_kernel::hosts::Hosts::open_in_memory().unwrap(),
            Operator::open_in_memory().unwrap(),
        );
        let isolation = AgentIsolation(
            [
                ("claude".to_string(), McpIsolation::ClaudeStrict),
                ("codex".to_string(), McpIsolation::None),
            ]
            .into(),
        );
        let takes = Capabilities(vec![Capability::McpServers]);
        let shown = host_item(&state, record(takes.clone(), Some(isolation.clone()))).mcp_delivery;
        assert_eq!(
            shown,
            Some(
                [
                    ("claude".to_string(), McpAgentDelivery::Isolated),
                    ("codex".to_string(), McpAgentDelivery::DefaultHatOnly),
                ]
                .into()
            )
        );
        assert_eq!(
            host_item(&state, record(Capabilities::default(), Some(isolation))).mcp_delivery,
            None
        );
        assert_eq!(host_item(&state, record(takes, None)).mcp_delivery, None);
    }
}
