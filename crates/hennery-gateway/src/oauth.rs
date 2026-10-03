//! The gateway's OAuth client (gateway spec §4): discovery, dynamic
//! registration, PKCE, the consent URL, and the token endpoint's two grants.
//! Hand-written on the kernel's egress clients (plan 8f decision 1), not on
//! `rmcp`'s `auth` module: that one tries the authorization server's
//! metadata in another order than §4.1, follows redirects in discovery
//! itself, and brings reqwest 0.13 beside the workspace's 0.12.
//!
//! - **Every request goes through the egress policy** (kernel spec §7.1),
//!   under the connection's allowance, and redirects are never followed:
//!   a 3xx is "not here". Before anything is sent, every metadata document,
//!   every endpoint and the consent URL must be `https`, or plain `http` to
//!   loopback ([`hennery_kernel::egress::is_https_or_loopback`]), even on a
//!   connection marked "internal network" (plan 8b-ii's hand-off).
//! - **Discovery order** (§4.1): the protected-resource (PR) document from
//!   a `WWW-Authenticate` challenge to one unauthenticated `initialize`,
//!   then at the path-inserted well-known URL, then at the origin's; then,
//!   for each authorization server it names (else the resource's origin),
//!   RFC 8414's inserted form, its appended form, then OpenID
//!   configuration (appended, then inserted). The first metadata with both
//!   endpoints and the `issuer` it was fetched for (RFC 8414 §3.3) wins.
//! - **Bodies are capped** (`MAX_DOCUMENT`) and token responses are never
//!   logged or echoed: they can repeat the code (§4.3). An error names only
//!   origins (lane L11), and of a token endpoint's refusal only its RFC 6749
//!   `error` when it is one of the fixed values.

use crate::model::{AuthMethod, MAX_URL, TokenClient, url_for_logs, vendor_text};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hennery_kernel::egress::{
    self, EgressClient, EgressError, Method, Request, StatusCode, Url, header, is_https_or_loopback,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use zeroize::Zeroizing;

/// The largest document or token response read (rmcp's cap too).
pub const MAX_DOCUMENT: usize = 1024 * 1024;

/// How long one OAuth request may take, body included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The most authorization servers of one PR document tried.
const MAX_SERVERS: usize = 4;

/// The `expires_in` margin (gateway spec §4.4).
pub const EXPIRY_MARGIN: i64 = 60;

/// The RFC 6749 §5.2 errors a refusal's message may name; any other is
/// shown only as its HTTP status (api-8e-8f B5 step 7).
const NAMED_ERRORS: &[&str] = &[
    "invalid_grant",
    "invalid_client",
    "invalid_request",
    "unauthorized_client",
    "unsupported_grant_type",
    "invalid_scope",
];

/// What discovery found (gateway spec §4.1).
#[derive(Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The PR document's `resource`; `None` when no PR document was found
    /// and the resource's origin was taken as the authorization server.
    pub resource: Option<String>,
    /// The PR document's `scopes_supported`, requested at consent (G-5).
    pub scopes: Vec<String>,
    pub issuer: String,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub registration_endpoint: Option<Url>,
    /// `code_challenge_methods_supported` names `S256`.
    pub s256: bool,
    /// `token_endpoint_auth_methods_supported`, as advertised.
    pub auth_methods: Vec<String>,
    /// RFC 9207's `authorization_response_iss_parameter_supported`.
    pub iss_parameter: bool,
}

impl std::fmt::Debug for Discovered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Discovered")
            .field("issuer", &url_for_logs(&self.issuer))
            .field("s256", &self.s256)
            .field("iss_parameter", &self.iss_parameter)
            .finish_non_exhaustive()
    }
}

/// Why discovery found nothing to use. Each message names only origins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    /// No usable PR or authorization-server document, or none whose
    /// `issuer` is the one it was fetched for.
    Failed(String),
    /// A document, an endpoint or a server named is not `https` (loopback
    /// `http` excepted).
    Insecure(String),
    /// The egress policy refused an address.
    Refused(String),
    /// Nothing answered: timeouts, transport failures or 5xx only.
    Unreachable(String),
}

/// A fetched candidate, as discovery reads it.
enum Fetched {
    Document(Map<String, Value>),
    /// Any other answer: a 3xx (never followed), 4xx, or not a JSON object.
    NotHere,
    /// A timeout, a transport failure, or a 5xx.
    Unreachable,
}

