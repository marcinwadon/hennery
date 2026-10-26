//! OAuth Connect (gateway spec §4, §11; plan 8f): authorize, consent at a
//! fake authorization server, and the callback, through the gateway's
//! router in-process. The fake is on loopback; its connections are marked
//! "internal network" (lane L7).

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::IntoResponse;
use hennery_gateway::api::{GatewayState, router};
use hennery_gateway::model::{ConnectionPatch, CredKind, NewConnection, Status};
use hennery_gateway::notify::Alert;
use hennery_gateway::runtime::Runtime;
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use support::oauth::{Config, FakeAs, MetaAt, PrAt};
use support::{Recorder, World};
use tower::ServiceExt;

const ORIGIN: &str = "https://hennery.example";
const PASSWORD: &str = "correct horse battery";
const CALLBACK: &str = "https://hennery.example/api/mcp/oauth/callback";

#[tokio::test]
async fn local_callback_uses_exact_redirect_for_dcr_consent_exchange_and_reuse() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("local", &fake.mcp_url(), CredKind::OauthDcr);
    // Explicit default port deliberately stays verbatim in the OAuth contract.
    let local = "http://localhost:80/custom/callback";
    let relay = hennery_gateway::local_callback::LocalCallback::parse(local)
        .unwrap()
        .router(ORIGIN)
        .unwrap();
    for (redirect, registrations) in [
        (local, 1),
        (local, 1),
        ("http://localhost:8977/other", 2),
        ("https://localhost:443/custom/callback", 3),
    ] {
        let (status, headers, answer) = h.authorize(&id, json!({ "local_redirect_uri": redirect })).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        let consent = answer["consent_url"].as_str().unwrap();
        let q: std::collections::HashMap<_, _> = url::Url::parse(consent).unwrap().query_pairs().into_owned().collect();
        assert_eq!(q["redirect_uri"], redirect);
        let location = fake.consent(consent).await;
        let local_url = url::Url::parse(&location).unwrap();
        // Use the real relay router for the first address. Subsequent addresses
        // exercise the same collector callback, independent of its public URL.
        let forwarded = if redirect == local {
            let response = relay
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("{}?{}", local_url.path(), local_url.query().unwrap()))
                        .header("host", "localhost")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FOUND);
            response.headers()[header::LOCATION].to_str().unwrap().to_string()
        } else {
            format!("{CALLBACK}?{}", local_url.query().unwrap())
        };
        let cookie = flow_cookie(&headers);
        let page = h
            .callback(&forwarded, Some(&cookie), &[("sec-fetch-site", "cross-site")])
            .await;
        assert_eq!(page.result(), "connected", "{}", page.html);
        assert_eq!(
            h.callback(&forwarded, Some(&cookie), &[]).await.result(),
            "flow_unknown"
        );
        assert_eq!(fake.with(|r| r.registered.len()), registrations);
        assert_eq!(
            fake.with(|r| r.registered_redirects.last().cloned()),
            Some(json!([redirect]))
        );
        assert_eq!(
            h.item(&id).await["oauth"]["redirect_uri"],
            CALLBACK,
            "the collector callback stays fixed"
        );
    }
    assert_eq!(fake.with(|r| r.exchanges), 4);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(
        fake.with(|r| r.registered.len()),
        4,
        "switching back to the collector requires another DCR client"
    );
}

#[tokio::test]
async fn local_callback_preserves_cookie_issuer_denial_and_session_guards() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("local-guards", &fake.mcp_url(), CredKind::OauthDcr);
    for mode in ["cookie", "issuer", "denied", "session", "state", "duplicate"] {
        fake.configure(|c| c.deny = (mode == "denied").then(|| "No thanks".into()));
        let session = h.session(0);
        let (status, headers, answer) = h
            .send(
                &session,
                "POST",
                &format!("/api/mcp/connections/{id}/authorize"),
                Some(json!({ "local_redirect_uri": "http://127.0.0.1:8976/callback" })),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
        let mut returned = url::Url::parse(&location).unwrap();
        if mode == "issuer" {
            let pairs: Vec<_> = returned
                .query_pairs()
                .into_owned()
                .filter(|(k, _)| k != "iss")
                .collect();
            returned.set_query(None);
            returned
                .query_pairs_mut()
                .extend_pairs(pairs)
                .append_pair("iss", "https://wrong.example");
        }
        if mode == "state" {
            returned.set_query(Some("state=unknown&code=unused"));
        }
        if mode == "duplicate" {
            returned.query_pairs_mut().append_pair("state", "other");
        }
        if mode == "session" {
            let auth = h.operator.authenticate(&session, unix_now()).unwrap().unwrap();
            h.operator.revoke_session(&auth.session_id, unix_now()).unwrap();
        }
        let forwarded = format!("{CALLBACK}?{}", returned.query().unwrap());
        let cookie = flow_cookie(&headers);
        let page = h
            .callback(&forwarded, (mode != "cookie").then_some(cookie.as_str()), &[])
            .await;
        let expected = match mode {
            "cookie" => "flow_mismatch",
            "issuer" => "issuer_mismatch",
            "denied" => "consent_denied",
            "session" => "session_ended",
            _ => "flow_unknown",
        };
        assert_eq!(page.result(), expected, "{mode}: {}", page.html);
        assert!(!h.item(&id).await["has_credential"].as_bool().unwrap());
    }
    assert_eq!(fake.with(|r| r.exchanges), 0);
}

#[tokio::test]
async fn local_redirect_rejects_non_loopback_before_discovery() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("bad-local", &fake.mcp_url(), CredKind::OauthDcr);
    for url in [
        "https://foreign.example/cb",
        "http://0.0.0.0:8976/cb",
        "http://localhost.evil:8976/cb",
        "http://localhost:8976/cb?state=x",
    ] {
        let (status, _, answer) = h.authorize(&id, json!({ "local_redirect_uri": url })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(answer["code"], "invalid_local_redirect_uri");
    }
    assert!(fake.paths().is_empty());
}

#[tokio::test]
async fn local_callback_keeps_manual_client_and_surfaces_provider_policy_refusal() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        registration: false,
        deny: Some("This client does not allow this redirect URI".into()),
        ..Config::default()
    })
    .await;
    let id = h.connection("local-manual", &fake.mcp_url(), CredKind::OauthClient);
    let (status, _, _) = h
        .send(
            &h.session(0),
            "PUT",
            &format!("/api/mcp/connections/{id}/oauth-client"),
            Some(json!({"client_id": "pre-manual", "client_secret": null})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    for allowed in [false, true] {
        if allowed {
            fake.configure(|c| c.deny = None);
        }
        let (status, headers, answer) = h
            .authorize(&id, json!({"local_redirect_uri": "https://127.0.0.1:9882/callback"}))
            .await;
        assert_eq!(status, StatusCode::OK);
        let consent = answer["consent_url"].as_str().unwrap();
        assert!(
            url::Url::parse(consent)
                .unwrap()
                .query_pairs()
                .any(|(k, v)| k == "client_id" && v == "pre-manual")
        );
        let local = fake.consent(consent).await;
        let page = h
            .callback(
                &format!("{CALLBACK}?{}", url::Url::parse(&local).unwrap().query().unwrap()),
                Some(&flow_cookie(&headers)),
                &[],
            )
            .await;
        assert_eq!(page.result(), if allowed { "connected" } else { "consent_denied" });
        if !allowed {
            assert!(page.html.contains("does not allow this redirect URI"));
        }
    }
    assert_eq!(fake.with(|r| r.exchanges), 1);
    assert!(
        fake.with(|r| r.registered.is_empty()),
        "manual clients never fall back to DCR"
    );
}

#[tokio::test]
async fn local_callback_pending_registration_and_expired_flow_are_not_reused() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("local-pending", &fake.mcp_url(), CredKind::OauthDcr);
    for (local, expected) in [
        ("http://localhost:8976/a", 1),
        ("http://localhost:8976/a", 1),
        ("http://localhost:8976/b", 2),
    ] {
        let (status, headers, answer) = h.authorize(&id, json!({ "local_redirect_uri": local })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fake.with(|r| r.registered.len()), expected);
        assert_eq!(
            fake.with(|r| r.registered_redirects.last().cloned()),
            Some(json!([local]))
        );
        let consent = answer["consent_url"].as_str().unwrap();
        let state = url::Url::parse(consent)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let cookie = flow_cookie(&headers);
        let value = cookie.split_once('=').unwrap().1;
        let hennery_gateway::flows::Taken::Taken(snapshot) = h.runtime.flows.take(&state, &[value], unix_now()) else {
            panic!("live snapshot");
        };
        let expired = h
            .runtime
            .flows
            .start(*snapshot, unix_now() - hennery_gateway::flows::FLOW_TTL - 1)
            .unwrap();
        let expired_cookie = format!(
            "{}={}",
            hennery_gateway::flows::cookie_name(&expired.state),
            expired.cookie.as_str()
        );
        let page = h
            .callback(
                &format!("{CALLBACK}?state={}&code=unused", expired.state),
                Some(&expired_cookie),
                &[],
            )
            .await;
        assert_eq!(page.result(), "flow_unknown");
    }
    assert_eq!(fake.with(|r| r.exchanges), 0);
}

struct Harness {
    world: World,
    app: Router,
    operator: Arc<Operator>,
    runtime: Arc<Runtime>,
    alerts: Arc<Recorder>,
    phc: String,
}

/// What a callback answered.
struct Page {
    status: StatusCode,
    headers: HeaderMap,
    html: String,
}

impl Page {
    fn result(&self) -> String {
        let at = self.html.find("data-result=\"").expect("a result") + "data-result=\"".len();
        self.html[at..].split('"').next().unwrap().to_string()
    }
}

