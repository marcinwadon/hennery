//! Collector-local session MCP. Stateless JSON responses; no control tools are
//! enabled until D2/D3. The proxy's safe body/header primitives are shared,
//! never its upstream forwarding path.
use crate::{AppState, api::error};
use axum::handler::Handler;
use axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use futures::{StreamExt, stream};
use hennery_gateway::{
    api::GatewayState,
    identity::SessionIdentity,
    proxy::{Limits, MAX_REQUEST_BODY, RequestBody, bearer, drain_rejected},
    revocation::Watch,
};
use hennery_kernel::json::ApiJson;
use hennery_proto::delegation::{HatSessionTools, SetSessionTools};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub const VERSIONS: &[&str] = &["2025-03-26", "2025-06-18", "2025-11-25"];
#[derive(Clone)]
struct BuiltinState {
    app: AppState,
    gateway: GatewayState,
    limits: Arc<Limits>,
}
pub fn router(app: AppState, gateway: GatewayState) -> Router {
    router_with_limits(app, gateway, Limits::default())
}
pub fn router_with_limits(app: AppState, gateway: GatewayState, limits: Limits) -> Router {
    Router::new()
        .route("/mcp/builtin/hennery", any(endpoint))
        .with_state(BuiltinState {
            app,
            gateway,
            limits: Arc::new(limits),
        })
}
fn no_scope() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "not found")
}
fn rpc_error(id: Value, code: i64, status: StatusCode) -> Response {
    (
        status,
        json_response(
            json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":"invalid or unavailable request"}}),
        ),
    )
        .into_response()
}
fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else { return Ok(None) };
    if values.next().is_some() {
        return Err(());
    }
    value.to_str().map(Some).map_err(|_| ())
}
fn origin_ok(headers: &HeaderMap, public: Option<&str>) -> bool {
    matches!(one_header(headers, "origin"), Ok(None))
        || matches!(one_header(headers, "origin"), Ok(Some(origin)) if Some(origin) == public)
}

