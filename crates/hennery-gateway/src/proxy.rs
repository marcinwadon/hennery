//! The proxy (gateway spec §5): `POST|GET|DELETE /mcp/<slug>`, with the
//! client's token in `Authorization: Bearer`. Hand-written on axum and
//! reqwest, streaming, through the kernel's egress policy (kernel spec
//! §7.1). It changes nothing but the credential, `initialize`'s
//! capabilities and, under an allowlist, `tools/list` and `tools/call`.
//!
//! - **Outside the operator's routes** (lane L8, kernel spec §3.3: "Exempt"):
//!   no session cookie, no `Origin` rule, no compression layer.
//! - **404 for everything out of scope** (§3.1): no token, an unknown,
//!   revoked or superseded one, a connection that is not mounted on the
//!   session's host or is another hat's, or no such slug. The same body
//!   every time, and never 401: an agent answers a 401 by starting an OAuth
//!   flow of its own (G-17).
//! - **Headers** both ways are allowlists (§5.2). The client's
//!   `Authorization` is the gateway's own token: it never goes upstream.
//!   `Content-Type` is the gateway's both ways: a `POST` goes up as
//!   `application/json`, which it checked, and an answer comes down as the
//!   one type the gateway judged it by (the review's B1).
//! - **Streaming** (§5.3): a JSON answer is read whole (8 MiB at most) and
//!   judged before any of it goes on: a client can use none of it before its
//!   end, and a server request may be in it (plan 2026-10-15 "gateway JSON
//!   answers"). An event stream is passed on event by event: a complete
//!   event is never held, a partial one waits for its end (plan 8d decision
//!   5).
//! - **401** (§5.4) is never passed on: `502 upstream_auth`. The one place
//!   an OAuth refresh and retry goes is `refreshed` (plan 8f).
//! - **Limits** (§5.7): per connection, the requests in flight and the open
//!   `GET` streams; past either, 503. A request body has 30 s to arrive.
//! - **Errors** the proxy answers itself are `ApiError` (`{code, message}`),
//!   as every other route's: 404 `not_found`; 400 `invalid_request`; 408
//!   `request_timeout`; 413 `body_too_large`; 503 `busy`; 502
//!   `upstream_auth`, `upstream_unreachable`, `upstream_redirect`,
//!   `upstream_content_type`, `upstream_too_large`, `upstream_invalid`; 500
//!   `internal`. A refused `tools/call` is a JSON-RPC error, in a 200.
//! - **Logs** name the connection and its slug, never the token, the
//!   credential or more of the upstream URL than its origin (lane L11).

use crate::jsonrpc::{self, Answered, BOM, EventOutcome, Inspected};
use crate::key::MasterKey;
use crate::model::{CredKind, url_for_logs};
use crate::scope::{ClientIdentity, MountPolicy, Principal, ProxyStore, ScopedConnection};
use crate::store::GatewayStore;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use futures::{Stream, StreamExt};
use hennery_kernel::egress::{Allowance, Egress, EgressClient, Limiter, Permit};
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::ApiError;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// The largest request body (gateway spec §5.1).
pub const MAX_REQUEST_BODY: usize = 4 * 1024 * 1024;

/// The largest `tools/list` answer read whole to filter it, and the
/// largest single event of a stream (gateway spec §5.3).
pub const MAX_FILTERED_BODY: usize = 8 * 1024 * 1024;

/// What the proxy sends as `Accept` when the client sends none (§5.2).
const DEFAULT_ACCEPT: &str = "application/json, text/event-stream";

/// The request headers passed upstream as they came (gateway spec §5.2).
/// `Content-Type` is the gateway's own (the review's O6).
const FORWARDED_REQUEST_HEADERS: &[&str] = &["accept", "mcp-session-id", "mcp-protocol-version", "last-event-id"];

/// The response headers passed downstream as they came (gateway spec
/// §5.2). `Content-Type` is set from what the gateway judged the body to be
/// (the review's B1), and `Cache-Control` is always `no-store` (plan 8d
/// decision 19).
const FORWARDED_RESPONSE_HEADERS: &[&str] = &["mcp-session-id"];

