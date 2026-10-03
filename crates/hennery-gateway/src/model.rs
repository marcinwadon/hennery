//! A connection as the store keeps it (gateway spec §2), and the rules
//! its parts must meet before anything is written.

use url::Url;

/// The most connections one owner has (plan 8a decision 13).
pub const MAX_CONNECTIONS: usize = 256;

/// The longest URL accepted, in bytes.
pub const MAX_URL: usize = 2048;

/// The longest static token accepted, in bytes.
pub const MAX_TOKEN: usize = 8192;

/// The most tools an allowlist names.
pub const MAX_TOOLS: usize = 1024;

/// The most hosts one mounts request names, and the longest host id it
/// takes, both checked before anything is read.
pub const MAX_MOUNTS: usize = 1024;
pub const MAX_HOST_ID: usize = 64;

/// The header a static token goes in unless the connection names another
/// (maintainer decision 6d), and the prefix before it.
pub const DEFAULT_HEADER: &str = "Authorization";
pub const DEFAULT_PREFIX: &str = "Bearer ";

/// Headers a static token may not be sent in: the ones the proxy itself
/// sets, frames or filters (gateway spec §5.2), and the cookie.
const RESERVED_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "content-type",
    "content-encoding",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "trailer",
    "proxy-authorization",
    "proxy-connection",
    "cookie",
    "accept",
    "accept-encoding",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
    "expect",
    "forwarded",
    "via",
    "max-forwards",
];

/// What of an upstream URL may be shown in a log line, an error or a
/// `Debug` (plan 8a decision 19): `scheme://host[:port]`, never its path or
/// query, which some vendors put a secret in.
pub fn url_for_logs(url: &str) -> String {
    match Url::parse(url) {
        Ok(url) => url.origin().ascii_serialization(),
        Err(_) => "<not a url>".into(),
    }
}

/// How a connection authenticates to its upstream (gateway spec §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredKind {
    None,
    Static,
    OauthDcr,
    OauthClient,
}

impl CredKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Static => "static",
            Self::OauthDcr => "oauth_dcr",
            Self::OauthClient => "oauth_client",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "none" => Self::None,
            "static" => Self::Static,
            "oauth_dcr" => Self::OauthDcr,
            "oauth_client" => Self::OauthClient,
            _ => return None,
        })
    }

    /// An OAuth kind (gateway spec §4): its credential is a grant, and it
    /// has an OAuth client.
    pub fn is_oauth(self) -> bool {
        matches!(self, Self::OauthDcr | Self::OauthClient)
    }
}

/// A connection's health (gateway spec §7), as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NotConnected,
    Ok,
    NeedsAuth,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotConnected => "not_connected",
            Self::Ok => "ok",
            Self::NeedsAuth => "needs_auth",
            Self::Error => "error",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "not_connected" => Self::NotConnected,
            "ok" => Self::Ok,
            "needs_auth" => Self::NeedsAuth,
            "error" => Self::Error,
            _ => return None,
        })
    }

    /// A problem the operator is told about (gateway spec §7).
    pub fn is_problem(self) -> bool {
        matches!(self, Self::NeedsAuth | Self::Error)
    }
}

/// A connection's status moved from `from` to `to` (gateway spec §7): what
/// the `Notifier` hears of, and only of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusChange {
    pub connection_id: String,
    pub hat_id: String,
    pub label: String,
    pub from: Status,
    pub to: Status,
}

/// The most characters of vendor text the operator is shown (gateway
/// spec §4.2, §4.3).
pub const MAX_VENDOR_TEXT: usize = 300;

/// Vendor text as the operator is shown it (plan 8f; api-8e-8f S4): no
/// control or bidirectional formatting character, cut to
/// `MAX_VENDOR_TEXT` characters, after "The vendor said: ". Rendered as
/// text, never as markup.
pub fn vendor_text(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .take(MAX_VENDOR_TEXT)
        .collect();
    format!("The vendor said: {}", kept.trim())
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// A client id the operator registered at a vendor: 1 to 512 visible
/// ASCII characters (api-8e-8f B2).
pub fn client_id_problem(client_id: &str) -> Option<String> {
    let ok = (1..=512).contains(&client_id.len()) && client_id.bytes().all(|b| (0x21..=0x7e).contains(&b));
    (!ok).then(|| "a client id is 1 to 512 visible ASCII characters".into())
}

/// A client secret: 1 to 8192 visible ASCII characters, without spaces,
/// as a static token (api-8e-8f B2).
pub fn client_secret_problem(secret: &str) -> Option<String> {
    token_problem(secret)
        .map(|_| format!("a client secret is 1 to {MAX_TOKEN} visible ASCII characters, without spaces"))
}

/// A connection's OAuth client, as the list shows it: never a secret,
/// only whether one is stored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OauthClientView {
    /// The client in use; `None` before an `oauth_dcr`'s first Connect or
    /// an `oauth_client`'s first `PUT …/oauth-client`.
    pub client_id: Option<String>,
    pub has_client_secret: bool,
    /// A pre-registered client saved while a grant is live (G-7): its id,
    /// and whether it has a secret.
    pub pending: Option<(String, bool)>,
}

