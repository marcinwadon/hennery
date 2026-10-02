//! The connections API (gateway spec §9): every route is the operator's
//! (`operator_only`: the browser rules, then the session cookie), and takes
//! its body as `ApiJson`. Creating a connection, deleting one, setting its
//! credential, and a change to where its token goes need a fresh step-up
//! (kernel spec §3.4; plan 8a decision 4). A token is never logged, never
//! answered, and stored only sealed.

use crate::key::MasterKey;
use crate::model::{
    Change, ConnectionPatch, ConnectionRecord, CredKind, CredentialChange, NewConnection, url_for_logs,
};
use crate::stdio::{self, StdioChange, StdioInput};
use crate::store::GatewayStore;
use axum::extract::rejection::QueryRejection;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::handler::Handler;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, put};
use axum::{Json, Router, middleware};
use hennery_kernel::auth::{operator_only, require_step_up, step_up_required};
use hennery_kernel::json::ApiJson;
use hennery_kernel::operator::{Authenticated, Operator};
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{
    ApiError, CreateMcpConnectionRequest, McpConnectionItem, McpConnectionStatus, McpCredKind, McpCredentialRequest,
    McpMountsRequest, McpStdioEnvItem, McpStdioServerItem, McpStdioServerSet, McpStdioServersRequest,
    UpdateMcpConnectionRequest,
};
use std::sync::Arc;

/// The largest body a route reads: an allowlist of `MAX_TOOLS` names of
/// 128 bytes fits, a token of `MAX_TOKEN` many times over.
const BODY_LIMIT: usize = 256 * 1024;

#[derive(Clone)]
pub struct GatewayState {
    pub store: Arc<GatewayStore>,
    /// What credentials are sealed with (gateway spec §6).
    pub key: Arc<MasterKey>,
    /// The owner and their sessions (kernel spec §3).
    pub operator: Arc<Operator>,
    /// What a revoke cuts (plan 8e decision 12): one per collector, shared
    /// by the proxy (`ProxyState::full`) and the sessions' side
    /// (`session::GatewayMcp`).
    pub revocations: crate::revocation::Revocations,
}

/// The routes, behind the operator's session. Step-up is layered on each
/// method that always needs it (`Handler::layer`), so a method added to a
/// route later gets none unless it is layered too; `PATCH` checks it in
/// its handler, for the fields that need it.
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
        // Plan 8e: a stdio server is a command the host runs, so a `PUT`
        // needs step-up (kernel spec §3.4); a `GET` does not.
        .route(
            "/api/mcp/stdio-servers",
            get(stdio_set).put(replace_stdio_set.layer(middleware::from_fn(require_step_up))),
        )
        .layer(DefaultBodyLimit::max(BODY_LIMIT));
    operator_only(routes, state.operator.clone())
        .layer(middleware::map_response(no_store))
        .with_state(state)
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

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
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