/// The proxy's limits (gateway spec §5.7; plan 8d decision 10).
#[derive(Clone, Debug)]
pub struct Limits {
    /// Per connection: `POST` and `DELETE` requests in flight, from before
    /// their body is read until their answer's body ends.
    pub requests: Limiter,
    /// Per connection: open `GET` streams, the server-to-client channel,
    /// for their whole life. A `GET` takes no request permit.
    pub streams: Limiter,
    /// How long an upstream may take to send its response head.
    pub head_timeout: Duration,
    /// How long a client may take to send its request body (the review's
    /// O3): a slow body holds a request permit.
    pub body_timeout: Duration,
}

impl Limits {
    pub const MAX_REQUESTS: usize = 64;
    pub const MAX_STREAMS: usize = 32;
    pub const HEAD_TIMEOUT: Duration = Duration::from_secs(300);
    pub const BODY_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn new(max_requests: usize, max_streams: usize, head_timeout: Duration, body_timeout: Duration) -> Self {
        Self {
            requests: Limiter::new(max_requests),
            streams: Limiter::new(max_streams),
            head_timeout,
            body_timeout,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(
            Self::MAX_REQUESTS,
            Self::MAX_STREAMS,
            Self::HEAD_TIMEOUT,
            Self::BODY_TIMEOUT,
        )
    }
}

/// What the proxy runs on.
#[derive(Clone)]
pub struct ProxyState {
    /// Token → principal.
    pub identity: Arc<dyn ClientIdentity>,
    /// Principal → connections.
    pub mounts: Arc<dyn MountPolicy>,
    /// Static credentials, with where they go (`static_credential`).
    pub credentials: Arc<GatewayStore>,
    /// What live traffic says of a connection's status.
    pub statuses: Arc<ProxyStore>,
    pub key: Arc<MasterKey>,
    pub egress: Egress,
    pub limits: Limits,
}

impl ProxyState {
    /// Full mode (umbrella §10.2): session tokens and host mounts, both from
    /// `store`.
    pub fn full(
        store: Arc<ProxyStore>,
        credentials: Arc<GatewayStore>,
        key: Arc<MasterKey>,
        egress: Egress,
        limits: Limits,
    ) -> Self {
        Self {
            identity: store.clone(),
            mounts: store.clone(),
            credentials,
            statuses: store,
            key,
            egress,
            limits,
        }
    }
}

/// The proxy's route. Merge it beside the operator's routes, never under
/// `operator_only` (lane L8). Every answer, the proxy's own errors too,
/// carries `X-Content-Type-Options: nosniff` and `Cache-Control: no-store`.
/// Any other method (`HEAD` too, which axum would hand to the `GET`
/// handler), and anything deeper under a slug, a bare trailing slash
/// included, is the same 404, never another router's (the review's O7; the
/// Task 2 review's findings 1 and 2).
pub fn router(state: ProxyState) -> Router {
    Router::new()
        .route(
            "/mcp/{slug}",
            post(proxy)
                .get(proxy)
                .delete(proxy)
                .head(async || not_found())
                .fallback(async || not_found()),
        )
        .route("/mcp/{slug}/", any(async || not_found()))
        .route("/mcp/{slug}/{*rest}", any(async || not_found()))
        .layer(middleware::map_response(nosniff))
        .layer(middleware::map_response(no_store))
        .with_state(state)
}

async fn nosniff(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

/// An answer to a request that carries a token is no shared cache's to
/// keep, whatever the upstream says (plan 8d decision 19).
async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The proxy's own answer: an `ApiError`, as every route's.
fn refuse(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        axum::Json(ApiError {
            code: code.into(),
            message: message.into(),
            session_id: None,
        }),
    )
        .into_response()
}

/// The one answer for everything out of scope.
fn not_found() -> Response {
    refuse(StatusCode::NOT_FOUND, "not_found", "no such MCP server")
}

fn internal(err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "gateway proxy: internal error");
    refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

fn upstream_auth(connection: &ScopedConnection) -> Response {
    refuse(
        StatusCode::BAD_GATEWAY,
        "upstream_auth",
        format!("connection {} needs re-authorization in hennery", connection.label),
    )
}

/// The bearer token of `Authorization`, if there is exactly one.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// How the request authenticates upstream. Its `Debug` shows no secret.
enum UpstreamAuth {
    /// `cred_kind = none`: no credential header.
    None,
    /// `cred_kind = static`: `<header>: <prefix><token>`.
    Static { header: HeaderName, value: HeaderValue },
}

impl std::fmt::Debug for UpstreamAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Static { header, .. } => write!(f, "Static({header}: <redacted>)"),
        }
    }
}