/// GET `url` as a JSON object the gateway trusts for `keys`. `Err` only
/// for the egress policy's refusal, which ends discovery.
async fn fetch(client: &EgressClient, url: &Url, keys: &[&str]) -> Result<Fetched, DiscoveryError> {
    let mut request = Request::new(Method::GET, url.clone());
    request
        .headers_mut()
        .insert(header::ACCEPT, header::HeaderValue::from_static("application/json"));
    *request.timeout_mut() = Some(REQUEST_TIMEOUT);
    let response = match client.send(request).await {
        Ok(response) => response,
        Err(EgressError::Refused(refused)) => {
            return Err(DiscoveryError::Refused(format!(
                "{} is refused by the egress policy: {refused}",
                url_for_logs(url.as_str())
            )));
        }
        Err(_) => return Ok(Fetched::Unreachable),
    };
    let status = response.status();
    if status.is_server_error() {
        return Ok(Fetched::Unreachable);
    }
    if status != StatusCode::OK {
        return Ok(Fetched::NotHere);
    }
    match egress::read_capped(response, MAX_DOCUMENT).await {
        // A key twice, or one spelt otherwise, is no document: which
        // `issuer` or endpoint it names depends on the parser (plan 8f
        // decision 8, lane L16).
        Ok(bytes) => Ok(trusted_object(&bytes, keys).map_or(Fetched::NotHere, Fetched::Document)),
        Err(egress::BodyError::TooLarge) => Ok(Fetched::NotHere),
        Err(egress::BodyError::Failed(_)) => Ok(Fetched::Unreachable),
    }
}

/// `url`, if it may carry OAuth (§4): else why not, by origin.
fn secure(url: &Url, what: &str) -> Result<(), DiscoveryError> {
    if is_https_or_loopback(url) {
        Ok(())
    } else {
        Err(DiscoveryError::Insecure(format!(
            "{what} at {} is not https",
            url_for_logs(url.as_str())
        )))
    }
}

/// The keys gateway reads of a protected-resource document, of an
/// authorization server's metadata, of a registration's answer and of a
/// token response. A document spelling one of them otherwise (`Issuer`,
/// `token-endpoint`), as a decoder matching names whatever their case
/// would read it, is no document (lane L16, as plan 8d's request bodies).
const RESOURCE_KEYS: &[&str] = &["resource", "authorization_servers", "scopes_supported"];
const SERVER_KEYS: &[&str] = &[
    "issuer",
    "authorization_endpoint",
    "token_endpoint",
    "registration_endpoint",
    "code_challenge_methods_supported",
    "token_endpoint_auth_methods_supported",
    "authorization_response_iss_parameter_supported",
];
const REGISTRATION_KEYS: &[&str] = &[
    "client_id",
    "client_secret",
    "token_endpoint_auth_method",
    "error",
    "error_description",
];
const TOKEN_KEYS: &[&str] = &["access_token", "token_type", "refresh_token", "expires_in", "error"];

/// Text the gateway trusts: no control character. `url::Url` drops a tab
/// or a newline inside a URL silently, so `https://as.exa\tmple` would be
/// read as another text than the one sent (lane L16).
fn clean(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

/// A string field the gateway trusts; `None` when absent, not a string or
/// not clean, which every caller reads as "not there" (fail closed).
fn text<'a>(document: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    document.get(field)?.as_str().filter(|text| clean(text))
}

/// A URL in a document: absolute, at most `MAX_URL` bytes, clean, no
/// fragment.
fn document_url(document: &Map<String, Value>, field: &str) -> Option<Url> {
    let text = text(document, field)?;
    if text.len() > MAX_URL {
        return None;
    }
    Url::parse(text).ok().filter(|url| url.fragment().is_none())
}

