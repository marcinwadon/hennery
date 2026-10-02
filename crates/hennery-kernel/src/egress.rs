//! The outbound HTTP policy (kernel spec §7.1; gateway spec §4, §5.7): every
//! request hennery itself sends to the network — the gateway's proxy, its
//! OAuth calls, Web Push — goes through an [`EgressClient`].
//!
//! - **Built once, cloned freely.** [`Egress::new`] builds its `reqwest`
//!   clients once: one per [`Allowance`], and a second, internal-only one
//!   for plain `http` under [`Allowance::InternalNetwork`]. [`Egress`] and
//!   [`EgressClient`] are cheap to clone (`reqwest::Client` is an `Arc`
//!   inside) and share connection pools with their clones. The two
//!   allowances never share a pool: a pooled
//!   connection skips the resolver, so a public-only request must never
//!   ride one an internal-network request opened.
//! - **No way around the check.** The only way to send is
//!   [`EgressClient::send`] or [`EgressClient::send_streaming`], which run
//!   [`check_url`] on the request's URL first. The `reqwest::Client` inside
//!   is never handed out. A caller holding an `http::Request` (rmcp's
//!   `OAuthHttpClient`, `web-push-native`) converts it with
//!   `Request::try_from` and sends it here.
//! - **The URL check** ([`check_url`]): `https`, or plain `http` to
//!   loopback (a loopback literal or `localhost`); under
//!   [`Allowance::InternalNetwork`] also plain `http` to an internal
//!   address or a name (plan 8b-ii, below). No credentials in the URL; an
//!   IP-literal host must be public under [`Allowance::PublicOnly`].
//!   reqwest never asks the resolver about a literal, so this check is the
//!   only one a literal gets. `url::Url` has
//!   already normalised the odd IPv4 spellings (`2130706433`, `0x7f.1`,
//!   `0177.0.0.1`, `127.1`) to the address they name.
//! - **Names are resolved by hennery** (the system resolver) and, under
//!   `PublicOnly`, refused when **any** address is not public — not filtered
//!   — so one record pointing inward refuses the name. The connection then
//!   goes only to the addresses that were checked. `localhost` is never
//!   looked up: it is `127.0.0.1` and `::1` (RFC 6761), so plain `http` to it
//!   stays on this machine whatever the resolver or hosts file says.
//! - **Public** ([`is_public`]): IPv4 outside the special-purpose ranges
//!   (this network and `0.0.0.0`, RFC 1918, CGNAT, loopback, link-local
//!   including `169.254.169.254`, IETF protocol assignments, the
//!   documentation and benchmarking ranges, 6to4 relay anycast, multicast,
//!   reserved and broadcast); IPv6 inside global unicast `2000::/3` and
//!   outside its special ranges (`2001::/23` with Teredo and benchmarking,
//!   documentation `2001:db8::/32` and `3fff::/20`, 6to4 `2002::/16`). So
//!   `::`, `::1`, IPv4-mapped `::ffff:0:0/96`, NAT64 `64:ff9b::/96` and
//!   `64:ff9b:1::/48`, unique-local `fc00::/7`, link-local `fe80::/10` and
//!   multicast `ff00::/8` are never public.
//! - **Plain `http` inside the internal network** (plan 8b-ii, the
//!   operator's decision of 2026-10-02): only under
//!   [`Allowance::InternalNetwork`], and only to internal addresses — RFC
//!   1918, loopback and unique-local IPv6. A public address stays `https`
//!   only even then, and so do the addresses that are neither: link-local
//!   (cloud metadata services answer plain `http` there), CGNAT (it may be
//!   the carrier's network), documentation, multicast, `0.0.0.0`,
//!   IPv4-mapped, NAT64, 6to4 and Teredo (they may lead to a public host). A
//!   literal is checked by the URL check; a name goes through a third
//!   client, used for nothing but plain `http` under `InternalNetwork`,
//!   whose resolver refuses the name if **any** address is not internal.
//! - **Never** a proxy from the environment (`HTTP_PROXY`, `HTTPS_PROXY`,
//!   `ALL_PROXY`): it would take the checked connection elsewhere and hand
//!   it the request. **Never** a followed redirect: a 3xx is the caller's
//!   answer.
//! - **Timeouts** ([`Timeouts`]): a connect timeout on every connection.
//!   [`EgressClient::send`] bounds the whole exchange, body included, by the
//!   request's own timeout or else [`Timeouts::request`].
//!   [`EgressClient::send_streaming`] bounds only the wait for the response
//!   head the same way; the caller reads the body for as long as it wants
//!   and decides for itself when a stream has been idle too long. A body
//!   that fails while it is read (`send`'s deadline included) fails with a
//!   `reqwest::Error` from [`Response`], not an [`EgressError`]: strip its
//!   URL with `without_url()` before logging it.
//! - **Errors** ([`EgressError`]) never carry the URL (its path or query can
//!   hold a secret), so they are safe to log.
//! - **Per-caller limits** ([`Limiter`]): a counter of permits per caller
//!   key (the gateway's is the connection), refusing past its cap with
//!   [`Busy`] instead of queueing. A caller keeps one for concurrent requests
//!   and one for open streams; the gateway answers [`Busy`] with 503.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
/// What a caller needs to build a request and read its answer, so it
/// depends on the kernel, not on reqwest.
pub use reqwest::{Method, Request, Response, StatusCode, Url, header};
use url::Host;