/// Where one request goes and how: read with the credential itself, so an
/// edit cannot move the URL between them (plan 8a's R2).
struct Upstream {
    url: Url,
    /// The connection's stored flag, as read for this request.
    internal_network: bool,
    auth: UpstreamAuth,
}

/// The egress client a connection's requests go through: chosen at every
/// request from the connection's stored `internal_network` flag, never from
/// anything in the request (plan 8d decision 9). The one place the choice
/// is made, so a later client for the internal network swaps in here.
fn egress_client(egress: &Egress, internal_network: bool) -> EgressClient {
    egress.client(if internal_network {
        Allowance::InternalNetwork
    } else {
        Allowance::PublicOnly
    })
}

/// The connection's upstream and credential now. `None`: it has none it
/// can use (a `static` connection without a token, or a kind the proxy
/// does not take yet), so nothing is sent (plan 8d decision 3).
fn upstream(state: &ProxyState, connection: &ScopedConnection) -> anyhow::Result<Option<Upstream>> {
    match connection.cred_kind {
        CredKind::None => Ok(Some(Upstream {
            url: Url::parse(&connection.url)?,
            internal_network: connection.internal_network,
            auth: UpstreamAuth::None,
        })),
        CredKind::Static => {
            let Some(credential) = state.credentials.static_credential(&connection.id, &state.key)? else {
                return Ok(None);
            };
            let header = HeaderName::from_bytes(credential.static_header.as_bytes())?;
            let mut value =
                HeaderValue::from_str(&format!("{}{}", credential.static_prefix, credential.token.as_str()))?;
            value.set_sensitive(true);
            Ok(Some(Upstream {
                url: Url::parse(&credential.url)?,
                internal_network: credential.internal_network,
                auth: UpstreamAuth::Static { header, value },
            }))
        }
        // Plan 8f.
        CredKind::OauthDcr | CredKind::OauthClient => Ok(None),
    }
}

/// The seam for plan 8f: after an upstream 401, a fresh credential to try
/// once more, single-flight per connection. Static and `none` credentials
/// have nothing to refresh.
async fn refreshed(_state: &ProxyState, _connection: &ScopedConnection, auth: &UpstreamAuth) -> Option<UpstreamAuth> {
    match auth {
        UpstreamAuth::None | UpstreamAuth::Static { .. } => None,
    }
}