async fn endpoint(
    State(state): State<BuiltinState>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let mut body = RequestBody::new(body, &headers);
    let mut permit = None;
    let origin = state.app.operator.public_url();
    let mut token_watch = None;
    let mut grant_watch = None;
    let answer = if !origin_ok(&headers, origin.as_ref().map(|p| p.origin())) {
        error(StatusCode::FORBIDDEN, "origin_refused", "origin refused")
    } else if uri.query().is_some() {
        no_scope()
    } else if let Some(token) = bearer(&headers) {
        token_watch = state
            .gateway
            .runtime
            .store
            .watch_session(token, &state.gateway.runtime.revocations);
        if let Some(watch) = &token_watch {
            let cancelled = watch.token();
            let admitted = state.gateway.runtime.store.authenticate_session(token).ok().flatten();
            if let Some(identity) = admitted {
                grant_watch = state.gateway.runtime.store.watch_delegation(&identity).ok().flatten();
                if let Some(grant) = &grant_watch {
                    let grant_cancelled = grant.token();
                    tokio::select! { biased;
                        () = cancelled.cancelled() => no_scope(),
                        () = grant_cancelled.cancelled() => no_scope(),
                        response = handle(&state, identity, method, &headers, &mut body, &mut permit) => response,
                    }
                } else {
                    no_scope()
                }
            } else {
                no_scope()
            }
        } else {
            no_scope()
        }
    } else {
        no_scope()
    };
    let answer = if answer.status().is_success() {
        answer
    } else {
        drain_rejected(body, answer, &state.limits).await
    };
    // A cut before response acceptance returns the uniform refusal. After
    // acceptance both watches stay on the response until its body ends.
    if answer.status() == StatusCode::NOT_FOUND {
        token_watch = None;
        grant_watch = None;
    }
    secure(watched_response(answer, token_watch, grant_watch, permit))
}
fn watched_response(
    answer: Response,
    token: Option<Watch>,
    grant: Option<Watch>,
    permit: Option<hennery_kernel::egress::Permit>,
) -> Response {
    let (parts, body) = answer.into_parts();
    let stream = stream::unfold(
        (Some(body.into_data_stream()), token, grant, permit),
        |(body, token, grant, permit)| async move {
            let mut body = body?;
            let token_cancelled = token.as_ref().map(Watch::token);
            let grant_cancelled = grant.as_ref().map(Watch::token);
            let cancelled = async move {
                if let Some(token) = token_cancelled {
                    token.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            let grant_cancelled = async move {
                if let Some(grant) = grant_cancelled {
                    grant.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! { biased;
                () = cancelled => Some((Err(std::io::Error::other("session authority ended")), (None, None, None, None))),
                () = grant_cancelled => Some((Err(std::io::Error::other("session authority ended")), (None, None, None, None))),
                next = body.next() => next.map(|chunk| (chunk.map_err(std::io::Error::other), (Some(body), token, grant, permit))),
            }
        },
    );
    Response::from_parts(parts, Body::from_stream(stream))
}
fn secure(mut answer: Response) -> Response {
    answer
        .headers_mut()
        .insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    answer.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    answer
}
fn json_response(mut value: Value) -> Response {
    hennery_kernel::redact::value(&mut value);
    Json(value).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Initialize {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    capabilities: serde_json::Map<String, Value>,
    #[serde(rename = "clientInfo")]
    client_info: ClientInfo,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientInfo {
    name: String,
    version: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "description")]
    _description: Option<String>,
    #[serde(default, rename = "websiteUrl")]
    _website_url: Option<String>,
    #[serde(default, rename = "icons")]
    _icons: Option<Vec<ClientIcon>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientIcon {
    #[serde(rename = "src")]
    _src: String,
    #[serde(default, rename = "mimeType")]
    _mime_type: Option<String>,
    #[serde(default, rename = "sizes")]
    _sizes: Option<Vec<String>>,
    #[serde(default, rename = "theme")]
    _theme: Option<IconTheme>,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum IconTheme {
    Light,
    Dark,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    name: String,
    arguments: serde_json::Map<String, Value>,
    /// MCP metadata is transport data, never caller identity or arguments.
    #[serde(default, rename = "_meta")]
    metadata: serde_json::Map<String, Value>,
}

async fn handle(
    state: &BuiltinState,
    identity: SessionIdentity,
    method: Method,
    headers: &HeaderMap,
    body: &mut RequestBody,
    permit: &mut Option<hennery_kernel::egress::Permit>,
) -> Response {
    let Some(revision) = identity.delivered_delegation_revision() else {
        return no_scope();
    };
    let context = state
        .app
        .store
        .session_tool_context(&identity, revision, false)
        .ok()
        .flatten();
    let Some(context) = context else { return no_scope() };
    let delivery = state.app.store.mcp_delivery(context.session_id()).ok().flatten();
    if !delivery.is_some_and(|d| {
        state
            .app
            .hub
            .delivery_supported(context.host_id(), context.agent(), d.mode.as_str())
    }) {
        return no_scope();
    }
    if method != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if !matches!(
        one_header(headers, "mcp-protocol-version"),
        Ok(None) | Ok(Some("2025-03-26" | "2025-06-18" | "2025-11-25"))
    ) {
        return rpc_error(Value::Null, -32600, StatusCode::BAD_REQUEST);
    }
    if !matches!(one_header(headers, "content-type"), Ok(Some(value)) if value.split(';').next().is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json")))
    {
        return rpc_error(Value::Null, -32600, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    if !matches!(one_header(headers, "accept"), Ok(Some(value)) if value.split(',').any(|v| v.trim().split(';').next() == Some("application/json")))
    {
        return rpc_error(Value::Null, -32600, StatusCode::NOT_ACCEPTABLE);
    }
    let Ok(acquired) = state.limits.requests.try_acquire("builtin") else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "busy", "busy");
    };
    *permit = Some(acquired);
    let read = async {
        let mut bytes = Vec::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| StatusCode::BAD_REQUEST)?;
            if bytes.len() + chunk.len() > MAX_REQUEST_BODY {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    let bytes = match tokio::time::timeout(state.limits.body_timeout, read).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(status)) => return rpc_error(Value::Null, -32600, status),
        Err(_) => return rpc_error(Value::Null, -32600, StatusCode::REQUEST_TIMEOUT),
    };
    let Some(value) = hennery_gateway::decode_json_once(&bytes) else {
        return rpc_error(Value::Null, -32700, StatusCode::BAD_REQUEST);
    };
    let Some(object) = value.as_object() else {
        return rpc_error(Value::Null, -32600, StatusCode::BAD_REQUEST);
    };
    let has_id = object.contains_key("id");
    let Ok(message) = serde_json::from_value::<Message>(value) else {
        return rpc_error(Value::Null, -32600, StatusCode::BAD_REQUEST);
    };
    if message.jsonrpc != "2.0"
        || message
            .id
            .as_ref()
            .is_some_and(|id| !(id.is_string() || id.as_i64().is_some() || id.as_u64().is_some()))
    {
        return rpc_error(Value::Null, -32600, StatusCode::BAD_REQUEST);
    }
    let no_params = message.params.as_ref().is_none_or(|p| {
        p.as_object()
            .is_some_and(|p| p.is_empty() || (p.len() == 1 && p.contains_key("_meta")))
    });
    let id = message.id.unwrap_or(Value::Null);
    if message.method == "notifications/initialized" && !has_id && no_params {
        return StatusCode::ACCEPTED.into_response();
    }
    if id.is_null() {
        return rpc_error(Value::Null, -32600, StatusCode::BAD_REQUEST);
    }
    let result = match message.method.as_str() {
        "initialize" => {
            let params = message.params.unwrap_or(Value::Null);
            if !params.is_object() || !params.get("clientInfo").is_some_and(Value::is_object) {
                return rpc_error(id, -32602, StatusCode::BAD_REQUEST);
            }
            if params["clientInfo"].get("icons").is_some_and(|icons| {
                !icons.is_null() && !icons.as_array().is_some_and(|items| items.iter().all(Value::is_object))
            }) {
                return rpc_error(id, -32602, StatusCode::BAD_REQUEST);
            }
            let Ok(init) = serde_json::from_value::<Initialize>(params) else {
                return rpc_error(id, -32602, StatusCode::BAD_REQUEST);
            };
            let _ = (
                &init.capabilities,
                &init.client_info.name,
                &init.client_info.version,
                &init.client_info.title,
            );
            let version = if VERSIONS.contains(&init.protocol_version.as_str()) {
                init.protocol_version.as_str()
            } else {
                VERSIONS[VERSIONS.len() - 1]
            };
            json!({"protocolVersion":version, "capabilities":{"tools":{}}, "serverInfo":{"name":"hennery","version":env!("CARGO_PKG_VERSION")}})
        }
        "ping" if no_params => json!({}),
        "tools/list" if no_params => crate::tool_args::discovery(),
        "tools/call" => {
            let params = message.params.unwrap_or(Value::Null);
            if !params.is_object() {
                return rpc_error(id, -32602, StatusCode::BAD_REQUEST);
            }
            let Ok(call) = serde_json::from_value::<Call>(params) else {
                return rpc_error(id, -32602, StatusCode::BAD_REQUEST);
            };
            if serde_json::to_vec(&call.metadata).map_or(true, |bytes| bytes.len() > 16 * 1024) {
                return rpc_error(id, -32602, StatusCode::BAD_REQUEST);
            }
            let active = state.app.hub.is_ready(context.host_id())
                && state
                    .app
                    .store
                    .session_tool_context(&identity, revision, true)
                    .ok()
                    .flatten()
                    .is_some();
            let (structured, failed) = if !active {
                (
                    json!({"code":"session_starting","message":"the session is starting"}),
                    true,
                )
            } else {
                match tokio::time::timeout(
                    state.limits.answer_timeout.min(std::time::Duration::from_secs(30)),
                    crate::session_tools::execute(
                        &state.app,
                        &identity,
                        revision,
                        &call.name,
                        Value::Object(call.arguments),
                    ),
                )
                .await
                {
                    Ok(Ok(value)) => (value, false),
                    Ok(Err(error)) => (
                        json!({"code":crate::session_tools::error_code(&error),"message":crate::session_tools::error_message(&error)}),
                        true,
                    ),

                    Err(_) => (
                        json!({"code":"request_timeout","message":"retry with the same operation key"}),
                        true,
                    ),
                }
            };
            json!({"isError":failed,"structuredContent":structured,"content":[{"type":"text","text":structured.to_string()}]})
        }
        "ping" | "tools/list" => return rpc_error(id, -32602, StatusCode::BAD_REQUEST),
        _ => return rpc_error(id, -32601, StatusCode::OK),
    };
    json_response(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

/// Policy routes use the existing operator Origin/cookie/step-up layers.
pub fn policy_router(state: AppState) -> Router {
    use axum::{middleware, routing::get};
    let routes = Router::new().route(
        "/api/hats/{id}/session-tools",
        get(get_policy).patch(set_policy.layer(middleware::from_fn(hennery_kernel::auth::require_step_up))),
    );
    hennery_kernel::auth::operator_only(routes, state.operator.clone()).with_state(state)
}
async fn get_policy(State(state): State<AppState>, axum::extract::Path(hat): axum::extract::Path<String>) -> Response {
    let Some(store) = state.store.mcp().authenticator() else {
        return no_scope();
    };
    policy_result(store.session_tools(&hat))
}
async fn set_policy(
    State(state): State<AppState>,
    axum::extract::Path(hat): axum::extract::Path<String>,
    ApiJson(request): ApiJson<SetSessionTools>,
) -> Response {
    let Some(store) = state.store.mcp().authenticator() else {
        return no_scope();
    };
    policy_result(store.set_session_tools(&hat, request.delegation_enabled))
}
fn policy_result(result: anyhow::Result<Option<HatSessionTools>>) -> Response {
    match result {
        Ok(Some(setting)) => Json(setting).into_response(),
        Ok(None) => no_scope(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_gateway::revocation::Revocations;
    #[tokio::test]
    async fn response_watches_cut_after_acceptance_and_release_on_drop() {
        for grant_cut in [false, true] {
            let tokens = Revocations::new();
            let grants = Revocations::new();
            let response = watched_response(
                Json(json!({"accepted":true})).into_response(),
                Some(tokens.watch("generation")),
                Some(grants.watch("grant")),
                None,
            );
            assert_eq!(tokens.watched(), 1);
            assert_eq!(grants.watched(), 1);
            if grant_cut {
                grants.cut_all();
            } else {
                tokens.cut_all();
            }
            assert!(axum::body::to_bytes(response.into_body(), 1024).await.is_err());
            assert_eq!(tokens.watched(), 0);
            assert_eq!(grants.watched(), 0);
        }
        let tokens = Revocations::new();
        let grants = Revocations::new();
        let response = watched_response(
            Json(json!({"accepted":true})).into_response(),
            Some(tokens.watch("generation")),
            Some(grants.watch("grant")),
            None,
        );
        assert!(axum::body::to_bytes(response.into_body(), 1024).await.is_ok());
        assert_eq!(tokens.watched(), 0);
        assert_eq!(grants.watched(), 0);
    }
}