/// Whether a request may reach non-public addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allowance {
    /// Public addresses only: Web Push always, and every gateway connection
    /// the operator has not marked "internal network".
    PublicOnly,
    /// Any address over `https`, and internal addresses only over plain
    /// `http`: a gateway connection the operator marked "internal network"
    /// (gateway spec §5.7), and nothing else.
    InternalNetwork,
}

/// The policy's timeouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// Opening one TCP connection.
    pub connect: Duration,
    /// The default deadline of a request with none of its own: for
    /// [`EgressClient::send`] the whole exchange, for
    /// [`EgressClient::send_streaming`] the response head.
    pub request: Duration,
}

impl Timeouts {
    pub const DEFAULT: Timeouts = Timeouts {
        connect: Duration::from_secs(10),
        request: Duration::from_secs(30),
    };
}

/// The policy's clients: one per [`Allowance`], and the internal-network
/// one with a second, for plain `http`.
#[derive(Debug, Clone)]
pub struct Egress {
    public: EgressClient,
    internal: EgressClient,
}

impl Egress {
    pub fn new(timeouts: Timeouts) -> anyhow::Result<Egress> {
        Ok(Egress {
            public: EgressClient::build(Allowance::PublicOnly, timeouts)?,
            internal: EgressClient::build(Allowance::InternalNetwork, timeouts)?,
        })
    }

    /// The client for `allowance`.
    pub fn client(&self, allowance: Allowance) -> EgressClient {
        match allowance {
            Allowance::PublicOnly => self.public.clone(),
            Allowance::InternalNetwork => self.internal.clone(),
        }
    }
}

/// Sends requests under one [`Allowance`]; see the module docs.
#[derive(Debug, Clone)]
pub struct EgressClient {
    http: reqwest::Client,
    /// Plain `http` under [`Allowance::InternalNetwork`]: its resolver lets
    /// a name reach internal addresses only. `None` under `PublicOnly`.
    plain: Option<reqwest::Client>,
    allowance: Allowance,
    timeouts: Timeouts,
}

impl EgressClient {
    fn build(allowance: Allowance, timeouts: Timeouts) -> anyhow::Result<EgressClient> {
        EgressClient::build_with(allowance, timeouts, system_lookup)
    }

    /// [`EgressClient::build`] with the lookup a name goes to after the
    /// `localhost` rule: the system resolver, or a unit test's table. The
    /// checks on its answer are the same either way.
    fn build_with(allowance: Allowance, timeouts: Timeouts, system: SystemLookup) -> anyhow::Result<EgressClient> {
        let (reach, plain) = match allowance {
            Allowance::PublicOnly => (Reach::Public, None),
            Allowance::InternalNetwork => (Reach::Any, Some(http_client(Reach::Internal, timeouts, system)?)),
        };
        Ok(EgressClient {
            http: http_client(reach, timeouts, system)?,
            plain,
            allowance,
            timeouts,
        })
    }

    /// The client a request to `url` goes through: plain `http` under
    /// `InternalNetwork` through the internal-only one.
    fn for_url(&self, url: &Url) -> &reqwest::Client {
        match &self.plain {
            Some(plain) if url.scheme() == "http" => plain,
            _ => &self.http,
        }
    }

    pub fn allowance(&self) -> Allowance {
        self.allowance
    }

    /// Checks `request`'s URL and sends it; the whole exchange, body
    /// included, must finish within the request's timeout, or else
    /// [`Timeouts::request`].
    pub async fn send(&self, mut request: Request) -> Result<Response, EgressError> {
        check_url(request.url(), self.allowance)?;
        if request.timeout().is_none() {
            *request.timeout_mut() = Some(self.timeouts.request);
        }
        self.for_url(request.url()).execute(request).await.map_err(classify)
    }

