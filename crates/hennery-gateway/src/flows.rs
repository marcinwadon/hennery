//! OAuth flows in flight (gateway spec §4.3; api-8e-8f F5, B5): in memory,
//! keyed by `state`, single use, for `FLOW_TTL` seconds. The callback uses
//! only what the flow snapshotted when it started (G-8), never storage
//! re-read.
//!
//! - **One flow per connection**: a new authorize supersedes the previous
//!   one, whose callback then finds no flow.
//! - **At most `MAX_FLOWS`** live flows per owner (one owner per
//!   installation in v1).
//! - **Edits drop flows** (the review's R1): a URL or kind change, a client
//!   `PUT` or a delete; the callback re-checks the connection besides.
//! - **The flow cookie** holds 256 random bits, and its name is derived from
//!   `state` (`hennery_mcp_flow_<16 hex of sha256(state)>`): a consent
//!   completed in another browser has no cookie to match (the review's R3).
//! - **A dynamically registered client** is held by the flow (S3) until its
//!   callback stores it, and the latest unconsumed one is reused by the
//!   next authorize of the same connection, so repeated clicks do not
//!   register again.

use crate::model::{AuthMethod, CredKind, TokenClient};
use crate::store::ClientSource;
use hennery_kernel::secret::{random_bytes, sha256_hex};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use zeroize::Zeroizing;

/// How long a flow lives, in seconds, and its cookie's `Max-Age` (gateway
/// spec §4.3: 15 minutes).
pub const FLOW_TTL: i64 = 15 * 60;

/// The most live flows (api-8e-8f F5).
pub const MAX_FLOWS: usize = 16;

/// What every flow cookie's name starts with.
pub const FLOW_COOKIE_PREFIX: &str = "hennery_mcp_flow_";

/// At most this many same-name flow cookies of a request are tried, as
/// `operator::MAX_SESSION_COOKIES` for the session's.
pub const MAX_FLOW_COOKIES: usize = hennery_kernel::operator::MAX_SESSION_COOKIES;

/// The cookie's name for `state`.
pub fn cookie_name(state: &str) -> String {
    format!("{FLOW_COOKIE_PREFIX}{}", &sha256_hex(state.as_bytes())[..16])
}

