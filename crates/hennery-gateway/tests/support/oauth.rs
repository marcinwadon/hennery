//! A fake OAuth authorization server with an MCP endpoint behind it (gateway
//! spec §11, plan 8f), on loopback: the protected-resource document where a
//! test puts it, the authorization server's metadata in one of its forms,
//! dynamic registration, consent, and a token endpoint that checks PKCE,
//! `resource`, the client and rotating refresh tokens with reuse detection.
//! Connections reach it under `internal_network`, as every test connection
//! must (lane L7).

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Where the protected-resource document is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrAt {
    /// Only at the URL a 401's `WWW-Authenticate` names.
    Challenge,
    /// Only at `/.well-known/oauth-protected-resource/mcp`.
    PathInserted,
    /// Only at `/.well-known/oauth-protected-resource`.
    Origin,
    /// Nowhere: the resource's origin is the authorization server.
    Nowhere,
}

/// Where the authorization server's metadata is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaAt {
    /// `/.well-known/oauth-authorization-server<path>` (RFC 8414).
    Inserted,
    /// `<path>/.well-known/oauth-authorization-server`.
    Appended,
    /// `<path>/.well-known/openid-configuration`.
    Openid,
}

/// What the fake does; a test changes it before and between requests.
#[derive(Debug, Clone)]
pub struct Config {
    pub pr_at: PrAt,
    /// The PR document's `resource`; `None`: the MCP URL.
    pub resource: Option<String>,
    /// The PR document as raw text, replacing the built one.
    pub raw_pr: Option<String>,
    /// The authorization server's path: `""` or `/tenant`.
    pub as_path: String,
    pub meta_at: MetaAt,
    /// The metadata as raw text, replacing the built one.
    pub raw_meta: Option<String>,
    /// An `issuer` other than the one the metadata is fetched for.
    pub issuer: Option<String>,
    /// Replaces the advertised endpoints.
    pub token_endpoint: Option<String>,
    pub authorization_endpoint: Option<String>,
    pub s256: bool,
    pub registration: bool,
    /// A registration refusal: its status and body.
    pub dcr_refusal: Option<(u16, Value)>,
    pub iss_parameter: bool,
    /// What `iss` the consent's redirect carries: `None` the issuer.
    pub iss_sent: Option<Option<String>>,
    /// A confidential client the token endpoint takes by Basic auth.
    pub confidential: Option<(String, String)>,
    pub auth_methods: Vec<String>,
    /// The token endpoint refuses `resource` (`invalid_target`).
    pub refuse_resource_at_token: bool,
    /// Consent refuses `resource` (`error=invalid_target`).
    pub refuse_resource_at_consent: bool,
    /// Consent is refused (`error=access_denied`) with this description.
    pub deny: Option<String>,
    /// A refresh issues no new refresh token.
    pub refresh_keeps_token: bool,
    pub expires_in: Option<i64>,
    /// Token responses wait until the gate opens.
    pub gate_token: bool,
    /// The token endpoint's refusal body (a canary for logs).
    pub refusal_body: Option<Value>,
    /// The scopes the PR document lists.
    pub scopes: Vec<String>,
    /// The MCP endpoint answers every authenticated request with this
    /// status instead.
    pub mcp_status: Option<u16>,
    /// The MCP endpoint never answers an authenticated request.
    pub mcp_hang: bool,
    /// Whether any authorization-server metadata is served.
    pub serve_metadata: bool,
    /// The path-inserted PR URL answers a redirect to `/elsewhere`, which
    /// serves a hostile document (never to be followed).
    pub pr_redirect: bool,
    /// The path-inserted PR URL answers this status.
    pub pr_status: Option<u16>,
    /// The token endpoint's success body as raw text.
    pub raw_token: Option<String>,
    /// What every code and token starts with (a canary for logs).
    pub prefix: String,
    /// Every metadata URL answers this status.
    pub meta_status: Option<u16>,
    /// The token endpoint answers this status, empty.
    pub token_status: Option<u16>,
    /// A registration issues a client secret too (Basic at the token
    /// endpoint).
    pub dcr_secret: bool,
    /// A registration answers 201 without a `client_id`.
    pub dcr_without_client_id: bool,
    /// The `resource_metadata` a 401's challenge names, instead of the
    /// fake's own.
    pub challenge_url: Option<String>,
    /// The MCP endpoint is stateful: a request other than `initialize`
    /// without the session's `Mcp-Session-Id`, or without the protocol
    /// version `initialize` answered, is 400 (G-21).
    pub stateful: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pr_at: PrAt::PathInserted,
            resource: None,
            raw_pr: None,
            as_path: String::new(),
            meta_at: MetaAt::Inserted,
            raw_meta: None,
            issuer: None,
            token_endpoint: None,
            authorization_endpoint: None,
            s256: true,
            registration: true,
            dcr_refusal: None,
            iss_parameter: false,
            iss_sent: None,
            confidential: None,
            auth_methods: Vec::new(),
            refuse_resource_at_token: false,
            refuse_resource_at_consent: false,
            deny: None,
            refresh_keeps_token: false,
            expires_in: Some(3600),
            gate_token: false,
            refusal_body: None,
            scopes: vec!["read".into(), "write".into()],
            mcp_status: None,
            mcp_hang: false,
            serve_metadata: true,
            pr_redirect: false,
            pr_status: None,
            raw_token: None,
            prefix: String::new(),
            meta_status: None,
            token_status: None,
            dcr_secret: false,
            dcr_without_client_id: false,
            challenge_url: None,
            stateful: false,
        }
    }
}

