//! The connections API (gateway spec §9): every route is the operator's
//! (`operator_only`: the browser rules, then the session cookie), and takes
//! its body as `ApiJson`. Creating a connection, deleting one, setting its
//! credential, and a change to where its token goes need a fresh step-up
//! (kernel spec §3.4; plan 8a decision 4). A token is never logged, never
//! answered, and stored only sealed.

use crate::model::{
    Change, ConnectionPatch, ConnectionRecord, CredKind, CredentialChange, NewConnection, url_for_logs,
};
use crate::oauth_api;
use crate::runtime::Runtime;
use axum::extract::{DefaultBodyLimit, Extension, Path, State};
use axum::handler::Handler;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router, middleware};
use hennery_kernel::auth::{operator_only, require_step_up, step_up_required};
use hennery_kernel::json::ApiJson;
use hennery_kernel::operator::{Authenticated, Operator};
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{
    ApiError, CreateMcpConnectionRequest, McpConnectionItem, McpConnectionStatus, McpCredKind, McpCredentialRequest,
    McpMountsRequest, McpOauthError, McpOauthPendingClient, McpOauthState, UpdateMcpConnectionRequest,
};
use std::sync::Arc;

/// The largest body a route reads: an allowlist of `MAX_TOOLS` names of
/// 128 bytes fits, a token of `MAX_TOKEN` many times over.
const BODY_LIMIT: usize = 256 * 1024;

/// The OAuth callback's path (gateway spec §4.2): what the redirect URI
/// names.
pub const CALLBACK_PATH: &str = "/api/mcp/oauth/callback";

#[derive(Clone)]
pub struct GatewayState {
    /// The stores, the key, the egress policy, the flows and the refresh
    /// locks, shared with the proxy (plan 8f decision 3).
    pub runtime: Arc<Runtime>,
    /// The owner and their sessions (kernel spec §3).
    pub operator: Arc<Operator>,
}

impl GatewayState {
    /// The redirect URI to register at a vendor (gateway spec §4.2):
    /// `<public_url>/api/mcp/oauth/callback`. `None` before setup.
    pub fn redirect_uri(&self) -> Option<String> {
        self.operator
            .public_url()
            .map(|url| format!("{}{CALLBACK_PATH}", url.origin()))
    }

    /// Whether cookies are `Secure`: as the kernel's session cookie (kernel
    /// spec §3.2), unless `public_url` is loopback `http`.
    pub fn secure_cookies(&self) -> bool {
        self.operator.public_url().is_none_or(|url| url.is_https())
    }
}

/// The routes, behind the operator's session. Step-up is layered on each
/// method that always needs it (`Handler::layer`), so a method added to a
/// route later gets none unless it is layered too; `PATCH` checks it in
/// its handler, for the fields that need it. The OAuth callback is beside
/// them, outside the operator's session and the browser rules: the vendor's
/// redirect arrives cross-site, without the `SameSite=Strict` session
/// cookie (`callback::router`).
pub fn router(state: GatewayState) -> Router {
    let routes = Router::new()
        .route(
            "/api/mcp/connections",
            get(list).post(create.layer(middleware::from_fn(require_step_up))),
        )
        .route(
            "/api/mcp/connections/{id}",
            patch(update).delete(delete.layer(middleware::from_fn(require_step_up))),
        )
        .route("/api/mcp/connections/{id}/mounts", put(mounts))
        .route(
            "/api/mcp/connections/{id}/credential",
            put(credential.layer(middleware::from_fn(require_step_up))),
        )
        .route(
            "/api/mcp/connections/{id}/oauth-client",
            put(oauth_api::oauth_client.layer(middleware::from_fn(require_step_up))),
        )
        .route(
            "/api/mcp/connections/{id}/authorize",
            post(oauth_api::authorize.layer(middleware::from_fn(require_step_up))),
        )
        .route("/api/mcp/connections/{id}/probe", post(oauth_api::probe))
        .route("/api/mcp/oauth/redirect-uri", get(oauth_api::redirect_uri))
        .layer(DefaultBodyLimit::max(BODY_LIMIT));
    operator_only(routes, state.operator.clone())
        .layer(middleware::map_response(no_store))
        .with_state(state.clone())
        .merge(crate::callback::router(state))
}

/// Every answer is private (connection URLs, hat and host ids), and none
/// is to be kept by a cache (the review's O6, as `projects.rs` does).
/// Outside `operator_only`, so its refusals carry it too (the
/// re-confirmation's finding 4).
async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
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