/// A list of strings; an entry that is not a clean string of 1 to 256
/// bytes is left out.
fn strings(document: &Map<String, Value>, field: &str) -> Vec<String> {
    document
        .get(field)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .filter(|s| (1..=256).contains(&s.len()) && clean(s))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// `bytes` as a JSON object the gateway trusts: no key twice anywhere, and
/// none of `keys` spelt otherwise (lane L16). `None` otherwise.
fn trusted_object(bytes: &[u8], keys: &[&str]) -> Option<Map<String, Value>> {
    if !crate::jsonrpc::unique_keys(bytes) {
        return None;
    }
    match serde_json::from_slice::<Value>(bytes) {
        // `respelt` reads a key as json-c and cJSON do, cut at its first
        // NUL: `token_endpoint\u0000x` is `token_endpoint` spelt otherwise.
        Ok(Value::Object(object)) if !crate::jsonrpc::respelt(&object, keys) => Some(object),
        _ => None,
    }
}

/// `url`'s path without its last `/`, as a well-known suffix: `""` for the
/// root.
fn path_suffix(url: &Url) -> String {
    url.path().trim_end_matches('/').to_string()
}

/// `url`'s origin with `path`.
fn at(url: &Url, path: &str) -> Url {
    let mut out = url.clone();
    out.set_path(path);
    out.set_query(None);
    out.set_fragment(None);
    out
}

/// The PR document's well-known URLs for `resource` (RFC 9728 §3.1):
/// path-inserted, then the origin's.
pub fn resource_metadata_urls(resource: &Url) -> Vec<Url> {
    let suffix = path_suffix(resource);
    let mut out = Vec::new();
    if !suffix.is_empty() {
        out.push(at(resource, &format!("/.well-known/oauth-protected-resource{suffix}")));
    }
    out.push(at(resource, "/.well-known/oauth-protected-resource"));
    out
}

/// The authorization server's metadata URLs for `issuer` (gateway spec
/// §4.1): RFC 8414 inserted, RFC 8414 appended, then OpenID configuration
/// appended (OpenID Connect Discovery) and inserted.
pub fn server_metadata_urls(issuer: &Url) -> Vec<Url> {
    let suffix = path_suffix(issuer);
    if suffix.is_empty() {
        return vec![
            at(issuer, "/.well-known/oauth-authorization-server"),
            at(issuer, "/.well-known/openid-configuration"),
        ];
    }
    vec![
        at(issuer, &format!("/.well-known/oauth-authorization-server{suffix}")),
        at(issuer, &format!("{suffix}/.well-known/oauth-authorization-server")),
        at(issuer, &format!("{suffix}/.well-known/openid-configuration")),
        at(issuer, &format!("/.well-known/openid-configuration{suffix}")),
    ]
}

/// Whether two issuer identifiers are the same: equal, but for one final
/// `/` (plan 8f decision 6; RFC 8414 §3.3 wants them identical, and
/// vendors differ on the slash).
pub fn same_issuer(a: &str, b: &str) -> bool {
    a.strip_suffix('/').unwrap_or(a) == b.strip_suffix('/').unwrap_or(b)
}

/// The `resource_metadata` of a `WWW-Authenticate` challenge (RFC 9728
/// §5.1), if any header names one.
pub fn challenge_metadata(headers: &header::HeaderMap) -> Option<String> {
    headers
        .get_all(header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(param_resource_metadata)
}

fn param_resource_metadata(challenge: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("resource_metadata") {
        let start = from + found;
        from = start + "resource_metadata".len();
        // A parameter's name starts the challenge's parameters or follows a
        // space or comma.
        let boundary = start == 0 || matches!(lower.as_bytes()[start - 1], b' ' | b',' | b'\t');
        let rest = challenge[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        if !boundary {
            continue;
        }
        let rest = rest.trim_start();
        if let Some(quoted) = rest.strip_prefix('"') {
            let mut value = String::new();
            let mut chars = quoted.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => value.push(chars.next()?),
                    '"' => return Some(value),
                    c => value.push(c),
                }
            }
            return None;
        }
        let value: String = rest.chars().take_while(|c| !matches!(c, ',' | ' ' | '\t')).collect();
        return (!value.is_empty()).then_some(value);
    }
    None
}

/// One unauthenticated `initialize` to the resource: the challenge's
/// `resource_metadata`, if it answers 401 with one. `Err` only for the
/// egress policy's refusal.
async fn challenge(client: &EgressClient, resource: &Url) -> Result<Option<String>, DiscoveryError> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "hennery", "version": env!("CARGO_PKG_VERSION") },
        },
    });
    let mut request = Request::new(Method::POST, resource.clone());
    let headers = request.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("application/json, text/event-stream"),
    );
    *request.body_mut() = Some(body.to_string().into());
    *request.timeout_mut() = Some(REQUEST_TIMEOUT);
    match client.send_streaming(request).await {
        Ok(response) if response.status() == StatusCode::UNAUTHORIZED => Ok(challenge_metadata(response.headers())),
        Ok(_) => Ok(None),
        Err(EgressError::Refused(refused)) => Err(DiscoveryError::Refused(format!(
            "{} is refused by the egress policy: {refused}",
            url_for_logs(resource.as_str())
        ))),
        Err(_) => Ok(None),
    }
}