/// The upstream request's headers: the allowlist, `Accept` defaulted, no
/// compression, `Content-Type: application/json` with a body (the gateway
/// checked it is JSON), and the credential, never the client's
/// `Authorization`.
fn upstream_headers(downstream: &HeaderMap, auth: &UpstreamAuth, json_body: bool) -> HeaderMap {
    let mut out = HeaderMap::new();
    if json_body {
        out.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    for name in FORWARDED_REQUEST_HEADERS {
        for value in downstream.get_all(*name) {
            out.append(HeaderName::from_static(name), value.clone());
        }
    }
    if !out.contains_key(header::ACCEPT) {
        out.insert(header::ACCEPT, HeaderValue::from_static(DEFAULT_ACCEPT));
    }
    out.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    if let UpstreamAuth::Static { header, value } = auth {
        out.insert(header.clone(), value.clone());
    }
    out
}

/// Read `body` whole, refusing past `cap` bytes.
async fn read_capped<S, E>(mut stream: S, cap: usize) -> Result<Vec<u8>, ReadError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ReadError::Failed)?;
        if out.len() + chunk.len() > cap {
            return Err(ReadError::TooLarge);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

enum ReadError {
    TooLarge,
    Failed,
}

/// `POST|GET|DELETE /mcp/{slug}`.
async fn proxy(
    State(state): State<ProxyState>,
    slug: Result<Path<String>, PathRejection>,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let now = unix_now();
    // A slug that is not UTF-8 is no slug: the same 404 (the whole-branch
    // review), not axum's plain-text 400.
    let Ok(Path(slug)) = slug else {
        return not_found();
    };
    let Some(token) = bearer(&headers) else {
        return not_found();
    };
    let principal = match state.identity.resolve(token, now) {
        Ok(Some(principal)) => principal,
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    };
    let connection = match state.mounts.connection(&principal, &slug) {
        Ok(Some(connection)) => connection,
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    };
    let permit = if method == Method::GET {
        state.limits.streams.try_acquire(&connection.id)
    } else {
        state.limits.requests.try_acquire(&connection.id)
    };
    let Ok(permit) = permit else {
        tracing::warn!(connection_id = %connection.id, slug = %connection.slug, %method, "gateway proxy: too many requests or streams");
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "too many requests or streams to this MCP server",
        );
    };
    let body = if method == Method::POST {
        let declared = headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if declared.is_some_and(|len| len > MAX_REQUEST_BODY as u64) {
            return body_too_large();
        }
        let read = read_capped(body.into_data_stream(), MAX_REQUEST_BODY);
        let bytes = match tokio::time::timeout(state.limits.body_timeout, read).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(ReadError::TooLarge)) => return body_too_large(),
            Ok(Err(ReadError::Failed)) => {
                return refuse(StatusCode::BAD_REQUEST, "invalid_request", "the body was not read");
            }
            Err(_) => {
                return refuse(
                    StatusCode::REQUEST_TIMEOUT,
                    "request_timeout",
                    "the request body did not arrive in time",
                );
            }
        };
        match jsonrpc::inspect_request(&bytes, connection.tool_allowlist.as_deref()) {
            Inspected::Forward(body) => Some(body),
            Inspected::Answer(Some(answer)) => {
                tracing::info!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: a tools/call outside the allowlist refused");
                return axum::Json(answer).into_response();
            }
            Inspected::Answer(None) => return StatusCode::ACCEPTED.into_response(),
            Inspected::Invalid(why) => return refuse(StatusCode::BAD_REQUEST, "invalid_request", why),
        }
    } else {
        None
    };
    let upstream = match upstream(&state, &connection) {
        Ok(Some(upstream)) => upstream,
        Ok(None) => {
            tracing::info!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: the connection has no credential to send");
            return upstream_auth(&connection);
        }
        Err(err) => return internal(err),
    };
    let client = egress_client(&state.egress, upstream.internal_network);
    let send = |auth: &UpstreamAuth| {
        let mut request = reqwest::Request::new(method.clone(), upstream.url.clone());
        *request.headers_mut() = upstream_headers(&headers, auth, body.is_some());
        *request.body_mut() = body.clone().map(reqwest::Body::from);
        *request.timeout_mut() = Some(state.limits.head_timeout);
        client.send_streaming(request)
    };
    let mut response = send(&upstream.auth).await;
    if matches!(&response, Ok(r) if r.status() == StatusCode::UNAUTHORIZED)
        && let Some(fresh) = refreshed(&state, &connection, &upstream.auth).await
    {
        response = send(&fresh).await;
    }
    let response = match response {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(
                connection_id = %connection.id,
                slug = %connection.slug,
                upstream = %url_for_logs(upstream.url.as_str()),
                error = %err,
                "gateway proxy: the upstream was not reached"
            );
            return refuse(
                StatusCode::BAD_GATEWAY,
                "upstream_unreachable",
                format!("connection {} could not be reached", connection.label),
            );
        }
    };
    let status = response.status();
    tracing::debug!(connection_id = %connection.id, slug = %connection.slug, %method, status = status.as_u16(), "gateway proxy: upstream answered");
    if status == StatusCode::UNAUTHORIZED {
        drop(response);
        tracing::warn!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: the upstream refused the credential");
        if let Err(err) = state.statuses.mark_needs_auth(
            &connection.id,
            upstream.url.as_str(),
            "the upstream refused the credential (401)",
            now,
        ) {
            tracing::error!(connection_id = %connection.id, error = %err, "gateway proxy: status not recorded");
        }
        return upstream_auth(&connection);
    }
    if status.is_redirection() {
        tracing::warn!(connection_id = %connection.id, slug = %connection.slug, status = status.as_u16(), "gateway proxy: the upstream redirected, not followed");
        return refuse(
            StatusCode::BAD_GATEWAY,
            "upstream_redirect",
            format!("connection {} answered with a redirect", connection.label),
        );
    }
    let kind = match body_kind(&response) {
        Ok(kind) => kind,
        Err(why) => {
            tracing::warn!(connection_id = %connection.id, slug = %connection.slug, status = status.as_u16(), why, "gateway proxy: the upstream's answer is not passed on");
            return refuse(
                StatusCode::BAD_GATEWAY,
                "upstream_content_type",
                format!("connection {} answered with {why}", connection.label),
            );
        }
    };
    // A body-less 2xx proves nothing: some upstreams take a notification
    // before they check its credential (the review's O5). An empty body
    // under a type is no more (the Task 2 review).
    if status.is_success()
        && !matches!(kind, BodyKind::Empty)
        && response.content_length() != Some(0)
        && let Err(err) = state
            .statuses
            .record_traffic_ok(&connection.id, upstream.url.as_str(), now)
    {
        tracing::error!(connection_id = %connection.id, error = %err, "gateway proxy: status not recorded");
    }
    let mut out = HeaderMap::new();
    for name in FORWARDED_RESPONSE_HEADERS {
        for value in response.headers().get_all(*name) {
            out.append(HeaderName::from_static(name), value.clone());
        }
    }
    match kind {
        BodyKind::Empty => {}
        BodyKind::Json => {
            out.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        BodyKind::EventStream => {
            out.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        }
    }
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .or_else(|| headers.get("mcp-session-id"))
        .cloned();
    let permits = Permits(permit);
    let allowlist = connection.tool_allowlist.clone();
    let body = match kind {
        BodyKind::Empty => {
            drop(permits);
            Body::empty()
        }
        BodyKind::Json => {
            let read = read_capped(response.bytes_stream(), MAX_FILTERED_BODY).await;
            drop(permits);
            let bytes = match read {
                Ok(bytes) => bytes,
                Err(ReadError::TooLarge) => {
                    return refuse(
                        StatusCode::BAD_GATEWAY,
                        "upstream_too_large",
                        format!("connection {} answered with more than 8 MiB", connection.label),
                    );
                }
                Err(ReadError::Failed) => {
                    tracing::debug!(connection_id = %connection.id, "gateway proxy: the upstream body failed");
                    return refuse(
                        StatusCode::BAD_GATEWAY,
                        "upstream_unreachable",
                        format!("connection {} stopped answering", connection.label),
                    );
                }
            };
            // Read as an event's data is (plan 2026-10-15 "gateway
            // differential"), and judged as one: no server request the
            // gateway refuses, every tools list filtered ("gateway JSON
            // answers").
            match jsonrpc::inspect_answer(&bytes, allowlist.as_deref()) {
                Answered::Unchanged => Body::from(bytes),
                Answered::Rewritten(bytes) => Body::from(bytes),
                Answered::Unreadable => {
                    tracing::warn!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: a JSON answer the gateway cannot read refused");
                    return refuse(
                        StatusCode::BAD_GATEWAY,
                        "upstream_invalid",
                        format!(
                            "connection {} answered with JSON the gateway cannot read",
                            connection.label
                        ),
                    );
                }
                Answered::ServerRequest => {
                    tracing::warn!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: a JSON answer holding a server request refused");
                    return refuse(
                        StatusCode::BAD_GATEWAY,
                        "upstream_invalid",
                        format!(
                            "connection {} answered with a server request inside a JSON answer",
                            connection.label
                        ),
                    );
                }
            }
        }
        BodyKind::EventStream => {
            let answerer = Answerer {
                state: state.clone(),
                principal,
                slug: slug.clone(),
                url: upstream.url.clone(),
                internal_network: upstream.internal_network,
                session_id,
                protocol_version: headers.get("mcp-protocol-version").cloned(),
                connection_id: connection.id.clone(),
            };
            Body::from_stream(events(response, permits, allowlist, answerer))
        }
    };
    let mut answer = Response::new(body);
    *answer.status_mut() = status;
    *answer.headers_mut() = out;
    answer
}