impl Harness {
    fn new() -> Self {
        let world = World::new();
        let operator = Arc::new(Operator::open(&world.db).unwrap());
        let now = unix_now();
        let setup = operator.issue_setup_token(now).unwrap().unwrap();
        let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, PASSWORD, ORIGIN, now).unwrap() else {
            panic!("setup failed");
        };
        let alerts = Arc::new(Recorder::default());
        let runtime = world.runtime(support::test_egress(), alerts.clone());
        let app = router(GatewayState {
            runtime: runtime.clone(),
            operator: operator.clone(),
        });
        Self {
            world,
            app,
            operator,
            runtime,
            alerts,
            phc,
        }
    }

    fn session(&self, age: i64) -> String {
        self.operator
            .open_session("test", &self.phc, unix_now() - age)
            .unwrap()
            .unwrap()
    }

    /// An OAuth connection of `kind` to `url`, on the internal network.
    fn connection(&self, slug: &str, url: &str, kind: CredKind) -> String {
        self.world.connection_with(NewConnection {
            slug: slug.into(),
            label: format!("Label {slug}"),
            url: url.into(),
            hat_id: self.world.hat(),
            cred_kind: kind,
            static_header: None,
            static_prefix: None,
            tool_allowlist: None,
            internal_network: true,
        })
    }

    async fn send(
        &self,
        session: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", format!("hennery_session={session}"));
        let body = match body {
            Some(body) => {
                req = req.header("content-type", "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn authorize(&self, id: &str, body: Value) -> (StatusCode, HeaderMap, Value) {
        self.send(
            &self.session(0),
            "POST",
            &format!("/api/mcp/connections/{id}/authorize"),
            Some(body),
        )
        .await
    }

    async fn item(&self, id: &str) -> Value {
        let (_, _, list) = self.send(&self.session(0), "GET", "/api/mcp/connections", None).await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .cloned()
            .unwrap()
    }

    /// The vendor's redirect back, with `cookie` (`name=value`) as the
    /// browser sends it, and any extra headers.
    async fn callback(&self, location: &str, cookie: Option<&str>, extra: &[(&str, &str)]) -> Page {
        let path = location.strip_prefix(ORIGIN).expect("back to hennery");
        let mut req = Request::builder().method("GET").uri(path);
        if let Some(cookie) = cookie {
            req = req.header("cookie", cookie);
        }
        for (name, value) in extra {
            req = req.header(*name, *value);
        }
        let resp = self
            .app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        Page {
            status,
            headers,
            html: String::from_utf8(bytes.to_vec()).unwrap(),
        }
    }

    /// Authorize, consent and come back: the callback's page.
    async fn connect(&self, id: &str, fake: &FakeAs) -> Page {
        let (status, headers, answer) = self.authorize(id, json!({})).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
        self.callback(&location, Some(&flow_cookie(&headers)), &[]).await
    }
}

/// The flow cookie an authorize set, as `name=value`.
fn flow_cookie(headers: &HeaderMap) -> String {
    let set = headers[header::SET_COOKIE].to_str().unwrap();
    set.split(';').next().unwrap().to_string()
}

fn query_of(url: &str) -> std::collections::HashMap<String, String> {
    url::Url::parse(url).unwrap().query_pairs().into_owned().collect()
}

/// Gateway spec §4.1–§4.3, end to end with dynamic registration: the PR
/// document at its path-inserted URL, RFC 8414 metadata, a public client,
/// PKCE S256 (the fake checks the verifier), `resource` on consent and
/// exchange, the grant stored sealed and the status `ok`.
#[tokio::test]
async fn a_dynamically_registered_connect_completes() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (status, headers, answer) = h.authorize(&id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let consent = answer["consent_url"].as_str().unwrap();
    let q = query_of(consent);
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["redirect_uri"], CALLBACK);
    assert_eq!(q["resource"], fake.mcp_url());
    assert_eq!(q["scope"], "read write", "scopes_supported is requested (G-5)");
    assert!(!q.contains_key("response_mode"), "never form_post");
    let location = fake.consent(consent).await;
    let page = h.callback(&location, Some(&flow_cookie(&headers)), &[]).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.html);
    assert_eq!(page.result(), "connected");
    assert!(page.html.contains(&format!("data-connection=\"{id}\"")));
    assert!(page.html.contains("Connected. You may close this window."));
    assert_eq!(fake.with(|r| r.registered.len()), 1);
    assert_eq!(
        fake.with(|r| r.token_resources.clone()),
        [("authorization_code".to_string(), Some(fake.mcp_url()))]
    );
    let item = h.item(&id).await;
    assert_eq!(item["status"], "ok");
    assert_eq!(item["has_credential"], true);
    assert_eq!(item["oauth"]["client_id"], fake.with(|r| r.registered[0].clone()));
    assert_eq!(item["oauth"]["redirect_uri"], CALLBACK);
    assert!(item.get("oauth_error").is_none());
    let grant = h.world.store.oauth_credential(&id, &h.world.key).unwrap().unwrap();
    assert!(fake.is_live(&grant.tokens.access_token));
    assert!(grant.expires_at.unwrap() <= unix_now() + 3600 - 60);
}

/// Gateway spec §4.1 step 1: the PR document named by a 401's
/// `WWW-Authenticate` `resource_metadata`, from one unauthenticated
/// `initialize`, before any well-known URL.
#[tokio::test]
async fn the_challenge_s_resource_metadata_is_tried_first() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Challenge,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    let paths = fake.paths();
    assert_eq!(paths[0], "/mcp", "the unauthenticated initialize");
    assert_eq!(paths[1], "/prm-from-challenge");
}

/// Gateway spec §4.1 steps 2 and 3, and G-3: path-inserted before the
/// origin's.
#[tokio::test]
async fn the_path_inserted_document_is_tried_before_the_origin_s() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Origin,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    let paths = fake.paths();
    let inserted = paths
        .iter()
        .position(|p| p == "/.well-known/oauth-protected-resource/mcp")
        .unwrap();
    let origin = paths
        .iter()
        .position(|p| p == "/.well-known/oauth-protected-resource")
        .unwrap();
    assert!(inserted < origin, "{paths:?}");
}

/// Gateway spec §4.1: without a PR document, the resource's origin is the
/// authorization server.
#[tokio::test]
async fn without_a_resource_document_the_origin_is_the_authorization_server() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Nowhere,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    // No resource was named: no scope either, and nothing to compare.
    assert!(h.item(&id).await["oauth"].get("resource_mismatch").is_none());
}

/// Gateway spec §4.1: RFC 8414 inserted before appended, then OpenID
/// configuration; each form is found, in that order.
#[tokio::test]
async fn server_metadata_forms_are_tried_in_order() {
    for (form, expected_at) in [(MetaAt::Inserted, 0), (MetaAt::Appended, 1), (MetaAt::Openid, 2)] {
        let h = Harness::new();
        let fake = FakeAs::start(Config {
            meta_at: form,
            as_path: "/tenant".into(),
            ..Config::default()
        })
        .await;
        let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
        assert_eq!(h.connect(&id, &fake).await.result(), "connected", "{form:?}");
        let tried: Vec<String> = fake
            .paths()
            .into_iter()
            .filter(|p| p.contains("oauth-authorization-server") || p.contains("openid-configuration"))
            .collect();
        let order = [
            "/.well-known/oauth-authorization-server/tenant",
            "/tenant/.well-known/oauth-authorization-server",
            "/tenant/.well-known/openid-configuration",
        ];
        assert_eq!(tried, order[..=expected_at], "{form:?}");
    }
}

/// The egress plan's O16: a redirect is "not here", never followed, and the
/// next candidate is tried.
#[tokio::test]
async fn a_redirecting_candidate_is_not_followed() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Origin,
        pr_redirect: true,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert!(!fake.paths().iter().any(|p| p == "/elsewhere"), "{:?}", fake.paths());
}

/// Plan 8f decision 5: a 5xx at a candidate moves on to the next.
#[tokio::test]
async fn a_failing_candidate_is_passed_over() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Origin,
        pr_status: Some(503),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
}

/// Each refusal of authorize, its code, and its record in `oauth_error`.
async fn refused(h: &Harness, id: &str, body: Value, status: StatusCode, code: &str) -> Value {
    let (got, _, answer) = h.authorize(id, body).await;
    assert_eq!(
        (got, answer["code"].as_str().unwrap_or_default()),
        (status, code),
        "{answer}"
    );
    let item = h.item(id).await;
    assert_eq!(item["oauth_error"]["code"], code, "{item}");
    assert_ne!(item["status"], "ok", "a failed Connect sets no status");
    answer
}

/// RFC 8414 §3.3: metadata whose `issuer` is not the identifier it was
/// fetched for is not used.
#[tokio::test]
async fn metadata_naming_another_issuer_is_refused() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        issuer: Some("https://evil.example".into()),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let answer = refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "discovery_failed").await;
    assert!(answer["message"].as_str().unwrap().contains("issuer"));
}

#[tokio::test]
async fn nothing_found_is_discovery_failed() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Nowhere,
        serve_metadata: false,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "discovery_failed").await;
}

/// Gateway spec §4.3: PKCE S256 only.
#[tokio::test]
async fn a_server_without_s256_is_refused() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        s256: false,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "pkce_unsupported").await;
}

/// Gateway spec §4.2: no registration endpoint suggests a pre-registered
/// client.
#[tokio::test]
async fn no_registration_endpoint_suggests_a_pre_registered_client() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        registration: false,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::CONFLICT, "no_registration_endpoint").await;
}