/// Discover `resource`'s authorization server (gateway spec §4.1).
pub async fn discover(client: &EgressClient, resource: &Url) -> Result<Discovered, DiscoveryError> {
    let origin = url_for_logs(resource.as_str());
    let mut unreachable = false;
    let mut candidates = Vec::new();
    if let Some(found) = challenge(client, resource).await? {
        let url = Url::parse(&found)
            .ok()
            .filter(|u| found.len() <= MAX_URL && u.fragment().is_none())
            .ok_or_else(|| DiscoveryError::Failed(format!("{origin} named a resource_metadata that is not a URL")))?;
        secure(&url, "the protected-resource document")?;
        candidates.push(url);
    }
    for url in resource_metadata_urls(resource) {
        if !candidates.contains(&url) {
            secure(&url, "the protected-resource document")?;
            candidates.push(url);
        }
    }
    let mut document = None;
    for url in &candidates {
        match fetch(client, url, RESOURCE_KEYS).await? {
            // RFC 9728 §3.2: `resource` is required; a document without it
            // is not one (plan 8f decision 7).
            Fetched::Document(found) if text(&found, "resource").is_some() => {
                document = Some(found);
                break;
            }
            Fetched::Document(_) | Fetched::NotHere => {}
            Fetched::Unreachable => unreachable = true,
        }
    }
    let (resource_named, scopes, servers) = match &document {
        Some(document) => {
            let named = text(document, "resource").map(String::from);
            let mut servers = Vec::new();
            for text in strings(document, "authorization_servers").iter().take(MAX_SERVERS) {
                let url = Url::parse(text)
                    .ok()
                    .filter(|u| text.len() <= MAX_URL && u.fragment().is_none())
                    .ok_or_else(|| {
                        DiscoveryError::Failed(format!(
                            "{origin}'s protected-resource document names a server that is not a URL"
                        ))
                    })?;
                // Its metadata URLs are checked before they are fetched.
                servers.push((text.clone(), url));
            }
            (named, strings(document, "scopes_supported"), servers)
        }
        None => (None, Vec::new(), Vec::new()),
    };
    let servers = if servers.is_empty() {
        let issuer = at(resource, "/");
        vec![(issuer.as_str().trim_end_matches('/').to_string(), issuer)]
    } else {
        servers
    };
    let mut wrong_issuer = false;
    for (identifier, issuer) in &servers {
        for url in server_metadata_urls(issuer) {
            secure(&url, "the authorization server's metadata")?;
            let document = match fetch(client, &url, SERVER_KEYS).await? {
                Fetched::Document(document) => document,
                Fetched::NotHere => continue,
                Fetched::Unreachable => {
                    unreachable = true;
                    continue;
                }
            };
            let (Some(authorization_endpoint), Some(token_endpoint)) = (
                document_url(&document, "authorization_endpoint"),
                document_url(&document, "token_endpoint"),
            ) else {
                continue;
            };
            let found_issuer = text(&document, "issuer").unwrap_or_default();
            if !same_issuer(found_issuer, identifier) {
                wrong_issuer = true;
                continue;
            }
            secure(&authorization_endpoint, "the authorization endpoint")?;
            secure(&token_endpoint, "the token endpoint")?;
            let registration_endpoint = document_url(&document, "registration_endpoint");
            if let Some(url) = &registration_endpoint {
                secure(url, "the registration endpoint")?;
            }
            return Ok(Discovered {
                resource: resource_named,
                scopes,
                issuer: found_issuer.to_string(),
                authorization_endpoint,
                token_endpoint,
                registration_endpoint,
                s256: strings(&document, "code_challenge_methods_supported")
                    .iter()
                    .any(|m| m == "S256"),
                auth_methods: strings(&document, "token_endpoint_auth_methods_supported"),
                iss_parameter: document
                    .get("authorization_response_iss_parameter_supported")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
    }
    let servers: Vec<String> = servers.iter().map(|(_, url)| url_for_logs(url.as_str())).collect();
    let servers = servers.join(", ");
    if wrong_issuer {
        Err(DiscoveryError::Failed(format!(
            "the authorization server metadata at {servers} does not name the issuer it was fetched for"
        )))
    } else if unreachable {
        Err(DiscoveryError::Unreachable(format!(
            "the authorization server at {servers} could not be reached"
        )))
    } else {
        Err(DiscoveryError::Failed(format!(
            "no protected-resource or authorization-server document with both endpoints was found for {origin}"
        )))
    }
}

/// A client registered dynamically (RFC 7591).
#[derive(Clone)]
pub struct Registered {
    pub client_id: String,
    /// Kept when the vendor issues one anyway.
    pub secret: Option<Zeroizing<String>>,
    pub auth_method: AuthMethod,
}

/// Why registration failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// The vendor refused: its RFC 7591 `error` and `error_description` as
    /// `model::vendor_text` shows them (G-6).
    Refused(String),
    Unreachable,
    EgressRefused(String),
    /// A success that is not one: no usable `client_id`.
    Invalid,
}

/// Register as a public client (gateway spec §4.2): `none` at the token
/// endpoint, `authorization_code` and `refresh_token`, the one redirect.
pub async fn register(
    client: &EgressClient,
    endpoint: &Url,
    redirect_uri: &str,
    scopes: &[String],
) -> Result<Registered, RegisterError> {
    if !is_https_or_loopback(endpoint) {
        return Err(RegisterError::EgressRefused(format!(
            "the registration endpoint at {} is not https",
            url_for_logs(endpoint.as_str())
        )));
    }
    let mut body = serde_json::json!({
        "client_name": "hennery",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    if !scopes.is_empty() {
        body["scope"] = Value::String(scopes.join(" "));
    }
    let mut request = Request::new(Method::POST, endpoint.clone());
    let headers = request.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    headers.insert(header::ACCEPT, header::HeaderValue::from_static("application/json"));
    *request.body_mut() = Some(body.to_string().into());
    *request.timeout_mut() = Some(REQUEST_TIMEOUT);
    let response = match client.send(request).await {
        Ok(response) => response,
        Err(EgressError::Refused(refused)) => {
            return Err(RegisterError::EgressRefused(format!(
                "{} is refused by the egress policy: {refused}",
                url_for_logs(endpoint.as_str())
            )));
        }
        Err(_) => return Err(RegisterError::Unreachable),
    };
    let status = response.status();
    if status.is_server_error() {
        return Err(RegisterError::Unreachable);
    }
    let bytes = match egress::read_capped(response, MAX_DOCUMENT).await {
        Ok(bytes) => bytes,
        Err(egress::BodyError::TooLarge) => return Err(RegisterError::Invalid),
        Err(egress::BodyError::Failed(_)) => return Err(RegisterError::Unreachable),
    };
    let document = trusted_object(&bytes, REGISTRATION_KEYS);
    if !status.is_success() {
        let said = document.as_ref().and_then(|d| d.get("error")).and_then(Value::as_str);
        let description = document
            .as_ref()
            .and_then(|d| d.get("error_description"))
            .and_then(Value::as_str);
        let text = match (said, description) {
            (Some(error), Some(description)) => format!("{error}: {description}"),
            (Some(error), None) => error.to_string(),
            (None, _) => format!("HTTP {}", status.as_u16()),
        };
        return Err(RegisterError::Refused(vendor_text(&text)));
    }
    let Some(document) = document else {
        return Err(RegisterError::Invalid);
    };
    let Some(client_id) = text(&document, "client_id").filter(|id| crate::model::client_id_problem(id).is_none())
    else {
        return Err(RegisterError::Invalid);
    };
    let secret = text(&document, "client_secret")
        .filter(|s| crate::model::client_secret_problem(s).is_none())
        .map(|s| Zeroizing::new(s.to_string()));
    let auth_method = match (&secret, text(&document, "token_endpoint_auth_method")) {
        (None, _) => AuthMethod::None,
        (Some(_), Some("client_secret_post")) => AuthMethod::Post,
        (Some(_), _) => AuthMethod::Basic,
    };
    Ok(Registered {
        client_id: client_id.to_string(),
        secret,
        auth_method,
    })
}

/// How a pre-registered client authenticates (RFC 6749 §2.3.1, RFC 8414):
/// `client_secret_basic`, the default, unless the server lists only
/// `client_secret_post`; a client without a secret is public.
pub fn auth_method_for(has_secret: bool, advertised: &[String]) -> AuthMethod {
    if !has_secret {
        return AuthMethod::None;
    }
    let basic = advertised.is_empty() || advertised.iter().any(|m| m == "client_secret_basic");
    let post = advertised.iter().any(|m| m == "client_secret_post");
    if !basic && post {
        AuthMethod::Post
    } else {
        AuthMethod::Basic
    }
}

/// A PKCE verifier (RFC 7636): 32 random bytes, base64url.
pub fn verifier() -> Zeroizing<String> {
    Zeroizing::new(URL_SAFE_NO_PAD.encode(Zeroizing::new(hennery_kernel::secret::random_bytes::<32>()).as_slice()))
}

/// Its S256 challenge.
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What the consent URL carries (gateway spec §4.3).
pub struct Consent<'a> {
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub challenge: &'a str,
    pub scopes: &'a [String],
    /// RFC 8707's `resource`: the connection URL, unless the server
    /// refused the parameter.
    pub resource: Option<&'a str>,
}

/// The consent URL: the authorization endpoint with the request's
/// parameters added to its own query. Never `response_mode=form_post`: the
/// flow cookie is `SameSite=Lax` (api-8e-8f R3).
pub fn consent_url(authorization_endpoint: &Url, consent: &Consent<'_>) -> Url {
    let mut url = authorization_endpoint.clone();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", consent.client_id)
            .append_pair("redirect_uri", consent.redirect_uri)
            .append_pair("state", consent.state)
            .append_pair("code_challenge", consent.challenge)
            .append_pair("code_challenge_method", "S256");
        if !consent.scopes.is_empty() {
            query.append_pair("scope", &consent.scopes.join(" "));
        }
        if let Some(resource) = consent.resource {
            query.append_pair("resource", resource);
        }
    }
    url
}

/// What the token endpoint is asked for.
pub enum Grant<'a> {
    /// The code a consent returned, with its PKCE verifier.
    Code {
        code: &'a str,
        redirect_uri: &'a str,
        verifier: &'a str,
    },
    /// A refresh, with the scopes the grant was asked for (G-10).
    Refresh {
        refresh_token: &'a str,
        scopes: &'a [String],
    },
}

/// Tokens the endpoint issued. Its `Debug` shows none.
pub struct Issued {
    pub access_token: Zeroizing<String>,
    pub refresh_token: Option<Zeroizing<String>>,
    pub expires_in: Option<i64>,
}

impl std::fmt::Debug for Issued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Issued")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// `now + expires_in − 60 s`; `None` when the server said nothing
/// (unknown, not expired: gateway spec §4.4).
pub fn expires_at(now: i64, expires_in: Option<i64>) -> Option<i64> {
    expires_in.map(|secs| now + (secs - EXPIRY_MARGIN).max(0))
}

/// Why the token endpoint gave nothing. None of them carries its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// The vendor refused (a 4xx): its RFC 6749 `error` when it is a named
    /// one, else only the status.
    Refused { error: Option<&'static str>, status: u16 },
    /// RFC 8707's `invalid_target`: the server does not take `resource`.
    InvalidTarget,
    /// A timeout, a transport failure, a 429 or a 5xx: not the vendor
    /// refusing the grant.
    Unavailable,
    /// The egress policy refused, or the endpoint is not `https`.
    EgressRefused(String),
    /// A 200 that is not a token response.
    Invalid,
}

impl TokenError {
    /// What the operator is shown (api-8e-8f B5 step 7).
    pub fn message(&self) -> String {
        match self {
            Self::Refused { error: Some(error), .. } => format!("the authorization server refused: {error}"),
            Self::Refused { error: None, status } => format!("the authorization server refused (HTTP {status})"),
            Self::InvalidTarget => "the authorization server refused: invalid_target".into(),
            Self::Unavailable => "the authorization server could not be reached".into(),
            Self::EgressRefused(why) => why.clone(),
            Self::Invalid => "the authorization server answered with no usable token".into(),
        }
    }
}

/// Ask `client`'s token endpoint for `grant` (gateway spec §4.3, §4.4).
/// The response is read into wiped memory and never logged or echoed.
pub async fn token(
    egress: &EgressClient,
    client: &TokenClient,
    grant: &Grant<'_>,
    resource: Option<&str>,
) -> Result<Issued, TokenError> {
    let endpoint = Url::parse(&client.token_endpoint).map_err(|_| TokenError::Invalid)?;
    if !is_https_or_loopback(&endpoint) {
        return Err(TokenError::EgressRefused(format!(
            "the token endpoint at {} is not https",
            url_for_logs(endpoint.as_str())
        )));
    }
    let (body, basic) = token_form(client, grant, resource)?;
    let mut request = Request::new(Method::POST, endpoint.clone());
    let headers = request.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers.insert(header::ACCEPT, header::HeaderValue::from_static("application/json"));
    if let Some(basic) = basic {
        headers.insert(header::AUTHORIZATION, basic);
    }
    *request.body_mut() = Some(body.as_bytes().to_vec().into());
    *request.timeout_mut() = Some(REQUEST_TIMEOUT);
    let response = match egress.send(request).await {
        Ok(response) => response,
        Err(EgressError::Refused(refused)) => {
            return Err(TokenError::EgressRefused(format!(
                "{} is refused by the egress policy: {refused}",
                url_for_logs(endpoint.as_str())
            )));
        }
        Err(_) => return Err(TokenError::Unavailable),
    };
    let status = response.status();
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        return Err(TokenError::Unavailable);
    }
    let bytes = match egress::read_capped(response, MAX_DOCUMENT).await {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(egress::BodyError::TooLarge) => return Err(TokenError::Invalid),
        Err(egress::BodyError::Failed(_)) => return Err(TokenError::Unavailable),
    };
    // Read once, into a map whose strings are wiped with it; never logged.
    let parsed = trusted_object(&bytes, TOKEN_KEYS);
    if !status.is_success() {
        let said = parsed.as_ref().and_then(|p| text(p, "error"));
        if said == Some("invalid_target") {
            return Err(TokenError::InvalidTarget);
        }
        let error = said.and_then(|said| NAMED_ERRORS.iter().copied().find(|named| *named == said));
        let refused = TokenError::Refused {
            error,
            status: status.as_u16(),
        };
        wipe(parsed);
        return Err(refused);
    }
    let Some(parsed) = parsed else {
        return Err(TokenError::Invalid);
    };
    let issued = (|| {
        // Absent `token_type` is taken as Bearer; any other is refused.
        let bearer = match parsed.get("token_type") {
            None => true,
            Some(value) => value.as_str().is_some_and(|t| t.eq_ignore_ascii_case("bearer")),
        };
        let access_token = text(&parsed, "access_token").filter(|t| crate::model::token_problem(t).is_none())?;
        let refresh_token = match parsed.get("refresh_token") {
            None | Some(Value::Null) => None,
            Some(_) => Some(text(&parsed, "refresh_token").filter(|t| crate::model::token_problem(t).is_none())?),
        };
        // A whole number of seconds, or a string of one; anything else is
        // unknown (not expired: gateway spec §4.4).
        let expires_in = match parsed.get("expires_in") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) if s.bytes().all(|b| b.is_ascii_digit()) && !s.is_empty() => s.parse().ok(),
            _ => None,
        }
        .filter(|secs| *secs >= 0);
        bearer.then(|| Issued {
            access_token: Zeroizing::new(access_token.to_string()),
            refresh_token: refresh_token.map(|t| Zeroizing::new(t.to_string())),
            expires_in,
        })
    })();
    wipe(Some(parsed));
    issued.ok_or(TokenError::Invalid)
}