/// A code waiting for its exchange.
#[derive(Debug, Clone)]
struct Code {
    client_id: String,
    challenge: String,
    redirect_uri: String,
    resource: Option<String>,
}

/// A refresh token, and whether it was used (rotation: once).
#[derive(Debug, Clone)]
struct Refresh {
    family: u32,
    used: bool,
}

#[derive(Default)]
pub struct Recorded {
    /// Every request's method and path (with its query).
    pub requests: Vec<(String, String)>,
    pub registered: Vec<String>,
    /// The `resource` each consent, exchange and refresh carried.
    pub consent_resources: Vec<Option<String>>,
    pub token_resources: Vec<(String, Option<String>)>,
    /// The `scope` each refresh carried.
    pub refresh_scopes: Vec<Option<String>>,
    pub refreshes: usize,
    pub exchanges: usize,
    /// Reuse of a spent refresh token was seen.
    pub reuse_detected: bool,
    /// Basic auth the token endpoint received.
    pub basic: Vec<String>,
    /// The PKCE verifiers exchanges carried.
    pub verifiers: Vec<String>,
    /// Every code, access and refresh token issued.
    pub issued: Vec<String>,
    /// A client secret came in the form (`client_secret_post`).
    pub posted_secret: bool,
    /// Every bearer token the MCP endpoint was sent, in order.
    pub bearers: Vec<String>,
}

pub struct Inner {
    pub config: Config,
    pub recorded: Recorded,
    codes: HashMap<String, Code>,
    access: HashMap<String, u32>,
    refresh: HashMap<String, Refresh>,
    revoked: Vec<u32>,
    next: u32,
    clients: Vec<String>,
    /// Registered confidential clients: (id, secret).
    secrets: Vec<(String, String)>,
}

#[derive(Clone)]
struct Shared {
    inner: Arc<Mutex<Inner>>,
    origin: String,
    gate: watch::Receiver<bool>,
}