    /// Checks `request`'s URL and sends it; the response head must arrive
    /// within the request's timeout, or else [`Timeouts::request`], and the
    /// body then has no deadline.
    pub async fn send_streaming(&self, mut request: Request) -> Result<Response, EgressError> {
        check_url(request.url(), self.allowance)?;
        let head = request.timeout_mut().take().unwrap_or(self.timeouts.request);
        let http = self.for_url(request.url());
        match tokio::time::timeout(head, http.execute(request)).await {
            Ok(sent) => sent.map_err(classify),
            Err(_) => Err(EgressError::Timeout),
        }
    }
}

fn http_client(reach: Reach, timeouts: Timeouts, system: SystemLookup) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("hennery/", env!("CARGO_PKG_VERSION")))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(CheckedResolver { reach, system }))
        .connect_timeout(timeouts.connect)
        .build()?)
}

/// Why the policy refused a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Not `https`, and not `http` to loopback (or, under
    /// [`Allowance::InternalNetwork`], to an internal address or a name).
    Scheme(String),
    /// The URL carries a user name or password.
    Credentials,
    /// The URL has no host.
    NoHost,
    /// An IP-literal host that is not public.
    Address(IpAddr),
    /// A name that resolved to an address the request may not reach: not
    /// public, or, for plain `http` under `InternalNetwork`, not internal.
    Resolved { host: String, addr: IpAddr },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Scheme(scheme) => write!(
                f,
                "{scheme:?} URLs are refused: https only, or http to loopback or, on an internal network, to an internal address or a name"
            ),
            Refused::Credentials => f.write_str("URLs with credentials are refused"),
            Refused::NoHost => f.write_str("the URL has no host"),
            Refused::Address(addr) => write!(f, "{addr} is not a public address"),
            Refused::Resolved { host, addr } => {
                write!(f, "{host} resolves to {addr}, which this request may not reach")
            }
        }
    }
}

impl std::error::Error for Refused {}

/// A request that did not get a response.
#[derive(Debug)]
pub enum EgressError {
    /// The policy refused it; nothing was sent.
    Refused(Refused),
    /// Its deadline passed.
    Timeout,
    /// Any other failure, without the URL.
    Http(reqwest::Error),
}

impl fmt::Display for EgressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EgressError::Refused(refused) => write!(f, "refused by the egress policy: {refused}"),
            EgressError::Timeout => f.write_str("the request timed out"),
            EgressError::Http(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for EgressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EgressError::Refused(refused) => Some(refused),
            EgressError::Timeout => None,
            EgressError::Http(err) => Some(err),
        }
    }
}

impl From<Refused> for EgressError {
    fn from(refused: Refused) -> EgressError {
        EgressError::Refused(refused)
    }
}

/// The resolver's refusal comes back wrapped by the connector; find it.
fn classify(err: reqwest::Error) -> EgressError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&err);
    while let Some(cause) = source {
        if let Some(refused) = cause.downcast_ref::<Refused>() {
            return EgressError::Refused(refused.clone());
        }
        source = cause.source();
    }
    if err.is_timeout() {
        EgressError::Timeout
    } else {
        EgressError::Http(err.without_url())
    }
}

/// The URL check every request passes before it is sent; see the module
/// docs. A name is checked later, by the resolver, once it has addresses.
pub fn check_url(url: &Url, allowance: Allowance) -> Result<(), Refused> {
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_host(url) => {}
        "http" if allowance == Allowance::InternalNetwork && may_be_internal(url) => {}
        other => return Err(Refused::Scheme(other.to_owned())),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Refused::Credentials);
    }
    let addr = match url.host() {
        None => return Err(Refused::NoHost),
        Some(Host::Domain(_)) => return Ok(()),
        Some(Host::Ipv4(addr)) => IpAddr::V4(addr),
        Some(Host::Ipv6(addr)) => IpAddr::V6(addr),
    };
    if allowance == Allowance::PublicOnly && !is_public(addr) {
        return Err(Refused::Address(addr));
    }
    Ok(())
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(name)) => is_localhost(name),
        Some(Host::Ipv4(addr)) => addr.is_loopback(),
        Some(Host::Ipv6(addr)) => addr.is_loopback(),
        None => false,
    }
}

/// Whether plain `http` under `InternalNetwork` may go to `url`'s host: an
/// internal literal, or a name, which the internal-only client's resolver
/// then checks.
fn may_be_internal(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(_)) => true,
        Some(Host::Ipv4(addr)) => is_internal(IpAddr::V4(addr)),
        Some(Host::Ipv6(addr)) => is_internal(IpAddr::V6(addr)),
        None => false,
    }
}

/// IPv4 ranges that are internal: plain `http` may reach them under
/// `InternalNetwork` (plan 8b-ii).
const V4_INTERNAL: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(10, 0, 0, 0), 8),     // RFC 1918
    (Ipv4Addr::new(127, 0, 0, 0), 8),    // loopback
    (Ipv4Addr::new(172, 16, 0, 0), 12),  // RFC 1918
    (Ipv4Addr::new(192, 168, 0, 0), 16), // RFC 1918
];