/// What a flow fixed when it started (G-8). Its `Debug` shows no secret.
#[derive(Clone)]
pub struct Snapshot {
    pub connection_id: String,
    pub url: String,
    pub cred_kind: CredKind,
    pub internal_network: bool,
    pub source: ClientSource,
    pub client: TokenClient,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub iss_parameter: bool,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    /// `resource`, sent on consent unless the server refused it before.
    pub resource: String,
    pub resource_param: bool,
    pub verifier: Zeroizing<String>,
    pub registered_at: i64,
    /// The operator's session that started it (`Authenticated::session_id`).
    pub auth_session: String,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("connection_id", &self.connection_id)
            .field("cred_kind", &self.cred_kind)
            .field("source", &self.source)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

struct Flow {
    cookie: Zeroizing<String>,
    expires_at: i64,
    snapshot: Snapshot,
}

/// A dynamically registered client not yet stored with a grant (S3).
#[derive(Clone)]
pub struct Registration {
    pub client: TokenClient,
    pub redirect_uri: String,
    pub registered_at: i64,
}

#[derive(Default)]
struct Table {
    flows: HashMap<String, Flow>,
    registrations: HashMap<String, Registration>,
    /// (connection, url) pairs whose authorization server refused
    /// `resource` at consent: the next authorize omits it (plan 8f
    /// decision 11).
    resource_refused: HashSet<(String, String)>,
}

/// The live flows, shared by authorize and the callback.
#[derive(Default)]
pub struct Flows {
    table: Mutex<Table>,
}

/// A started flow: its `state`, its cookie's value and when it expires.
pub struct Started {
    pub state: String,
    pub cookie: Zeroizing<String>,
    pub expires_at: i64,
}

/// What the callback found for a `state`.
pub enum Taken {
    /// No flow, or one past its time.
    Unknown,
    /// The flow, consumed, but its cookie did not match.
    Mismatch(Box<Snapshot>),
    /// The flow, consumed: its snapshot.
    Taken(Box<Snapshot>),
}

impl Flows {
    fn table(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// How many flows are live now, `except` a connection's (its flow is
    /// superseded by the next).
    pub fn live(&self, except: &str, now: i64) -> usize {
        let mut table = self.table();
        table.flows.retain(|_, flow| flow.expires_at > now);
        table
            .flows
            .values()
            .filter(|flow| flow.snapshot.connection_id != except)
            .count()
    }

    /// Start a flow for `snapshot`, superseding the connection's previous
    /// one. `None`: `MAX_FLOWS` are live.
    pub fn start(&self, snapshot: Snapshot, now: i64) -> Option<Started> {
        let mut table = self.table();
        table.flows.retain(|_, flow| flow.expires_at > now);
        table
            .flows
            .retain(|_, flow| flow.snapshot.connection_id != snapshot.connection_id);
        if table.flows.len() >= MAX_FLOWS {
            return None;
        }
        let state = hex::encode(random_bytes::<32>());
        let cookie = Zeroizing::new(hex::encode(Zeroizing::new(random_bytes::<32>()).as_slice()));
        let expires_at = now + FLOW_TTL;
        table.flows.insert(
            state.clone(),
            Flow {
                cookie: cookie.clone(),
                expires_at,
                snapshot,
            },
        );
        Some(Started {
            state,
            cookie,
            expires_at,
        })
    }

    /// Take the flow of `state` (single use), if `cookies` holds its
    /// cookie: the callback's first check, in memory, before any storage.
    /// A flow is consumed whether or not the cookie matched; one past its
    /// time is unknown.
    pub fn take(&self, state: &str, cookies: &[&str], now: i64) -> Taken {
        let Some(flow) = self.table().flows.remove(state) else {
            return Taken::Unknown;
        };
        if flow.expires_at <= now {
            return Taken::Unknown;
        }
        let matches = cookies
            .iter()
            .take(MAX_FLOW_COOKIES)
            .any(|value| constant_time_eq(value.as_bytes(), flow.cookie.as_bytes()));
        if matches {
            Taken::Taken(Box::new(flow.snapshot))
        } else {
            Taken::Mismatch(Box::new(flow.snapshot))
        }
    }

    /// Drop every live flow of `connection_id` and its held registration
    /// (the review's R1): after a URL or kind edit, a client `PUT` or a
    /// delete.
    pub fn drop_connection(&self, connection_id: &str) {
        let mut table = self.table();
        table
            .flows
            .retain(|_, flow| flow.snapshot.connection_id != connection_id);
        table.registrations.remove(connection_id);
        table.resource_refused.retain(|(id, _)| id != connection_id);
    }

    /// The registration an authorize may reuse: the latest unconsumed one
    /// of the connection, for the same token endpoint and redirect URI.
    pub fn registration(&self, connection_id: &str, token_endpoint: &str, redirect_uri: &str) -> Option<Registration> {
        self.table()
            .registrations
            .get(connection_id)
            .filter(|r| r.client.token_endpoint == token_endpoint && r.redirect_uri == redirect_uri)
            .cloned()
    }

    pub fn hold_registration(&self, connection_id: &str, registration: Registration) {
        self.table()
            .registrations
            .insert(connection_id.to_string(), registration);
    }

    /// The authorization server refused `resource` at consent for this
    /// connection's `url`: the next authorize omits it.
    pub fn refuse_resource(&self, connection_id: &str, url: &str) {
        self.table()
            .resource_refused
            .insert((connection_id.to_string(), url.to_string()));
    }

    pub fn resource_refused(&self, connection_id: &str, url: &str) -> bool {
        self.table()
            .resource_refused
            .contains(&(connection_id.to_string(), url.to_string()))
    }
}

/// Compare without stopping at the first difference.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A dynamically registered client as a token client (S3).
pub fn registered_client(
    client_id: String,
    secret: Option<Zeroizing<String>>,
    auth_method: AuthMethod,
    token_endpoint: String,
) -> TokenClient {
    TokenClient {
        client_id,
        secret,
        auth_method,
        token_endpoint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(connection_id: &str) -> Snapshot {
        Snapshot {
            connection_id: connection_id.into(),
            url: "https://mcp.example/mcp".into(),
            cred_kind: CredKind::OauthDcr,
            internal_network: false,
            source: ClientSource::Registered,
            client: registered_client("c".into(), None, AuthMethod::None, "https://as.example/token".into()),
            issuer: "https://as.example".into(),
            authorization_endpoint: "https://as.example/authorize".into(),
            iss_parameter: false,
            redirect_uri: "https://h.example/api/mcp/oauth/callback".into(),
            scopes: Vec::new(),
            resource: "https://mcp.example/mcp".into(),
            resource_param: true,
            verifier: Zeroizing::new("v".into()),
            registered_at: 0,
            auth_session: "s".into(),
        }
    }

    #[test]
    fn a_flow_is_single_use_and_needs_its_cookie() {
        let flows = Flows::default();
        let started = flows.start(snapshot("conn-a"), 100).unwrap();
        assert!(matches!(
            flows.take(&started.state, &["wrong", started.cookie.as_str()], 101),
            Taken::Taken(_)
        ));
        assert!(matches!(
            flows.take(&started.state, &[started.cookie.as_str()], 101),
            Taken::Unknown
        ));
        let other = flows.start(snapshot("conn-a"), 100).unwrap();
        // A cookie of the right length, not the flow's.
        let wrong = "0".repeat(other.cookie.len());
        assert!(matches!(
            flows.take(&other.state, &[wrong.as_str()], 101),
            Taken::Mismatch(_)
        ));
        // Consumed by the mismatch: the right cookie is too late.
        assert!(matches!(
            flows.take(&other.state, &[other.cookie.as_str()], 101),
            Taken::Unknown
        ));
    }

    #[test]
    fn a_flow_expires_and_a_new_one_supersedes_it() {
        let flows = Flows::default();
        let first = flows.start(snapshot("conn-a"), 100).unwrap();
        assert_eq!(first.expires_at, 100 + FLOW_TTL);
        assert!(matches!(
            flows.take(&first.state, &[first.cookie.as_str()], 100 + FLOW_TTL),
            Taken::Unknown
        ));
        let first = flows.start(snapshot("conn-a"), 100).unwrap();
        let second = flows.start(snapshot("conn-a"), 100).unwrap();
        assert!(matches!(
            flows.take(&first.state, &[first.cookie.as_str()], 101),
            Taken::Unknown
        ));
        assert!(matches!(
            flows.take(&second.state, &[second.cookie.as_str()], 100 + FLOW_TTL - 1),
            Taken::Taken(_)
        ));
    }

    #[test]
    fn at_most_sixteen_flows_live_and_an_edit_drops_its_connection_s() {
        let flows = Flows::default();
        let started: Vec<Started> = (0..MAX_FLOWS)
            .map(|i| flows.start(snapshot(&format!("conn-{i}")), 100).unwrap())
            .collect();
        assert!(flows.start(snapshot("conn-x"), 100).is_none());
        assert_eq!(flows.live("conn-0", 100), MAX_FLOWS - 1);
        flows.drop_connection("conn-0");
        assert!(matches!(
            flows.take(&started[0].state, &[started[0].cookie.as_str()], 100),
            Taken::Unknown
        ));
        assert!(flows.start(snapshot("conn-x"), 100).is_some());
    }

    /// api-8e-8f S3: a held registration is reused only for the same token
    /// endpoint and redirect URI.
    #[test]
    fn a_registration_is_reused_only_for_the_same_endpoint_and_redirect() {
        let flows = Flows::default();
        let client = registered_client("c".into(), None, AuthMethod::None, "https://as.example/token".into());
        flows.hold_registration(
            "conn-a",
            Registration {
                client,
                redirect_uri: "https://h.example/cb".into(),
                registered_at: 1,
            },
        );
        assert!(
            flows
                .registration("conn-a", "https://as.example/token", "https://h.example/cb")
                .is_some()
        );
        assert!(
            flows
                .registration("conn-a", "https://as.example/other", "https://h.example/cb")
                .is_none()
        );
        assert!(
            flows
                .registration("conn-a", "https://as.example/token", "https://h2.example/cb")
                .is_none()
        );
        assert!(
            flows
                .registration("conn-b", "https://as.example/token", "https://h.example/cb")
                .is_none()
        );
        flows.drop_connection("conn-a");
        assert!(
            flows
                .registration("conn-a", "https://as.example/token", "https://h.example/cb")
                .is_none()
        );
    }

    /// An edit forgets that the server refused `resource` (the review's
    /// R1): the next Connect sends it again.
    #[test]
    fn dropping_a_connection_forgets_its_refused_resource() {
        let flows = Flows::default();
        flows.refuse_resource("conn-a", "https://mcp.example/mcp");
        flows.refuse_resource("conn-b", "https://mcp.example/mcp");
        flows.drop_connection("conn-a");
        assert!(!flows.resource_refused("conn-a", "https://mcp.example/mcp"));
        assert!(flows.resource_refused("conn-b", "https://mcp.example/mcp"));
    }

    /// An expired flow does not count toward `MAX_FLOWS`.
    #[test]
    fn expired_flows_are_not_live() {
        let flows = Flows::default();
        for i in 0..3 {
            flows.start(snapshot(&format!("conn-{i}")), 100).unwrap();
        }
        assert_eq!(flows.live("none", 100 + FLOW_TTL - 1), 3);
        assert_eq!(flows.live("none", 100 + FLOW_TTL), 0);
    }

    #[test]
    fn a_cookie_s_name_comes_from_its_state() {
        let name = cookie_name("abc");
        assert_eq!(name.len(), FLOW_COOKIE_PREFIX.len() + 16);
        assert_ne!(name, cookie_name("abd"));
    }
}