fn body_too_large() -> Response {
    refuse(
        StatusCode::PAYLOAD_TOO_LARGE,
        "body_too_large",
        "a request body is at most 4 MiB",
    )
}

/// The permit a response holds until its body ends or is dropped.
struct Permits(#[allow(dead_code)] Permit);

/// What an upstream answered with, as the proxy passes it on.
enum BodyKind {
    /// No body at all.
    Empty,
    Json,
    EventStream,
}

/// Only JSON and event streams pass, uncompressed (gateway spec §5.2); a
/// body-less answer (202, 204, or `Content-Length: 0`) has no type to
/// judge (plan 8d decision 4). One `Content-Type` at most: a client could
/// read two otherwise than the gateway (the review's B1).
fn body_kind(response: &reqwest::Response) -> Result<BodyKind, &'static str> {
    let headers = response.headers();
    if headers.get_all(header::CONTENT_TYPE).iter().count() > 1 {
        return Err("more than one content type");
    }
    if headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .any(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err("a compressed body");
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or_default().trim().to_ascii_lowercase());
    let bodiless = matches!(response.status(), StatusCode::ACCEPTED | StatusCode::NO_CONTENT)
        || response.content_length() == Some(0);
    match content_type.as_deref() {
        Some("application/json") => Ok(BodyKind::Json),
        Some("text/event-stream") => Ok(BodyKind::EventStream),
        None if bodiless => Ok(BodyKind::Empty),
        None => Err("a body of no content type"),
        Some(_) => Err("a content type other than JSON or an event stream"),
    }
}