/// IPv6 ranges that are internal; no IPv4-mapped, NAT64, 6to4 or Teredo
/// address is, since any of them may lead to a public IPv4 host.
const V6_INTERNAL: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::LOCALHOST, 128),                      // loopback
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7), // unique-local
];

/// Whether `addr` is internal; see the module docs. Only an allowlist, so
/// everything else — public or neither — is not.
fn is_internal(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(addr) => V4_INTERNAL.iter().any(|&range| in_v4(addr, range)),
        IpAddr::V6(addr) => V6_INTERNAL.iter().any(|&range| in_v6(addr, range)),
    }
}

/// IPv4 ranges that are never public (the IANA special-purpose registry).
const V4_NOT_PUBLIC: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(0, 0, 0, 0), 8),       // "this network", 0.0.0.0
    (Ipv4Addr::new(10, 0, 0, 0), 8),      // RFC 1918
    (Ipv4Addr::new(100, 64, 0, 0), 10),   // CGNAT
    (Ipv4Addr::new(127, 0, 0, 0), 8),     // loopback
    (Ipv4Addr::new(169, 254, 0, 0), 16),  // link-local, 169.254.169.254
    (Ipv4Addr::new(172, 16, 0, 0), 12),   // RFC 1918
    (Ipv4Addr::new(192, 0, 0, 0), 24),    // IETF protocol assignments
    (Ipv4Addr::new(192, 0, 2, 0), 24),    // documentation (TEST-NET-1)
    (Ipv4Addr::new(192, 88, 99, 0), 24),  // 6to4 relay anycast
    (Ipv4Addr::new(192, 168, 0, 0), 16),  // RFC 1918
    (Ipv4Addr::new(198, 18, 0, 0), 15),   // benchmarking
    (Ipv4Addr::new(198, 51, 100, 0), 24), // documentation (TEST-NET-2)
    (Ipv4Addr::new(203, 0, 113, 0), 24),  // documentation (TEST-NET-3)
    (Ipv4Addr::new(224, 0, 0, 0), 4),     // multicast
    (Ipv4Addr::new(240, 0, 0, 0), 4),     // reserved, 255.255.255.255
];

/// Global unicast: the only IPv6 that can be public.
const V6_GLOBAL: (Ipv6Addr, u8) = (Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3);

/// The ranges inside global unicast that are not public.
const V6_NOT_PUBLIC: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23), // IETF protocol assignments: Teredo, benchmarking
    (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32), // documentation
    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16), // 6to4, which embeds any IPv4 address
    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20), // documentation
];

fn in_v4(addr: Ipv4Addr, (net, len): (Ipv4Addr, u8)) -> bool {
    let mask = u32::MAX.checked_shl(32 - u32::from(len)).unwrap_or(0);
    u32::from(addr) & mask == u32::from(net) & mask
}

fn in_v6(addr: Ipv6Addr, (net, len): (Ipv6Addr, u8)) -> bool {
    let mask = u128::MAX.checked_shl(128 - u32::from(len)).unwrap_or(0);
    u128::from(addr) & mask == u128::from(net) & mask
}

/// Whether `addr` is a public address; see the module docs.
pub fn is_public(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(addr) => !V4_NOT_PUBLIC.iter().any(|&range| in_v4(addr, range)),
        IpAddr::V6(addr) => in_v6(addr, V6_GLOBAL) && !V6_NOT_PUBLIC.iter().any(|&range| in_v6(addr, range)),
    }
}

/// Which addresses a client's resolver lets a name reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// Public only: every `PublicOnly` request.
    Public,
    /// Any: `https` under `InternalNetwork`.
    Any,
    /// Internal only: plain `http` under `InternalNetwork`.
    Internal,
}

/// The addresses a name resolved to, if `reach` permits all of them.
fn checked(host: &str, addrs: Vec<SocketAddr>, reach: Reach) -> Result<Vec<SocketAddr>, Refused> {
    let permitted = |addr: &SocketAddr| match reach {
        Reach::Public => is_public(addr.ip()),
        Reach::Any => true,
        Reach::Internal => is_internal(addr.ip()),
    };
    if let Some(addr) = addrs.iter().find(|addr| !permitted(addr)) {
        return Err(Refused::Resolved {
            host: host.to_owned(),
            addr: addr.ip(),
        });
    }
    Ok(addrs)
}

/// `localhost` (RFC 6761), with or without its trailing dot.
fn is_localhost(name: &str) -> bool {
    name.trim_end_matches('.').eq_ignore_ascii_case("localhost")
}