pub struct FakeAs {
    pub addr: SocketAddr,
    inner: Arc<Mutex<Inner>>,
    gate: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeAs {
    fn drop(&mut self) {
        let _ = self.gate.send(true);
        self.task.abort();
    }
}

impl FakeAs {
    pub async fn start(config: Config) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let inner = Arc::new(Mutex::new(Inner {
            config,
            recorded: Recorded::default(),
            codes: HashMap::new(),
            access: HashMap::new(),
            refresh: HashMap::new(),
            revoked: Vec::new(),
            next: 0,
            clients: Vec::new(),
            secrets: Vec::new(),
        }));
        let (gate, open) = watch::channel(false);
        let shared = Shared {
            inner: inner.clone(),
            origin: format!("http://{addr}"),
            gate: open,
        };
        let app = Router::new().fallback(handle).with_state(shared);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            inner,
            gate,
            task,
        }
    }

    pub fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The MCP endpoint's URL: the connection's.
    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.origin())
    }

    pub fn issuer(&self) -> String {
        let path = self.inner.lock().unwrap().config.as_path.clone();
        format!("{}{path}", self.origin())
    }

    pub fn configure(&self, change: impl FnOnce(&mut Config)) {
        change(&mut self.inner.lock().unwrap().config);
    }

    pub fn with<T>(&self, read: impl FnOnce(&Recorded) -> T) -> T {
        read(&self.inner.lock().unwrap().recorded)
    }

    /// The paths requested, in order.
    pub fn paths(&self) -> Vec<String> {
        self.with(|r| r.requests.iter().map(|(_, p)| p.clone()).collect())
    }

    /// Let gated token responses go.
    pub fn open_gate(&self) {
        let _ = self.gate.send(true);
    }

    /// A grant issued directly, as if consented: its access and refresh
    /// tokens, valid at the token endpoint and the MCP endpoint.
    pub fn issue(&self) -> (String, String) {
        let mut inner = self.inner.lock().unwrap();
        inner.next += 1;
        let family = inner.next;
        let prefix = inner.config.prefix.clone();
        let access = format!("{prefix}access-{family}-0");
        let refresh = format!("{prefix}refresh-{family}-0");
        inner.access.insert(access.clone(), family);
        inner.refresh.insert(refresh.clone(), Refresh { family, used: false });
        (access, refresh)
    }

    /// Invalidate every access token (they "expired"); refresh tokens stay.
    pub fn expire_access(&self) {
        self.inner.lock().unwrap().access.clear();
    }

    /// Whether `token` is a live access token.
    pub fn is_live(&self, token: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.access.get(token).is_some_and(|f| !inner.revoked.contains(f))
    }

    /// Consent as a browser would: GET the consent URL and return the
    /// redirect's `Location`, which goes back to hennery's callback.
    pub async fn consent(&self, consent_url: &str) -> String {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = client.get(consent_url).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::FOUND, "consent did not redirect");
        response.headers()[header::LOCATION].to_str().unwrap().to_string()
    }
}

/// Store a grant for `id` as a completed Connect would, without the
/// consent: `access` and `refresh` from `fake.issue()`, the fake's token
/// endpoint, the MCP URL as its `resource`, `read write` as its scopes.
pub fn seed_grant(
    world: &super::World,
    id: &str,
    fake: &FakeAs,
    access: &str,
    refresh: Option<&str>,
    expires_at: Option<i64>,
) {
    use hennery_gateway::model::{AuthMethod, GrantTokens, TokenClient};
    use hennery_gateway::store::{ClientSource, GrantStored, GrantToStore};
    let record = world.store.connection(id).unwrap().unwrap();
    let client = TokenClient {
        client_id: "pre-seeded".into(),
        secret: None,
        auth_method: AuthMethod::None,
        token_endpoint: format!("{}/token", fake.origin()),
    };
    let tokens = GrantTokens {
        access_token: zeroize::Zeroizing::new(access.to_string()),
        refresh_token: refresh.map(|r| zeroize::Zeroizing::new(r.to_string())),
    };
    let scopes = vec!["read".to_string(), "write".to_string()];
    let stored = world
        .store
        .store_grant(
            &GrantToStore {
                connection_id: id,
                url: &record.url,
                cred_kind: record.cred_kind,
                internal_network: record.internal_network,
                source: ClientSource::Registered,
                client: &client,
                issuer: &fake.issuer(),
                authorization_endpoint: &format!("{}/authorize", fake.origin()),
                redirect_uri: "https://hennery.example/api/mcp/oauth/callback",
                scopes: &scopes,
                resource: &record.url,
                resource_param_accepted: true,
                registered_at: 1,
                tokens: &tokens,
                expires_at,
            },
            &world.key,
            hennery_kernel::secret::unix_now(),
        )
        .unwrap();
    assert!(matches!(stored, GrantStored::Stored(_)), "{stored:?}");
}

fn json_response(status: u16, value: &Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

fn raw_json(text: &str) -> Response {
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(text.to_string()))
        .unwrap()
}

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