pub(crate) fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "no such connection")
}

fn wire_kind(kind: CredKind) -> McpCredKind {
    match kind {
        CredKind::None => McpCredKind::None,
        CredKind::Static => McpCredKind::Static,
        CredKind::OauthDcr => McpCredKind::OauthDcr,
        CredKind::OauthClient => McpCredKind::OauthClient,
    }
}

pub(crate) fn model_kind(kind: McpCredKind) -> CredKind {
    match kind {
        McpCredKind::None => CredKind::None,
        McpCredKind::Static => CredKind::Static,
        McpCredKind::OauthDcr => CredKind::OauthDcr,
        McpCredKind::OauthClient => CredKind::OauthClient,
    }
}

/// The wire item of `record`; `redirect_uri` is what OAuth connections
/// show to register at the vendor.
pub(crate) fn item(record: ConnectionRecord, redirect_uri: &str) -> McpConnectionItem {
    let oauth = record.oauth_client.map(|client| McpOauthState {
        redirect_uri: redirect_uri.to_string(),
        client_id: client.client_id,
        has_client_secret: client.has_client_secret,
        pending_client: client
            .pending
            .map(|(client_id, has_client_secret)| McpOauthPendingClient {
                client_id,
                has_client_secret,
            }),
        resource_mismatch: record.resource_mismatch,
        accepted_resource: record.accepted_resource,
    });
    McpConnectionItem {
        status: match crate::model::Status::parse(&record.status) {
            Some(crate::model::Status::Ok) => McpConnectionStatus::Ok,
            Some(crate::model::Status::NeedsAuth) => McpConnectionStatus::NeedsAuth,
            Some(crate::model::Status::Error) => McpConnectionStatus::Error,
            Some(crate::model::Status::NotConnected) => McpConnectionStatus::NotConnected,
            // The schema's CHECK keeps any other out; logged, not hidden
            // (plan 8a's follow-up).
            None => {
                tracing::warn!(connection_id = %record.id, "gateway: a stored status the API does not know");
                McpConnectionStatus::NotConnected
            }
        },
        checked_at: record.checked_at.map(rfc3339),
        oauth,
        oauth_error: record.oauth_error.map(|e| McpOauthError {
            code: e.code,
            message: e.message,
            at: rfc3339(e.at),
        }),
        id: record.id,
        slug: record.slug,
        label: record.label,
        url: record.url,
        hat_id: record.hat_id,
        cred_kind: wire_kind(record.cred_kind),
        static_header: record.static_header,
        static_prefix: record.static_prefix,
        tool_allowlist: record.tool_allowlist,
        internal_network: record.internal_network,
        status_note: record.status_note,
        account_label: record.account_label,
        status_at: rfc3339(record.status_at),
        created_at: rfc3339(record.created_at),
        updated_at: rfc3339(record.updated_at),
        has_credential: record.has_credential,
        mounts: record.mounts,
    }
}

fn changed(state: &GatewayState, change: Change, status: StatusCode) -> Response {
    match change {
        Change::Done(record) => {
            (status, Json(item(*record, &state.redirect_uri().unwrap_or_default()))).into_response()
        }
        Change::NotFound => not_found(),
        Change::SlugTaken => error(StatusCode::CONFLICT, "slug_taken", "another connection has this slug"),
        Change::TooMany => error(
            StatusCode::CONFLICT,
            "too_many_connections",
            format!("there are at most {} connections", crate::model::MAX_CONNECTIONS),
        ),
        Change::Invalid(why) => error(StatusCode::BAD_REQUEST, "invalid", why),
    }
}