/// G-6: the vendor's refusal of a registration is shown, its control and
/// bidirectional characters stripped, cut to 300 characters.
#[tokio::test]
async fn a_registration_refusal_shows_the_vendor_s_text() {
    let h = Harness::new();
    let long = format!("redirect_uri must be https\u{202e}\u{0007} {}", "x".repeat(400));
    let fake = FakeAs::start(Config {
        dcr_refusal: Some((
            400,
            json!({ "error": "invalid_redirect_uri", "error_description": long }),
        )),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let answer = refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "registration_refused").await;
    let message = answer["message"].as_str().unwrap();
    let said = message.strip_prefix("The vendor said: ").expect(message);
    assert!(
        said.starts_with("invalid_redirect_uri: redirect_uri must be https"),
        "{said}"
    );
    assert!(said.chars().count() <= 300, "{}", said.chars().count());
    assert!(!said.contains('\u{202e}') && !said.contains('\u{0007}'));
}

/// G-1: a pre-registered confidential client, its secret sent as Basic
/// auth to the token endpoint only.
#[tokio::test]
async fn a_pre_registered_confidential_client_connects() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        registration: false,
        confidential: Some(("pre-1".into(), "s3cret-value".into())),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    refused(&h, &id, json!({}), StatusCode::CONFLICT, "no_oauth_client").await;
    let (status, _, item) = h
        .send(
            &h.session(0),
            "PUT",
            &format!("/api/mcp/connections/{id}/oauth-client"),
            Some(json!({ "client_id": "pre-1", "client_secret": "s3cret-value" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert_eq!(item["oauth"]["client_id"], "pre-1");
    assert_eq!(item["oauth"]["has_client_secret"], true);
    assert!(item.to_string().find("s3cret").is_none(), "no route answers the secret");
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(fake.with(|r| r.basic.clone()), ["pre-1:s3cret-value"]);
    assert_eq!(fake.with(|r| r.registered.len()), 0);
    assert!(!fake.paths().iter().any(|p| p.contains("s3cret")), "never in a URL");
}

/// The review's R2: a pre-registered client is pinned to the issuer its
/// first authorize found; another one after is `issuer_changed` until the
/// client is entered again.
#[tokio::test]
async fn a_pinned_client_refuses_another_issuer() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    let put = json!({ "client_id": "pre-1", "client_secret": null });
    let path = format!("/api/mcp/connections/{id}/oauth-client");
    assert_eq!(
        h.send(&h.session(0), "PUT", &path, Some(put.clone())).await.0,
        StatusCode::OK
    );
    assert_eq!(h.authorize(&id, json!({})).await.0, StatusCode::OK);
    fake.configure(|c| c.as_path = "/other".into());
    refused(&h, &id, json!({}), StatusCode::CONFLICT, "issuer_changed").await;
    assert_eq!(h.send(&h.session(0), "PUT", &path, Some(put)).await.0, StatusCode::OK);
    assert_eq!(h.authorize(&id, json!({})).await.0, StatusCode::OK);
}

/// api-8e-8f B2: a secret is kept only for the same client id; a new id
/// needs one, or `null`.
#[tokio::test]
async fn a_secret_never_moves_to_another_client() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    let path = format!("/api/mcp/connections/{id}/oauth-client");
    let s = h.session(0);
    let (status, _, item) = h
        .send(
            &s,
            "PUT",
            &path,
            Some(json!({ "client_id": "a", "client_secret": "one" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{item}");
    let (status, _, item) = h.send(&s, "PUT", &path, Some(json!({ "client_id": "a" }))).await;
    assert_eq!(
        (status, item["oauth"]["has_client_secret"].clone()),
        (StatusCode::OK, json!(true))
    );
    let (status, _, answer) = h.send(&s, "PUT", &path, Some(json!({ "client_id": "b" }))).await;
    assert_eq!(
        (status, answer["code"].clone()),
        (StatusCode::BAD_REQUEST, json!("invalid"))
    );
    let (status, _, item) = h
        .send(
            &s,
            "PUT",
            &path,
            Some(json!({ "client_id": "b", "client_secret": null })),
        )
        .await;
    assert_eq!(
        (status, item["oauth"]["has_client_secret"].clone()),
        (StatusCode::OK, json!(false))
    );
    // Step-up, and only for `oauth_client`.
    let stale = h.session(10 * 60);
    assert_eq!(
        h.send(
            &stale,
            "PUT",
            &path,
            Some(json!({ "client_id": "c", "client_secret": null }))
        )
        .await
        .2["code"],
        "step_up_required"
    );
    let dcr = h.connection("other", &fake.mcp_url(), CredKind::OauthDcr);
    let (status, _, answer) = h
        .send(
            &s,
            "PUT",
            &format!("/api/mcp/connections/{dcr}/oauth-client"),
            Some(json!({ "client_id": "c", "client_secret": null })),
        )
        .await;
    assert_eq!(
        (status, answer["code"].clone()),
        (StatusCode::CONFLICT, json!("wrong_cred_kind"))
    );
}

/// G-7: a client saved while a grant is live waits, pending, and the grant
/// keeps working until a Connect with the new client completes; an
/// abandoned Connect leaves it so.
#[tokio::test]
async fn a_new_client_replaces_a_live_grant_s_only_when_its_connect_completes() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    let path = format!("/api/mcp/connections/{id}/oauth-client");
    let s = h.session(0);
    h.send(
        &s,
        "PUT",
        &path,
        Some(json!({ "client_id": "pre-old", "client_secret": null })),
    )
    .await;
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    let old = h.world.store.oauth_credential(&id, &h.world.key).unwrap().unwrap();
    let (_, _, item) = h
        .send(
            &s,
            "PUT",
            &path,
            Some(json!({ "client_id": "pre-new", "client_secret": null })),
        )
        .await;
    assert_eq!(item["oauth"]["client_id"], "pre-old");
    assert_eq!(item["oauth"]["pending_client"]["client_id"], "pre-new");
    // An abandoned Connect with the new one: the old grant stands.
    let (status, _, answer) = h.authorize(&id, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        query_of(answer["consent_url"].as_str().unwrap())["client_id"],
        "pre-new"
    );
    let still = h.world.store.oauth_credential(&id, &h.world.key).unwrap().unwrap();
    assert_eq!(still.client.client_id, "pre-old");
    assert_eq!(still.tokens.access_token, old.tokens.access_token);
    // Completed: the new one is the client.
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    let item = h.item(&id).await;
    assert_eq!(item["oauth"]["client_id"], "pre-new");
    assert!(item["oauth"].get("pending_client").is_none());
}

/// api-8e-8f S3: repeated clicks reuse the registration the latest
/// unconsumed flow holds, instead of registering again.
#[tokio::test]
async fn repeated_clicks_register_once() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    for _ in 0..3 {
        assert_eq!(h.authorize(&id, json!({})).await.0, StatusCode::OK);
    }
    assert_eq!(fake.with(|r| r.registered.len()), 1);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    // A grant made with it: the next Connect reuses the stored client.
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(fake.with(|r| r.registered.len()), 1);
}

/// Gateway spec §4.3: a server refusing `resource` at the token endpoint
/// is asked once more without it, and that is recorded (and used on
/// refresh).
#[tokio::test]
async fn the_exchange_is_retried_once_without_resource() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        refuse_resource_at_token: true,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(
        fake.with(|r| r.token_resources.clone()),
        [
            ("authorization_code".to_string(), Some(fake.mcp_url())),
            ("authorization_code".to_string(), None),
        ]
    );
    let grant = h.world.store.oauth_credential(&id, &h.world.key).unwrap().unwrap();
    assert!(!grant.resource_param_accepted);
}

/// Plan 8f decision 11: a consent refusing `resource` cannot be retried
/// by the callback, which redirects nowhere; the next Connect omits it.
#[tokio::test]
async fn a_consent_refusing_resource_is_retried_without_it_by_the_next_connect() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        refuse_resource_at_consent: true,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let page = h.connect(&id, &fake).await;
    assert_eq!(page.result(), "consent_denied");
    assert!(page.html.contains("Connect again"), "{}", page.html);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(fake.with(|r| r.consent_resources.clone()), [Some(fake.mcp_url()), None]);
}

/// Gateway spec §4.3: a consent completed in another browser has no flow
/// cookie, and is refused; the flow is consumed, so it cannot be retried
/// with a guessed cookie; nothing is stored.
#[tokio::test]
async fn a_callback_without_the_flow_s_cookie_is_refused() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let cookie = flow_cookie(&headers);
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let (name, _) = cookie.split_once('=').unwrap();
    let wrong = format!("{name}={}", "0".repeat(64));
    let page = h.callback(&location, Some(&wrong), &[]).await;
    assert_eq!(
        (page.status, page.result()),
        (StatusCode::BAD_REQUEST, "flow_mismatch".into())
    );
    assert!(page.headers[header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    let page = h.callback(&location, None, &[]).await;
    assert_eq!(page.result(), "flow_unknown");
    let page = h.callback(&location, Some(&cookie), &[]).await;
    assert_eq!(page.result(), "flow_unknown", "consumed");
    assert_eq!(fake.with(|r| r.exchanges), 0);
    let item = h.item(&id).await;
    assert_eq!(item["has_credential"], false);
    assert!(item.get("oauth_error").is_none(), "steps 1 and 2 name no connection");
}

/// api-8e-8f R3: one of several same-name cookies may be the flow's (a
/// sibling subdomain can set the name too).
#[tokio::test]
async fn the_flow_s_cookie_is_found_among_same_name_ones() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let cookie = flow_cookie(&headers);
    let (name, _) = cookie.split_once('=').unwrap();
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let both = format!("{name}=shadow; {cookie}");
    assert_eq!(h.callback(&location, Some(&both), &[]).await.result(), "connected");
}

/// Lane L16: `state` (or `code`) twice, or with a control character, is
/// read as neither, before any storage.
#[tokio::test]
async fn a_callback_s_parameters_are_read_only_once() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let cookie = flow_cookie(&headers);
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let q = query_of(&location);
    for doubled in [
        format!("{CALLBACK}?state={}&state=other&code={}", q["state"], q["code"]),
        format!("{CALLBACK}?state={}&code={}&code=other", q["state"], q["code"]),
        format!("{CALLBACK}?state={}%00&code={}", q["state"], q["code"]),
        // The flow is known here: only the control character refuses it.
        format!("{CALLBACK}?state={}&code={}%00", q["state"], q["code"]),
        format!("{CALLBACK}?state={}&code={}&STATE=other", q["state"], q["code"]),
    ] {
        let page = h.callback(&doubled, Some(&cookie), &[]).await;
        assert_eq!(page.result(), "flow_unknown", "{doubled}");
    }
    // The flow was not touched: the real redirect still completes.
    assert_eq!(h.callback(&location, Some(&cookie), &[]).await.result(), "connected");
}

/// The review's R6 (RFC 9207): an `iss` other than the flow's issuer, or
/// none when the server says it sends one, is a mix-up.
#[tokio::test]
async fn a_callback_from_another_issuer_is_refused() {
    for config in [
        Config {
            iss_sent: Some(Some("https://evil.example".into())),
            ..Config::default()
        },
        Config {
            iss_parameter: true,
            iss_sent: Some(None),
            ..Config::default()
        },
    ] {
        let h = Harness::new();
        let fake = FakeAs::start(config).await;
        let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
        let page = h.connect(&id, &fake).await;
        assert_eq!(page.result(), "issuer_mismatch");
        assert_eq!(h.item(&id).await["oauth_error"]["code"], "issuer_mismatch");
        assert_eq!(fake.with(|r| r.exchanges), 0);
    }
}

/// A consent the operator (or the vendor) refused: its text, as text.
#[tokio::test]
async fn a_denied_consent_shows_the_vendor_s_text_escaped() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        deny: Some("<b>no</b> thanks".into()),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let page = h.connect(&id, &fake).await;
    assert_eq!(page.result(), "consent_denied");
    assert!(
        page.html
            .contains("The vendor said: access_denied: &lt;b&gt;no&lt;/b&gt; thanks"),
        "{}",
        page.html
    );
    assert!(!page.html.contains("<b>"));
}

/// api-8e-8f B5 step 4: the operator's session that started the flow must
/// still be live.
#[tokio::test]
async fn a_callback_after_the_session_ended_is_refused() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let session = h.session(0);
    let (_, headers, answer) = h
        .send(
            &session,
            "POST",
            &format!("/api/mcp/connections/{id}/authorize"),
            Some(json!({})),
        )
        .await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let operator_session = h.operator.authenticate(&session, unix_now()).unwrap().unwrap();
    assert!(
        h.operator
            .revoke_session(&operator_session.session_id, unix_now())
            .unwrap()
    );
    let page = h.callback(&location, Some(&flow_cookie(&headers)), &[]).await;
    assert_eq!(page.result(), "session_ended");
}