fn form(body: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

fn query(uri: &axum::http::Uri) -> HashMap<String, String> {
    url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
        .into_owned()
        .collect()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(String::from)
}

async fn handle(State(shared): State<Shared>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    let path = parts.uri.path().to_string();
    let path_and_query = parts.uri.to_string();
    let method = parts.method.to_string();
    let origin = shared.origin.clone();
    let config = {
        let mut inner = shared.inner.lock().unwrap();
        inner.recorded.requests.push((method.clone(), path_and_query));
        inner.config.clone()
    };
    let issuer = config
        .issuer
        .clone()
        .unwrap_or_else(|| format!("{origin}{}", config.as_path));
    let mcp_url = format!("{origin}/mcp");
    let pr = || match &config.raw_pr {
        Some(raw) => raw_json(raw),
        None => json_response(
            200,
            &json!({
                "resource": config.resource.clone().unwrap_or_else(|| mcp_url.clone()),
                "authorization_servers": [format!("{origin}{}", config.as_path)],
                "scopes_supported": config.scopes,
            }),
        ),
    };
    let meta_path = match config.meta_at {
        MetaAt::Inserted => format!("/.well-known/oauth-authorization-server{}", config.as_path),
        MetaAt::Appended => format!("{}/.well-known/oauth-authorization-server", config.as_path),
        MetaAt::Openid => format!("{}/.well-known/openid-configuration", config.as_path),
    };
    match (method.as_str(), path.as_str()) {
        (_, p) if p == "/mcp" || (p.ends_with("/mcp") && !p.starts_with("/.well-known")) => {
            if config.mcp_hang && bearer(&parts.headers).is_some() {
                std::future::pending::<()>().await;
            }
            mcp(&shared, &config, &parts.headers, &method, &body, &origin)
        }
        ("GET", "/prm-from-challenge") if config.pr_at == PrAt::Challenge => pr(),
        // The redirect carries a document of its own, as hostile as the
        // one it points to: a 3xx is never read.
        ("GET", "/.well-known/oauth-protected-resource/mcp") if config.pr_redirect => Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, format!("{origin}/elsewhere"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "resource": mcp_url, "authorization_servers": ["https://evil.example"] }).to_string(),
            ))
            .unwrap(),
        ("GET", "/.well-known/oauth-protected-resource/mcp") if config.pr_status.is_some() => {
            StatusCode::from_u16(config.pr_status.unwrap()).unwrap().into_response()
        }
        ("GET", "/elsewhere") => json_response(
            200,
            &json!({ "resource": mcp_url, "authorization_servers": ["https://evil.example"] }),
        ),
        ("GET", "/.well-known/oauth-protected-resource/mcp") if config.pr_at == PrAt::PathInserted => pr(),
        ("GET", "/.well-known/oauth-protected-resource") if config.pr_at == PrAt::Origin => pr(),
        ("GET", p)
            if config.meta_status.is_some()
                && (p.contains("oauth-authorization-server") || p.contains("openid-configuration")) =>
        {
            StatusCode::from_u16(config.meta_status.unwrap())
                .unwrap()
                .into_response()
        }
        ("GET", p) if p == meta_path && config.serve_metadata => match &config.raw_meta {
            Some(raw) => raw_json(raw),
            None => {
                let mut meta = json!({
                    "issuer": issuer,
                    "authorization_endpoint": config.authorization_endpoint.clone().unwrap_or_else(|| format!("{origin}/authorize")),
                    "token_endpoint": config.token_endpoint.clone().unwrap_or_else(|| format!("{origin}/token")),
                    "code_challenge_methods_supported": if config.s256 { json!(["S256"]) } else { json!(["plain"]) },
                    "authorization_response_iss_parameter_supported": config.iss_parameter,
                });
                if config.registration {
                    meta["registration_endpoint"] = json!(format!("{origin}/register"));
                }
                if !config.auth_methods.is_empty() {
                    meta["token_endpoint_auth_methods_supported"] = json!(config.auth_methods);
                }
                json_response(200, &meta)
            }
        },
        ("POST", "/register") => register(&shared, &config, &body),
        ("GET", "/authorize") => authorize(&shared, &config, &parts.uri, &issuer),
        ("POST", "/token") => {
            if config.gate_token {
                let mut gate = shared.gate.clone();
                while !*gate.borrow() {
                    if gate.changed().await.is_err() {
                        break;
                    }
                }
            }
            if let Some(status) = config.token_status {
                return StatusCode::from_u16(status).unwrap().into_response();
            }
            token(&shared, &config, &parts.headers, &body)
        }
        _ => not_found(),
    }
}

