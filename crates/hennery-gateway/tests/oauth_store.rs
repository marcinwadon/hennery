//! The store's OAuth half (gateway spec §2, §6; plan 8a's hand-offs to
//! 8f): grants and client secrets sealed each under a field of its own, a
//! stored secret counting as ciphertext, the start's key check opening each
//! kind with its field, a hat purge taking the OAuth clients, and a list
//! that reads no ciphertext.

mod support;

use hennery_gateway::crypto::{self, CLIENT_SECRET, OAUTH_TOKENS, PENDING_CLIENT_SECRET, STATIC_TOKEN};
use hennery_gateway::key::MasterKey;
use hennery_gateway::model::{CredKind, NewConnection};
use hennery_gateway::store::{ClientChange, SecretInput};
use hennery_kernel::secret::unix_now;
use support::World;
use support::oauth::{Config, FakeAs, seed_grant};
use zeroize::Zeroizing;

fn oauth(w: &World, slug: &str, kind: CredKind) -> String {
    w.connection_with(NewConnection {
        slug: slug.into(),
        label: slug.into(),
        url: "https://mcp.vendor.example/mcp".into(),
        hat_id: w.hat(),
        cred_kind: kind,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: false,
    })
}

fn blob(w: &World, sql: &str, id: &str) -> (u32, Vec<u8>) {
    w.raw().query_row(sql, [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
}

/// Gateway spec §6 (plan 8a's O1): a grant is sealed as
/// `gw_credentials.oauth_tokens`, a client secret as
/// `gw_oauth_clients.client_secret`, a pending one as its own field; none
/// opens as another.
#[tokio::test]
async fn each_secret_is_sealed_under_its_own_field() {
    let w = World::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = w.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    let set = |client: &str, secret: &str| {
        w.store
            .set_oauth_client(
                &id,
                client,
                SecretInput::Set(Zeroizing::new(secret.into())),
                &w.key,
                unix_now(),
            )
            .unwrap()
    };
    assert!(matches!(set("pre-1", "first-secret"), ClientChange::Done(_)));
    let (access, refresh) = fake.issue();
    seed_grant(&w, &id, &fake, &access, Some(&refresh), None);
    // A grant is live: the next client waits as pending.
    assert!(matches!(set("pre-2", "second-secret"), ClientChange::Done(_)));
    let (version, grant) = blob(
        &w,
        "SELECT key_version, ciphertext FROM gw_credentials WHERE connection_id = ?1",
        &id,
    );
    assert!(crypto::open(&w.key, &id, OAUTH_TOKENS, version, &grant).is_ok());
    assert!(crypto::open(&w.key, &id, STATIC_TOKEN, version, &grant).is_err());
    let (version, pending) = blob(
        &w,
        "SELECT key_version, pending_secret_ciphertext FROM gw_oauth_clients WHERE connection_id = ?1",
        &id,
    );
    assert_eq!(
        &*crypto::open(&w.key, &id, PENDING_CLIENT_SECRET, version, &pending).unwrap(),
        b"second-secret"
    );
    assert!(crypto::open(&w.key, &id, CLIENT_SECRET, version, &pending).is_err());
    // A static token's field does not open a grant either way round.
    let fixed = w.connection("fixed", &fake.mcp_url(), CredKind::Static);
    w.set_token(&fixed, "tok");
    let (version, token) = blob(
        &w,
        "SELECT key_version, ciphertext FROM gw_credentials WHERE connection_id = ?1",
        &fixed,
    );
    assert!(crypto::open(&w.key, &fixed, OAUTH_TOKENS, version, &token).is_err());
}

/// Plan 8a's O4: a stored client secret is ciphertext, so a missing key is
/// an error then, never a new key.
#[test]
fn a_client_secret_counts_as_ciphertext() {
    let w = World::new();
    let id = oauth(&w, "linear", CredKind::OauthClient);
    assert!(!w.store.has_ciphertext().unwrap());
    w.store
        .set_oauth_client(&id, "pre-1", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    assert!(!w.store.has_ciphertext().unwrap(), "a public client holds no secret");
    w.store
        .set_oauth_client(
            &id,
            "pre-1",
            SecretInput::Set(Zeroizing::new("s".into())),
            &w.key,
            unix_now(),
        )
        .unwrap();
    assert!(w.store.has_ciphertext().unwrap());
}

/// Plan 8a's O2: the start's key check opens an OAuth grant with its own
/// field, and a client secret, and refuses another key for either.
#[tokio::test]
async fn the_key_check_opens_each_kind_with_its_field() {
    let w = World::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = w.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (access, refresh) = fake.issue();
    seed_grant(&w, &id, &fake, &access, Some(&refresh), None);
    w.store.check_key(&w.key).unwrap();
    assert!(w.store.check_key(&MasterKey::from_bytes([1; 32])).is_err());
    let w = World::new();
    let id = oauth(&w, "secret-only", CredKind::OauthClient);
    w.store
        .set_oauth_client(
            &id,
            "pre-1",
            SecretInput::Set(Zeroizing::new("s".into())),
            &w.key,
            unix_now(),
        )
        .unwrap();
    w.store.check_key(&w.key).unwrap();
    let err = w.store.check_key(&MasterKey::from_bytes([1; 32])).unwrap_err();
    assert!(
        format!("{err:#}").contains("gw_oauth_clients"),
        "the way out names the table: {err:#}"
    );
}

/// Plan 8a's O18: a hat purge takes its connections' OAuth clients; the
/// other hat's stay.
#[test]
fn a_hat_purge_takes_its_oauth_clients() {
    let w = World::new();
    let work = w.other_hat();
    let mut gone = NewConnection {
        slug: "gone".into(),
        label: "gone".into(),
        url: "https://mcp.vendor.example/mcp".into(),
        hat_id: work.clone(),
        cred_kind: CredKind::OauthClient,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: false,
    };
    let gone_id = w.connection_with(gone.clone());
    gone.slug = "kept".into();
    gone.hat_id = w.hat();
    let kept = w.connection_with(gone);
    for id in [&gone_id, &kept] {
        w.store
            .set_oauth_client(id, "pre-1", SecretInput::Clear, &w.key, unix_now())
            .unwrap();
    }
    w.store.purge_hat(&work).unwrap();
    let count: i64 = w
        .raw()
        .query_row("SELECT count(*) FROM gw_oauth_clients", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    assert!(
        w.store
            .connection(&kept)
            .unwrap()
            .unwrap()
            .oauth_client
            .unwrap()
            .client_id
            .is_some()
    );
    w.store.purge_hat(&work).unwrap();
}

/// The review's R1: a grant is stored only for the connection as its flow
/// found it: the same URL, kind and internal marking, and for a
/// pre-registered client the same client in the same place.
#[test]
fn a_grant_is_stored_only_for_the_connection_as_it_was() {
    use hennery_gateway::model::{AuthMethod, GrantTokens, TokenClient};
    use hennery_gateway::store::{ClientSource, GrantStored, GrantToStore};
    let w = World::new();
    let id = oauth(&w, "linear", CredKind::OauthClient);
    let url = "https://mcp.vendor.example/mcp";
    w.store
        .set_oauth_client(&id, "pre-1", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    let client = |id: &str| TokenClient {
        client_id: id.into(),
        secret: None,
        auth_method: AuthMethod::None,
        token_endpoint: "https://as.example/token".into(),
    };
    let tokens = GrantTokens::from_opened(br#"{"access_token":"a"}"#).unwrap();
    let store = |url: &str, kind: CredKind, internal: bool, source: ClientSource, client: &TokenClient| {
        w.store
            .store_grant(
                &GrantToStore {
                    connection_id: &id,
                    url,
                    cred_kind: kind,
                    internal_network: internal,
                    source,
                    client,
                    issuer: "https://as.example",
                    authorization_endpoint: "https://as.example/authorize",
                    redirect_uri: "https://hennery.example/api/mcp/oauth/callback",
                    scopes: &[],
                    resource: url,
                    resource_param_accepted: true,
                    registered_at: 1,
                    tokens: &tokens,
                    expires_at: None,
                },
                &w.key,
                unix_now(),
            )
            .unwrap()
    };
    let active = ClientSource::Active;
    for (what, stored) in [
        (
            "another url",
            store(
                "https://mcp.vendor.example/other",
                CredKind::OauthClient,
                false,
                active,
                &client("pre-1"),
            ),
        ),
        (
            "another kind",
            store(url, CredKind::OauthDcr, false, active, &client("pre-1")),
        ),
        (
            "another marking",
            store(url, CredKind::OauthClient, true, active, &client("pre-1")),
        ),
        (
            "another client",
            store(url, CredKind::OauthClient, false, active, &client("pre-2")),
        ),
        (
            "a pending one that is not",
            store(
                url,
                CredKind::OauthClient,
                false,
                ClientSource::Pending,
                &client("pre-1"),
            ),
        ),
    ] {
        assert_eq!(stored, GrantStored::ConnectionChanged, "{what}");
    }
    assert!(!w.store.connection(&id).unwrap().unwrap().has_credential);
    assert!(matches!(
        store(url, CredKind::OauthClient, false, active, &client("pre-1")),
        GrantStored::Stored(_)
    ));
    // With a grant, a new client is pending; a flow of the old active one
    // no longer matches, the pending one's does.
    w.store
        .set_oauth_client(&id, "pre-2", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    assert_eq!(
        store(url, CredKind::OauthClient, false, active, &client("pre-1")),
        GrantStored::ConnectionChanged
    );
    assert!(matches!(
        store(
            url,
            CredKind::OauthClient,
            false,
            ClientSource::Pending,
            &client("pre-2")
        ),
        GrantStored::Stored(_)
    ));
    assert_eq!(
        w.store
            .connection(&id)
            .unwrap()
            .unwrap()
            .oauth_client
            .unwrap()
            .client_id
            .as_deref(),
        Some("pre-2")
    );
}

/// Gateway spec §7: `status_at` is when the status last changed; a write
/// of the same status moves only `checked_at`.
#[test]
fn the_same_status_again_keeps_status_at() {
    use hennery_gateway::model::Status;
    let w = World::new();
    let url = "https://mcp.vendor.example/mcp";
    let id = oauth(&w, "linear", CredKind::OauthDcr);
    w.proxy_store
        .record_status(&id, url, Status::Error, Some("down"), 100)
        .unwrap();
    let change = w
        .proxy_store
        .record_status(&id, url, Status::Error, Some("down"), 200)
        .unwrap();
    assert!(change.is_none());
    let record = w.store.connection(&id).unwrap().unwrap();
    assert_eq!((record.status_at, record.checked_at), (100, Some(200)));
}

/// The review's R2: a pin is set once, for the client it was found for.
#[test]
fn a_client_is_pinned_once() {
    use hennery_gateway::store::Slot;
    let w = World::new();
    let id = oauth(&w, "linear", CredKind::OauthClient);
    w.store
        .set_oauth_client(&id, "pre-1", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    assert!(
        !w.store
            .pin_client(&id, Slot::Active, "other", "https://as.example", "https://as.example/t")
            .unwrap()
    );
    assert!(
        w.store
            .pin_client(&id, Slot::Active, "pre-1", "https://as.example", "https://as.example/t")
            .unwrap()
    );
    assert!(
        !w.store
            .pin_client(
                &id,
                Slot::Active,
                "pre-1",
                "https://evil.example",
                "https://evil.example/t"
            )
            .unwrap()
    );
    let slots = w.store.oauth_clients(&id, &w.key).unwrap().unwrap();
    assert_eq!(slots.active.unwrap().issuer.as_deref(), Some("https://as.example"));
}

/// A pre-registered client with a live grant: `pending`, its secret opened
/// for the next authorize, and pinned once, for that client only (G-7, the
/// review's R2).
#[tokio::test]
async fn a_pending_client_opens_its_secret_and_is_pinned_once() {
    use hennery_gateway::store::Slot;
    let w = World::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = w.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    w.store
        .set_oauth_client(&id, "pre-1", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    let (access, refresh) = fake.issue();
    seed_grant(&w, &id, &fake, &access, Some(&refresh), None);
    w.store
        .set_oauth_client(
            &id,
            "pre-2",
            SecretInput::Set(Zeroizing::new("pending-secret".into())),
            &w.key,
            unix_now(),
        )
        .unwrap();
    let slots = w.store.oauth_clients(&id, &w.key).unwrap().unwrap();
    let pending = slots.pending.unwrap();
    assert_eq!(pending.client_id, "pre-2");
    assert_eq!(pending.secret.as_deref().map(String::as_str), Some("pending-secret"));
    let pin = |client: &str, issuer: &str| {
        w.store
            .pin_client(&id, Slot::Pending, client, issuer, &format!("{issuer}/t"))
            .unwrap()
    };
    assert!(!pin("other", "https://as.example"));
    assert!(pin("pre-2", "https://as.example"));
    assert!(!pin("pre-2", "https://evil.example"));
    let slots = w.store.oauth_clients(&id, &w.key).unwrap().unwrap();
    assert_eq!(slots.pending.unwrap().issuer.as_deref(), Some("https://as.example"));
}

/// The review's R1: a flow of a pending client stores its grant only while
/// that client is still the pending one.
#[test]
fn a_grant_for_a_pending_client_needs_that_very_client() {
    use hennery_gateway::model::{AuthMethod, GrantTokens, TokenClient};
    use hennery_gateway::store::{ClientSource, GrantStored, GrantToStore};
    let w = World::new();
    let id = oauth(&w, "linear", CredKind::OauthClient);
    let url = "https://mcp.vendor.example/mcp";
    let tokens = GrantTokens::from_opened(br#"{"access_token":"a"}"#).unwrap();
    let store = |source: ClientSource, client_id: &str| {
        let client = TokenClient {
            client_id: client_id.into(),
            secret: None,
            auth_method: AuthMethod::None,
            token_endpoint: "https://as.example/token".into(),
        };
        w.store
            .store_grant(
                &GrantToStore {
                    connection_id: &id,
                    url,
                    cred_kind: CredKind::OauthClient,
                    internal_network: false,
                    source,
                    client: &client,
                    issuer: "https://as.example",
                    authorization_endpoint: "https://as.example/authorize",
                    redirect_uri: "https://hennery.example/api/mcp/oauth/callback",
                    scopes: &[],
                    resource: url,
                    resource_param_accepted: true,
                    registered_at: 1,
                    tokens: &tokens,
                    expires_at: None,
                },
                &w.key,
                unix_now(),
            )
            .unwrap()
    };
    w.store
        .set_oauth_client(&id, "pre-1", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    assert!(matches!(store(ClientSource::Active, "pre-1"), GrantStored::Stored(_)));
    w.store
        .set_oauth_client(&id, "pre-2", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    assert_eq!(store(ClientSource::Pending, "pre-3"), GrantStored::ConnectionChanged);
}

/// What a completed Connect leaves in the row, beyond what the API shows:
/// no time of a failure that is gone, no pins of a pending client that is
/// gone; clearing a failure clears its time.
#[tokio::test]
async fn a_completed_connect_leaves_no_stale_columns() {
    let w = World::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = w.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    w.store
        .set_oauth_client(&id, "pre-1", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    let (access, refresh) = fake.issue();
    seed_grant(&w, &id, &fake, &access, Some(&refresh), None);
    w.store
        .set_oauth_client(&id, "pre-2", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    w.store
        .pin_client(
            &id,
            hennery_gateway::store::Slot::Pending,
            "pre-2",
            "https://as.example",
            "https://as.example/t",
        )
        .unwrap();
    w.store
        .set_oauth_error(&id, Some(("consent_denied", "x")), unix_now())
        .unwrap();
    let columns = || -> (Option<i64>, Option<String>, Option<String>) {
        w.raw()
            .query_row(
                "SELECT c.oauth_error_at, o.pending_issuer, o.pending_token_endpoint
                 FROM gw_connections c JOIN gw_oauth_clients o ON o.connection_id = c.id WHERE c.id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    };
    assert!(columns().0.is_some());
    // The pending client's Connect completes.
    let (access, refresh) = fake.issue();
    let tokens = hennery_gateway::model::GrantTokens {
        access_token: Zeroizing::new(access),
        refresh_token: Some(Zeroizing::new(refresh)),
    };
    let client = hennery_gateway::model::TokenClient {
        client_id: "pre-2".into(),
        secret: None,
        auth_method: hennery_gateway::model::AuthMethod::None,
        token_endpoint: format!("{}/token", fake.origin()),
    };
    let record = w.store.connection(&id).unwrap().unwrap();
    let stored = w
        .store
        .store_grant(
            &hennery_gateway::store::GrantToStore {
                connection_id: &id,
                url: &record.url,
                cred_kind: CredKind::OauthClient,
                internal_network: record.internal_network,
                source: hennery_gateway::store::ClientSource::Pending,
                client: &client,
                issuer: &fake.issuer(),
                authorization_endpoint: &format!("{}/authorize", fake.origin()),
                redirect_uri: "https://hennery.example/api/mcp/oauth/callback",
                scopes: &[],
                resource: &record.url,
                resource_param_accepted: true,
                registered_at: 1,
                tokens: &tokens,
                expires_at: None,
            },
            &w.key,
            unix_now(),
        )
        .unwrap();
    assert!(matches!(stored, hennery_gateway::store::GrantStored::Stored(_)));
    assert_eq!(columns(), (None, None, None));
    // Clearing a failure clears its time.
    w.store
        .set_oauth_error(&id, Some(("consent_denied", "x")), unix_now())
        .unwrap();
    w.store.set_oauth_error(&id, None, unix_now()).unwrap();
    assert_eq!(columns().0, None);
}

/// Gateway spec §4.1 (G-4): a `resource` found or accepted for a URL is
/// recorded only while the connection still has that URL.
#[test]
fn a_resource_is_recorded_only_for_the_url_it_was_found_for() {
    let w = World::new();
    let id = oauth(&w, "linear", CredKind::OauthDcr);
    let stale = "https://mcp.vendor.example/old";
    w.store
        .set_resource_mismatch(&id, stale, Some("https://mcp.vendor.example/v2"))
        .unwrap();
    assert!(
        !w.store
            .accept_resource(&id, stale, "https://mcp.vendor.example/v2")
            .unwrap()
    );
    let record = w.store.connection(&id).unwrap().unwrap();
    assert_eq!((record.resource_mismatch, record.accepted_resource), (None, None));
}

/// Plan 8a's O4, for the key's sake: a pending client's secret alone (an
/// operator who ran only half of `GIVE_UP`'s statement) is still
/// ciphertext.
#[test]
fn a_pending_secret_alone_counts_as_ciphertext() {
    let w = World::new();
    let id = oauth(&w, "linear", CredKind::OauthClient);
    w.store
        .set_oauth_client(&id, "pre-1", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    w.raw()
        .execute(
            "UPDATE gw_oauth_clients SET pending_client_id = 'pre-2', pending_has_secret = 1,
                 pending_secret_ciphertext = x'0102', key_version = 1 WHERE connection_id = ?1",
            [&id],
        )
        .unwrap();
    assert!(w.store.has_ciphertext().unwrap());
}

/// An OAuth grant is read only for an OAuth kind: a static connection's
/// token is never opened as a grant.
#[test]
fn a_static_token_is_no_grant() {
    let w = World::new();
    let id = w.connection("fixed", "https://mcp.vendor.example/mcp", CredKind::Static);
    w.set_token(&id, "tok");
    w.store
        .set_oauth_client(&id, "pre-1", SecretInput::Clear, &w.key, unix_now())
        .unwrap();
    w.raw()
        .execute(
            "INSERT INTO gw_oauth_clients(connection_id, owner_id, client_id, has_client_secret, registered_at,
                                          token_endpoint_auth_method, token_endpoint)
             SELECT id, owner_id, 'pre-1', 0, 1, 'none', 'https://as.example/token' FROM gw_connections WHERE id = ?1",
            [&id],
        )
        .unwrap();
    assert!(w.store.oauth_credential(&id, &w.key).unwrap().is_none());
}

/// Gateway spec §7 and the 8d hand-off: what live traffic and the probe
/// record is about the URL they reached, and only with a credential;
/// `checked_at` moves at most every 60 seconds on traffic to an `ok`
/// connection.
#[test]
fn traffic_and_checks_record_only_for_the_url_reached() {
    use hennery_gateway::model::Status;
    let w = World::new();
    let url = "https://mcp.vendor.example/mcp";
    let stale = "https://mcp.vendor.example/old";
    let id = oauth(&w, "linear", CredKind::OauthDcr);
    let checked = || w.store.connection(&id).unwrap().unwrap().checked_at;
    // No credential: traffic's 2xx says nothing.
    w.proxy_store
        .record_status(&id, url, Status::Error, Some("down"), 100)
        .unwrap();
    assert!(w.proxy_store.record_traffic_ok(&id, url, 200).unwrap().is_none());
    assert_eq!(w.status(&id), "error");
    w.raw()
        .execute(
            "INSERT INTO gw_credentials(connection_id, owner_id, key_version, ciphertext, updated_at)
             SELECT id, owner_id, 1, x'00', 1 FROM gw_connections WHERE id = ?1",
            [&id],
        )
        .unwrap();
    // Another URL: nothing.
    assert!(w.proxy_store.record_traffic_ok(&id, stale, 200).unwrap().is_none());
    w.proxy_store.record_checked(&id, stale, 300).unwrap();
    assert_eq!((w.status(&id), checked()), ("error".to_string(), Some(100)));
    // This URL: ok, then `checked_at` throttled.
    assert!(w.proxy_store.record_traffic_ok(&id, url, 400).unwrap().is_some());
    assert_eq!(checked(), Some(400));
    w.proxy_store.record_traffic_ok(&id, url, 459).unwrap();
    assert_eq!(checked(), Some(400));
    w.proxy_store.record_traffic_ok(&id, url, 460).unwrap();
    assert_eq!(checked(), Some(460));
}