/// The review's R1: a connection edited while its flow ran is not given
/// the grant, even when nothing dropped the flow.
#[tokio::test]
async fn a_connection_changed_meanwhile_gets_no_grant() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    // Behind the API's back (which would drop the flow): a path edit.
    h.world
        .store
        .update(
            &id,
            &ConnectionPatch {
                url: Some(format!("{}/moved", fake.mcp_url())),
                ..ConnectionPatch::default()
            },
            unix_now(),
        )
        .unwrap();
    let page = h.callback(&location, Some(&flow_cookie(&headers)), &[]).await;
    assert_eq!(page.result(), "connection_changed");
    assert_eq!(fake.with(|r| r.exchanges), 0, "no code sent after an edit");
    assert_eq!(h.item(&id).await["has_credential"], false);
}

/// The review's R1: an edit through the API drops the connection's flows.
#[tokio::test]
async fn an_edit_drops_the_connection_s_flows() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let (status, _, _) = h
        .send(
            &h.session(0),
            "PATCH",
            &format!("/api/mcp/connections/{id}"),
            Some(json!({ "url": format!("{}/moved", fake.mcp_url()) })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.callback(&location, Some(&flow_cookie(&headers)), &[]).await.result(),
        "flow_unknown"
    );
}

/// Gateway spec §4.1 (G-4) and api-8e-8f F3, R4: another path on the same
/// origin is shown and may be accepted, exactly; another origin may not.
#[tokio::test]
async fn a_mismatching_resource_is_accepted_only_exactly() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let other = format!("{}/v2/mcp", fake.origin());
    fake.configure(|c| c.resource = Some(other.clone()));
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let answer = refused(&h, &id, json!({}), StatusCode::CONFLICT, "resource_mismatch").await;
    assert!(
        !answer["message"].as_str().unwrap().contains("v2"),
        "only origins in an error (L11)"
    );
    assert_eq!(h.item(&id).await["oauth"]["resource_mismatch"], other);
    // A stale acceptance is a mismatch again, with the value found now
    // (api-8e-8f F3: the frontend refetches the item only on 409).
    let stale = other.clone();
    let other = format!("{}/v3/mcp", fake.origin());
    fake.configure(|c| c.resource = Some(other.clone()));
    let answer = refused(
        &h,
        &id,
        json!({ "accept_resource": stale }),
        StatusCode::CONFLICT,
        "resource_mismatch",
    )
    .await;
    assert!(
        !answer["message"].as_str().unwrap().contains("v3"),
        "only origins in an error (L11)"
    );
    let item = h.item(&id).await;
    assert_eq!(item["oauth"]["resource_mismatch"], other);
    assert!(item["oauth"].get("accepted_resource").is_none());
    let (status, _, _) = h.authorize(&id, json!({ "accept_resource": other })).await;
    assert_eq!(status, StatusCode::OK);
    let item = h.item(&id).await;
    assert_eq!(item["oauth"]["accepted_resource"], other);
    assert!(item["oauth"].get("resource_mismatch").is_none());
    // Kept for the next authorize; cleared by any URL change (F1).
    assert_eq!(h.authorize(&id, json!({})).await.0, StatusCode::OK);
    h.send(
        &h.session(0),
        "PATCH",
        &format!("/api/mcp/connections/{id}"),
        Some(json!({ "url": format!("{}/mcp/", fake.origin()) })),
    )
    .await;
    assert!(h.item(&id).await["oauth"].get("accepted_resource").is_none());
    // A resource on another origin cannot be accepted (R4).
    fake.configure(|c| c.resource = Some("https://other.example/mcp".into()));
    refused(
        &h,
        &id,
        json!({ "accept_resource": "https://other.example/mcp" }),
        StatusCode::BAD_GATEWAY,
        "resource_foreign",
    )
    .await;
}

/// O13 (plan 8b-ii's hand-off): OAuth URLs are `https` or loopback `http`,
/// even on a connection marked internal network, whose egress client would
/// send plain `http` to an internal address.
#[tokio::test]
async fn plain_http_endpoints_are_refused_even_on_the_internal_network() {
    for endpoint in ["http://auth.example/token", "http://10.0.0.1/token"] {
        let h = Harness::new();
        let fake = FakeAs::start(Config {
            token_endpoint: Some(endpoint.into()),
            ..Config::default()
        })
        .await;
        let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
        refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "insecure_metadata").await;
    }
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        authorization_endpoint: Some("http://auth.example/authorize".into()),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "insecure_metadata").await;
}

/// The egress policy covers discovery: a loopback upstream not marked
/// internal is refused, and a closed port is unreachable.
#[tokio::test]
async fn egress_refusals_and_unreachable_servers_are_told_apart() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.world.connection_with(NewConnection {
        slug: "public".into(),
        label: "Public".into(),
        url: format!("https://127.0.0.1:{}/mcp", fake.addr.port()),
        hat_id: h.world.hat(),
        cred_kind: CredKind::OauthDcr,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: false,
    });
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "egress_refused").await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let id = h.connection("closed", &format!("http://127.0.0.1:{closed}/mcp"), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "upstream_unreachable").await;
}

/// api-8e-8f B3: authorize needs step-up (F2) and an OAuth kind.
#[tokio::test]
async fn authorize_needs_step_up_and_an_oauth_kind() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (status, _, answer) = h
        .send(
            &h.session(10 * 60),
            "POST",
            &format!("/api/mcp/connections/{id}/authorize"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (status, answer["code"].clone()),
        (StatusCode::FORBIDDEN, json!("step_up_required"))
    );
    let fixed = h.world.connection("static", &fake.mcp_url(), CredKind::Static);
    let (status, _, answer) = h.authorize(&fixed, json!({})).await;
    assert_eq!(
        (status, answer["code"].clone()),
        (StatusCode::CONFLICT, json!("wrong_cred_kind"))
    );
    let (status, _, answer) = h.authorize("conn-0000000000000000", json!({})).await;
    assert_eq!(
        (status, answer["code"].clone()),
        (StatusCode::NOT_FOUND, json!("not_found"))
    );
    assert_eq!(fake.paths().len(), 0, "nothing was sent");
}

/// api-8e-8f B3: the flow cookie's attributes, tied to the flow's TTL.
#[tokio::test]
async fn the_flow_cookie_is_http_only_lax_secure_and_scoped_to_the_callback() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let set = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(set.starts_with("hennery_mcp_flow_"), "{set}");
    for part in [
        "HttpOnly",
        "SameSite=Lax",
        "Path=/api/mcp/oauth/callback",
        "Max-Age=900",
        "Secure",
    ] {
        assert!(set.contains(part), "{set}");
    }
    let now = unix_now();
    let expected: Vec<String> = (now + 897..=now + 900).map(hennery_kernel::secret::rfc3339).collect();
    assert!(
        expected.contains(&answer["expires_at"].as_str().unwrap().to_string()),
        "{answer}"
    );
}

/// api-8e-8f F5: at most 16 Connects in flight.
#[tokio::test]
async fn at_most_sixteen_connects_are_in_flight() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    for i in 0..16 {
        let id = h.connection(&format!("c{i}"), &fake.mcp_url(), CredKind::OauthDcr);
        assert_eq!(h.authorize(&id, json!({})).await.0, StatusCode::OK);
    }
    let id = h.connection("one-more", &fake.mcp_url(), CredKind::OauthDcr);
    let sent = fake.paths().len();
    refused(&h, &id, json!({}), StatusCode::TOO_MANY_REQUESTS, "too_many_flows").await;
    assert_eq!(fake.paths().len(), sent, "refused before discovery or registration");
}

/// Gateway spec §4.2: a dynamically registered client stored with its
/// grant is reused by the next Connect, also after a restart (no flow
/// holds it then): re-registration is for a new redirect or token
/// endpoint only.
#[tokio::test]
async fn a_stored_dynamic_client_is_reused_after_a_restart() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(fake.with(|r| r.registered.len()), 1);
    // A restart: a new runtime on the same database.
    let restarted = router(GatewayState {
        runtime: h.world.runtime(support::test_egress(), Arc::new(Recorder::default())),
        operator: h.operator.clone(),
    });
    let h = Harness { app: restarted, ..h };
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(fake.with(|r| r.registered.len()), 1, "no registration again");
}