fn mcp(shared: &Shared, config: &Config, headers: &HeaderMap, method: &str, body: &[u8], origin: &str) -> Response {
    let live = {
        let mut inner = shared.inner.lock().unwrap();
        if let Some(token) = bearer(headers) {
            inner.recorded.bearers.push(token);
        }
        bearer(headers).is_some_and(|t| inner.access.get(&t).is_some_and(|f| !inner.revoked.contains(f)))
    };
    if !live {
        let mut response = StatusCode::UNAUTHORIZED.into_response();
        if config.pr_at == PrAt::Challenge {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                format!(
                    "Bearer resource_metadata=\"{}\"",
                    config
                        .challenge_url
                        .clone()
                        .unwrap_or_else(|| format!("{origin}/prm-from-challenge"))
                )
                .parse()
                .unwrap(),
            );
        } else {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap());
        }
        return response;
    }
    if let Some(status) = config.mcp_status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    if method == "DELETE" {
        return StatusCode::OK.into_response();
    }
    let message: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if config.stateful
        && message["method"] != "initialize"
        && (header("mcp-session-id") != Some("fake-session") || header("mcp-protocol-version") != Some("2025-06-18"))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(id) = message.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match message["method"].as_str() {
        Some("initialize") => {
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": { "name": "fake" } })
        }
        Some("tools/list") => json!({ "tools": [{ "name": "search", "inputSchema": { "type": "object" } }] }),
        _ => json!({}),
    };
    let mut response = json_response(200, &json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    response
        .headers_mut()
        .insert("mcp-session-id", "fake-session".parse().unwrap());
    response
}

fn register(shared: &Shared, config: &Config, body: &[u8]) -> Response {
    if let Some((status, refusal)) = &config.dcr_refusal {
        return json_response(*status, refusal);
    }
    let request: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(request["token_endpoint_auth_method"], "none", "a public client");
    if config.dcr_without_client_id {
        return json_response(201, &json!({ "redirect_uris": request["redirect_uris"] }));
    }
    let mut inner = shared.inner.lock().unwrap();
    inner.next += 1;
    let client_id = format!("client-{}", inner.next);
    inner.recorded.registered.push(client_id.clone());
    if config.dcr_secret {
        let secret = format!("dcr-secret-{}", inner.next);
        inner.secrets.push((client_id.clone(), secret.clone()));
        return json_response(
            201,
            &json!({ "client_id": client_id, "client_secret": secret, "redirect_uris": request["redirect_uris"] }),
        );
    }
    inner.clients.push(client_id.clone());
    json_response(
        201,
        &json!({ "client_id": client_id, "redirect_uris": request["redirect_uris"] }),
    )
}

fn authorize(shared: &Shared, config: &Config, uri: &axum::http::Uri, issuer: &str) -> Response {
    let q = query(uri);
    assert_eq!(q.get("response_type").map(String::as_str), Some("code"));
    assert_eq!(q.get("code_challenge_method").map(String::as_str), Some("S256"));
    let redirect_uri = q["redirect_uri"].clone();
    let state = q["state"].clone();
    let resource = q.get("resource").cloned();
    let mut inner = shared.inner.lock().unwrap();
    inner.recorded.consent_resources.push(resource.clone());
    let iss = match &config.iss_sent {
        Some(sent) => sent.clone(),
        None => Some(issuer.to_string()),
    };
    let mut location = url::Url::parse(&redirect_uri).unwrap();
    {
        let mut pairs = location.query_pairs_mut();
        if config.refuse_resource_at_consent && resource.is_some() {
            pairs.append_pair("error", "invalid_target");
        } else if let Some(description) = &config.deny {
            pairs
                .append_pair("error", "access_denied")
                .append_pair("error_description", description);
        } else {
            inner.next += 1;
            let code = format!("{}code-{}", config.prefix, inner.next);
            inner.recorded.issued.push(code.clone());
            inner.codes.insert(
                code.clone(),
                Code {
                    client_id: q["client_id"].clone(),
                    challenge: q["code_challenge"].clone(),
                    redirect_uri: redirect_uri.clone(),
                    resource,
                },
            );
            pairs.append_pair("code", &code);
        }
        pairs.append_pair("state", &state);
        if let Some(iss) = iss {
            pairs.append_pair("iss", &iss);
        }
    }
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location.as_str())
        .body(Body::empty())
        .unwrap()
}