/// What `localhost` resolves to, without a lookup (RFC 6761 §6.3): no
/// resolver or hosts file can point it anywhere but loopback, which the
/// scheme rule's plain `http` to `localhost` relies on.
const LOCALHOST: [SocketAddr; 2] = [
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0),
];

/// Where a name other than `localhost` is looked up.
type SystemLookup = fn(String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>>;

/// The system resolver.
fn system_lookup(host: String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>> {
    Box::pin(async move { Ok(tokio::net::lookup_host((host.as_str(), 0)).await?.collect()) })
}

/// The addresses `host` resolves to: [`LOCALHOST`] for `localhost`, the
/// system resolver's answer for any other name.
async fn lookup(host: &str, system: SystemLookup) -> std::io::Result<Vec<SocketAddr>> {
    if is_localhost(host) {
        return Ok(LOCALHOST.to_vec());
    }
    system(host.to_owned()).await
}

/// Resolves names with [`lookup`] and applies [`checked`].
struct CheckedResolver {
    reach: Reach,
    system: SystemLookup,
}

impl Resolve for CheckedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let (reach, system) = (self.reach, self.system);
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs = lookup(&host, system).await?;
            let addrs = checked(&host, addrs, reach)?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// A caller is over its cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("too many requests in flight for this caller")
    }
}

impl std::error::Error for Busy {}

/// At most `cap` permits held at once per caller key. Clones share the
/// counts. Only keys holding a permit are tracked, so the table is bounded
/// by what is in flight.
#[derive(Debug, Clone)]
pub struct Limiter {
    cap: usize,
    in_use: Arc<Mutex<HashMap<String, usize>>>,
}

impl Limiter {
    pub fn new(cap: usize) -> Limiter {
        Limiter {
            cap,
            in_use: Arc::default(),
        }
    }

    /// A permit for `key`, released when dropped, or [`Busy`] at once if
    /// `key` already holds `cap`.
    pub fn try_acquire(&self, key: &str) -> Result<Permit, Busy> {
        let mut in_use = self.in_use.lock().unwrap_or_else(PoisonError::into_inner);
        let held = in_use.get(key).copied().unwrap_or(0);
        if held >= self.cap {
            return Err(Busy);
        }
        in_use.insert(key.to_owned(), held + 1);
        Ok(Permit {
            key: key.to_owned(),
            in_use: self.in_use.clone(),
        })
    }
}