/// api-8e-8f B5: the page's contract. It is served cross-site (the vendor's
/// redirect), outside the browser rules, with the kernel's CSP, no-store,
/// no-referrer and nosniff, its one script `self`, and nothing secret in it.
#[tokio::test]
async fn the_callback_page_keeps_its_contract() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let q = query_of(&location);
    let page = h
        .callback(
            &location,
            Some(&flow_cookie(&headers)),
            &[("sec-fetch-site", "cross-site")],
        )
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.html);
    assert_eq!(page.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(page.headers[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(page.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(
        page.headers[header::CONTENT_SECURITY_POLICY],
        hennery_kernel::csp::POLICY
    );
    assert!(page.html.contains("data-outcome=\"connected\""));
    assert!(
        page.html
            .contains("<script src=\"/api/mcp/oauth/callback.js\"></script>")
    );
    assert!(!page.html.contains("<script>"), "no inline script");
    for secret in [&q["code"], &q["state"], &fake.origin(), &"Label linear".to_string()] {
        assert!(!page.html.contains(secret.as_str()), "{secret} is on the page");
    }
    let resp = h
        .app
        .clone()
        .oneshot(Request::get("/api/mcp/oauth/callback.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    let script = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    for part in [
        "history.replaceState(null, \"\", \"/api/mcp/oauth/callback\")",
        "new BroadcastChannel(\"hennery-mcp-oauth\")",
        "type: \"done\"",
        "window.close()",
    ] {
        assert!(script.contains(part), "{part}");
    }
}

/// The frontend's wait ends on `ok` with a newer `status_at`: a reconnect
/// of a working grant moves it too.
#[tokio::test]
async fn a_completed_reconnect_moves_status_at_even_when_ok() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    let before = h.world.store.connection(&id).unwrap().unwrap().status_at;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    let after = h.world.store.connection(&id).unwrap().unwrap().status_at;
    assert!(after > before, "{before} {after}");
}

/// api-8e-8f B1: the redirect URI to register, before any connection.
#[tokio::test]
async fn the_redirect_uri_is_the_public_url_s_callback() {
    let h = Harness::new();
    let (status, _, answer) = h.send(&h.session(0), "GET", "/api/mcp/oauth/redirect-uri", None).await;
    assert_eq!((status, answer), (StatusCode::OK, json!({ "redirect_uri": CALLBACK })));
}

/// Gateway spec §4.6 (O3): another origin or kind deletes the OAuth client
/// with the grant; a delete takes both.
#[tokio::test]
async fn an_origin_change_or_a_delete_takes_the_client_and_the_grant() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    let count = |table: &str| -> i64 {
        h.world
            .raw()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!((count("gw_credentials"), count("gw_oauth_clients")), (1, 1));
    let (status, _, item) = h
        .send(
            &h.session(0),
            "PATCH",
            &format!("/api/mcp/connections/{id}"),
            Some(json!({ "url": "https://elsewhere.example/mcp" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert_eq!(item["status"], "not_connected");
    assert_eq!(item["oauth"]["client_id"], Value::Null);
    assert_eq!((count("gw_credentials"), count("gw_oauth_clients")), (0, 0));
    let other = h.connection("other", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&other, &fake).await.result(), "connected");
    let (status, _, _) = h
        .send(&h.session(0), "DELETE", &format!("/api/mcp/connections/{other}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!((count("gw_credentials"), count("gw_oauth_clients")), (0, 0));
}

/// api-8e-8f B6 (may change): a probe the operator asks for, through the
/// API; `none` and static connections too, and 409 without a credential.
#[tokio::test]
async fn a_probe_now_answers_the_item_after_it() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let s = h.session(10 * 60);
    let probe = |id: &str| format!("/api/mcp/connections/{id}/probe");
    let oauth = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (status, _, answer) = h.send(&s, "POST", &probe(&oauth), Some(json!({}))).await;
    assert_eq!(
        (status, answer["code"].clone()),
        (StatusCode::CONFLICT, json!("no_credential"))
    );
    assert_eq!(h.connect(&oauth, &fake).await.result(), "connected");
    let (status, _, item) = h.send(&s, "POST", &probe(&oauth), Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "no step-up: {item}");
    assert_eq!(item["status"], "ok");
    assert!(item["checked_at"].is_string());
    // A static token the upstream refuses: `needs_auth`, no refresh.
    let fixed = h.world.connection("fixed", &fake.mcp_url(), CredKind::Static);
    let (status, _, _) = h.send(&s, "POST", &probe(&fixed), Some(json!({}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    h.world.set_token(&fixed, "not-live");
    let (_, _, item) = h.send(&s, "POST", &probe(&fixed), Some(json!({}))).await;
    assert_eq!(item["status"], "needs_auth");
    // A body is required, and nothing may be in it.
    let (status, _, _) = h.send(&s, "POST", &probe(&fixed), None).await;
    assert_ne!(status, StatusCode::OK);
    let (status, _, _) = h.send(&s, "POST", &probe(&fixed), Some(json!({ "x": 1 }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _, _) = h
        .send(&s, "POST", &probe("conn-0000000000000000"), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Lane L16: a document the gateway trusts reads one way only. A key
/// twice (first or last wins elsewhere), a `\u`-escaped spelling of one, a
/// key spelt in another case or with `-`, a byte-order mark, a NUL or a
/// tab inside a URL: each document is refused, never read some way.
#[tokio::test]
async fn an_authorization_server_document_reads_one_way_or_not_at_all() {
    let good = |origin: &str| {
        format!(
            r#""issuer":"{origin}","authorization_endpoint":"{origin}/authorize","token_endpoint":"{origin}/token","registration_endpoint":"{origin}/register","code_challenge_methods_supported":["S256"]"#
        )
    };
    type Document = Box<dyn Fn(&str) -> String>;
    let cases: Vec<(&str, Document)> = vec![
        ("the baseline", Box::new(move |o| format!("{{{}}}", good(o)))),
        (
            "a key twice",
            Box::new(move |o| format!(r#"{{{},"token_endpoint":"https://evil.example/token"}}"#, good(o))),
        ),
        (
            "an escaped key twice",
            Box::new(move |o| format!(r#"{{{},"token_endpoint":"https://evil.example/token"}}"#, good(o))),
        ),
        (
            "a key in another case",
            Box::new(move |o| format!(r#"{{{},"Token_Endpoint":"https://evil.example/token"}}"#, good(o))),
        ),
        (
            "a key with a dash",
            Box::new(move |o| format!(r#"{{{},"token-endpoint":"https://evil.example/token"}}"#, good(o))),
        ),
        (
            "a byte-order mark",
            Box::new(move |o| format!("\u{feff}{{{}}}", good(o))),
        ),
        (
            "a NUL in the issuer",
            Box::new(move |o| {
                format!(
                    "{{{}}}",
                    good(o).replace(&format!("\"issuer\":\"{o}\""), &format!("\"issuer\":\"{o}\\u0000\""))
                )
            }),
        ),
        (
            "a tab inside the token endpoint",
            Box::new(move |o| {
                format!(
                    "{{{}}}",
                    good(o).replace(&format!("{o}/token"), &format!("{o}/to\\tken"))
                )
            }),
        ),
    ];
    for (what, document) in cases {
        let h = Harness::new();
        let fake = FakeAs::start(Config::default()).await;
        fake.configure(|c| c.raw_meta = Some(document(&fake.origin())));
        let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
        let (status, _, answer) = h.authorize(&id, json!({})).await;
        if what == "the baseline" {
            assert_eq!(status, StatusCode::OK, "{what}: {answer}");
        } else {
            // Refused whole; a token endpoint with a tab in it is no
            // endpoint, so its document has no usable pair either.
            assert_eq!(answer["code"], "discovery_failed", "{what}: {answer}");
        }
        assert!(!answer.to_string().contains("evil"), "{what}");
    }
}

/// Lane L16: a protected-resource document with its `resource` twice, or
/// an escaped twin, is no document; another candidate (none here) or
/// none at all.
#[tokio::test]
async fn a_resource_document_reads_one_way_or_not_at_all() {
    for raw in [
        r#"{"resource":"RESOURCE","resource":"https://evil.example/mcp","authorization_servers":["ISSUER"]}"#,
        r#"{"resource":"RESOURCE","resource":"https://evil.example/mcp","authorization_servers":["ISSUER"]}"#,
        r#"{"resource":"RESOURCE","Authorization_Servers":["https://evil.example"],"authorization_servers":["ISSUER"]}"#,
    ] {
        let h = Harness::new();
        let fake = FakeAs::start(Config {
            pr_at: PrAt::Origin,
            serve_metadata: false,
            ..Config::default()
        })
        .await;
        let raw = raw
            .replace("RESOURCE", &fake.mcp_url())
            .replace("ISSUER", &fake.issuer());
        fake.configure(|c| c.raw_pr = Some(raw.clone()));
        let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
        let (_, _, answer) = h.authorize(&id, json!({})).await;
        assert_eq!(answer["code"], "discovery_failed", "{raw}: {answer}");
        assert!(!fake.paths().iter().any(|p| p.contains("evil")));
    }
}

/// Lane L16: a token response reads one way or not at all: an access
/// token twice, or spelt otherwise, or a type that is not Bearer, is no
/// token; `expires_in` as a number or a string of digits, anything else
/// unknown.
#[tokio::test]
async fn a_token_response_reads_one_way_or_not_at_all() {
    for (raw, connected) in [
        (
            r#"{"access_token":"good-1","token_type":"Bearer","expires_in":"3600"}"#,
            true,
        ),
        (
            r#"{"access_token":"good-1","token_type":"bearer","expires_in":3600}"#,
            true,
        ),
        (
            r#"{"access_token":"good-1","access_token":"evil-1","token_type":"Bearer"}"#,
            false,
        ),
        (r#"{"access_token":"good-1","access_token":"evil-1"}"#, false),
        (r#"{"access_token":"good-1","Access_Token":"evil-1"}"#, false),
        (r#"{"access_token":"good-1","token_type":"mac"}"#, false),
        (r#"{"access_token":"good\u00001"}"#, false),
        ("\u{feff}{\"access_token\":\"good-1\"}", false),
        (
            r#"{"access_token":"good-1","refresh_token":"r","Refresh_Token":"evil"}"#,
            false,
        ),
    ] {
        let h = Harness::new();
        let fake = FakeAs::start(Config {
            raw_token: Some(raw.into()),
            ..Config::default()
        })
        .await;
        let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
        let page = h.connect(&id, &fake).await;
        if connected {
            assert_eq!(page.result(), "connected", "{raw}");
            let grant = h.world.store.oauth_credential(&id, &h.world.key).unwrap().unwrap();
            assert_eq!(grant.tokens.access_token.as_str(), "good-1");
            assert!(grant.expires_at.is_some(), "{raw}");
        } else {
            assert_eq!(page.result(), "exchange_failed", "{raw}");
            assert_eq!(h.item(&id).await["has_credential"], false, "{raw}");
        }
    }
    // `expires_in` that is not a whole number, or below zero: unknown,
    // not expired.
    for raw in [
        r#"{"access_token":"good-1","expires_in":"36e2"}"#,
        r#"{"access_token":"good-1","expires_in":-5}"#,
    ] {
        let h = Harness::new();
        let fake = FakeAs::start(Config {
            raw_token: Some(raw.into()),
            ..Config::default()
        })
        .await;
        let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
        assert_eq!(h.connect(&id, &fake).await.result(), "connected", "{raw}");
        assert_eq!(
            h.world
                .store
                .oauth_credential(&id, &h.world.key)
                .unwrap()
                .unwrap()
                .expires_at,
            None,
            "{raw}"
        );
    }
}

/// api-8e-8f B5 step 7: a refusal names only a fixed RFC 6749 `error`,
/// never the body.
#[tokio::test]
async fn an_exchange_refusal_names_only_a_known_error() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        raw_token: None,
        refusal_body: Some(json!({ "error": "invalid_grant", "error_description": "body-canary-7f3a" })),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    // A code the fake does not know: refused with its canary body.
    let tampered = location.replace("code=code-", "code=codex-");
    let page = h.callback(&tampered, Some(&flow_cookie(&headers)), &[]).await;
    assert_eq!(page.result(), "exchange_failed");
    assert!(page.html.contains("invalid_grant"), "{}", page.html);
    assert!(!page.html.contains("body-canary"));
    assert!(!h.item(&id).await.to_string().contains("body-canary"));
}

/// api-8e-8f B1, B5: the next authorize clears the latest failure, and so
/// does a completed Connect; a reconnect out of `needs_auth` is a
/// recovery the `Notifier` hears of.
#[tokio::test]
async fn a_connect_clears_the_last_failure_and_recovers_the_status() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        s256: false,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    // Recovery is from a real stored grant; a probe without a credential
    // can no longer manufacture needs_auth (Task 10's status guard).
    let (access, refresh) = fake.issue();
    support::oauth::seed_grant(&h.world, &id, &fake, &access, Some(&refresh), None);
    h.world
        .proxy_store
        .mark_needs_auth(
            &id,
            &fake.mcp_url(),
            &h.world
                .proxy_store
                .credential_stamp(&id, &fake.mcp_url())
                .unwrap()
                .unwrap(),
            "refused",
            1,
        )
        .unwrap();
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "pkce_unsupported").await;
    fake.configure(|c| c.s256 = true);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    assert!(
        h.item(&id).await.get("oauth_error").is_none(),
        "the next authorize clears it"
    );
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    // A failure recorded meanwhile: the completed Connect clears it too.
    h.world
        .store
        .set_oauth_error(&id, Some(("consent_denied", "x")), unix_now())
        .unwrap();
    let url = h.world.store.connection(&id).unwrap().unwrap().url;
    h.world
        .proxy_store
        .record_status(&id, &url, Status::NeedsAuth, Some("refused"), 1)
        .unwrap();
    let page = h.callback(&location, Some(&flow_cookie(&headers)), &[]).await;
    assert_eq!(page.result(), "connected");
    assert!(page.headers[header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    let item = h.item(&id).await;
    assert!(item.get("oauth_error").is_none());
    assert_eq!(item["status"], "ok");
    assert_eq!(h.alerts.alerts(), [(id, Alert::Recovered)]);
    assert!(
        !h.world
            .store
            .oauth_credential(item["id"].as_str().unwrap(), &h.world.key)
            .unwrap()
            .unwrap()
            .tokens
            .refresh_dead
    );
}

/// The review's R1: a client `PUT`, a kind change and a delete drop the
/// connection's flows too.
#[tokio::test]
async fn a_client_put_a_kind_change_or_a_delete_drops_the_flows() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let s = h.session(0);
    // A client `PUT`.
    let id = h.connection("pre", &fake.mcp_url(), CredKind::OauthClient);
    let path = format!("/api/mcp/connections/{id}/oauth-client");
    h.send(
        &s,
        "PUT",
        &path,
        Some(json!({ "client_id": "pre-1", "client_secret": null })),
    )
    .await;
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    h.send(
        &s,
        "PUT",
        &path,
        Some(json!({ "client_id": "pre-1", "client_secret": null })),
    )
    .await;
    assert_eq!(
        h.callback(&location, Some(&flow_cookie(&headers)), &[]).await.result(),
        "flow_unknown"
    );
    // A kind change.
    let id = h.connection("kind", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let (status, _, _) = h
        .send(
            &s,
            "PATCH",
            &format!("/api/mcp/connections/{id}"),
            Some(json!({ "cred_kind": "oauth_client" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.callback(&location, Some(&flow_cookie(&headers)), &[]).await.result(),
        "flow_unknown"
    );
    // A delete.
    let id = h.connection("gone", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let (status, _, _) = h.send(&s, "DELETE", &format!("/api/mcp/connections/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.callback(&location, Some(&flow_cookie(&headers)), &[]).await.result(),
        "flow_unknown"
    );
}

/// Gateway spec §4.5 (G-13): every credential write takes the
/// connection's refresh lock: a `PATCH`, a delete, a static token and a
/// client `PUT` each wait while a refresh holds it.
#[tokio::test]
async fn every_credential_write_waits_for_the_refresh_lock() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let s = h.session(0);
    let oauth = h.connection("pre", &fake.mcp_url(), CredKind::OauthClient);
    let fixed = h.world.connection("fixed", &fake.mcp_url(), CredKind::Static);
    let writes = [
        (
            "PATCH",
            format!("/api/mcp/connections/{oauth}"),
            Some(json!({ "label": "x" })),
            &oauth,
        ),
        (
            "PUT",
            format!("/api/mcp/connections/{oauth}/oauth-client"),
            Some(json!({ "client_id": "pre-1", "client_secret": null })),
            &oauth,
        ),
        (
            "PUT",
            format!("/api/mcp/connections/{fixed}/credential"),
            Some(json!({ "token": "tok" })),
            &fixed,
        ),
        ("DELETE", format!("/api/mcp/connections/{oauth}"), None, &oauth),
    ];
    for (method, path, body, id) in writes {
        let held = h.runtime.lock(id).await;
        let write = h.send(&s, method, &path, body.clone());
        tokio::pin!(write);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut write)
                .await
                .is_err(),
            "{method} {path} did not wait"
        );
        drop(held);
        let (status, _, answer) = write.await;
        assert!(status.is_success(), "{method} {path}: {answer}");
    }
}

/// Gateway spec §4.1: a resource found to match again clears the mismatch
/// an earlier authorize recorded.
#[tokio::test]
async fn a_resource_that_matches_again_clears_the_mismatch() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let other = format!("{}/v2/mcp", fake.origin());
    fake.configure(|c| c.resource = Some(other));
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::CONFLICT, "resource_mismatch").await;
    fake.configure(|c| c.resource = None);
    assert_eq!(h.authorize(&id, json!({})).await.0, StatusCode::OK);
    assert!(h.item(&id).await["oauth"].get("resource_mismatch").is_none());
}

/// Plan 8f decision 5: 5xx at every metadata URL is unreachable, not
/// "nothing found".
#[tokio::test]
async fn failing_metadata_everywhere_is_unreachable() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        meta_status: Some(503),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "upstream_unreachable").await;
}

/// Plan 8f decision 7 (RFC 9728 §3.2): a protected-resource document
/// without `resource` is no document: the servers it names are never
/// asked.
#[tokio::test]
async fn a_resource_document_without_resource_is_passed_over() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Origin,
        ..Config::default()
    })
    .await;
    fake.configure(|c| {
        c.raw_pr = Some(format!(
            r#"{{"authorization_servers":["{}/elsewhere-as"]}}"#,
            fake.origin()
        ));
    });
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    // The resource's origin is asked instead, and serves the metadata.
    assert_eq!(h.authorize(&id, json!({})).await.0, StatusCode::OK);
    assert!(
        !fake.paths().iter().any(|p| p.contains("elsewhere-as")),
        "{:?}",
        fake.paths()
    );
}

/// Gateway spec §4: every metadata URL is `https` or loopback `http`: a
/// challenge naming a plain `http` document, a document naming a plain
/// `http` server, and a plain `http` registration endpoint are refused
/// before anything is fetched there.
#[tokio::test]
async fn plain_http_documents_and_servers_are_refused() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Origin,
        ..Config::default()
    })
    .await;
    fake.configure(|c| {
        c.raw_pr = Some(format!(
            r#"{{"resource":"{}","authorization_servers":["http://as.example"]}}"#,
            fake.mcp_url()
        ));
    });
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "insecure_metadata").await;
    // A registration endpoint at plain `http`.
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        raw_meta: Some("PLACEHOLDER".into()),
        ..Config::default()
    })
    .await;
    let o = fake.origin();
    fake.configure(|c| {
        c.raw_meta = Some(format!(
            r#"{{"issuer":"{o}","authorization_endpoint":"{o}/authorize","token_endpoint":"{o}/token","registration_endpoint":"http://as.example/register","code_challenge_methods_supported":["S256"]}}"#
        ));
    });
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "insecure_metadata").await;
    // A challenge naming a plain `http` document elsewhere.
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Challenge,
        challenge_url: Some("http://as.example/prm".into()),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "insecure_metadata").await;
}

/// RFC 7591: a vendor that issues a secret to a dynamically registered
/// client gets it back as Basic auth; one that answers without a client
/// id is a refusal.
#[tokio::test]
async fn a_registration_with_a_secret_uses_it_and_one_without_an_id_is_refused() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        dcr_secret: true,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(fake.with(|r| r.basic.len()), 1);
    assert_eq!(h.item(&id).await["oauth"]["has_client_secret"], true);
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        dcr_without_client_id: true,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "registration_refused").await;
}

/// RFC 8414: a server that lists only `client_secret_post` gets the
/// secret in the form.
#[tokio::test]
async fn a_server_taking_only_client_secret_post_gets_the_secret_in_the_form() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        registration: false,
        confidential: Some(("pre-1".into(), "posted".into())),
        auth_methods: vec!["client_secret_post".into()],
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    h.send(
        &h.session(0),
        "PUT",
        &format!("/api/mcp/connections/{id}/oauth-client"),
        Some(json!({ "client_id": "pre-1", "client_secret": "posted" })),
    )
    .await;
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert!(fake.with(|r| r.posted_secret && r.basic.is_empty()));
}

/// api-8e-8f B5 step 7: a token endpoint that is down is unreachable, not
/// a refusal.
#[tokio::test]
async fn an_exchange_with_a_failing_token_endpoint_is_unreachable() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        token_status: Some(503),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let page = h.connect(&id, &fake).await;
    assert_eq!(
        (page.status, page.result()),
        (StatusCode::BAD_GATEWAY, "upstream_unreachable".into())
    );
    assert_eq!(h.item(&id).await["oauth_error"]["code"], "upstream_unreachable");
}

#[tokio::test]
async fn a_client_for_an_unknown_connection_is_not_found() {
    let h = Harness::new();
    let (status, _, answer) = h
        .send(
            &h.session(0),
            "PUT",
            "/api/mcp/connections/conn-0000000000000000/oauth-client",
            Some(json!({ "client_id": "pre-1", "client_secret": null })),
        )
        .await;
    assert_eq!(
        (status, answer["code"].clone()),
        (StatusCode::NOT_FOUND, json!("not_found"))
    );
}

/// The runtime is one: the API and the proxy share its locks (plan 8f
/// decision 3).
#[tokio::test]
async fn the_api_s_runtime_is_the_one_it_was_given() {
    let h = Harness::new();
    let _guard = h.runtime.lock("conn-x").await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), h.runtime.lock("conn-x"))
            .await
            .is_err()
    );
}

/// api-8e-8f B4: `accept_resource` with no protected-resource document to
/// name one is refused, not ignored.
#[tokio::test]
async fn accepting_a_resource_no_document_names_is_invalid() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Nowhere,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(
        &h,
        &id,
        json!({ "accept_resource": fake.mcp_url() }),
        StatusCode::BAD_REQUEST,
        "invalid",
    )
    .await;
}

/// The review's R2: a pinned client's secret goes only to the token
/// endpoint it was pinned with, even under the same issuer.
#[tokio::test]
async fn a_pinned_client_refuses_another_token_endpoint() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    let path = format!("/api/mcp/connections/{id}/oauth-client");
    let put = json!({ "client_id": "pre-1", "client_secret": null });
    assert_eq!(h.send(&h.session(0), "PUT", &path, Some(put)).await.0, StatusCode::OK);
    assert_eq!(h.authorize(&id, json!({})).await.0, StatusCode::OK);
    let moved = format!("{}/token2", fake.origin());
    fake.configure(|c| c.token_endpoint = Some(moved));
    refused(&h, &id, json!({}), StatusCode::CONFLICT, "issuer_changed").await;
}

/// The egress policy's refusal of a document discovery fetches ends it
/// as that (`egress_refused`), never as a server that was not there or did
/// not answer: here an authorization server named with credentials in its
/// URL, which egress refuses whatever the allowance.
#[tokio::test]
async fn a_metadata_fetch_the_egress_policy_refuses_ends_discovery() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let refused_at = format!("https://u:p@127.0.0.1:{}", fake.addr.port());
    let mcp = fake.mcp_url();
    fake.configure(|c| {
        c.raw_pr = Some(json!({ "resource": mcp, "authorization_servers": [refused_at] }).to_string());
    });
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "egress_refused").await;
}

/// Each `RegisterError` (gateway spec §4.2) has its code: the egress
/// policy's refusal and an endpoint that fails (5xx) besides the vendor's
/// refusal and an answer without a client, pinned above.
#[tokio::test]
async fn a_registration_refused_by_egress_or_failing_has_its_code() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let o = fake.origin();
    // Credentials in a URL are refused by the egress policy, whatever the
    // allowance.
    let refused_at = format!("https://u:p@127.0.0.1:{}/register", fake.addr.port());
    fake.configure(|c| {
        c.raw_meta = Some(format!(
            r#"{{"issuer":"{o}","authorization_endpoint":"{o}/authorize","token_endpoint":"{o}/token","registration_endpoint":"{refused_at}","code_challenge_methods_supported":["S256"],"authorization_response_iss_parameter_supported":true}}"#
        ));
    });
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "egress_refused").await;
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        dcr_refusal: Some((503, json!({ "error": "temporarily_unavailable" }))),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "upstream_unreachable").await;
}