/// Why the latest Connect did not complete (api-8e-8f B3, B5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthError {
    pub code: String,
    pub message: String,
    pub at: i64,
}

/// One stored connection, without its secret: whether it has one is all
/// that is read of `gw_credentials` (plan 8a decision 3).
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionRecord {
    pub id: String,
    pub slug: String,
    pub label: String,
    /// As parsed and serialised (`url::Url`).
    pub url: String,
    pub hat_id: String,
    pub cred_kind: CredKind,
    pub static_header: String,
    pub static_prefix: String,
    /// `None`: every tool.
    pub tool_allowlist: Option<Vec<String>>,
    pub internal_network: bool,
    /// `not_connected`, `ok`, `needs_auth` or `error` (gateway spec §7).
    pub status: String,
    pub status_note: Option<String>,
    pub account_label: Option<String>,
    pub status_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub has_credential: bool,
    /// The hosts it is mounted on that are not revoked, by id.
    pub mounts: Vec<String>,
    /// When the gateway last learned its status (plan 8f).
    pub checked_at: Option<i64>,
    /// For an OAuth kind: its client (plan 8f).
    pub oauth_client: Option<OauthClientView>,
    /// The protected-resource document's `resource`, when the latest
    /// authorize found one that is not the URL, and the one the operator
    /// accepted (gateway spec §4.1, G-4). Never in a `Debug` (lane L11).
    pub resource_mismatch: Option<String>,
    pub accepted_resource: Option<String>,
    pub oauth_error: Option<OauthError>,
}

/// A connection to create. `None` header or prefix: the default.
#[derive(Clone, PartialEq, Eq)]
pub struct NewConnection {
    pub slug: String,
    pub label: String,
    pub url: String,
    pub hat_id: String,
    pub cred_kind: CredKind,
    pub static_header: Option<String>,
    pub static_prefix: Option<String>,
    pub tool_allowlist: Option<Vec<String>>,
    pub internal_network: bool,
}

/// A change to a connection: a field left `None` keeps its value. The
/// allowlist is `Some(None)` to clear it (every tool) and `Some(Some(…))`
/// to set it (gateway spec §4.6). A connection's slug and hat never change
/// (plan 8a decision 5).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ConnectionPatch {
    pub label: Option<String>,
    pub url: Option<String>,
    pub cred_kind: Option<CredKind>,
    pub static_header: Option<String>,
    pub static_prefix: Option<String>,
    pub tool_allowlist: Option<Option<Vec<String>>>,
    pub internal_network: Option<bool>,
}

// `Debug` by hand: the URL shows only its origin (decision 19).
impl std::fmt::Debug for ConnectionRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionRecord")
            .field("id", &self.id)
            .field("slug", &self.slug)
            .field("label", &self.label)
            .field("url", &url_for_logs(&self.url))
            .field("hat_id", &self.hat_id)
            .field("cred_kind", &self.cred_kind)
            .field("static_header", &self.static_header)
            .field("static_prefix", &self.static_prefix)
            .field("tool_allowlist", &self.tool_allowlist)
            .field("internal_network", &self.internal_network)
            .field("status", &self.status)
            .field("has_credential", &self.has_credential)
            .field("mounts", &self.mounts)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for NewConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewConnection")
            .field("slug", &self.slug)
            .field("label", &self.label)
            .field("url", &url_for_logs(&self.url))
            .field("hat_id", &self.hat_id)
            .field("cred_kind", &self.cred_kind)
            .field("static_header", &self.static_header)
            .field("static_prefix", &self.static_prefix)
            .field("tool_allowlist", &self.tool_allowlist)
            .field("internal_network", &self.internal_network)
            .finish()
    }
}

impl std::fmt::Debug for ConnectionPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionPatch")
            .field("label", &self.label)
            .field("url", &self.url.as_deref().map(url_for_logs))
            .field("cred_kind", &self.cred_kind)
            .field("static_header", &self.static_header)
            .field("static_prefix", &self.static_prefix)
            .field("tool_allowlist", &self.tool_allowlist)
            .field("internal_network", &self.internal_network)
            .finish()
    }
}