fn refused(config: &Config, error: &str) -> Response {
    json_response(400, config.refusal_body.as_ref().unwrap_or(&json!({ "error": error })))
}

fn token(shared: &Shared, config: &Config, headers: &HeaderMap, body: &[u8]) -> Response {
    let f = form(body);
    let mut inner = shared.inner.lock().unwrap();
    // The client.
    let basic = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .map(|b| String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b).unwrap()).unwrap());
    if let Some(basic) = &basic {
        inner.recorded.basic.push(basic.clone());
    }
    let posted = f.get("client_id").zip(f.get("client_secret"));
    let client_ok = match (&config.confidential, &basic) {
        (Some((id, secret)), Some(basic)) => *basic == format!("{id}:{secret}"),
        // `client_secret_post`.
        (Some((id, secret)), None) => posted == Some((id, secret)),
        (None, Some(basic)) => inner
            .secrets
            .iter()
            .any(|(id, secret)| *basic == format!("{id}:{secret}")),
        (None, None) => f
            .get("client_id")
            .is_some_and(|id| inner.clients.contains(id) || id.starts_with("pre-")),
    };
    if posted.is_some() {
        inner.recorded.posted_secret = true;
    }
    if !client_ok {
        return json_response(401, &json!({ "error": "invalid_client" }));
    }
    let resource = f.get("resource").cloned();
    let grant_type = f.get("grant_type").cloned().unwrap_or_default();
    inner
        .recorded
        .token_resources
        .push((grant_type.clone(), resource.clone()));
    if config.refuse_resource_at_token && resource.is_some() {
        return json_response(400, &json!({ "error": "invalid_target" }));
    }
    let family = match grant_type.as_str() {
        "authorization_code" => {
            inner.recorded.exchanges += 1;
            let Some(code) = f.get("code").and_then(|c| inner.codes.remove(c)) else {
                return refused(config, "invalid_grant");
            };
            let verifier = f.get("code_verifier").cloned().unwrap_or_default();
            inner.recorded.verifiers.push(verifier.clone());
            let challenge =
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            if challenge != code.challenge
                || f.get("redirect_uri") != Some(&code.redirect_uri)
                || (basic.is_none() && f.get("client_id") != Some(&code.client_id))
                || basic
                    .as_ref()
                    .is_some_and(|b| !b.starts_with(&format!("{}:", code.client_id)))
                || (resource.is_some() && resource != code.resource)
            {
                return refused(config, "invalid_grant");
            }
            inner.next += 1;
            inner.next
        }
        "refresh_token" => {
            inner.recorded.refreshes += 1;
            inner.recorded.refresh_scopes.push(f.get("scope").cloned());
            let Some(presented) = f.get("refresh_token").cloned() else {
                return refused(config, "invalid_request");
            };
            let Some(held) = inner.refresh.get(&presented).cloned() else {
                return refused(config, "invalid_grant");
            };
            if inner.revoked.contains(&held.family) {
                return refused(config, "invalid_grant");
            }
            if held.used {
                // Reuse: the whole family is revoked (OAuth 2.0 Security BCP).
                inner.recorded.reuse_detected = true;
                inner.revoked.push(held.family);
                return refused(config, "invalid_grant");
            }
            if !config.refresh_keeps_token {
                inner.refresh.get_mut(&presented).unwrap().used = true;
            }
            held.family
        }
        _ => return refused(config, "unsupported_grant_type"),
    };
    if let Some(raw) = &config.raw_token {
        return raw_json(raw);
    }
    let n = inner.access.len() + inner.refresh.len();
    let access = format!("{}access-{family}-{n}", config.prefix);
    inner.recorded.issued.push(access.clone());
    inner.access.insert(access.clone(), family);
    let mut issued = json!({ "access_token": access, "token_type": "Bearer" });
    if let Some(expires_in) = config.expires_in {
        issued["expires_in"] = json!(expires_in);
    }
    let rotate = grant_type == "authorization_code" || !config.refresh_keeps_token;
    if rotate {
        let refresh = format!("{}refresh-{family}-{n}", config.prefix);
        inner.recorded.issued.push(refresh.clone());
        inner.refresh.insert(refresh.clone(), Refresh { family, used: false });
        issued["refresh_token"] = json!(refresh);
    }
    json_response(200, &issued)
}