/// The token request's form, and the client's `Authorization: Basic` when
/// it authenticates so (RFC 6749 §2.3.1: id and secret form-encoded first).
fn token_form(
    client: &TokenClient,
    grant: &Grant<'_>,
    resource: Option<&str>,
) -> Result<(Zeroizing<String>, Option<header::HeaderValue>), TokenError> {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    match grant {
        Grant::Code {
            code,
            redirect_uri,
            verifier,
        } => {
            form.append_pair("grant_type", "authorization_code")
                .append_pair("code", code)
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("code_verifier", verifier);
        }
        Grant::Refresh { refresh_token, scopes } => {
            form.append_pair("grant_type", "refresh_token")
                .append_pair("refresh_token", refresh_token);
            if !scopes.is_empty() {
                form.append_pair("scope", &scopes.join(" "));
            }
        }
    }
    if let Some(resource) = resource {
        form.append_pair("resource", resource);
    }
    let mut basic = None;
    match (client.auth_method, &client.secret) {
        (AuthMethod::Basic, Some(secret)) => {
            let encode = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
            let pair = Zeroizing::new(format!("{}:{}", encode(&client.client_id), encode(secret)));
            let mut value = header::HeaderValue::from_str(&format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(pair.as_bytes())
            ))
            .map_err(|_| TokenError::Invalid)?;
            value.set_sensitive(true);
            basic = Some(value);
        }
        (AuthMethod::Post, Some(secret)) => {
            form.append_pair("client_id", &client.client_id)
                .append_pair("client_secret", secret);
        }
        _ => {
            form.append_pair("client_id", &client.client_id);
        }
    }
    Ok((Zeroizing::new(form.finish()), basic))
}

