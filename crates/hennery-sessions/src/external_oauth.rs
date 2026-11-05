//! Incoming OAuth authorization server for external MCP, separate from outbound gateway OAuth.
use crate::{
    AppState,
    external_oauth_store::{Authorization, Consent, DEFAULT_SCOPE, Metadata, SCOPE, TokenRequest, validate_metadata},
};
use axum::{
    Json, Router,
    extract::{Form, FromRequest, Path, Query, Request, State},
    handler::Handler,
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use hennery_kernel::{
    client_address::ClientAddress,
    egress::{Allowance, Egress, EgressClient, Timeouts, Url},
    ratelimit::{Limiter, Policy},
};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

pub(crate) struct Runtime {
    egress: EgressClient,
    rate: Limiter,
}
impl Runtime {
    pub(crate) fn new() -> Self {
        Self {
            egress: Egress::new(Timeouts {
                connect: Duration::from_secs(3),
                request: Duration::from_secs(5),
            })
            .expect("OAuth TLS client")
            .client(Allowance::PublicOnly),
            rate: Limiter::new(Policy {
                free_failures: 60,
                window: Duration::from_secs(60),
                first_lockout: Duration::from_secs(5),
                max_lockout: Duration::from_secs(60),
            }),
        }
    }
}
fn private(value: impl IntoResponse) -> Response {
    let mut r = value.into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    r.headers_mut()
        .insert(header::PRAGMA, header::HeaderValue::from_static("no-cache"));
    r
}
pub(crate) fn location(state: &AppState) -> Option<(String, String)> {
    state
        .operator
        .public_url()
        .filter(|p| p.is_https())
        .map(|p| (p.origin().to_string(), format!("{}/mcp/external/hennery", p.origin())))
}
pub(crate) fn challenge(state: &AppState) -> Response {
    let mut r = private(StatusCode::UNAUTHORIZED);
    let value = match location(state) {
        Some((issuer, _)) => format!(
            "Bearer resource_metadata=\"{issuer}/.well-known/oauth-protected-resource/mcp/external/hennery\", scope=\"{DEFAULT_SCOPE}\""
        ),
        None => "Bearer".into(),
    };
    r.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_str(&value).expect("validated public origin"),
    );
    r
}
pub(crate) fn insufficient_scope(state: &AppState) -> Response {
    scope_challenge(state, SCOPE)
}
pub(crate) fn scope_challenge(state: &AppState, scope: &str) -> Response {
    let mut response = challenge(state);
    *response.status_mut() = StatusCode::FORBIDDEN;
    let metadata = location(state)
        .map(|(issuer, _)| {
            format!(", resource_metadata=\"{issuer}/.well-known/oauth-protected-resource/mcp/external/hennery\"")
        })
        .unwrap_or_default();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_str(&format!(
            "Bearer error=\"insufficient_scope\", scope=\"{scope}\"{metadata}"
        ))
        .expect("validated public origin"),
    );
    response
}
fn error(code: &str) -> Response {
    let (status, code) = match code {
        "temporarily_unavailable" => (StatusCode::TOO_MANY_REQUESTS, code),
        "invalid_request"
        | "invalid_client"
        | "invalid_grant"
        | "invalid_scope"
        | "invalid_target"
        | "invalid_redirect_uri"
        | "invalid_client_metadata"
        | "unsupported_response_type"
        | "unsupported_grant_type"
        | "invalid_grant_scope" => (StatusCode::BAD_REQUEST, code),
        "invalid_grant_configuration" => (StatusCode::BAD_REQUEST, "invalid_grant"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let mut response = private((status, Json(json!({"error":code}))));
    if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, header::HeaderValue::from_static("60"));
    }
    response
}
fn unavailable() -> Response {
    private((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error":"https_public_url_required"})),
    ))
}
pub(crate) fn at_capacity() -> Response {
    let mut response = error("temporarily_unavailable");
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, header::HeaderValue::from_static("1"));
    response
}
/// Body extraction can await a slow client. Revalidate the browser session
/// after that await, before issuing durable authority.
pub(crate) fn fresh_operator(state: &AppState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    match state
        .operator
        .authenticate_cookies(headers, hennery_kernel::secret::unix_now())
    {
        Ok(Some((_, session))) if session.stepped_up(hennery_kernel::secret::unix_now()) => Ok(()),
        Ok(Some(_)) => Err(Box::new(hennery_kernel::auth::step_up_required())),
        Ok(None) => Err(Box::new(crate::api::error(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "sign in first",
        ))),
        Err(e) => Err(Box::new(crate::api::internal(e))),
    }
}
fn charged(state: &AppState, addr: ClientAddress) -> Result<(), Box<Response>> {
    state
        .external_oauth
        .rate
        .attempt(addr.effective, Instant::now())
        .map_err(|_| Box::new(error("temporarily_unavailable")))
}
pub fn router(state: AppState) -> Router {
    let manage = Router::new()
        .route("/api/external-oauth/requests/{id}", get(pending))
        .route(
            "/api/external-oauth/consent",
            post(consent.layer(middleware::from_fn(hennery_kernel::auth::require_step_up))),
        );
    hennery_kernel::auth::operator_only(manage, state.operator.clone())
        .with_state(state.clone())
        .merge(
            Router::new()
                .route("/.well-known/oauth-protected-resource", get(resource_metadata))
                .route(
                    "/.well-known/oauth-protected-resource/mcp/external/hennery",
                    get(resource_metadata),
                )
                .route("/.well-known/oauth-authorization-server", get(server_metadata))
                .route("/oauth/external/register", post(register))
                .route("/oauth/external/authorize", get(authorize))
                .route("/oauth/external/token", post(token))
                .route("/oauth/external/revoke", post(revoke))
                .with_state(state),
        )
}
async fn resource_metadata(State(state): State<AppState>) -> Response {
    let Some((issuer, resource)) = location(&state) else {
        return unavailable();
    };
    private(Json(
        json!({"resource":resource,"authorization_servers":[issuer],"scopes_supported":["hennery:discover","hennery:sessions:read","hennery:sessions:transcript","hennery:sessions:control"],"bearer_methods_supported":["header"],"resource_name":"Hennery external sessions"}),
    ))
}
async fn server_metadata(State(state): State<AppState>) -> Response {
    let Some((issuer, _)) = location(&state) else {
        return unavailable();
    };
    private(Json(
        json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/oauth/external/authorize"),"token_endpoint":format!("{issuer}/oauth/external/token"),"registration_endpoint":format!("{issuer}/oauth/external/register"),"revocation_endpoint":format!("{issuer}/oauth/external/revoke"),"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"token_endpoint_auth_methods_supported":["none"],"revocation_endpoint_auth_methods_supported":["none"],"code_challenge_methods_supported":["S256"],"scopes_supported":["hennery:discover","hennery:sessions:read","hennery:sessions:transcript","hennery:sessions:control","offline_access"],"client_id_metadata_document_supported":true,"authorization_response_iss_parameter_supported":true}),
    ))
}
async fn body(req: Request) -> Result<axum::body::Bytes, Box<Response>> {
    match tokio::time::timeout(Duration::from_secs(5), axum::body::to_bytes(req.into_body(), 16 * 1024)).await {
        Ok(Ok(b)) => Ok(b),
        _ => Err(Box::new(error("invalid_request"))),
    }
}
async fn register(State(state): State<AppState>, addr: ClientAddress, req: Request) -> Response {
    let Some(_permit) = state.external_admission.try_acquire(addr.effective) else {
        return at_capacity();
    };
    if location(&state).is_none() {
        return unavailable();
    }
    if let Err(r) = charged(&state, addr) {
        return *r;
    }
    if !req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.split(';').next() == Some("application/json"))
    {
        return error("invalid_request");
    }
    let bytes = match body(req).await {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let metadata: Metadata = match serde_json::from_slice(&bytes) {
        Ok(m) => m,
        Err(_) => return error("invalid_client_metadata"),
    };
    match state.store.register_oauth_client(metadata) {
        Ok(m) => {
            let mut value = serde_json::to_value(m).expect("metadata");
            value["client_id_issued_at"] = json!(hennery_kernel::secret::unix_now());
            private((StatusCode::CREATED, Json(value)))
        }
        Err(e) => error(&e.to_string()),
    }
}
fn metadata_url(id: &str) -> anyhow::Result<Url> {
    anyhow::ensure!(id.len() <= 2048, "invalid_client");
    let u = Url::parse(id).map_err(|_| anyhow::anyhow!("invalid_client"))?;
    anyhow::ensure!(
        u.scheme() == "https"
            && u.path() != "/"
            && u.query().is_none()
            && u.fragment().is_none()
            && u.username().is_empty()
            && u.password().is_none(),
        "invalid_client"
    );
    hennery_kernel::egress::check_url(&u, Allowance::PublicOnly).map_err(|_| anyhow::anyhow!("invalid_client"))?;
    Ok(u)
}
async fn metadata(state: &AppState, id: &str) -> anyhow::Result<Metadata> {
    if !id.starts_with("https:") {
        return state
            .store
            .oauth_client(id)?
            .ok_or_else(|| anyhow::anyhow!("invalid_client"));
    }
    let url = metadata_url(id)?;
    let request = hennery_kernel::egress::Request::new(hennery_kernel::egress::Method::GET, url);
    let response = state
        .external_oauth
        .egress
        .send(request)
        .await
        .map_err(|_| anyhow::anyhow!("invalid_client"))?;
    anyhow::ensure!(response.status().is_success(), "invalid_client");
    let bytes = hennery_kernel::egress::read_capped(response, 32 * 1024)
        .await
        .map_err(|_| anyhow::anyhow!("invalid_client"))?;
    metadata_document(&bytes, id)
}
#[derive(serde::Deserialize)]
struct MetadataDocument {
    #[serde(flatten)]
    metadata: Metadata,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Vec<String>,
}
fn metadata_document(bytes: &[u8], id: &str) -> anyhow::Result<Metadata> {
    let document: MetadataDocument =
        serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("invalid_client_metadata"))?;
    // A client may prefer a confidential method and still support "none"
    // (ChatGPT prefers private_key_jwt). We advertise only "none", so such a
    // client is a public client here; PKCE still binds the code.
    let public = document
        .token_endpoint_auth_methods_supported
        .iter()
        .any(|a| a == "none");
    let parsed = Metadata {
        token_endpoint_auth_method: if public {
            Some("none".into())
        } else {
            document.metadata.token_endpoint_auth_method.clone()
        },
        ..document.metadata
    };
    anyhow::ensure!(parsed.client_id.as_deref() == Some(id), "invalid_client");
    // The client hosts this document for every server it talks to, so it may
    // list grants we never issue (Claude lists jwt-bearer). Keep only ours;
    // the token endpoint refuses any other grant_type on its own.
    let supported: Vec<String> = parsed
        .grant_types
        .iter()
        .filter(|g| matches!(g.as_str(), "authorization_code" | "refresh_token"))
        .cloned()
        .collect();
    anyhow::ensure!(
        parsed.grant_types.is_empty() || !supported.is_empty(),
        "invalid_client_metadata"
    );
    let m = Metadata {
        grant_types: supported,
        ..parsed
    };
    validate_metadata(&m)?;
    Ok(m)
}
async fn authorize(State(state): State<AppState>, addr: ClientAddress, Query(a): Query<Authorization>) -> Response {
    let Some(_permit) = state.external_admission.try_acquire(addr.effective) else {
        return at_capacity();
    };
    let Some((_, resource)) = location(&state) else {
        return unavailable();
    };
    if let Err(r) = charged(&state, addr) {
        return *r;
    }
    let m = match metadata(&state, &a.client_id).await {
        Ok(m) => m,
        Err(e) => return error(&e.to_string()),
    };
    match state.store.begin_oauth(&a, &m, &resource) {
        Ok(id) => private(Redirect::to(&format!("/external-mcp/consent?request={id}"))),
        Err(e) => error(&e.to_string()),
    }
}
async fn pending(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some((_, resource)) = location(&state) else {
        return unavailable();
    };
    let result = (|| -> anyhow::Result<_> {
        let pending = state.store.pending_oauth(&id, &resource)?;
        let hats = state
            .hosts
            .hats()?
            .into_iter()
            .map(|h| json!({"id":h.id,"name":h.name}))
            .collect::<Vec<_>>();
        let hosts = state
            .hosts
            .list()?
            .into_iter()
            .filter(|h| h.revoked_at.is_none())
            .map(|h| json!({"id":h.id,"name":h.name}))
            .collect::<Vec<_>>();
        Ok(json!({"request":pending,"hats":hats,"hosts":hosts}))
    })();
    match result {
        Ok(value) => private(Json(value)),
        Err(e) => error(&e.to_string()),
    }
}
async fn consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    hennery_kernel::json::ApiJson(input): hennery_kernel::json::ApiJson<Consent>,
) -> Response {
    if let Err(response) = fresh_operator(&state, &headers) {
        return *response;
    }
    let Some((issuer, resource)) = location(&state) else {
        return unavailable();
    };
    match state.store.consent_oauth(&input, &resource, &issuer) {
        Ok(redirect_uri) => private(Json(json!({"redirect_uri":redirect_uri}))),
        Err(e) if e.to_string() == "invalid_grant" => error("invalid_grant_configuration"),
        Err(e) => error(&e.to_string()),
    }
}
async fn form<T: serde::de::DeserializeOwned>(req: Request) -> Result<T, Box<Response>> {
    if req.headers().contains_key(header::AUTHORIZATION) {
        return Err(Box::new(error("invalid_client")));
    }
    // Keep the form extractor's RFC6749 content-type and duplicate-field validation,
    // but bound the bytes and slow-body time before invoking it.
    let (parts, body_) = req.into_parts();
    let bytes = body(Request::new(body_)).await?;
    let request = Request::from_parts(parts, axum::body::Body::from(bytes));
    Form::<T>::from_request(request, &())
        .await
        .map(|Form(value)| value)
        .map_err(|_| Box::new(error("invalid_request")))
}
async fn token(State(state): State<AppState>, addr: ClientAddress, req: Request) -> Response {
    let Some(_permit) = state.external_admission.try_acquire(addr.effective) else {
        return at_capacity();
    };
    let Some((_, resource)) = location(&state) else {
        return unavailable();
    };
    if let Err(r) = charged(&state, addr) {
        return *r;
    }
    let input = match form::<TokenRequest>(req).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match state.store.exchange_oauth(&input, &resource) {
        Ok(value) => private(Json(value)),
        Err(e) => error(&e.to_string()),
    }
}
#[derive(Deserialize)]
struct Revoke {
    token: String,
    client_id: String,
    resource: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}
