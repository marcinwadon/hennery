//! Stateless HTTP MCP and distinct generic lifecycle operations. Parse typed
//! arguments directly from raw JSON, preserving duplicate-key evidence.
use crate::{
    consumer::Forget,
    store::{Query, Remember, Store},
};
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json, value::RawValue};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::Semaphore;
#[derive(Clone)]
struct Service {
    store: Arc<Store>,
    permits: Arc<Semaphore>,
    admin_permits: Arc<Semaphore>,
    scopes: Arc<Mutex<HashMap<String, Weak<Semaphore>>>>,
    // Pre-authentication admission per claimed credential kind, taken before
    // any store lock or SQL.
    admission: Arc<Semaphore>,
    admin_admission: Arc<Semaphore>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rpc {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Box<RawValue>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    name: String,
    arguments: Box<RawValue>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilitiesProbe {
    version: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Initialize {
    protocol_version: String,
    capabilities: Value,
    #[serde(deserialize_with = "object_field")]
    client_info: ClientInfo,
}
fn object<T: DeserializeOwned>(raw: &str) -> anyhow::Result<T> {
    anyhow::ensure!(raw.trim_start().starts_with('{'), "object required");
    Ok(serde_json::from_str(raw)?)
}
fn object_field<'de, D: serde::Deserializer<'de>, T: DeserializeOwned>(de: D) -> Result<T, D::Error> {
    let raw = Box::<RawValue>::deserialize(de)?;
    object(raw.get()).map_err(serde::de::Error::custom)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientInfo {
    name: String,
    version: String,
    title: Option<String>,
    #[serde(default, rename = "description")]
    _description: Option<String>,
    #[serde(default, rename = "websiteUrl")]
    _website_url: Option<String>,
    #[serde(default, rename = "icons", deserialize_with = "object_icons")]
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
fn object_icons<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Option<Vec<ClientIcon>>, D::Error> {
    let raw = Option::<Vec<Box<RawValue>>>::deserialize(de)?;
    raw.map(|items| {
        items
            .into_iter()
            .map(|item| object(item.get()))
            .collect::<anyhow::Result<Vec<_>>>()
    })
    .transpose()
    .map_err(serde::de::Error::custom)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recent {
    project_key: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    query: String,
    kind: Option<String>,
    project_key: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Deterministic,
    Model,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Brief {
    subject: String,
    project_key: Option<String>,
    mode: Option<Mode>,
}
pub fn router(store: Arc<Store>) -> Router {
    routes(Service::new(store))
}
impl Service {
    fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            permits: Arc::new(Semaphore::new(32)),
            admin_permits: Arc::new(Semaphore::new(4)),
            scopes: Arc::default(),
            admission: Arc::new(Semaphore::new(32)),
            admin_admission: Arc::new(Semaphore::new(4)),
        }
    }
}
fn routes(service: Service) -> Router {
    Router::new().route("/mcp", post(rpc)).with_state(service)
}
const VERSIONS: &[&str] = &["2025-03-26", "2025-06-18", "2025-11-25"];
fn response(status: StatusCode, value: Value) -> Response {
    let mut response = (status, Json(value)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("header"));
    response
}
fn refused() -> Response {
    response(StatusCode::NOT_FOUND, json!({"error":"unavailable"}))
}
fn rpc_error(id: Value, code: i64) -> Response {
    response(
        StatusCode::OK,
        json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":"operation refused"}}),
    )
}
/// Store work runs on the blocking pool, never a runtime worker. The permits
/// move into that work: a cancelled caller cannot release them before it ends.
async fn blocking<P: Send + 'static, T: Send + 'static>(
    permits: P,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<(P, T)> {
    tokio::task::spawn_blocking(move || {
        let value = work();
        (permits, value)
    })
    .await
    .ok()
}
async fn rpc(State(service): State<Service>, request: Request) -> Response {
    if request.uri().query().is_some()
        || request.headers().contains_key(header::ORIGIN)
        || request.headers().contains_key(header::COOKIE)
    {
        return refused();
    }
    let mut headers = request.headers().get_all(header::AUTHORIZATION).iter();
    let Some(token) = headers
        .next()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::to_string)
    else {
        return refused();
    };
    if headers.next().is_some() || token.len() > 256 {
        return refused();
    }
    let credential_admin = token.starts_with("hnry_lifecycle_");
    if !credential_admin && !token.starts_with("hnry_plugin_") {
        return refused();
    }
    // Admit before the store lock: anonymous tokens wait on it too.
    let admission = if credential_admin {
        &service.admin_admission
    } else {
        &service.admission
    };
    let Ok(admitted) = admission.clone().try_acquire_owned() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, json!({"error":"busy"}));
    };
    let (store, credential) = (service.store.clone(), token.clone());
    let Some((_, Ok((namespace, stamp)))) = blocking(admitted, move || {
        let namespace = store.authenticated_namespace(&credential, credential_admin)?;
        let stamp = if credential_admin {
            None
        } else {
            Some(store.delivery_stamp(&credential)?)
        };
        anyhow::Ok((namespace, stamp))
    })
    .await
    else {
        return refused();
    };
    let versions = request
        .headers()
        .get_all("mcp-protocol-version")
        .iter()
        .collect::<Vec<_>>();
    if versions.len() > 1
        || versions
            .first()
            .is_some_and(|h| h.to_str().ok().is_none_or(|v| !VERSIONS.contains(&v)))
    {
        return response(StatusCode::BAD_REQUEST, json!({"error":"invalid protocol version"}));
    }
    let pool = if credential_admin {
        &service.admin_permits
    } else {
        &service.permits
    };
    let Ok(permit) = pool.clone().try_acquire_owned() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, json!({"error":"busy"}));
    };
    let scoped = {
        let mut scopes = service.scopes.lock().expect("scopes");
        scopes.retain(|_, value| value.strong_count() > 0);
        let name = format!("{namespace}:{credential_admin}");
        if let Some(scope) = scopes.get(&name).and_then(Weak::upgrade) {
            scope
        } else {
            let scope = Arc::new(Semaphore::new(if credential_admin { 1 } else { 4 }));
            scopes.insert(name, Arc::downgrade(&scope));
            scope
        }
    };
    let Ok(scoped_permit) = scoped.try_acquire_owned() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, json!({"error":"busy"}));
    };
    if !request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.split(';').next() == Some("application/json"))
    {
        return response(StatusCode::UNSUPPORTED_MEDIA_TYPE, json!({"error":"invalid request"}));
    }
    let raw = match tokio::time::timeout(Duration::from_secs(5), to_bytes(request.into_body(), 128 * 1024)).await {
        Ok(Ok(raw)) => raw,
        _ => return response(StatusCode::BAD_REQUEST, json!({"error":"invalid request"})),
    };
    let Ok(syntax) = serde_json::from_slice::<Box<RawValue>>(&raw) else {
        return rpc_error(Value::Null, -32700);
    };
    let Ok(rpc) = object::<Rpc>(syntax.get()) else {
        return rpc_error(Value::Null, -32600);
    };
    let id = rpc.id.clone().unwrap_or(Value::Null);
    if rpc.jsonrpc != "2.0"
        || !matches!(&id, Value::Null | Value::Number(_) | Value::String(_))
        || id.as_str().is_some_and(|s| s.len() > 128)
    {
        return rpc_error(Value::Null, -32600);
    }
    let admin = rpc.method.starts_with("hennery_plugin_");
    if admin != credential_admin {
        return refused();
    }
    let (store, credential) = (service.store.clone(), token.clone());
    let Some((permits, Ok(()))) = blocking((permit, scoped_permit), move || store.available(&credential, admin)).await
    else {
        return refused();
    };
    if rpc.method == "notifications/initialized" && rpc.id.is_none() {
        if rpc
            .params
            .as_deref()
            .is_some_and(|raw| object::<Empty>(raw.get()).is_err())
        {
            return rpc_error(Value::Null, -32602);
        }
        return (StatusCode::ACCEPTED, [(header::CACHE_CONTROL, "no-store")]).into_response();
    }
    if rpc.id.is_none() {
        return rpc_error(Value::Null, -32600);
    }
    if !admin && !["initialize", "ping", "tools/list", "tools/call"].contains(&rpc.method.as_str()) {
        return rpc_error(id, -32601);
    }
    let method = rpc.method;
    let params = rpc.params.as_deref().map(RawValue::get).unwrap_or("{}").to_string();
    let (store, credential) = (service.store.clone(), token.clone());
    let Some((permits, result)) = blocking(permits, move || -> Result<Value, anyhow::Error> {
        let (params, token, method) = (params.as_str(), credential.as_str(), method.as_str());
        if admin {
            if method == "hennery_plugin_forget" {
                return Ok(serde_json::to_value(
                    store.forget(token, object::<Forget>(params)?)?,
                )?);
            }
            anyhow::ensure!(
                method == "hennery_plugin_capabilities",
                "unsupported lifecycle operation"
            );
            store.lifecycle_active(token)?;
            let probe: CapabilitiesProbe = object(params)?;
            anyhow::ensure!(probe.version.is_none_or(|v| v == 1), "unsupported contract version");
            return Ok(json!({"version":1,"activity_v1":true,"forget_v1":true,"startup_context_v1":false}));
        }
        match method {
            "initialize" => {
                let init: Initialize = object(params)?;
                anyhow::ensure!(
                    init.protocol_version.len() <= 128
                        && init.capabilities.is_object()
                        && init.client_info.name.len() <= 256
                        && init.client_info.version.len() <= 128
                        && init.client_info.title.as_ref().is_none_or(|s| s.len() <= 256),
                    "unsupported initialize"
                );
                let version = if VERSIONS.contains(&init.protocol_version.as_str()) {
                    init.protocol_version.as_str()
                } else {
                    VERSIONS[VERSIONS.len() - 1]
                };
                Ok(
                    json!({"protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"hennery-memory","version":env!("CARGO_PKG_VERSION")}}),
                )
            }
            "ping" => {
                let _: Empty = object(params)?;
                Ok(json!({}))
            }
            "tools/list" => {
                let _: Empty = object(params)?;
                Ok(json!({"tools":tools()}))
            }
            "tools/call" => {
                let call: Call = object(params)?;
                let text = call.arguments.get();
                let value = match call.name.as_str() {
                    "memory_recent" => {
                        let q: Recent = object(text)?;
                        serde_json::to_value(store.recent(
                            token,
                            &Query {
                                project_key: q.project_key,
                                limit: q.limit,
                                cursor: q.cursor,
                                ..Query::default()
                            },
                        )?)?
                    }
                    "memory_search" => {
                        let q: Search = object(text)?;
                        serde_json::to_value(store.search(
                            token,
                            &Query {
                                query: Some(q.query),
                                kind: q.kind,
                                project_key: q.project_key,
                                limit: q.limit,
                                cursor: q.cursor,
                            },
                        )?)?
                    }
                    "memory_remember" => {
                        serde_json::to_value(store.remember(token, object::<Remember>(text)?)?)?
                    }
                    "memory_brief" => {
                        let q: Brief = object(text)?;
                        let mut value = store.brief(token, &q.subject, q.project_key)?;
                        if matches!(q.mode, Some(Mode::Model)) {
                            value["fallback_reason"] = json!("model support is not configured");
                        }
                        value
                    }
                    _ => anyhow::bail!("unknown tool"),
                };
                Ok(json!({"content":[{"type":"text","text":serde_json::to_string(&value)?}],"isError":false}))
            }
            _ => anyhow::bail!("unsupported method"),
        }
    })
    .await
    else {
        return rpc_error(id, -32603);
    };
    match result {
        Ok(result) => {
            let bytes = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"result":result})).expect("result");
            if bytes.len() > 16384 {
                return rpc_error(id, -32603);
            }
            // Recheck at delivery, after serialization. No mutex/DB guard is
            // held across the HTTP exchange or any network wait.
            let stream = futures::stream::once(async move {
                let checked = blocking(permits, move || {
                    service.store.available(&token, admin)?;
                    if let Some(stamp) = &stamp {
                        service.store.deliverable(&token, stamp)?;
                    }
                    anyhow::Ok(())
                })
                .await;
                if !matches!(checked, Some((_, Ok(())))) {
                    return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
                }
                Ok::<Bytes, std::io::Error>(Bytes::from(bytes))
            });
            let mut response = Response::new(Body::from_stream(stream));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, "application/json".parse().expect("header"));
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, "no-store".parse().expect("header"));
            response
        }
        Err(_) => rpc_error(id, -32602),
    }
}
fn tools() -> Vec<Value> {
    let optional = json!({"project_key":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20},"cursor":{"type":"string"}});
    let mut recent = optional.clone();
    let mut search = optional;
    search["query"] = json!({"type":"string","maxLength":512});
    search["kind"] = json!({"type":"string","enum":["memory","note","source"]});
    let entries = [
        (
            "memory_recent",
            "Recent completed-session observations",
            &mut recent,
            vec![],
        ),
        (
            "memory_search",
            "Search this credential's namespace",
            &mut search,
            vec!["query"],
        ),
    ];
    let mut tools=entries.into_iter().map(|(name,description,properties,required)|json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})).collect::<Vec<_>>();
    tools.push(json!({"name":"memory_brief","description":"Dated quotations and typed evidence; deterministic, no model","inputSchema":{"type":"object","properties":{"subject":{"type":"string","maxLength":512},"project_key":{"type":"string"},"mode":{"type":"string","enum":["deterministic","model"]}},"required":["subject"],"additionalProperties":false}}));
    tools.push(json!({"name":"memory_remember","description":"Retain agent-authored, unverified text; no verified session identity","inputSchema":{"type":"object","properties":{"title":{"type":"string","maxLength":160},"body":{"type":"string","maxLength":16384},"kind":{"type":"string","enum":["user","feedback","project","reference","decision","entity"]},"tags":{"type":"array","maxItems":16,"items":{"type":"string","maxLength":64}},"idempotency_key":{"type":"string","maxLength":128}},"required":["title","body","idempotency_key"],"additionalProperties":false}}));
    tools
}
#[cfg(test)]
mod admission_tests {
    use super::*;
    use std::sync::mpsc;
    use tower::ServiceExt;
    const TOOL: &str = "hnry_plugin_unknown";
    const LIFECYCLE: &str = "hnry_lifecycle_unknown";
    fn mcp(token: &str) -> Request {
        Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
            .unwrap()
    }
    async fn send(app: &Router, token: &str) -> StatusCode {
        app.clone().oneshot(mcp(token)).await.unwrap().status()
    }
    /// A refusal is decided before the first await: poll once, never wait.
    fn immediate(app: &Router, token: &str) -> Option<StatusCode> {
        futures::FutureExt::now_or_never(app.clone().oneshot(mcp(token))).map(|response| response.unwrap().status())
    }
    fn saturated(service: &Service) -> bool {
        service.admission.available_permits() == 0 && service.admin_admission.available_permits() == 0
    }
    /// Holds the store's own mutex, the contention point every store call
    /// takes, while a one-thread runtime serves requests. The runtime reports
    /// through `ready` while the lock is still held; an inline store call
    /// would stall it, so the bound only detects that failure.
    fn with_blocked_store<T: Send + 'static>(
        run: impl FnOnce(Service, mpsc::Sender<()>) -> std::pin::Pin<Box<dyn Future<Output = T> + Send>> + Send + 'static,
    ) -> (bool, T) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("memory.db")).unwrap());
        let service = Service::new(store.clone());
        let guard = store.inner.lock().unwrap();
        let (ready, observed) = mpsc::channel();
        let runtime = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(run(service, ready))
        });
        let responsive = observed.recv_timeout(Duration::from_secs(30)).is_ok();
        drop(guard);
        (responsive, runtime.join().unwrap())
    }

    #[test]
    fn blocked_store_bounds_pre_auth_admission_per_kind_without_stalling_the_runtime() {
        let (responsive, (extra, admitted)) = with_blocked_store(|service, ready| {
            Box::pin(async move {
                let app = routes(service.clone());
                let mut pending = Vec::new();
                for token in [TOOL; 32].into_iter().chain([LIFECYCLE; 4]) {
                    let app = app.clone();
                    pending.push(tokio::spawn(async move { send(&app, token).await }));
                }
                for _ in 0..1000 {
                    if saturated(&service) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                let extra = (immediate(&app, TOOL), immediate(&app, LIFECYCLE));
                let _ = ready.send(());
                let mut admitted = Vec::new();
                for request in pending {
                    admitted.push(request.await.unwrap());
                }
                (extra, admitted)
            })
        });
        assert!(responsive, "the runtime stalled behind the store lock");
        assert_eq!(
            extra,
            (
                Some(StatusCode::SERVICE_UNAVAILABLE),
                Some(StatusCode::SERVICE_UNAVAILABLE)
            )
        );
        assert_eq!(admitted, vec![StatusCode::NOT_FOUND; 36]);
    }

    #[test]
    fn cancelled_requests_hold_admission_until_blocking_auth_finishes() {
        let (responsive, (held, refused, released, after)) = with_blocked_store(|service, ready| {
            Box::pin(async move {
                let app = routes(service.clone());
                let mut pending = Vec::new();
                for _ in 0..32 {
                    let app = app.clone();
                    pending.push(tokio::spawn(async move { send(&app, TOOL).await }));
                }
                for _ in 0..1000 {
                    if service.admission.available_permits() == 0 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                for request in pending {
                    request.abort();
                    assert!(request.await.unwrap_err().is_cancelled());
                }
                let held = service.admission.available_permits();
                let refused = immediate(&app, TOOL);
                let _ = ready.send(());
                let released = tokio::time::timeout(
                    Duration::from_secs(30),
                    service.admission.clone().acquire_many_owned(32),
                )
                .await
                .map(drop)
                .is_ok();
                (held, refused, released, send(&app, TOOL).await)
            })
        });
        assert!(responsive, "the runtime stalled behind the store lock");
        assert_eq!(held, 0, "cancelled callers released permits early");
        assert_eq!(refused, Some(StatusCode::SERVICE_UNAVAILABLE));
        assert!(released, "blocking work never returned its permits");
        assert_eq!(after, StatusCode::NOT_FOUND);
    }
}