/// RFC 7591 answers are checked before use (lane L16): a client id that is
/// not 1 to 512 visible characters is no client; a secret that is not a
/// valid one is dropped (a public client); a `client_secret_post` client
/// gets its secret in the form; a public client is stored as `none`.
#[tokio::test]
async fn a_registration_s_answer_is_checked_before_use() {
    let registered = |body: Value| Config {
        dcr_refusal: Some((201, body)),
        ..Config::default()
    };
    let h = Harness::new();
    let fake = FakeAs::start(registered(json!({ "client_id": "has space" }))).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "registration_refused").await;
    // A secret with a space: dropped; the client is public.
    let h = Harness::new();
    let fake = FakeAs::start(registered(
        json!({ "client_id": "pre-x", "client_secret": "two words" }),
    ))
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(h.item(&id).await["oauth"]["has_client_secret"], false);
    assert!(fake.with(|r| r.basic.is_empty() && !r.posted_secret));
    let method: String = h
        .world
        .raw()
        .query_row(
            "SELECT token_endpoint_auth_method FROM gw_oauth_clients WHERE connection_id = ?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(method, "none");
    // `client_secret_post`, as the registration said.
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        confidential: Some(("pre-y".into(), "posted-secret".into())),
        ..registered(json!({
            "client_id": "pre-y",
            "client_secret": "posted-secret",
            "token_endpoint_auth_method": "client_secret_post",
        }))
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert!(fake.with(|r| r.posted_secret && r.basic.is_empty()));
}