async fn revoke(State(state): State<AppState>, addr: ClientAddress, req: Request) -> Response {
    let Some(_permit) = state.external_admission.try_acquire(addr.effective) else {
        return at_capacity();
    };
    let Some((_, resource)) = location(&state) else {
        return unavailable();
    };
    if let Err(r) = charged(&state, addr) {
        return *r;
    }
    let input = match form::<Revoke>(req).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    if input.client_secret.is_some() {
        return error("invalid_client");
    }
    if input.resource.is_some_and(|target| target != resource) {
        return error("invalid_target");
    }
    match state
        .store
        .revoke_oauth_token(&input.token, &input.client_id, &resource)
    {
        Ok(()) => private(StatusCode::OK),
        Err(e) => error(&e.to_string()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cimd_grant_types_this_server_does_not_support_are_dropped_not_fatal() {
        // Claude's published document (fetched 2026-11-04) also lists jwt-bearer.
        let id = "https://claude.ai/oauth/mcp-oauth-client-metadata";
        let claude = json!({"client_id":id,"client_name":"Claude","client_uri":"https://claude.ai","redirect_uris":["https://claude.ai/api/mcp/auth_callback"],"grant_types":["authorization_code","refresh_token","urn:ietf:params:oauth:grant-type:jwt-bearer"],"response_types":["code"],"token_endpoint_auth_method":"none"});
        let m = metadata_document(&serde_json::to_vec(&claude).unwrap(), id).unwrap();
        assert_eq!(m.grant_types, ["authorization_code", "refresh_token"]);
        let only_unsupported = json!({"client_id":id,"client_name":"Claude","redirect_uris":["https://claude.ai/api/mcp/auth_callback"],"grant_types":["urn:ietf:params:oauth:grant-type:jwt-bearer"],"token_endpoint_auth_method":"none"});
        assert!(metadata_document(&serde_json::to_vec(&only_unsupported).unwrap(), id).is_err());
    }
    #[test]
    fn cimd_that_prefers_private_key_jwt_but_supports_none_is_a_public_client() {
        // ChatGPT's published document (fetched 2026-11-05) prefers
        // private_key_jwt and also lists "none" as supported.
        let id = "https://chatgpt.com/oauth/client.json";
        let chatgpt = json!({"client_id":id,"client_uri":"https://chatgpt.com/","redirect_uris":["https://chatgpt.com/connector_platform_oauth_redirect"],"token_endpoint_auth_method":"private_key_jwt","token_endpoint_auth_methods_supported":["none","private_key_jwt"],"grant_types":["authorization_code","refresh_token"],"response_types":["code"],"client_name":"ChatGPT","logo_uri":"https://persistent.oaistatic.com/sonic/misc/openai-logo.png","token_endpoint_auth_signing_alg":"RS256","jwks_uri":"https://chatgpt.com/oauth/jwks.json"});
        let m = metadata_document(&serde_json::to_vec(&chatgpt).unwrap(), id).unwrap();
        assert_eq!(m.token_endpoint_auth_method.as_deref(), Some("none"));
        let no_public = json!({"client_id":id,"client_name":"ChatGPT","redirect_uris":["https://chatgpt.com/connector_platform_oauth_redirect"],"token_endpoint_auth_method":"private_key_jwt","token_endpoint_auth_methods_supported":["private_key_jwt"]});
        assert!(metadata_document(&serde_json::to_vec(&no_public).unwrap(), id).is_err());
    }
    #[test]
    fn cimd_identity_and_redirects_are_verified_without_trusting_extra_branding() {
        let id = "https://client.example/metadata.json";
        let good = json!({"client_id":id,"client_name":"Remote client","redirect_uris":["https://client.example/callback"],"token_endpoint_auth_method":"none","grant_types":["authorization_code","refresh_token"],"logo_uri":"http://127.0.0.1/never-fetched"});
        assert!(metadata_document(&serde_json::to_vec(&good).unwrap(), id).is_ok());
        for mode in 0..4 {
            let mut bad = good.clone();
            match mode {
                0 => bad["client_id"] = json!("https://other.example/metadata.json"),
                1 => bad["redirect_uris"] = json!(["javascript:alert(1)"]),
                2 => bad["token_endpoint_auth_method"] = json!("private_key_jwt"),
                _ => bad["client_name"] = json!("\nunsafe"),
            };
            assert!(
                metadata_document(&serde_json::to_vec(&bad).unwrap(), id).is_err(),
                "{mode}"
            );
        }
    }
    #[test]
    fn metadata_document_locations_cannot_target_private_networks_or_credentials() {
        for u in [
            "http://example.com/client.json",
            "https://127.0.0.1/client.json",
            "https://[::1]/client.json",
            "https://10.1.2.3/client.json",
            "https://user:secret@example.com/client.json",
            "https://example.com",
            "https://example.com/meta?secret=1",
            "https://example.com/meta#x",
        ] {
            assert!(metadata_url(u).is_err(), "{u}");
        }
        assert!(metadata_url("https://client.example/metadata.json").is_ok());
    }
}
