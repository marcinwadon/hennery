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

    /// Whether this plan's store takes it: OAuth comes with plan 8f
    /// (plan 8a decision 10).
    pub fn is_supported(self) -> bool {
        matches!(self, Self::None | Self::Static)
    }
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
    /// A kind this plan does not take yet.
    Unsupported(CredKind),
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