/// Answers server-to-client requests the gateway refuses (gateway spec
/// §5.6), on the stream's upstream session. It keeps no client and no
/// credential: each answer reads the connection again, as a request does
/// (plan 8b-ii's obligation: nothing cached across a `PATCH`).
struct Answerer {
    state: ProxyState,
    principal: Principal,
    slug: String,
    /// Where the stream's request went, and under which allowance.
    url: Url,
    internal_network: bool,
    session_id: Option<HeaderValue>,
    protocol_version: Option<HeaderValue>,
    connection_id: String,
}

impl Answerer {
    /// Post `answer` upstream, in the background. Skipped if the
    /// connection is at its request cap, or is no longer the one the
    /// stream came from: out of the principal's scope, at another URL or
    /// allowance, or without its credential.
    fn send(&self, answer: &Value) {
        let Ok(permit) = self.state.limits.requests.try_acquire(&self.connection_id) else {
            tracing::warn!(connection_id = %self.connection_id, "gateway proxy: a refused server request left unanswered, busy");
            return;
        };
        let current = match self.state.mounts.connection(&self.principal, &self.slug) {
            Ok(Some(connection)) if connection.id == self.connection_id => upstream(&self.state, &connection),
            Ok(_) => Ok(None),
            Err(err) => Err(err),
        };
        let auth = match current {
            Ok(Some(now)) if now.url == self.url && now.internal_network == self.internal_network => now.auth,
            Ok(_) => {
                tracing::warn!(connection_id = %self.connection_id, "gateway proxy: a refused server request left unanswered, the connection changed");
                return;
            }
            Err(err) => {
                tracing::error!(connection_id = %self.connection_id, error = %err, "gateway proxy: a refused server request left unanswered");
                return;
            }
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(header::ACCEPT, HeaderValue::from_static(DEFAULT_ACCEPT));
        headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        if let Some(id) = &self.session_id {
            headers.insert("mcp-session-id", id.clone());
        }
        if let Some(version) = &self.protocol_version {
            headers.insert("mcp-protocol-version", version.clone());
        }
        if let UpstreamAuth::Static { header, value } = &auth {
            headers.insert(header.clone(), value.clone());
        }
        let mut request = reqwest::Request::new(Method::POST, self.url.clone());
        *request.headers_mut() = headers;
        *request.body_mut() = Some(serde_json::to_vec(answer).expect("a JSON value serialises").into());
        let client = egress_client(&self.state.egress, self.internal_network);
        let connection_id = self.connection_id.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match client.send(request).await {
                Ok(response) => {
                    tracing::debug!(connection_id = %connection_id, status = response.status().as_u16(), "gateway proxy: a refused server request answered")
                }
                Err(err) => {
                    tracing::debug!(connection_id = %connection_id, error = %err, "gateway proxy: a refused server request's answer failed")
                }
            }
        });
    }
}

