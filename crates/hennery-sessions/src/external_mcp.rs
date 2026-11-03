//! Stateless MCP 2025 HTTP adapter for scoped external clients. No operator
//! cookie, delegated session token or machine activity grant authenticates it.
use crate::{
    AppState,
    external_clients::{ExternalIdentity, NewClient},
};
use axum::handler::Handler;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    middleware,
    routing::{any, delete, get},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

const VERSION: &str = "2025-11-25";
/// Single-message profiles shared with the optional stdio bridge. March 2025
/// allowed batches, which this bounded adapter does not implement.
pub const PROTOCOL_VERSIONS: &[&str] = &[VERSION, "2025-06-18"];
const MAX_BODY: usize = 64 * 1024;
fn private(response: impl IntoResponse) -> Response {
    let mut response = response.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    response
}
pub fn router(state: AppState) -> Router {
    let manage = Router::new()
        .route(
            "/api/external-clients",
            get(list).post(issue.layer(middleware::from_fn(hennery_kernel::auth::require_step_up))),
        )
        .route(
            "/api/external-clients/{id}",
            delete(revoke).route_layer(middleware::from_fn(hennery_kernel::auth::require_step_up)),
        );
    hennery_kernel::auth::operator_only(manage, state.operator.clone())
        .with_state(state.clone())
        .merge(
            Router::new()
                .route("/mcp/external/hennery", any(endpoint))
                .with_state(state),
        )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantPage {
    #[serde(default)]
    after: String,
}
async fn list(State(state): State<AppState>, Query(page): Query<GrantPage>) -> Response {
    match state.store.external_clients_after(&page.after) {
        Ok(rows) => private(Json(rows)),
        Err(e) if e.to_string() == "invalid_arguments" => {
            crate::api::error(StatusCode::BAD_REQUEST, "invalid_arguments", "invalid cursor")
        }
        Err(e) => crate::api::internal(e),
    }
}
async fn issue(
    State(state): State<AppState>,
    headers: HeaderMap,
    hennery_kernel::json::ApiJson(input): hennery_kernel::json::ApiJson<NewClient>,
) -> Response {
    if let Err(response) = crate::external_oauth::fresh_operator(&state, &headers) {
        return *response;
    }
    match state.store.issue_external_client(&input) {
        Ok((client, token)) => private((StatusCode::CREATED, Json(json!({"client":client,"token":token})))),
        Err(e) if e.to_string() == "invalid_grant" => crate::api::error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "review hat, host, canonical roots and expiration",
        ),
        Err(e) => crate::api::internal(e),
    }
}
async fn revoke(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.store.revoke_external_client(&id) {
        Ok(true) => private(StatusCode::NO_CONTENT),
        Ok(false) => crate::api::error(StatusCode::NOT_FOUND, "not_found", "no such client"),
        Err(e) => crate::api::internal(e),
    }
}
fn rpc(id: Value, code: i32, message: &str) -> Response {
    private(Json(
        json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
    ))
}
fn result(id: Value, value: Value) -> Response {
    private(Json(json!({"jsonrpc":"2.0","id":id,"result":value})))
}
fn tools(identity: &ExternalIdentity) -> Vec<Value> {
    let mut tools = vec![
        json!({"name":"hennery_capabilities","description":"Read this external client's effective scope, session-control capability and limits.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}),
        json!({"name":"host_list","description":"List only hosts explicitly granted to this external client.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}),
        json!({"name":"workspace_list","description":"List explicitly granted canonical roots, not arbitrary files or other projects.","inputSchema":{"type":"object","properties":{"host_id":{"type":"string"}},"required":["host_id"],"additionalProperties":false}}),
    ];
    if identity.client.read_metadata {
        tools.push(json!({"name":"session_list","description":"Read bounded session metadata in this client's hat/host/root and created-or-existing session grant; never acknowledges unseen activity.","inputSchema":{"type":"object","properties":{"after":{"type":"string","maxLength":128},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}}));
        tools.push(json!({"name":"session_get","description":"Read one authorized session's metadata. Transcript and permission answers are not exposed.","inputSchema":{"type":"object","properties":{"session_id":{"type":"string","minLength":1,"maxLength":128}},"required":["session_id"],"additionalProperties":false}}));
    }
    for tool in &mut tools {
        tool["annotations"] =
            json!({"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false});
        tool["outputSchema"] =
            json!({"type":"object","properties":{"data":{}},"required":["data"],"additionalProperties":false});
    }
    tools.extend(crate::external_control::tools(identity));
    tools
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    name: String,
    #[serde(default = "empty_args")]
    arguments: Value,
    /// Protocol metadata is transport data, never grant or caller identity.
    #[serde(default, rename = "_meta")]
    _meta: serde_json::Map<String, Value>,
}
fn empty_args() -> Value {
    json!({})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostArg {
    host_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionArg {
    session_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    after: String,
    #[serde(default = "default_limit")]
    limit: u32,
}
fn default_limit() -> u32 {
    25
}
fn arguments<T: serde::de::DeserializeOwned>(v: Value) -> anyhow::Result<T> {
    serde_json::from_value(v).map_err(|_| anyhow::anyhow!("invalid_arguments"))
}
fn invoke(state: &AppState, identity: &ExternalIdentity, call: Call) -> anyhow::Result<Value> {
    let roots = state.store.external_roots(identity)?; // Recheck even for capabilities.
    let before_roots = roots.clone();
    let value = match call.name.as_str() {
        "hennery_capabilities" => {
            let _: std::collections::BTreeMap<String, Value> = arguments(call.arguments.clone())?;
            anyhow::ensure!(call.arguments == json!({}), "invalid_arguments");
            json!({"client_id":identity.client.client_id,"hat_id":identity.client.hat_id,"revision":identity.client.revision,"expires_at":identity.client.expires_at,"existing_sessions":identity.client.existing_sessions,"read_metadata":identity.client.read_metadata,"read_transcript":identity.client.read_transcript,"execution":identity.client.control_sessions,"max_page":50,"permissions":"operator_ui","source_policy":"server_resolved_per_hat_actual_acp"})
        }
        "host_list" => {
            anyhow::ensure!(call.arguments == json!({}), "invalid_arguments");
            let mut rows = vec![];
            let mut ids = std::collections::BTreeSet::new();
            for root in roots {
                if !ids.insert(root.host_id.clone()) {
                    continue;
                }
                if let Some(host) = state.hosts.host(&root.host_id)? {
                    rows.push(json!({"host_id":host.id,"name":host.name,"connected":state.hub.is_ready(&root.host_id),"agents":state.hub.runtime_ids(&root.host_id)}));
                }
            }
            json!({"hosts":rows})
        }
        "workspace_list" => {
            let arg: HostArg = arguments(call.arguments)?;
            let roots: Vec<_> = roots.into_iter().filter(|r| r.host_id == arg.host_id).collect();
            anyhow::ensure!(!roots.is_empty(), "not_found");
            json!({"roots":roots})
        }
        "session_list" => {
            let page: Page = arguments(call.arguments)?;
            let rows = state.store.external_sessions(identity, &page.after, None, page.limit)?;
            let next = if rows.len() == page.limit as usize {
                rows.last().map(|s| s.session_id.clone())
            } else {
                None
            };
            json!({"sessions":rows,"next_cursor":next})
        }
        "session_get" => {
            let arg: SessionArg = arguments(call.arguments)?;
            let row = state
                .store
                .external_sessions(identity, "", Some(&arg.session_id), 1)?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("not_found"))?;
            json!({"session":row})
        }
        _ => anyhow::bail!("unknown_tool"),
    };
    // Metadata authorization linearizes at its Store query gate. This final
    // check catches grant/root revocation, not session changes after collection;
    // an already-authorized metadata snapshot may finish after reassignment.
    anyhow::ensure!(
        state.store.external_roots(identity)? == before_roots,
        "external_unauthenticated"
    );
    anyhow::ensure!(serde_json::to_vec(&value)?.len() <= 256 * 1024, "result_too_large");
    Ok(json!({"data":value}))
}
async fn endpoint(
    State(state): State<AppState>,
    address: hennery_kernel::client_address::ClientAddress,
    req: Request,
) -> Response {
    let Some(_permit) = state.external_admission.try_acquire(address.effective) else {
        return crate::external_oauth::at_capacity();
    };
    let headers = req.headers();
    if let Some(origin) = headers.get(header::ORIGIN)
        && (headers.get_all(header::ORIGIN).iter().count() != 1
            || !state
                .operator
                .public_url()
                .is_some_and(|url| origin.to_str().ok() == Some(url.origin())))
    {
        return private(StatusCode::FORBIDDEN);
    }
    let mut auth = headers.get_all(header::AUTHORIZATION).iter();
    let token = auth
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if auth.next().is_some() {
        return crate::external_oauth::challenge(&state);
    }
    let resource = state
        .operator
        .public_url()
        .map(|p| format!("{}/mcp/external/hennery", p.origin()))
        .unwrap_or_default();
    let identity = match token.map(|t| state.store.authenticate_external_resource(t, &resource)) {
        Some(Ok(Some(i))) => i,
        Some(Err(e)) => return crate::api::internal(e),
        _ => return crate::external_oauth::challenge(&state),
    };
    if req.method() != Method::POST {
        let mut response = private(StatusCode::METHOD_NOT_ALLOWED);
        response
            .headers_mut()
            .insert(header::ALLOW, header::HeaderValue::from_static("POST"));
        return response;
    }
    if !headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next() == Some("application/json"))
    {
        return private(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    if headers
        .get("mcp-protocol-version")
        .is_some_and(|v| !v.to_str().is_ok_and(|version| PROTOCOL_VERSIONS.contains(&version)))
    {
        return private(StatusCode::BAD_REQUEST);
    }
    let bytes =
        match tokio::time::timeout(Duration::from_secs(5), axum::body::to_bytes(req.into_body(), MAX_BODY)).await {
            Ok(Ok(b)) => b,
            Ok(Err(_)) => return private(StatusCode::PAYLOAD_TOO_LARGE),
            Err(_) => return private(StatusCode::REQUEST_TIMEOUT),
        };
    let request: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return rpc(Value::Null, -32700, "parse_error"),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if state.store.external_roots(&identity).is_err() {
        return crate::external_oauth::challenge(&state);
    }
    if !request.is_object()
        || request.get("jsonrpc") != Some(&json!("2.0"))
        || request.get("method").and_then(Value::as_str).is_none()
        || (request.get("id").is_some() && !id.is_string() && !id.is_i64() && !id.is_u64())
    {
        return rpc(Value::Null, -32600, "invalid_request");
    }
    if request.get("id").is_none() {
        return private(StatusCode::ACCEPTED);
    }
    match request.get("method").and_then(Value::as_str) {
        Some("initialize") => {
            let offered = request
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(VERSION);
            let version = if PROTOCOL_VERSIONS.contains(&offered) {
                offered
            } else {
                VERSION
            };
            result(
                id,
                json!({"protocolVersion":version,"capabilities":{"tools":{}},"serverInfo":{"name":"hennery-external","version":env!("CARGO_PKG_VERSION")},"instructions":"Scoped external session management. Use request_key for retry-safe mutations and operation_get/session_wait for outcomes. Conversation access and session control require separate consent; permission requests stay in Hennery's operator UI."}),
            )
        }
        Some("ping") => result(id, json!({})),
        Some("tools/list") => result(id, json!({"tools":tools(&identity)})),
        Some("tools/call") => {
            let call: Call = match serde_json::from_value(request.get("params").cloned().unwrap_or(Value::Null)) {
                Ok(v) => v,
                Err(_) => return rpc(id, -32602, "invalid_arguments"),
            };
            let outcome = if crate::external_control::is_control_tool(&call.name) {
                crate::external_control::invoke(&state, &identity, &call.name, call.arguments).await
            } else {
                invoke(&state, &identity, call)
            };
            match outcome {
                Ok(data) => result(
                    id,
                    json!({"content":[{"type":"text","text":serde_json::to_string(&data).expect("JSON value")}],"structuredContent":data,"isError":false}),
                ),
                Err(e) if e.to_string() == "external_unauthenticated" => crate::external_oauth::challenge(&state),
                Err(e) if e.to_string() == "external_forbidden" => crate::external_oauth::insufficient_scope(&state),
                Err(e) if e.to_string() == "external_control_forbidden" => crate::external_oauth::scope_challenge(
                    &state,
                    "hennery:discover hennery:sessions:read hennery:sessions:control",
                ),
                Err(e) if e.to_string() == "external_transcript_forbidden" => crate::external_oauth::scope_challenge(
                    &state,
                    "hennery:discover hennery:sessions:read hennery:sessions:transcript",
                ),
                Err(e) if e.to_string() == "unknown_tool" => rpc(id, -32602, "unknown_tool"),
                Err(e) => {
                    let code = match e.to_string().as_str() {
                        "invalid_arguments" => "invalid_arguments",
                        "not_found" => "not_found",
                        "result_too_large" => "result_too_large",
                        "invalid_request_key" => "invalid_request_key",
                        "request_key_conflict" => "request_key_conflict",
                        "operation_limit" => "operation_limit",
                        "session_limit" => "session_limit",
                        "resync_required" => "resync_required",
                        _ => return rpc(id, -32603, "internal_error"),
                    };
                    result(id, json!({"content":[{"type":"text","text":code}],"isError":true}))
                }
            }
        }
        _ => rpc(id, -32601, "method_not_found"),
    }
}