/// Each `TokenError` at the exchange has its code (api-8e-8f B5 step 7):
/// the egress policy's refusal, and a transport failure (a closed port).
#[tokio::test]
async fn an_exchange_refused_by_egress_or_unreachable_has_its_code() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        iss_parameter: true,
        ..Config::default()
    })
    .await;
    let port = fake.addr.port();
    fake.configure(|c| c.token_endpoint = Some(format!("https://u:p@127.0.0.1:{port}/token")));
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let page = h.connect(&id, &fake).await;
    assert_eq!(
        (page.status, page.result()),
        (StatusCode::BAD_GATEWAY, "egress_refused".into())
    );
    // api-8e-8f B5: the page names no address; the operator's item keeps
    // the egress policy's reason, by origin only (lane L11).
    assert!(!page.html.contains("127.0.0.1"), "{}", page.html);
    let error = &h.item(&id).await["oauth_error"];
    assert_eq!(error["code"], "egress_refused");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("refused by the egress policy"),
        "{error}"
    );
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        iss_parameter: true,
        ..Config::default()
    })
    .await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    fake.configure(|c| c.token_endpoint = Some(format!("http://127.0.0.1:{closed}/token")));
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let page = h.connect(&id, &fake).await;
    assert_eq!(
        (page.status, page.result()),
        (StatusCode::BAD_GATEWAY, "upstream_unreachable".into())
    );
}

/// api-8e-8f B5 step 7: an `error` that is not one of RFC 6749's fixed
/// values is shown as the HTTP status only.
#[tokio::test]
async fn an_exchange_refusal_with_an_unknown_error_shows_only_its_status() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        refusal_body: Some(json!({ "error": "weird-canary-error" })),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let tampered = location.replace("code=code-", "code=codex-");
    let page = h.callback(&tampered, Some(&flow_cookie(&headers)), &[]).await;
    assert_eq!(page.result(), "exchange_failed");
    assert!(page.html.contains("(HTTP 400)"), "{}", page.html);
    assert!(!page.html.contains("weird-canary"));
}

/// Lane L16: a token response's tokens are checked: an access token that
/// is not a valid token, or a refresh token that is not a string, is no
/// token response.
#[tokio::test]
async fn a_token_response_s_tokens_are_checked() {
    for raw in [
        r#"{"access_token":"two words","token_type":"Bearer"}"#,
        r#"{"access_token":"good-1","refresh_token":5}"#,
    ] {
        let h = Harness::new();
        let fake = FakeAs::start(Config {
            raw_token: Some(raw.into()),
            ..Config::default()
        })
        .await;
        let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
        assert_eq!(h.connect(&id, &fake).await.result(), "exchange_failed", "{raw}");
        assert_eq!(h.item(&id).await["has_credential"], false, "{raw}");
    }
}

/// Gateway spec §4.1: an endpoint with a fragment is no endpoint.
#[tokio::test]
async fn a_token_endpoint_with_a_fragment_is_no_endpoint() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let o = fake.origin();
    fake.configure(|c| c.token_endpoint = Some(format!("{o}/token#x")));
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "discovery_failed").await;
}

/// Plan 8f decision 5: an oversized document is "not here", not an
/// outage; one candidate that fails (5xx) makes nothing found an outage.
#[tokio::test]
async fn an_oversized_document_is_not_here_and_a_failing_one_is_an_outage() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Origin,
        serve_metadata: false,
        ..Config::default()
    })
    .await;
    let huge = format!(
        r#"{{"resource":"{}","pad":"{}"}}"#,
        fake.mcp_url(),
        "x".repeat(1024 * 1024)
    );
    fake.configure(|c| c.raw_pr = Some(huge));
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "discovery_failed").await;
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Nowhere,
        pr_status: Some(503),
        serve_metadata: false,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "upstream_unreachable").await;
}

/// The review's R1, gateway spec §4.6: an edit that drops the flows also
/// forgets that the server refused `resource`: the next Connect sends it
/// again.
#[tokio::test]
async fn an_edit_forgets_a_refused_resource() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        refuse_resource_at_consent: true,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(h.connect(&id, &fake).await.result(), "consent_denied");
    let s = h.session(0);
    let path = format!("/api/mcp/connections/{id}");
    for kind in ["oauth_client", "oauth_dcr"] {
        let (status, _, _) = h.send(&s, "PATCH", &path, Some(json!({ "cred_kind": kind }))).await;
        assert_eq!(status, StatusCode::OK, "{kind}");
    }
    fake.configure(|c| c.refuse_resource_at_consent = false);
    assert_eq!(h.connect(&id, &fake).await.result(), "connected");
    assert_eq!(
        fake.with(|r| r.consent_resources.clone()),
        [Some(fake.mcp_url()), Some(fake.mcp_url())]
    );
}

/// Gateway spec §4.6: another origin clears the latest Connect's failure
/// with the grant.
#[tokio::test]
async fn an_origin_change_clears_the_last_failure() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        s256: false,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    refused(&h, &id, json!({}), StatusCode::BAD_GATEWAY, "pkce_unsupported").await;
    let (status, _, item) = h
        .send(
            &h.session(0),
            "PATCH",
            &format!("/api/mcp/connections/{id}"),
            Some(json!({ "url": "https://elsewhere.example/mcp" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert!(item.get("oauth_error").is_none(), "{item}");
}

/// api-8e-8f B2: a client id and a secret are checked: 400 `invalid`.
#[tokio::test]
async fn a_pre_registered_client_s_id_and_secret_are_checked() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    let path = format!("/api/mcp/connections/{id}/oauth-client");
    for body in [
        json!({ "client_id": "has space", "client_secret": null }),
        json!({ "client_id": "", "client_secret": null }),
        json!({ "client_id": "pre-1", "client_secret": "two words" }),
    ] {
        let (status, _, answer) = h.send(&h.session(0), "PUT", &path, Some(body.clone())).await;
        assert_eq!(
            (status, answer["code"].clone()),
            (StatusCode::BAD_REQUEST, json!("invalid")),
            "{body}"
        );
    }
    assert_eq!(h.item(&id).await["oauth"]["client_id"], Value::Null);
}

/// Gateway spec §4.5: the callback stores the grant under the
/// connection's refresh lock.
#[tokio::test]
async fn the_callback_stores_the_grant_under_the_lock() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let cookie = flow_cookie(&headers);
    let held = h.runtime.lock(&id).await;
    let callback = h.callback(&location, Some(&cookie), &[]);
    tokio::pin!(callback);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut callback)
            .await
            .is_err(),
        "the callback did not wait"
    );
    assert_eq!(h.item(&id).await["has_credential"], false);
    drop(held);
    assert_eq!(callback.await.result(), "connected");
}

