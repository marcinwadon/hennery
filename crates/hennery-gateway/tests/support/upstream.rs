//! A fake streamable-HTTP MCP upstream, and the proxy served over loopback
//! on a `World` (plan 8d). Connections reach the fake under
//! `internal_network`, as every test connection must (lane L7): there is
//! no test-only bypass.

use super::World;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::Response;
use hennery_gateway::proxy::{Limits, ProxyState, router};
use hennery_kernel::egress::{Egress, Timeouts};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// One request the fake upstream received.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    /// Path and query, as sent.
    pub uri: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|v| v.to_str().unwrap())
    }
}

type Handler = Arc<dyn Fn(&Seen, &watch::Receiver<()>) -> Response + Send + Sync>;

/// The fake upstream: records every request and answers with whatever the
/// test's handler builds. A body that blocks waits on the hold, which is
/// released when the fake is dropped.
pub struct FakeUpstream {
    pub addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    handler: Arc<Mutex<Handler>>,
    release: Option<watch::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeUpstream {
    pub async fn start() -> Self {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let default: Handler = Arc::new(|_, _| json(StatusCode::OK, &serde_json::json!({})));
        let handler = Arc::new(Mutex::new(default));
        let (release, hold) = watch::channel(());
        let app = {
            let seen = seen.clone();
            let handler = handler.clone();
            Router::new().fallback(move |req: Request<Body>| {
                let seen = seen.clone();
                let handler = handler.clone();
                let hold = hold.clone();
                async move {
                    let (parts, body) = req.into_parts();
                    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec();
                    let one = Seen {
                        method: parts.method.to_string(),
                        uri: parts.uri.to_string(),
                        headers: parts.headers,
                        body,
                    };
                    seen.lock().unwrap().push(one.clone());
                    let handler = handler.lock().unwrap().clone();
                    handler(&one, &hold)
                }
            })
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            seen,
            handler,
            release: Some(release),
            task,
        }
    }

    /// Answer every request with `handler`'s response.
    pub fn reply(&self, handler: impl Fn(&Seen, &watch::Receiver<()>) -> Response + Send + Sync + 'static) {
        *self.handler.lock().unwrap() = Arc::new(handler);
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

impl Drop for FakeUpstream {
    fn drop(&mut self) {
        self.release.take();
        self.task.abort();
    }
}

/// A JSON answer.
pub fn json(status: StatusCode, value: &serde_json::Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

/// An event stream of `events`, each a complete event's text.
pub fn sse(events: &[String]) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(events.concat()))
        .unwrap()
}

/// One `data:` event holding `value`.
pub fn event(value: &serde_json::Value) -> String {
    format!("event: message\ndata: {value}\n\n")
}

/// A body of `content_type` that sends `first` and then blocks until the
/// fake is dropped (gateway spec §5.3's streaming test).
pub fn first_then_block(content_type: &str, first: &str, hold: &watch::Receiver<()>) -> Response {
    let first = Bytes::from(first.to_string());
    let hold = hold.clone();
    let stream = futures::stream::unfold(Some(first), move |state| {
        let mut hold = hold.clone();
        async move {
            match state {
                Some(first) => Some((Ok::<_, std::io::Error>(first), None)),
                None => {
                    // Until the sender is dropped.
                    while hold.changed().await.is_ok() {}
                    None
                }
            }
        }
    });
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(stream))
        .unwrap()
}

/// A body that sends `first` and then, once the head and that chunk have
/// gone out, fails, cutting the connection.
pub fn first_then_fail(content_type: &str, first: &str) -> Response {
    let first = Bytes::from(first.to_string());
    let stream = futures::stream::unfold(Some(first), |state| async move {
        match state {
            Some(first) => Some((Ok(first), None)),
            None => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Some((Err(std::io::Error::other("cut")), None))
            }
        }
    });
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(stream))
        .unwrap()
}

/// The proxy over loopback, on a `World` (its fields and helpers through
/// `Deref`).
pub struct Harness {
    pub world: World,
    pub addr: SocketAddr,
    pub client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
}

impl std::ops::Deref for Harness {
    type Target = World;

    fn deref(&self) -> &World {
        &self.world
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Harness {
    pub async fn new() -> Self {
        Self::with_limits(Limits::new(8, 8, Duration::from_secs(10), Duration::from_secs(2))).await
    }

    pub async fn with_limits(limits: Limits) -> Self {
        let world = World::new();
        let egress = Egress::new(Timeouts {
            connect: Duration::from_secs(2),
            request: Duration::from_secs(10),
        })
        .unwrap();
        let gateway = world.gateway();
        let app = router(ProxyState::full(world.proxy_store.clone(), &gateway, egress, limits));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        Self {
            world,
            addr,
            client,
            task,
        }
    }

    pub fn url(&self, slug: &str) -> String {
        format!("http://{}/mcp/{slug}", self.addr)
    }

    /// `POST /mcp/<slug>` with `token`, a JSON-RPC `body`.
    pub async fn post(&self, slug: &str, token: &str, body: &serde_json::Value) -> reqwest::Response {
        self.client
            .post(self.url(slug))
            .bearer_auth(token)
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::ACCEPT,
                HeaderValue::from_static("application/json, text/event-stream"),
            )
            .body(body.to_string())
            .send()
            .await
            .unwrap()
    }
}

/// A `tools/call` of `name`.
pub fn call(id: i64, name: &str) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": {} } })
}

/// A `tools/list` request.
pub fn list(id: i64) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list" })
}

/// A `tools/list` answer naming `tools`.
pub fn listed(id: i64, tools: &[&str]) -> serde_json::Value {
    let tools: Vec<serde_json::Value> = tools
        .iter()
        .map(|name| serde_json::json!({ "name": name, "inputSchema": { "type": "object" } }))
        .collect();
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools } })
}
