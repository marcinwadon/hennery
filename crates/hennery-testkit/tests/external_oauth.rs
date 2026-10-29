//! Isolated OAuth/Streamable HTTP, including actual TLS through both shipped proxy recipes.
//! This is not a claim of public Internet reachability or real Desktop acceptance.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hennery_kernel::{
    egress::Url,
    hosts::{Enrollment, Hosts},
    operator::Operator,
    secret::unix_now,
};
use hennery_sessions::{AppState, store::Store};
use hennery_testkit::{OWNER_PASSWORD, reverse_proxy::RecipeProxy, tls::TestCa};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
struct Fixture {
    body_entered: std::sync::Arc<tokio::sync::Notify>,
    body_release: std::sync::Arc<tokio::sync::Notify>,
    _dir: tempfile::TempDir,
    state: AppState,
    base: String,
    origin: String,
    http: reqwest::Client,
    operator: reqwest::Client,
    _proxy: Option<RecipeProxy>,
    task: tokio::task::JoinHandle<()>,
    hat: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.shutdown.cancel();
        self.task.abort();
    }
}
impl Fixture {
    async fn new(proxy: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let hosts = Hosts::open(&db).unwrap();
        hosts
            .register(
                "host",
                &Enrollment {
                    public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                    name: "Scratch host".into(),
                    platform: "test".into(),
                    host_version: "test".into(),
                },
                0,
            )
            .unwrap();
        let hat = hosts.host("host").unwrap().unwrap().default_hat_id;
        let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body_entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let body_release = std::sync::Arc::new(tokio::sync::Notify::new());
        let entered = body_entered.clone();
        let release = body_release.clone();
        let router = hennery_sessions::router(state.clone()).layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    let req = if req.uri().path() == "/api/external-oauth/consent"
                        && req.headers().contains_key("x-fixture-pause-body")
                    {
                        let (parts, body) = req.into_parts();
                        let body = axum::body::Body::from_stream(futures::stream::once(async move {
                            // Polled by JSON extraction only after real operator/step-up middleware.
                            entered.notify_one();
                            release.notified().await;
                            axum::body::to_bytes(body, 16 * 1024).await
                        }));
                        axum::extract::Request::from_parts(parts, body)
                    } else {
                        req
                    };
                    next.run(req).await
                }
            },
        ));
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let ca = TestCa::new();
        let front = if let Some(kind) = proxy {
            Some(RecipeProxy::start(kind, addr, &ca, dir.path()).await)
        } else {
            None
        };
        let origin = front
            .as_ref()
            .map(|p| p.origin())
            .unwrap_or_else(|| "https://collector.example".into());
        let base = front
            .as_ref()
            .map(|p| p.origin())
            .unwrap_or_else(|| format!("http://{addr}"));
        let setup = state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
        state
            .operator
            .set_up(&setup, OWNER_PASSWORD, &origin, unix_now())
            .unwrap();
        let cookie = state
            .operator
            .open_session("fixture", &hennery_testkit::owner_phc(&state.operator), unix_now())
            .unwrap()
            .unwrap();
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_pem(ca.pem.as_bytes()).unwrap())
            .build()
            .unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("cookie", format!("hennery_session={cookie}").parse().unwrap());
        headers.insert("origin", origin.parse().unwrap());
        let operator = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_pem(ca.pem.as_bytes()).unwrap())
            .default_headers(headers)
            .build()
            .unwrap();
        Self {
            body_entered,
            body_release,
            _dir: dir,
            state,
            base,
            origin,
            http,
            operator,
            _proxy: front,
            task,
            hat,
        }
    }
    fn resource(&self) -> String {
        format!("{}/mcp/external/hennery", self.origin)
    }
    async fn client(&self) -> String {
        let r=self.http.post(format!("{}/oauth/external/register",self.base)).json(&json!({"client_name":"Remote fixture","redirect_uris":["https://claude.ai/api/mcp/auth_callback"],"token_endpoint_auth_method":"none","grant_types":["authorization_code","refresh_token"],"response_types":["code"]})).send().await.unwrap();
        assert_eq!(r.status(), 201);
        let m: Value = r.json().await.unwrap();
        assert!(m["client_secret"].is_null());
        m["client_id"].as_str().unwrap().into()
    }
    fn auth(&self, client: &str) -> Vec<(&str, String)> {
        vec![
            ("client_id", client.into()),
            ("redirect_uri", "https://claude.ai/api/mcp/auth_callback".into()),
            ("response_type", "code".into()),
            (
                "code_challenge",
                URL_SAFE_NO_PAD.encode(Sha256::digest("a".repeat(43).as_bytes())),
            ),
            ("code_challenge_method", "S256".into()),
            ("resource", self.resource()),
            ("scope", "hennery:discover hennery:sessions:read offline_access".into()),
            ("state", "fixture-state".into()),
        ]
    }
    async fn pending(&self, client: &str) -> String {
        let r = self
            .http
            .get(format!("{}/oauth/external/authorize", self.base))
            .query(&self.auth(client))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 303);
        let next = Url::parse(&self.origin)
            .unwrap()
            .join(r.headers()["location"].to_str().unwrap())
            .unwrap();
        assert_eq!(next.origin().ascii_serialization(), self.origin);
        next.query_pairs().find(|(k, _)| k == "request").unwrap().1.into()
    }
    fn grant(&self, id: &str, allow: bool) -> Value {
        json!({"request_id":id,"allow":allow,"hat_id":self.hat,"roots":[{"host_id":"host","root":"/project"}],"read_metadata":true,"existing_sessions":true,"expires_in_days":7})
    }
    async fn code(&self, id: &str) -> String {
        let r = self
            .operator
            .post(format!("{}/api/external-oauth/consent", self.base))
            .json(&self.grant(id, true))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let value: Value = r.json().await.unwrap();
        let callback = Url::parse(value["redirect_uri"].as_str().unwrap()).unwrap();
        assert_eq!(callback.host_str(), Some("claude.ai"));
        assert_eq!(
            callback.query_pairs().find(|(k, _)| k == "state").unwrap().1,
            "fixture-state"
        );
        assert_eq!(callback.query_pairs().find(|(k, _)| k == "iss").unwrap().1, self.origin);
        callback.query_pairs().find(|(k, _)| k == "code").unwrap().1.into()
    }
    fn token_form(&self, client: &str, code: &str) -> Vec<(&str, String)> {
        vec![
            ("grant_type", "authorization_code".into()),
            ("client_id", client.into()),
            ("resource", self.resource()),
            ("redirect_uri", "https://claude.ai/api/mcp/auth_callback".into()),
            ("code", code.into()),
            ("code_verifier", "a".repeat(43)),
        ]
    }
    async fn token(&self, client: &str, code: &str) -> Value {
        let r = self
            .http
            .post(format!("{}/oauth/external/token", self.base))
            .form(&self.token_form(client, code))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()["cache-control"], "no-store");
        r.json().await.unwrap()
    }
    async fn rpc(&self, token: &str, name: &str, args: Value) -> Value {
        self.http
            .post(format!("{}/mcp/external/hennery", self.base))
            .bearer_auth(token)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn logout_after_stepup_but_before_body_completion_cannot_issue_authority() {
    let f = Fixture::new(None).await;
    let client = f.client().await;
    let id = f.pending(&client).await;
    let http = f.operator.clone();
    let url = format!("{}/api/external-oauth/consent", f.base);
    let grant = f.grant(&id, true);
    let consent = tokio::spawn(async move {
        http.post(url)
            .header("x-fixture-pause-body", "1")
            .json(&grant)
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), f.body_entered.notified())
        .await
        .unwrap();
    assert_eq!(
        f.operator
            .post(format!("{}/api/auth/logout", f.base))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    f.body_release.notify_one();
    assert_eq!(consent.await.unwrap().status(), 401);
    assert!(f.state.store.external_clients().unwrap().is_empty());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_remote_oauth_round_trip_over_real_tls_and_shipped_proxies() {
    for kind in ["caddy", "nginx"] {
        let f = Fixture::new(Some(kind)).await;
        assert!(
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .get(format!("{}/.well-known/oauth-authorization-server", f.base))
                .send()
                .await
                .is_err(),
            "untrusted CA must fail"
        );
        let unauth = f
            .http
            .post(format!("{}/mcp/external/hennery", f.base))
            .send()
            .await
            .unwrap();
        assert_eq!(unauth.status(), 401);
        assert!(
            unauth.headers()["www-authenticate"]
                .to_str()
                .unwrap()
                .contains(&format!(
                    "resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp/external/hennery\"",
                    f.origin
                ))
        );
        let prm: Value = f
            .http
            .get(format!(
                "{}/.well-known/oauth-protected-resource/mcp/external/hennery",
                f.base
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(prm["resource"], f.resource());
        assert_eq!(prm["authorization_servers"][0], f.origin);
        let metadata: Value = f
            .http
            .get(format!("{}/.well-known/oauth-authorization-server", f.base))
            .header("host", "attacker.example")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(metadata["issuer"], f.origin);
        assert_eq!(metadata["client_id_metadata_document_supported"], true);
        assert_eq!(metadata["token_endpoint_auth_methods_supported"], json!(["none"]));
        assert_eq!(metadata["code_challenge_methods_supported"], json!(["S256"]));
        f.state
            .store
            .create_session("allowed", "host", "claude", "/project", &f.hat, None)
            .unwrap();
        f.state
            .store
            .create_session("outside", "host", "claude", "/project-other", &f.hat, None)
            .unwrap();
        let client = f.client().await;
        let id = f.pending(&client).await;
        let pending = f
            .operator
            .get(format!("{}/api/external-oauth/requests/{id}", f.base))
            .send()
            .await
            .unwrap();
        assert_eq!(pending.status(), 200);
        assert!(
            f.state.store.external_clients().unwrap().is_empty(),
            "no grant before consent"
        );
        let code = f.code(&id).await;
        let token = f.token(&client, &code).await;
        let access = token["access_token"].as_str().unwrap();
        let init:Value=f.http.post(format!("{}/mcp/external/hennery",f.base)).bearer_auth(access).header("accept","application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}})).send().await.unwrap().json().await.unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
        let list = f.rpc(access, "session_list", json!({})).await;
        assert_eq!(
            list["result"]["structuredContent"]["data"]["sessions"][0]["session_id"],
            "allowed"
        );
        assert_eq!(
            list["result"]["structuredContent"]["data"]["sessions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            f.rpc(access, "session_get", json!({"session_id":"outside"})).await["result"]["isError"],
            true
        );
        let write = f
            .rpc(
                access,
                "session_prompt",
                json!({"session_id":"allowed","content":"blocked"}),
            )
            .await;
        assert_eq!(write["id"], 1);
        assert_eq!(write["error"]["code"], -32602);
        assert!(write.get("result").is_none());
        assert_eq!(
            f.http
                .get(format!("{}/api/sessions", f.base))
                .bearer_auth(access)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let refresh = f
            .http
            .post(format!("{}/oauth/external/token", f.base))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &client),
                ("resource", &f.resource()),
                ("refresh_token", token["refresh_token"].as_str().unwrap()),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(refresh.status(), 200);
        let next: Value = refresh.json().await.unwrap();
        assert_ne!(next["refresh_token"], token["refresh_token"]);
        let revoke = f
            .http
            .post(format!("{}/oauth/external/revoke", f.base))
            .form(&[
                ("client_id", client.as_str()),
                ("token", next["refresh_token"].as_str().unwrap()),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(revoke.status(), 200);
        assert_eq!(
            f.http
                .get(format!("{}/mcp/external/hennery", f.base))
                .bearer_auth(next["access_token"].as_str().unwrap())
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
}
#[tokio::test]
async fn consent_is_operator_only_origin_checked_and_requires_fresh_stepup() {
    let f = Fixture::new(None).await;
    let client = f.client().await;
    let id = f.pending(&client).await;
    let url = format!("{}/api/external-oauth/consent", f.base);
    assert_eq!(
        f.http
            .post(&url)
            .header("origin", &f.origin)
            .json(&f.grant(&id, true))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        f.operator
            .post(&url)
            .header("origin", "https://attacker.example")
            .json(&f.grant(&id, true))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let stale = f
        .state
        .operator
        .open_session(
            "fixture",
            &hennery_testkit::owner_phc(&f.state.operator),
            unix_now() - 301,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        f.http
            .post(&url)
            .header("origin", &f.origin)
            .header("cookie", format!("hennery_session={stale}"))
            .json(&f.grant(&id, true))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert!(f.state.store.external_clients().unwrap().is_empty());
    let denied: Value = f
        .operator
        .post(&url)
        .json(&f.grant(&id, false))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(denied["redirect_uri"].as_str().unwrap().contains("error=access_denied"));
    assert!(f.state.store.external_clients().unwrap().is_empty());
    assert_eq!(
        f.operator
            .post(&url)
            .json(&f.grant(&id, true))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
}
#[tokio::test]
async fn invalid_exchange_parameters_are_standard_errors_and_do_not_burn_the_code() {
    let f = Fixture::new(None).await;
    let client = f.client().await;
    let id = f.pending(&client).await;
    let code = f.code(&id).await;
    for (field, value, expected) in [
        ("code_verifier", "b".repeat(43), "invalid_grant"),
        ("client_id", "foreign".into(), "invalid_grant"),
        ("resource", "https://other.example/mcp".into(), "invalid_target"),
        ("redirect_uri", "https://attacker.example".into(), "invalid_grant"),
    ] {
        let mut form = f.token_form(&client, &code);
        form.iter_mut().find(|(k, _)| *k == field).unwrap().1 = value;
        let r = f
            .http
            .post(format!("{}/oauth/external/token", f.base))
            .form(&form)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
        let e: Value = r.json().await.unwrap();
        assert_eq!(e["error"], expected);
    }
    let t = f.token(&client, &code).await;
    let replay = f
        .http
        .post(format!("{}/oauth/external/token", f.base))
        .form(&f.token_form(&client, &code))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 400);
    assert_eq!(
        f.http
            .get(format!("{}/mcp/external/hennery", f.base))
            .bearer_auth(t["access_token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}
#[tokio::test]
async fn invalid_registration_cimd_private_targets_and_bad_authorizations_never_redirect() {
    let f = Fixture::new(None).await;
    for uri in [
        "javascript:alert(1)",
        "http://attacker.example/callback",
        "https://client.example/callback#secret",
    ] {
        let r = f
            .http
            .post(format!("{}/oauth/external/register", f.base))
            .json(&json!({"client_name":"Bad","redirect_uris":[uri],"token_endpoint_auth_method":"none"}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
    }
    let client = f.client().await;
    for (field, value) in [
        ("client_id", "https://127.0.0.1/private.json"),
        ("redirect_uri", "https://attacker.example/callback"),
        ("code_challenge_method", "plain"),
        ("scope", "operator:approve"),
    ] {
        let mut query = f.auth(&client);
        query.iter_mut().find(|(k, _)| *k == field).unwrap().1 = value.into();
        let r = f
            .http
            .get(format!("{}/oauth/external/authorize", f.base))
            .query(&query)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
        assert!(!r.headers().contains_key("location"));
    }
    assert!(f.state.store.external_clients().unwrap().is_empty());
}
#[tokio::test]
async fn https_configuration_and_reserved_paths_are_enforced() {
    let f = Fixture::new(None).await;
    f.state.operator.reset_public_url("http://localhost").unwrap();
    let r = f
        .http
        .get(format!("{}/.well-known/oauth-authorization-server", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 503);
    for path in ["/oauth/unknown", "/.well-known/unknown"] {
        let r = f.http.get(format!("{}{path}", f.base)).send().await.unwrap();
        assert_eq!(r.status(), 404);
        assert!(r.headers()["content-type"].to_str().unwrap().contains("json"));
    }
    let doc = f
        .http
        .get(format!("{}/external-mcp/consent?request=invalid", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(doc.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn reduced_consent_requires_a_scope_challenge_without_widening_the_grant() {
    let f = Fixture::new(None).await;
    let client = f.client().await;
    let id = f.pending(&client).await;
    let mut grant = f.grant(&id, true);
    grant["read_metadata"] = json!(false);
    grant["existing_sessions"] = json!(false);
    let response: Value = f
        .operator
        .post(format!("{}/api/external-oauth/consent", f.base))
        .json(&grant)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let callback = Url::parse(response["redirect_uri"].as_str().unwrap()).unwrap();
    let code = callback.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string();
    let token = f.token(&client, &code).await;
    assert!(!token["scope"].as_str().unwrap().contains("hennery:sessions:read"));
    let denied = f
        .http
        .post(format!("{}/mcp/external/hennery", f.base))
        .bearer_auth(token["access_token"].as_str().unwrap())
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_list","arguments":{}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    assert!(
        denied.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("insufficient_scope")
    );
    assert!(!f.state.store.external_clients().unwrap()[0].read_metadata);
}