/// F1: without RFC 9207, the three endpoints must have one origin.
#[tokio::test]
async fn endpoints_without_iss_share_one_origin() {
    for endpoint in ["authorization_endpoint", "token_endpoint", "registration_endpoint"] {
        for iss in [false, true] {
            let h = Harness::new();
            let fake = FakeAs::start(Config::default()).await;
            let o = fake.origin();
            let mut meta = json!({ "issuer": o, "authorization_endpoint": format!("{o}/authorize"),
                "token_endpoint": format!("{o}/token"), "registration_endpoint": format!("{o}/register"),
                "code_challenge_methods_supported": ["S256"],
                "authorization_response_iss_parameter_supported": iss });
            meta[endpoint] = json!(format!("https://other.example/{endpoint}"));
            fake.configure(|c| c.raw_meta = Some(meta.to_string()));
            // A pre-registered client avoids actually calling a foreign registration endpoint.
            let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
            h.world
                .store
                .set_oauth_client(
                    &id,
                    "pre-1",
                    hennery_gateway::store::SecretInput::Clear,
                    &h.world.key,
                    unix_now(),
                )
                .unwrap();
            let (status, _, answer) = h.authorize(&id, json!({})).await;
            assert_eq!(
                status,
                if iss { StatusCode::OK } else { StatusCode::BAD_GATEWAY },
                "{endpoint}, iss={iss}: {answer}"
            );
            if !iss {
                assert_eq!(answer["code"], "discovery_failed");
            }
        }
    }
}

/// F2: check every part of the generation before sending the code, even if
/// the edit bypassed flow invalidation (the purge hook cannot hold this lock).
#[tokio::test]
async fn a_changed_connection_never_exchanges_its_code() {
    for change in ["deleted", "kind", "allowance", "client", "pending", "secret"] {
        let h = Harness::new();
        let fake = FakeAs::start(Config {
            registration: false,
            ..Config::default()
        })
        .await;
        let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
        h.world
            .store
            .set_oauth_client(
                &id,
                "pre-1",
                hennery_gateway::store::SecretInput::Clear,
                &h.world.key,
                unix_now(),
            )
            .unwrap();
        let (_, headers, answer) = h.authorize(&id, json!({})).await;
        let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
        match change {
            "deleted" => {
                h.world.store.delete(&id).unwrap();
            }
            "kind" => {
                h.world
                    .raw()
                    .execute("UPDATE gw_connections SET cred_kind = 'static' WHERE id = ?1", [&id])
                    .unwrap();
            }
            "allowance" => {
                h.world
                    .raw()
                    .execute("UPDATE gw_connections SET internal_network = 0 WHERE id = ?1", [&id])
                    .unwrap();
            }
            "client" => {
                h.world
                    .raw()
                    .execute(
                        "UPDATE gw_oauth_clients SET client_id = 'pre-other' WHERE connection_id = ?1",
                        [&id],
                    )
                    .unwrap();
            }
            "pending" => {
                h.world
                    .raw()
                    .execute(
                        "UPDATE gw_oauth_clients SET pending_client_id = 'pre-other' WHERE connection_id = ?1",
                        [&id],
                    )
                    .unwrap();
            }
            "secret" => {
                h.world
                    .store
                    .set_oauth_client(
                        &id,
                        "pre-1",
                        hennery_gateway::store::SecretInput::Set(zeroize::Zeroizing::new("replacement-secret".into())),
                        &h.world.key,
                        unix_now(),
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let page = h.callback(&location, Some(&flow_cookie(&headers)), &[]).await;
        assert_eq!(page.result(), "connection_changed", "{change}");
        assert_eq!(fake.with(|r| r.exchanges), 0, "{change}: no exchange");
    }
}

/// F3: discovery may race another authorize's successful issuer pin.
#[tokio::test]
async fn a_lost_client_pin_is_reread_before_consent() {
    use hennery_gateway::store::{SecretInput, Slot};
    for slot in [Slot::Active, Slot::Pending] {
        for winner in ["same", "issuer", "endpoint", "client", "secret", "missing", "deleted"] {
            let h = Harness::new();
            let server = support::upstream::FakeUpstream::start().await;
            let o = server.url("");
            let id = h.connection("linear", &server.url("/mcp"), CredKind::OauthClient);
            h.world
                .store
                .set_oauth_client(&id, "pre-1", SecretInput::Clear, &h.world.key, unix_now())
                .unwrap();
            if matches!(slot, Slot::Pending) {
                let fake = FakeAs::start(Config::default()).await;
                let (a, r) = fake.issue();
                support::oauth::seed_grant(&h.world, &id, &fake, &a, Some(&r), None);
                h.world
                    .store
                    .set_oauth_client(&id, "pre-1", SecretInput::Clear, &h.world.key, unix_now())
                    .unwrap();
            }
            let store = h.world.store.clone();
            let pin_id = id.clone();
            let key = h.world.key.clone();
            let db = h.world.db.clone();
            server.reply(move |seen, _| {
                if seen.uri.contains("oauth-authorization-server") {
                    let pinned = if winner == "issuer" {
                        "https://other.example".to_string()
                    } else {
                        o.clone()
                    };
                    let endpoint = if winner == "endpoint" {
                        format!("{o}/other-token")
                    } else {
                        format!("{o}/token")
                    };
                    assert!(store.pin_client(&pin_id, slot, "pre-1", &pinned, &endpoint).unwrap());
                    let conn = rusqlite::Connection::open(&db).unwrap();
                    match winner {
                        "client" => {
                            let column = if slot == Slot::Active {
                                "client_id"
                            } else {
                                "pending_client_id"
                            };
                            conn.execute(
                                &format!("UPDATE gw_oauth_clients SET {column} = 'pre-other' WHERE connection_id = ?1"),
                                [&pin_id],
                            )
                            .unwrap();
                        }
                        "secret" => {
                            let (column, field) = if slot == Slot::Active {
                                ("client_secret_ciphertext", hennery_gateway::crypto::CLIENT_SECRET)
                            } else {
                                (
                                    "pending_secret_ciphertext",
                                    hennery_gateway::crypto::PENDING_CLIENT_SECRET,
                                )
                            };
                            let sealed = hennery_gateway::crypto::seal(&key, &pin_id, field, b"new-secret");
                            conn.execute(
                                &format!("UPDATE gw_oauth_clients SET {column} = ?2 WHERE connection_id = ?1"),
                                rusqlite::params![pin_id, sealed],
                            )
                            .unwrap();
                        }
                        "deleted" => {
                            assert!(store.delete(&pin_id).unwrap());
                        }
                        "missing" => {
                            conn.execute("DELETE FROM gw_oauth_clients WHERE connection_id = ?1", [&pin_id])
                                .unwrap();
                        }
                        _ => {}
                    }
                    support::upstream::json(
                        StatusCode::OK,
                        &json!({ "issuer": o,
                        "authorization_endpoint": format!("{o}/authorize"), "token_endpoint": format!("{o}/token"),
                        "code_challenge_methods_supported": ["S256"] }),
                    )
                } else {
                    StatusCode::NOT_FOUND.into_response()
                }
            });
            let (status, headers, answer) = h.authorize(&id, json!({})).await;
            assert_eq!(
                status,
                if winner == "same" {
                    StatusCode::OK
                } else if winner == "deleted" {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::CONFLICT
                },
                "{answer}"
            );
            if winner != "same" {
                assert_eq!(
                    answer["code"],
                    if winner == "deleted" {
                        "not_found"
                    } else {
                        "issuer_changed"
                    }
                );
                assert!(answer.get("consent_url").is_none());
                assert!(!headers.contains_key(header::SET_COOKIE));
                assert_eq!(h.runtime.flows.live("", unix_now()), 0);
            }
        }
    }
}

/// Cheap review note: after the pre-exchange check, a same-ID secret edit
/// still must not be overwritten when the exchange finally finishes.
#[tokio::test]
async fn a_secret_changed_during_exchange_is_not_overwritten() {
    let h = Harness::new();
    let fake = FakeAs::start(Config {
        gate_token: true,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    h.world
        .store
        .set_oauth_client(
            &id,
            "pre-1",
            hennery_gateway::store::SecretInput::Clear,
            &h.world.key,
            unix_now(),
        )
        .unwrap();
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let cookie = flow_cookie(&headers);
    let change = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !fake.paths().iter().any(|p| p == "/token") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        h.world
            .store
            .set_oauth_client(
                &id,
                "pre-1",
                hennery_gateway::store::SecretInput::Set(zeroize::Zeroizing::new("replacement-secret".into())),
                &h.world.key,
                unix_now(),
            )
            .unwrap();
        fake.open_gate();
    };
    let (page, _) = tokio::join!(h.callback(&location, Some(&cookie), &[]), change);
    assert_eq!(page.result(), "connection_changed");
    assert_eq!(
        h.world
            .store
            .oauth_clients(&id, &h.world.key)
            .unwrap()
            .unwrap()
            .active
            .unwrap()
            .secret
            .as_deref()
            .map(String::as_str),
        Some("replacement-secret")
    );
}

#[tokio::test]
async fn an_unreadable_connection_before_exchange_sends_nothing() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthClient);
    h.world
        .store
        .set_oauth_client(
            &id,
            "pre-1",
            hennery_gateway::store::SecretInput::Clear,
            &h.world.key,
            unix_now(),
        )
        .unwrap();
    let (_, headers, answer) = h.authorize(&id, json!({})).await;
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    h.world.raw().execute("DROP TABLE gw_oauth_clients", []).unwrap();
    let page = h.callback(&location, Some(&flow_cookie(&headers)), &[]).await;
    assert_eq!(
        (page.status, page.result()),
        (StatusCode::INTERNAL_SERVER_ERROR, "internal".into())
    );
    assert_eq!(fake.with(|r| r.exchanges), 0);
}

/// F2 on the merged 8e hook: purge drops only that hat's pending flows.
#[tokio::test]
async fn a_hat_purge_drops_its_oauth_flows() {
    use hennery_gateway::session::{GatewayMcp, SessionMcp};
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let other_hat = h.world.other_hat();
    let mut new = h.world.new_connection("other", &other_hat);
    new.url = fake.mcp_url();
    new.cred_kind = CredKind::OauthDcr;
    new.internal_network = true;
    new.hat_id = other_hat;
    let other = h.world.connection_with(new);
    assert_eq!(h.authorize(&id, json!({})).await.0, StatusCode::OK);
    assert_eq!(h.authorize(&other, json!({})).await.0, StatusCode::OK);
    assert_eq!(h.runtime.flows.live("", unix_now()), 2);
    let gateway = GatewayState {
        runtime: h.runtime.clone(),
        operator: h.operator.clone(),
    };
    GatewayMcp::new(&gateway).purge_hat(&h.world.hat()).await.unwrap();
    assert_eq!(h.runtime.flows.live("", unix_now()), 1);
    assert!(h.world.store.connection(&id).unwrap().is_none());
    assert!(h.world.store.connection(&other).unwrap().is_some());
}