/// A `static` connection's token with where it goes, read in one statement
/// (the review's R2): the proxy (plan 8d) must take all four from here, so
/// an edit that moves the URL cannot land between reading the token and
/// reading where to send it. Its `Debug` shows neither the token nor more
/// of the URL than its origin.
#[derive(Clone, PartialEq, Eq)]
pub struct StaticCredential {
    pub token: zeroize::Zeroizing<String>,
    pub url: String,
    pub static_header: String,
    pub static_prefix: String,
    pub internal_network: bool,
}

impl std::fmt::Debug for StaticCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticCredential")
            .field("token", &"<redacted>")
            .field("url", &url_for_logs(&self.url))
            .field("static_header", &self.static_header)
            .field("static_prefix", &self.static_prefix)
            .field("internal_network", &self.internal_network)
            .finish()
    }
}

/// The outcome of a change to a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Done(Box<ConnectionRecord>),
    NotFound,
    /// Another connection of the owner's has this slug.
    SlugTaken,
    /// The owner has `MAX_CONNECTIONS` already.
    TooMany,
    /// Why it was refused; nothing was written.
    Invalid(String),
}

/// The outcome of setting a static credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialChange {
    Done,
    NotFound,
    /// The connection's kind takes no static token.
    WrongKind(CredKind),
    Invalid(String),
}

/// `^[a-z0-9][a-z0-9-]{0,47}$` (gateway spec §2).
pub fn slug_problem(slug: &str) -> Option<String> {
    let bytes = slug.as_bytes();
    let ok = (1..=48).contains(&bytes.len())
        && bytes[0] != b'-'
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    (!ok).then(|| "a slug is 1 to 48 of a-z, 0-9 and -, not starting with -".into())
}

/// 1 to 64 bytes once trimmed, nothing invisible or controlling.
pub fn label_problem(label: &str) -> Option<String> {
    let label = label.trim();
    let ok = !label.is_empty() && hennery_kernel::hosts::is_displayable_text(label, 64);
    (!ok).then(|| "a label is 1 to 64 printable characters".into())
}

/// The upstream URL, parsed: `http` or `https`, absolute, without a user
/// name, password or fragment; `http` only to an upstream marked internal
/// (plan 8a decision 9). The URL is listed and logged: secrets go in the
/// credential.
pub fn parse_url(input: &str, internal_network: bool) -> Result<Url, String> {
    if input.len() > MAX_URL {
        return Err(format!("a url is at most {MAX_URL} bytes"));
    }
    let url = Url::parse(input).map_err(|_| "the url is not an absolute http or https URL".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if internal_network => {}
        "http" => return Err("an http url needs internal_network: the token would cross the network in clear".into()),
        _ => return Err("the url is not an absolute http or https URL".into()),
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("the url names no host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the url may not hold a user name or password: set a credential".into());
    }
    if url.fragment().is_some() {
        return Err("the url may not have a fragment".into());
    }
    if url.as_str().len() > MAX_URL {
        return Err(format!("a url is at most {MAX_URL} bytes"));
    }
    Ok(url)
}

/// A header name the proxy can send a token in.
pub fn header_problem(name: &str) -> Option<String> {
    if name.is_empty() || name.len() > 64 || axum::http::HeaderName::from_bytes(name.as_bytes()).is_err() {
        return Some("the static header is not a header name".into());
    }
    let lower = name.to_ascii_lowercase();
    if RESERVED_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-") || lower.starts_with("sec-") {
        return Some(format!("the static header may not be {name}"));
    }
    None
}

/// At most 32 visible ASCII characters or spaces.
pub fn prefix_problem(prefix: &str) -> Option<String> {
    let ok = prefix.len() <= 32 && prefix.bytes().all(|b| (0x20..=0x7e).contains(&b));
    (!ok).then(|| "the static prefix is at most 32 visible ASCII characters or spaces".into())
}

/// 1 to `MAX_TOKEN` visible ASCII characters: no space, nothing a header
/// could break on.
pub fn token_problem(token: &str) -> Option<String> {
    let ok = (1..=MAX_TOKEN).contains(&token.len()) && token.bytes().all(|b| (0x21..=0x7e).contains(&b));
    (!ok).then(|| format!("a token is 1 to {MAX_TOKEN} visible ASCII characters, without spaces"))
}

/// The allowlist without duplicates, in its order, if each tool is 1 to
/// 128 visible ASCII characters and there are at most `MAX_TOOLS`.
pub fn tools(list: &[String]) -> Result<Vec<String>, String> {
    if list.len() > MAX_TOOLS {
        return Err(format!("a tool allowlist names at most {MAX_TOOLS} tools"));
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for tool in list {
        if !(1..=128).contains(&tool.len()) || !tool.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            return Err("a tool name is 1 to 128 visible ASCII characters".into());
        }
        if !out.contains(tool) {
            out.push(tool.clone());
        }
    }
    Ok(out)
}