/// Wipe the strings of a parsed token response before it is dropped.
fn wipe(object: Option<Map<String, Value>>) {
    use zeroize::Zeroize;
    for (_, value) in object.into_iter().flatten() {
        if let Value::String(mut text) = value {
            text.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    /// Gateway spec §4.1: RFC 8414 inserted before appended, then OpenID
    /// configuration.
    #[test]
    fn server_metadata_is_tried_inserted_then_appended_then_openid() {
        let found: Vec<String> = server_metadata_urls(&url("https://as.example/tenant/"))
            .iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(
            found,
            [
                "https://as.example/.well-known/oauth-authorization-server/tenant",
                "https://as.example/tenant/.well-known/oauth-authorization-server",
                "https://as.example/tenant/.well-known/openid-configuration",
                "https://as.example/.well-known/openid-configuration/tenant",
            ]
        );
        let root: Vec<String> = server_metadata_urls(&url("https://as.example"))
            .iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(
            root,
            [
                "https://as.example/.well-known/oauth-authorization-server",
                "https://as.example/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn the_resource_document_is_tried_path_inserted_then_at_the_origin() {
        let found: Vec<String> = resource_metadata_urls(&url("https://mcp.example/v1/mcp?x=1"))
            .iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(
            found,
            [
                "https://mcp.example/.well-known/oauth-protected-resource/v1/mcp",
                "https://mcp.example/.well-known/oauth-protected-resource",
            ]
        );
    }

    #[test]
    fn a_challenge_s_resource_metadata_is_read_quoted_or_not() {
        for (challenge, expected) in [
            (
                r#"Bearer realm="x", resource_metadata="https://m.example/.well-known/oauth-protected-resource""#,
                Some("https://m.example/.well-known/oauth-protected-resource"),
            ),
            (
                "Bearer resource_metadata=https://m.example/prm, error=\"invalid_token\"",
                Some("https://m.example/prm"),
            ),
            (
                r#"Bearer RESOURCE_METADATA="https://m.example/a\"b""#,
                Some("https://m.example/a\"b"),
            ),
            (r#"Bearer xresource_metadata="https://evil.example""#, None),
            ("Bearer realm=\"x\"", None),
        ] {
            assert_eq!(param_resource_metadata(challenge).as_deref(), expected, "{challenge}");
        }
    }

    #[test]
    fn the_s256_challenge_is_rfc_7636_s() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(verifier().len(), 43);
    }

    #[test]
    fn expiry_keeps_a_60_second_margin_and_unknown_stays_unknown() {
        assert_eq!(expires_at(1000, Some(3600)), Some(1000 + 3540));
        assert_eq!(expires_at(1000, Some(10)), Some(1000));
        assert_eq!(expires_at(1000, None), None);
    }

    #[test]
    fn a_secret_client_uses_basic_unless_only_post_is_offered() {
        assert_eq!(auth_method_for(false, &[]), AuthMethod::None);
        assert_eq!(auth_method_for(true, &[]), AuthMethod::Basic);
        assert_eq!(auth_method_for(true, &["client_secret_post".into()]), AuthMethod::Post);
        assert_eq!(
            auth_method_for(true, &["client_secret_post".into(), "client_secret_basic".into()]),
            AuthMethod::Basic
        );
    }

    /// O13: a token endpoint the policy refuses, or one at plain `http` to
    /// a name, is refused before anything is sent; neither is a vendor's
    /// refusal.
    #[tokio::test]
    async fn a_refused_token_endpoint_is_egress_refused() {
        let egress = hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap();
        let public = egress.client(hennery_kernel::egress::Allowance::PublicOnly);
        let internal = egress.client(hennery_kernel::egress::Allowance::InternalNetwork);
        let grant = Grant::Refresh {
            refresh_token: "r",
            scopes: &[],
        };
        for (client, endpoint) in [
            (&public, "https://127.0.0.1:1/token"),
            (&internal, "http://10.0.0.1/token"),
            (&internal, "http://as.example/token"),
        ] {
            let token_client = TokenClient {
                client_id: "c".into(),
                secret: None,
                auth_method: AuthMethod::None,
                token_endpoint: endpoint.into(),
            };
            let err = token(client, &token_client, &grant, None).await.unwrap_err();
            assert!(matches!(err, TokenError::EgressRefused(_)), "{endpoint}: {err:?}");
            assert!(!err.message().contains("/token"), "origins only: {}", err.message());
        }
    }

    /// O13: a registration endpoint that is not `https` (or loopback
    /// `http`) is refused before anything is sent, whatever discovery let
    /// through.
    #[tokio::test]
    async fn a_plain_http_registration_endpoint_is_egress_refused() {
        let egress = hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap();
        let internal = egress.client(hennery_kernel::egress::Allowance::InternalNetwork);
        let err = register(
            &internal,
            &url("http://as.example/register"),
            "https://h.example/cb",
            &[],
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, RegisterError::EgressRefused(_)), "{err:?}");
    }

    #[test]
    fn issuers_compare_but_for_one_final_slash() {
        assert!(same_issuer("https://as.example/", "https://as.example"));
        assert!(same_issuer("https://as.example/t", "https://as.example/t/"));
        assert!(!same_issuer("https://as.example/t", "https://as.example/u"));
        assert!(!same_issuer("", "https://as.example"));
    }
}