/// The upstream's event stream, event by event: each complete event goes
/// on at once, rewritten or dropped where a rule says so; a partial one
/// waits for its end. An event over 8 MiB, or a failure, ends the stream.
/// Fail closed (the review's B2): a byte-order mark at the start is
/// dropped, as a client's parser would; an event whose data is not JSON,
/// has a key twice, or has a line that starts with a mark, is dropped, since the gateway
/// cannot read what a client might; and a last event the stream never ends
/// is dropped, as a client drops it.
fn events(
    response: reqwest::Response,
    permits: Permits,
    allowlist: Option<Vec<String>>,
    answerer: Answerer,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    struct State<S> {
        upstream: S,
        pending: Vec<u8>,
        /// Where in `pending` to go on looking for an event's end: the
        /// start of its last line seen (the review's O2).
        resume: usize,
        /// Whether a byte-order mark at the start was looked for.
        started: bool,
        done: bool,
        _permits: Permits,
        allowlist: Option<Vec<String>>,
        answerer: Answerer,
    }
    let state = State {
        upstream: response.bytes_stream(),
        pending: Vec::new(),
        resume: 0,
        started: false,
        done: false,
        _permits: permits,
        allowlist,
        answerer,
    };
    futures::stream::unfold(state, |mut state| async move {
        loop {
            if state.done {
                return None;
            }
            match state.upstream.next().await {
                None => {
                    state.done = true;
                    if !state.pending.is_empty() {
                        tracing::debug!(connection_id = %state.answerer.connection_id, "gateway proxy: an unended last event dropped");
                    }
                    return None;
                }
                Some(Err(err)) => {
                    state.done = true;
                    let err = err.without_url();
                    tracing::debug!(connection_id = %state.answerer.connection_id, error = %err, "gateway proxy: the upstream stream failed");
                    return Some((Err(std::io::Error::other(err)), state));
                }
                Some(Ok(chunk)) => {
                    state.pending.extend_from_slice(&chunk);
                    if !state.started {
                        if BOM.starts_with(&state.pending) {
                            // A mark still arriving, or none yet.
                            continue;
                        }
                        state.started = true;
                        if state.pending.starts_with(BOM) {
                            state.pending.drain(..BOM.len());
                        }
                    }
                    let mut out = Vec::new();
                    let mut answers = Vec::new();
                    let mut start = 0;
                    loop {
                        let end = match jsonrpc::event_end(&state.pending[start..], state.resume) {
                            Ok(end) => end,
                            Err(resume) => {
                                state.resume = resume;
                                break;
                            }
                        };
                        state.resume = 0;
                        let event = &state.pending[start..start + end];
                        match jsonrpc::rewrite_event(event, state.allowlist.as_deref(), &mut answers) {
                            EventOutcome::Unchanged => out.extend_from_slice(event),
                            EventOutcome::Rewritten(bytes) => out.extend_from_slice(&bytes),
                            EventOutcome::Dropped => {}
                            EventOutcome::Unreadable => {
                                tracing::warn!(connection_id = %state.answerer.connection_id, "gateway proxy: an event the gateway cannot read dropped");
                            }
                        }
                        start += end;
                    }
                    state.pending.drain(..start);
                    for answer in &answers {
                        tracing::info!(connection_id = %state.answerer.connection_id, "gateway proxy: a server request for a capability not forwarded refused");
                        state.answerer.send(answer);
                    }
                    if state.pending.len() > MAX_FILTERED_BODY {
                        state.done = true;
                        tracing::warn!(connection_id = %state.answerer.connection_id, "gateway proxy: an event over 8 MiB, the stream ended");
                        return Some((Err(std::io::Error::other("an event over 8 MiB")), state));
                    }
                    if !out.is_empty() {
                        return Some((Ok(Bytes::from(out)), state));
                    }
                }
            }
        }
    })
}