/// How an OAuth client authenticates at the token endpoint (RFC 6749
/// §2.3, RFC 8414 `token_endpoint_auth_methods_supported`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// A public client: its `client_id` in the form, no secret.
    None,
    /// `Authorization: Basic`, the id and secret form-encoded first.
    Basic,
    /// `client_id` and `client_secret` in the form.
    Post,
}

impl AuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Basic => "client_secret_basic",
            Self::Post => "client_secret_post",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "none" => Self::None,
            "client_secret_basic" => Self::Basic,
            "client_secret_post" => Self::Post,
            _ => return None,
        })
    }
}

/// An OAuth grant's tokens, sealed together as one JSON object
/// (`crypto::OAUTH_TOKENS`). Wiped when dropped; its `Debug` shows neither.
#[derive(Clone, PartialEq, Eq)]
pub struct GrantTokens {
    pub access_token: zeroize::Zeroizing<String>,
    /// `None`: the vendor gave none, so the grant cannot be refreshed.
    pub refresh_token: Option<zeroize::Zeroizing<String>>,
}

impl std::fmt::Debug for GrantTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrantTokens")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl GrantTokens {
    /// The plaintext that is sealed, built in a buffer that is wiped.
    pub fn to_sealable(&self) -> zeroize::Zeroizing<Vec<u8>> {
        #[derive(serde::Serialize)]
        struct Out<'a> {
            access_token: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            refresh_token: Option<&'a str>,
        }
        let mut out = zeroize::Zeroizing::new(Vec::with_capacity(
            64 + self.access_token.len() + self.refresh_token.as_ref().map_or(0, |t| t.len()),
        ));
        serde_json::to_writer(
            &mut *out,
            &Out {
                access_token: &self.access_token,
                refresh_token: self.refresh_token.as_ref().map(|t| t.as_str()),
            },
        )
        .expect("two strings serialise");
        out
    }

    /// What `to_sealable` made, opened.
    pub fn from_opened(bytes: &[u8]) -> Option<Self> {
        #[derive(serde::Deserialize)]
        struct In {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
        }
        let parsed: In = serde_json::from_slice(bytes).ok()?;
        Some(Self {
            access_token: zeroize::Zeroizing::new(parsed.access_token),
            refresh_token: parsed.refresh_token.map(zeroize::Zeroizing::new),
        })
    }
}

/// The client a grant is refreshed with, at its token endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenClient {
    pub client_id: String,
    pub secret: Option<zeroize::Zeroizing<String>>,
    pub auth_method: AuthMethod,
    /// `https` (or loopback `http`), checked when it was discovered.
    pub token_endpoint: String,
}

impl std::fmt::Debug for TokenClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenClient")
            .field("client_id", &self.client_id)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("auth_method", &self.auth_method)
            .field("token_endpoint", &url_for_logs(&self.token_endpoint))
            .finish()
    }
}

/// An OAuth connection's grant with everything a request or a refresh
/// needs, read in one statement (as `StaticCredential`, plan 8a's R2): the
/// URL it goes to, the allowance, the client and where to refresh it.
#[derive(Clone)]
pub struct OauthCredential {
    pub tokens: GrantTokens,
    /// `now + expires_in − 60 s` (gateway spec §4.4); `None`: unknown.
    pub expires_at: Option<i64>,
    pub url: String,
    pub internal_network: bool,
    pub cred_kind: CredKind,
    pub client: TokenClient,
    /// The scopes the grant was asked for, sent again on refresh (G-10).
    pub scopes: Vec<String>,
    /// The `resource` the grant was made for, sent on refresh (RFC 8707)
    /// unless the authorization server refused it.
    pub resource: Option<String>,
    pub resource_param_accepted: bool,
    /// The sealed blob as read: a refresh stores its result only while the
    /// row still holds it (a compare-and-swap; plan 8f decision 9).
    pub(crate) sealed: Vec<u8>,
}

impl std::fmt::Debug for OauthCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OauthCredential")
            .field("tokens", &self.tokens)
            .field("expires_at", &self.expires_at)
            .field("url", &url_for_logs(&self.url))
            .field("internal_network", &self.internal_network)
            .field("cred_kind", &self.cred_kind)
            .field("client", &self.client)
            .field("scopes", &self.scopes)
            .field("resource_param_accepted", &self.resource_param_accepted)
            .finish_non_exhaustive()
    }
}