/// `GET /api/mcp/connections`: every connection, oldest first, without a
/// secret.
async fn list(State(state): State<GatewayState>) -> Response {
    let redirect_uri = state.redirect_uri().unwrap_or_default();
    match state.runtime.store.list() {
        Ok(records) => Json(
            records
                .into_iter()
                .map(|record| item(record, &redirect_uri))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(err) => internal(err),
    }
}

/// `POST /api/mcp/connections` (step-up): 201 with the new connection.
async fn create(State(state): State<GatewayState>, ApiJson(req): ApiJson<CreateMcpConnectionRequest>) -> Response {
    let new = NewConnection {
        slug: req.slug,
        label: req.label,
        url: req.url,
        hat_id: req.hat_id,
        cred_kind: model_kind(req.cred_kind),
        static_header: req.static_header,
        static_prefix: req.static_prefix,
        tool_allowlist: req.tool_allowlist,
        internal_network: req.internal_network,
    };
    match state.runtime.store.create(&new, unix_now()) {
        Ok(change) => {
            if let Change::Done(record) = &change {
                tracing::info!(
                    connection_id = %record.id,
                    slug = %record.slug,
                    upstream = %url_for_logs(&record.url),
                    "gateway connection created"
                );
            }
            changed(&state, change, StatusCode::CREATED)
        }
        Err(err) => internal(err),
    }
}

/// `PATCH /api/mcp/connections/{id}`: 200 with the connection as it is
/// now. Naming where its token goes (the URL, the kind, the internal
/// network, the header or the prefix) needs step-up, checked before
/// anything is read: those decide which upstream receives the credential
/// and how (plan 8a decision 4).
async fn update(
    State(state): State<GatewayState>,
    Extension(operator_session): Extension<Authenticated>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<UpdateMcpConnectionRequest>,
) -> Response {
    let moves_the_token = req.url.is_some()
        || req.cred_kind.is_some()
        || req.internal_network.is_some()
        || req.static_header.is_some()
        || req.static_prefix.is_some();
    if moves_the_token && !operator_session.stepped_up(unix_now()) {
        return step_up_required();
    }
    let patch = ConnectionPatch {
        label: req.label,
        url: req.url,
        cred_kind: req.cred_kind.map(model_kind),
        static_header: req.static_header,
        static_prefix: req.static_prefix,
        tool_allowlist: req.tool_allowlist,
        internal_network: req.internal_network,
    };
    // Every credential write takes the connection's refresh lock (gateway
    // spec §4.5): an origin or kind change deletes the grant, which a
    // refresh in flight must not write back (G-13).
    let _lock = state.runtime.lock(&id).await;
    let before = match state.runtime.store.connection(&id) {
        Ok(before) => before,
        Err(err) => return internal(err),
    };
    match state.runtime.store.update(&id, &patch, unix_now()) {
        Ok(change) => {
            if let (Change::Done(record), Some(before)) = (&change, &before) {
                tracing::info!(
                    connection_id = %record.id,
                    upstream = %url_for_logs(&record.url),
                    has_credential = record.has_credential,
                    "gateway connection changed"
                );
                // Its flows were for what it was (the review's R1).
                if record.url != before.url
                    || record.cred_kind != before.cred_kind
                    || record.internal_network != before.internal_network
                {
                    state.runtime.flows.drop_connection(&id);
                }
            }
            changed(&state, change, StatusCode::OK)
        }
        Err(err) => internal(err),
    }
}

/// `DELETE /api/mcp/connections/{id}` (step-up): 204, with its mounts,
/// credential and OAuth client gone, and its flows dropped.
async fn delete(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    let _lock = state.runtime.lock(&id).await;
    match state.runtime.store.delete(&id) {
        Ok(true) => {
            state.runtime.flows.drop_connection(&id);
            tracing::info!(connection_id = %id, "gateway connection deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => not_found(),
        Err(err) => internal(err),
    }
}

/// `PUT /api/mcp/connections/{id}/mounts`: 200 with the connection, its
/// mounts the given set.
async fn mounts(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<McpMountsRequest>,
) -> Response {
    match state.runtime.store.replace_mounts(&id, &req.host_ids) {
        Ok(change) => {
            if let Change::Done(record) = &change {
                tracing::info!(connection_id = %record.id, hosts = ?record.mounts, "gateway connection mounted");
            }
            changed(&state, change, StatusCode::OK)
        }
        Err(err) => internal(err),
    }
}

/// `PUT /api/mcp/connections/{id}/credential` (step-up): 204, the static
/// token sealed and stored. Only the connection is logged.
async fn credential(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<McpCredentialRequest>,
) -> Response {
    let _lock = state.runtime.lock(&id).await;
    match state
        .runtime
        .store
        .set_static_credential(&id, &req.token, &state.runtime.key, unix_now())
    {
        Ok(CredentialChange::Done) => {
            tracing::info!(connection_id = %id, "gateway connection's static credential set");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(CredentialChange::NotFound) => not_found(),
        Ok(CredentialChange::WrongKind(kind)) => error(
            StatusCode::CONFLICT,
            "wrong_cred_kind",
            format!("a {} connection takes no static token", kind.as_str()),
        ),
        Ok(CredentialChange::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}