/// One of a [`Limiter`]'s permits; dropping it releases it.
#[derive(Debug)]
#[must_use = "the permit is released when dropped"]
pub struct Permit {
    key: String,
    in_use: Arc<Mutex<HashMap<String, usize>>>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut in_use = self.in_use.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = in_use.get_mut(&self.key) {
            *held -= 1;
            if *held == 0 {
                in_use.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(url: &str) -> bool {
        let url = Url::parse(url).unwrap();
        let public = check_url(&url, Allowance::PublicOnly);
        assert_eq!(check_url(&url, Allowance::InternalNetwork), Ok(()), "{url}");
        matches!(public, Err(Refused::Address(_)))
    }

    /// Plan 8b (b): every non-public literal is refused under `PublicOnly`
    /// and allowed under `InternalNetwork`.
    #[test]
    fn non_public_literals_are_refused() {
        for url in [
            "https://0.0.0.0/",
            "https://0.1.2.3/",
            "https://10.0.0.1/",
            "https://100.64.0.1/",
            "https://100.127.255.254/",
            "https://127.0.0.1/",
            "https://127.255.255.254/",
            "https://169.254.169.254/",
            "https://172.16.0.1/",
            "https://172.31.255.254/",
            "https://192.0.0.1/",
            "https://192.0.2.1/",
            "https://192.88.99.1/",
            "https://192.168.1.1/",
            "https://198.18.0.1/",
            "https://198.19.255.254/",
            "https://198.51.100.1/",
            "https://203.0.113.1/",
            "https://224.0.0.1/",
            "https://239.255.255.250/",
            "https://240.0.0.1/",
            "https://255.255.255.255/",
            "https://[::]/",
            "https://[::1]/",
            "https://[::ffff:127.0.0.1]/",
            "https://[::ffff:10.0.0.1]/",
            "https://[::ffff:8.8.8.8]/",
            "https://[::127.0.0.1]/",
            "https://[64:ff9b::7f00:1]/",
            "https://[64:ff9b::808:808]/",
            "https://[64:ff9b:1::1]/",
            "https://[100::1]/",
            "https://[2001::1]/",
            "https://[2001:0:4136:e378:8000:63bf:3fff:fdd2]/",
            "https://[2001:2::1]/",
            "https://[2001:db8::1]/",
            "https://[2002:c0a8:101::1]/",
            "https://[2002:808:808::1]/",
            "https://[3fff::1]/",
            "https://[5f00::1]/",
            "https://[fc00::1]/",
            "https://[fd12:3456::1]/",
            "https://[fe80::1]/",
            "https://[fec0::1]/",
            "https://[ff02::1]/",
        ] {
            assert!(refused(url), "{url} was not refused");
        }
    }

    /// Plan 8b (b): `url::Url` normalises the odd IPv4 spellings to the
    /// address they name, so they are refused like the dotted form.
    #[test]
    fn odd_ipv4_spellings_are_refused_as_the_address_they_name() {
        for url in [
            "http://2130706433/",
            "http://0x7f000001/",
            "http://0x7f.1/",
            "http://0177.0.0.1/",
            "http://127.1/",
            "http://127.0.0.1./",
            "https://0/",
        ] {
            let parsed = Url::parse(url).unwrap();
            assert!(
                matches!(parsed.host(), Some(Host::Ipv4(_))),
                "{url} -> {:?}",
                parsed.host()
            );
            assert!(refused(url), "{url} was not refused");
        }
        assert_eq!(
            Url::parse("http://2130706433/").unwrap().host(),
            Some(Host::Ipv4(Ipv4Addr::LOCALHOST))
        );
    }

    /// Well-known public resolvers' addresses: only parsed and classified,
    /// never connected to. The documentation ranges cannot stand in for
    /// them, since they are rightly not public.
    #[test]
    fn public_literals_and_names_pass() {
        for url in [
            "https://8.8.8.8/",
            "https://1.1.1.1/",
            "https://100.63.255.255/",
            "https://100.128.0.0/",
            "https://172.15.255.255/",
            "https://172.32.0.0/",
            "https://198.17.255.255/",
            "https://198.20.0.0/",
            "https://223.255.255.255/",
            "https://[2606:4700:4700::1111]/",
            "https://[2001:200::1]/",
            "https://[2a00:1450:4001::1]/",
            "https://example.com/",
        ] {
            let parsed = Url::parse(url).unwrap();
            assert_eq!(check_url(&parsed, Allowance::PublicOnly), Ok(()), "{url}");
        }
    }

    /// Plan 8b (e): https, or http to loopback; under `PublicOnly`, plain
    /// http to anything else is refused by its scheme.
    #[test]
    fn plain_http_only_to_loopback() {
        for url in [
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://localhost/",
            "http://LOCALHOST./",
        ] {
            let parsed = Url::parse(url).unwrap();
            assert_eq!(check_url(&parsed, Allowance::InternalNetwork), Ok(()), "{url}");
            // Loopback passes the scheme rule under `PublicOnly` too (its
            // address is what refuses it there).
            assert!(
                !matches!(check_url(&parsed, Allowance::PublicOnly), Err(Refused::Scheme(_))),
                "{url}"
            );
        }
        for url in [
            "http://example.com/",
            "http://10.0.0.1/",
            "http://localhost.example.com/",
            "http://8.8.8.8/",
        ] {
            let parsed = Url::parse(url).unwrap();
            assert!(
                matches!(check_url(&parsed, Allowance::PublicOnly), Err(Refused::Scheme(_))),
                "{url}"
            );
        }
        for url in ["ws://localhost/", "ws://10.0.0.1/", "file:///etc/passwd"] {
            let parsed = Url::parse(url).unwrap();
            for allowance in [Allowance::PublicOnly, Allowance::InternalNetwork] {
                assert!(
                    matches!(check_url(&parsed, allowance), Err(Refused::Scheme(_))),
                    "{url} {allowance:?}"
                );
            }
        }
    }

    /// Plan 8b-ii: under `InternalNetwork`, plain `http` may go to an
    /// internal literal — every internal range, at its edges — or to a
    /// name (the internal-only client's resolver checks it). A public
    /// literal, or one that is neither public nor internal, stays `https`
    /// only. Under `PublicOnly` nothing changes.
    #[test]
    fn plain_http_under_internal_network_only_to_internal_addresses() {
        for url in [
            "http://10.0.0.1/",
            "http://10.255.255.254/",
            "http://127.0.0.2/",
            "http://127.255.255.254/",
            "http://172.16.0.1/",
            "http://172.31.255.254/",
            "http://192.168.0.1/",
            "http://192.168.255.254/",
            "http://[::1]/",
            "http://[fc00::1]/",
            "http://[fdff:ffff::1]/",
            "http://nas.lan/",
            "http://example.com/",
        ] {
            let parsed = Url::parse(url).unwrap();
            assert_eq!(check_url(&parsed, Allowance::InternalNetwork), Ok(()), "{url}");
            if !is_loopback_host(&parsed) {
                assert_eq!(
                    check_url(&parsed, Allowance::PublicOnly),
                    Err(Refused::Scheme("http".into())),
                    "{url}"
                );
            }
        }
        for url in [
            "http://8.8.8.8/",
            "http://9.255.255.255/",
            "http://11.0.0.0/",
            "http://126.255.255.255/",
            "http://128.0.0.0/",
            "http://100.64.0.1/",
            "http://100.100.100.200/",
            "http://169.254.169.254/",
            "http://169.254.0.1/",
            "http://172.15.255.255/",
            "http://172.32.0.0/",
            "http://192.167.255.255/",
            "http://192.169.0.0/",
            "http://0.0.0.0/",
            "http://192.0.2.1/",
            "http://198.18.0.1/",
            "http://224.0.0.1/",
            "http://255.255.255.255/",
            "http://[2606:4700:4700::1111]/",
            "http://[::]/",
            "http://[::2]/",
            "http://[fbff:ffff::1]/",
            "http://[fe00::]/",
            "http://[fe80::1]/",
            "http://[fec0::1]/",
            "http://[ff02::1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://[::ffff:8.8.8.8]/",
            "http://[64:ff9b::a00:1]/",
            "http://[2002:a00:1::1]/",
            "http://[2001:0:4136:e378:8000:63bf:3fff:fdd2]/",
        ] {
            let parsed = Url::parse(url).unwrap();
            for allowance in [Allowance::PublicOnly, Allowance::InternalNetwork] {
                assert_eq!(
                    check_url(&parsed, allowance),
                    Err(Refused::Scheme("http".into())),
                    "{url} {allowance:?}"
                );
            }
        }
        // Everything else a public-only request may not carry, it still may not.
        for (url, refusal) in [
            ("http://user@10.0.0.1/", Refused::Credentials),
            ("http://user:secret@nas.lan/", Refused::Credentials),
            ("ws://10.0.0.1/", Refused::Scheme("ws".into())),
            ("ftp://nas.lan/", Refused::Scheme("ftp".into())),
        ] {
            let parsed = Url::parse(url).unwrap();
            assert_eq!(check_url(&parsed, Allowance::InternalNetwork), Err(refusal), "{url}");
        }
    }

    /// Plan 8b-ii: the internal-only client's resolver refuses a name with
    /// **any** address that is not internal, public or not, in any order.
    #[test]
    fn a_name_for_plain_http_needs_every_address_internal() {
        let lan: SocketAddr = "192.168.1.10:0".parse().unwrap();
        let ula: SocketAddr = "[fd12:3456::1]:0".parse().unwrap();
        let public: SocketAddr = "93.184.215.14:0".parse().unwrap();
        let doc: SocketAddr = "192.0.2.1:0".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:10.0.0.1]:0".parse().unwrap();
        assert_eq!(checked("nas.lan", vec![lan, ula], Reach::Internal), Ok(vec![lan, ula]));
        for addrs in [
            vec![public],
            vec![lan, public],
            vec![public, lan],
            vec![lan, doc],
            vec![lan, mapped],
        ] {
            let Err(Refused::Resolved { host, addr }) = checked("nas.lan", addrs.clone(), Reach::Internal) else {
                panic!("{addrs:?} was not refused");
            };
            assert_eq!(host, "nas.lan");
            assert!(!is_internal(addr), "{addr}");
        }
    }

    /// A lookup table standing in for the system resolver.
    fn table(host: String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>> {
        Box::pin(async move {
            let addrs: &[&str] = match host.as_str() {
                "lan.test" => &["127.0.0.1:0"],
                "mixed.test" => &["127.0.0.1:0", "192.0.2.1:0"],
                "public.test" => &["192.0.2.1:0"],
                _ => return Err(std::io::Error::other("no such name")),
            };
            Ok(addrs.iter().map(|addr| addr.parse().unwrap()).collect())
        })
    }

    /// Plan 8b-ii: under `InternalNetwork`, plain `http` to a name goes
    /// through the internal-only client: a name whose addresses are all
    /// internal connects, one with any other address is refused and nothing
    /// connects; `https` does not go through it. (Names come from `table`,
    /// whose other address is a documentation one, never connected to; the
    /// listener is on loopback, which is internal.)
    #[tokio::test]
    async fn plain_http_to_a_name_reaches_only_internal_addresses() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let count = accepted.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                        .await;
                });
            }
        });
        let timeouts = Timeouts {
            connect: Duration::from_secs(2),
            request: Duration::from_secs(5),
        };
        let client = EgressClient::build_with(Allowance::InternalNetwork, timeouts, table).unwrap();
        let get = |host: &str| Request::new(Method::GET, Url::parse(&format!("http://{host}:{port}/x")).unwrap());
        let response = client.send(get("lan.test")).await.unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
        for host in ["mixed.test", "public.test"] {
            for streaming in [false, true] {
                let sent = if streaming {
                    client.send_streaming(get(host)).await
                } else {
                    client.send(get(host)).await
                };
                let Err(EgressError::Refused(Refused::Resolved { addr, .. })) = sent else {
                    panic!("{host}: {sent:?}");
                };
                assert_eq!(addr, "192.0.2.1".parse::<IpAddr>().unwrap());
            }
        }
        assert_eq!(accepted.load(Ordering::SeqCst), 1, "a refused name connected");
        // `https` under `InternalNetwork` keeps the client that reaches any
        // address: `mixed.test` connects (and fails the TLS handshake with
        // the plain listener), rather than being refused.
        let https = Request::new(
            Method::GET,
            Url::parse(&format!("https://mixed.test:{port}/x")).unwrap(),
        );
        let sent = client.send(https).await;
        assert!(matches!(sent, Err(EgressError::Http(_))), "{sent:?}");
        assert_eq!(accepted.load(Ordering::SeqCst), 2, "https did not connect");
    }

    /// Plan 8b (d): refuse, not filter: one inward record refuses the name.
    #[test]
    fn a_name_with_any_non_public_address_is_refused() {
        let public: SocketAddr = "93.184.215.14:0".parse().unwrap();
        let private: SocketAddr = "10.0.0.1:0".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:127.0.0.1]:0".parse().unwrap();
        assert_eq!(checked("example.com", vec![public], Reach::Public), Ok(vec![public]));
        for addrs in [vec![public, private], vec![private, public], vec![public, mapped]] {
            let Err(Refused::Resolved { host, addr }) = checked("example.com", addrs.clone(), Reach::Public) else {
                panic!("{addrs:?} was not refused");
            };
            assert_eq!(host, "example.com");
            assert!(!is_public(addr));
            assert_eq!(checked("example.com", addrs.clone(), Reach::Any), Ok(addrs));
        }
    }

    /// The review's B1: `localhost` is loopback by rule, not by whatever the
    /// system resolver answers, so plain `http` to it never leaves the
    /// machine.
    #[tokio::test]
    async fn localhost_is_loopback_without_a_lookup() {
        fn no_lookup(host: String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>> {
            panic!("{host} was looked up")
        }
        for reach in [Reach::Any, Reach::Internal, Reach::Public] {
            let resolver = CheckedResolver {
                reach,
                system: no_lookup,
            };
            for name in ["localhost", "localhost.", "LocalHost."] {
                match resolver.resolve(name.parse().unwrap()).await {
                    Ok(addrs) => {
                        assert_ne!(reach, Reach::Public, "{name}");
                        assert_eq!(addrs.collect::<Vec<_>>(), LOCALHOST.to_vec(), "{name}");
                    }
                    Err(err) => {
                        assert_eq!(reach, Reach::Public, "{name}: {err}");
                        assert!(err.downcast_ref::<Refused>().is_some(), "{name}: {err}");
                    }
                }
            }
        }
        assert!(!is_localhost("localhost.example.com"));
        assert!(!is_localhost("foo.localhost"));
        // And under `PublicOnly`, loopback is refused like any inward answer.
        let Err(Refused::Resolved { .. }) = checked("localhost", LOCALHOST.to_vec(), Reach::Public) else {
            panic!("localhost was not refused");
        };
    }

    /// The contract's types cross tasks: the proxy moves a `Permit` into a
    /// response body stream, and every caller shares its clients.
    const _: fn() = || {
        fn send_sync<T: Send + Sync + 'static>() {}
        send_sync::<Egress>();
        send_sync::<EgressClient>();
        send_sync::<Limiter>();
        send_sync::<Permit>();
        send_sync::<EgressError>();
    };

    #[test]
    fn a_limiter_refuses_past_its_cap_per_key_and_forgets_released_keys() {
        let limiter = Limiter::new(2);
        let a1 = limiter.try_acquire("a").unwrap();
        let a2 = limiter.try_acquire("a").unwrap();
        assert_eq!(limiter.try_acquire("a").unwrap_err(), Busy);
        // Another key has its own count; a clone shares them.
        let b1 = limiter.clone().try_acquire("b").unwrap();
        assert_eq!(limiter.clone().try_acquire("a").unwrap_err(), Busy);
        drop(a1);
        let a3 = limiter.try_acquire("a").unwrap();
        assert_eq!(limiter.try_acquire("a").unwrap_err(), Busy);
        drop((a2, a3, b1));
        assert!(limiter.in_use.lock().unwrap().is_empty(), "released keys are forgotten");
        // A refusal leaves nothing behind either.
        let none = Limiter::new(0);
        assert_eq!(none.try_acquire("a").unwrap_err(), Busy);
        assert!(none.in_use.lock().unwrap().is_empty());
    }
}