fn internal(err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "internal error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

fn not_found() -> Response {
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

fn model_kind(kind: McpCredKind) -> CredKind {
    match kind {
        McpCredKind::None => CredKind::None,
        McpCredKind::Static => CredKind::Static,
        McpCredKind::OauthDcr => CredKind::OauthDcr,
        McpCredKind::OauthClient => CredKind::OauthClient,
    }
}

fn item(record: ConnectionRecord) -> McpConnectionItem {
    McpConnectionItem {
        status: match record.status.as_str() {
            "ok" => McpConnectionStatus::Ok,
            "needs_auth" => McpConnectionStatus::NeedsAuth,
            "error" => McpConnectionStatus::Error,
            _ => McpConnectionStatus::NotConnected,
        },
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

fn changed(change: Change, status: StatusCode) -> Response {
    match change {
        Change::Done(record) => (status, Json(item(*record))).into_response(),
        Change::NotFound => not_found(),
        Change::SlugTaken => error(StatusCode::CONFLICT, "slug_taken", "another connection has this slug"),
        Change::TooMany => error(
            StatusCode::CONFLICT,
            "too_many_connections",
            format!("there are at most {} connections", crate::model::MAX_CONNECTIONS),
        ),
        Change::Unsupported(kind) => error(
            StatusCode::BAD_REQUEST,
            "unsupported_cred_kind",
            format!("{} connections are not supported yet", kind.as_str()),
        ),
        Change::Invalid(why) => error(StatusCode::BAD_REQUEST, "invalid", why),
    }
}

/// `GET /api/mcp/connections`: every connection, oldest first, without a
/// secret.
async fn list(State(state): State<GatewayState>) -> Response {
    match state.store.list() {
        Ok(records) => Json(records.into_iter().map(item).collect::<Vec<_>>()).into_response(),
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
    match state.store.create(&new, unix_now()) {
        Ok(change) => {
            if let Change::Done(record) = &change {
                tracing::info!(
                    connection_id = %record.id,
                    slug = %record.slug,
                    upstream = %url_for_logs(&record.url),
                    "gateway connection created"
                );
            }
            changed(change, StatusCode::CREATED)
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
    match state.store.update(&id, &patch, unix_now()) {
        Ok(change) => {
            if let Change::Done(record) = &change {
                tracing::info!(
                    connection_id = %record.id,
                    upstream = %url_for_logs(&record.url),
                    has_credential = record.has_credential,
                    "gateway connection changed"
                );
            }
            changed(change, StatusCode::OK)
        }
        Err(err) => internal(err),
    }
}

/// `DELETE /api/mcp/connections/{id}` (step-up): 204, with its mounts and
/// credential gone.
async fn delete(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    match state.store.delete(&id) {
        Ok(true) => {
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
    match state.store.replace_mounts(&id, &req.host_ids) {
        Ok(change) => {
            if let Change::Done(record) = &change {
                tracing::info!(connection_id = %record.id, hosts = ?record.mounts, "gateway connection mounted");
            }
            changed(change, StatusCode::OK)
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
    match state
        .store
        .set_static_credential(&id, &req.token, &state.key, unix_now())
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

/// `?host_id=&hat_id=` of the stdio routes. Read as text: a missing or
/// malformed one is an `ApiError`.
#[derive(serde::Deserialize)]
struct StdioQuery {
    host_id: Option<String>,
    hat_id: Option<String>,
}

/// Both ids, each 1 to 64 bytes; neither is quoted back when refused.
fn stdio_place(query: Result<Query<StdioQuery>, QueryRejection>) -> Result<(String, String), Box<Response>> {
    let refused = || {
        error(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("host_id and hat_id are each 1 to {} bytes", stdio::MAX_ID),
        )
    };
    let Ok(Query(query)) = query else {
        return Err(Box::new(refused()));
    };
    match (query.host_id, query.hat_id) {
        (Some(host), Some(hat))
            if (1..=stdio::MAX_ID).contains(&host.len()) && (1..=stdio::MAX_ID).contains(&hat.len()) =>
        {
            Ok((host, hat))
        }
        _ => Err(Box::new(refused())),
    }
}

fn stdio_answer(host_id: String, hat_id: String, change: StdioChange) -> Response {
    match change {
        StdioChange::Done(servers) => Json(McpStdioServerSet {
            host_id,
            hat_id,
            servers: servers
                .into_iter()
                .map(|server| McpStdioServerItem {
                    name: server.name,
                    command: server.command,
                    args: server.args,
                    // Every name stored has its value stored (`""` too).
                    env: server
                        .env
                        .into_iter()
                        .map(|name| McpStdioEnvItem { name, has_value: true })
                        .collect(),
                    created_at: rfc3339(server.created_at),
                    updated_at: rfc3339(server.updated_at),
                })
                .collect(),
        })
        .into_response(),
        StdioChange::NotFound => error(StatusCode::NOT_FOUND, "not_found", "no such host or hat"),
        StdioChange::HostRevoked => error(
            StatusCode::CONFLICT,
            "host_revoked",
            "the host is revoked: its stdio servers can be read, not changed",
        ),
        StdioChange::Invalid(why) => error(StatusCode::BAD_REQUEST, "invalid", why),
        StdioChange::EnvValueMissing(why) => error(StatusCode::BAD_REQUEST, "env_value_missing", why),
        StdioChange::SlugTaken(why) => error(StatusCode::CONFLICT, "slug_taken", why),
        StdioChange::TooMany(why) => error(StatusCode::CONFLICT, "too_many_stdio_servers", why),
    }
}

/// `GET /api/mcp/stdio-servers?host_id=&hat_id=`: the set, without a value.
async fn stdio_set(State(state): State<GatewayState>, query: Result<Query<StdioQuery>, QueryRejection>) -> Response {
    let (host_id, hat_id) = match stdio_place(query) {
        Ok(place) => place,
        Err(refused) => return *refused,
    };
    match state.store.stdio_set(&host_id, &hat_id) {
        Ok(change) => stdio_answer(host_id, hat_id, change),
        Err(err) => internal(err),
    }
}

/// `PUT /api/mcp/stdio-servers?host_id=&hat_id=` (step-up): the whole set,
/// replaced. Logged by its host, hat and size only: a command's arguments
/// and its values are the operator's secrets.
async fn replace_stdio_set(
    State(state): State<GatewayState>,
    query: Result<Query<StdioQuery>, QueryRejection>,
    ApiJson(req): ApiJson<McpStdioServersRequest>,
) -> Response {
    let (host_id, hat_id) = match stdio_place(query) {
        Ok(place) => place,
        Err(refused) => return *refused,
    };
    let servers: Vec<StdioInput> = req
        .servers
        .into_iter()
        .map(|server| StdioInput {
            name: server.name,
            command: server.command,
            args: server.args,
            env: server.env.into_iter().map(|var| (var.name, var.value)).collect(),
        })
        .collect();
    match state
        .store
        .replace_stdio_set(&host_id, &hat_id, &servers, &state.key, unix_now())
    {
        Ok(change) => {
            if let StdioChange::Done(stored) = &change {
                tracing::info!(%host_id, %hat_id, servers = stored.len(), "gateway stdio servers replaced");
            }
            stdio_answer(host_id, hat_id, change)
        }
        Err(err) => internal(err),
    }
}
