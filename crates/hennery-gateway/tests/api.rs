//! The connections API (gateway spec §9, kernel spec §3.4): every route is
//! the operator's, behind the browser rules; creating, deleting, setting a
//! credential and changing where a token goes need a fresh step-up.
//! Driven through the router in-process.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use hennery_gateway::api::{GatewayState, router};
use hennery_gateway::key::MasterKey;
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const ORIGIN: &str = "https://hennery.example";
const PASSWORD: &str = "correct horse battery";

struct Api {
    _dir: tempfile::TempDir,
    app: Router,
    operator: Arc<Operator>,
    store: Arc<GatewayStore>,
    key: Arc<MasterKey>,
    hosts: Hosts,
    phc: String,
}

impl Api {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let operator = Arc::new(Operator::open(&db).unwrap());
        let now = unix_now();
        let setup = operator.issue_setup_token(now).unwrap().unwrap();
        let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, PASSWORD, ORIGIN, now).unwrap() else {
            panic!("setup failed");
        };
        let store = Arc::new(GatewayStore::open(&db).unwrap());
        let key = Arc::new(MasterKey::from_bytes([5; 32]));
        let app = router(GatewayState {
            store: store.clone(),
            key: key.clone(),
            operator: operator.clone(),
            revocations: Default::default(),
        });
        Self {
            _dir: dir,
            app,
            operator,
            store,
            key,
            hosts: Hosts::open(&db).unwrap(),
            phc,
        }
    }

    /// A session whose last password check was `age` seconds ago.
    fn session(&self, age: i64) -> String {
        self.operator
            .open_session("test", &self.phc, unix_now() - age)
            .unwrap()
            .unwrap()
    }

    fn hat(&self) -> String {
        self.hosts.default_hat_for_new_hosts().unwrap()
    }

    fn host(&self, id: &str, seed: u8) {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let enrollment = Enrollment {
            public_key: hex::encode(key.verifying_key().as_bytes()),
            name: format!("host {seed}"),
            host_version: "0.0.0".into(),
            platform: "macos-aarch64".into(),
        };
        self.hosts.register(id, &enrollment, unix_now()).unwrap();
    }

    async fn send(&self, session: &str, method: &str, path: &str, body: Option<&Value>) -> (StatusCode, Value) {
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
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn new_body(&self, slug: &str) -> Value {
        json!({
            "slug": slug,
            "label": "Linear",
            "url": "https://mcp.linear.example/mcp",
            "hat_id": self.hat(),
            "cred_kind": "static",
        })
    }

    async fn create(&self, slug: &str) -> String {
        let (status, item) = self
            .send(
                &self.session(0),
                "POST",
                "/api/mcp/connections",
                Some(&self.new_body(slug)),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{item}");
        item["id"].as_str().unwrap().to_string()
    }
}

fn code(body: &Value) -> &str {
    body["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn a_connection_is_created_listed_changed_and_deleted() {
    let api = Api::new();
    let s = api.session(0);
    let (status, item) = api
        .send(&s, "POST", "/api/mcp/connections", Some(&api.new_body("linear")))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = item["id"].as_str().unwrap().to_string();
    assert_eq!(item["slug"], "linear");
    assert_eq!(item["cred_kind"], "static");
    assert_eq!(item["static_header"], "Authorization");
    assert_eq!(item["static_prefix"], "Bearer ");
    assert_eq!(item["tool_allowlist"], Value::Null);
    assert_eq!(item["status"], "not_connected");
    assert_eq!(item["has_credential"], false);
    assert_eq!(item["mounts"], json!([]));
    assert!(item["created_at"].as_str().unwrap().ends_with('Z'), "{item}");
    assert!(
        item.get("status_note").is_none() && item.get("account_label").is_none(),
        "{item}"
    );

    let (status, list) = api.send(&s, "GET", "/api/mcp/connections", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([item]));

    let path = format!("/api/mcp/connections/{id}");
    let (status, changed) = api
        .send(
            &s,
            "PATCH",
            &path,
            Some(&json!({ "label": "Linear (work)", "tool_allowlist": ["search"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["label"], "Linear (work)");
    assert_eq!(changed["tool_allowlist"], json!(["search"]));
    // Absent keeps; `null` clears.
    let (_, kept) = api.send(&s, "PATCH", &path, Some(&json!({}))).await;
    assert_eq!(kept["tool_allowlist"], json!(["search"]));
    let (_, cleared) = api
        .send(&s, "PATCH", &path, Some(&json!({ "tool_allowlist": null })))
        .await;
    assert_eq!(cleared["tool_allowlist"], Value::Null);

    let (status, body) = api.send(&s, "DELETE", &path, None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    for (method, path, body) in [
        ("PATCH", path.clone(), Some(json!({ "label": "x" }))),
        ("DELETE", path.clone(), None),
        ("PUT", format!("{path}/mounts"), Some(json!({ "host_ids": [] }))),
        ("PUT", format!("{path}/credential"), Some(json!({ "token": "t" }))),
    ] {
        let (status, body) = api.send(&s, method, &path, body.as_ref()).await;
        assert_eq!(
            (status, code(&body)),
            (StatusCode::NOT_FOUND, "not_found"),
            "{method} {path}"
        );
    }
}

/// Kernel spec §3.4, gateway spec §9, plan 8a decision 4: creating a
/// connection, deleting one, setting its credential and changing where its
/// token goes (url, kind, internal network, header, prefix) need a check
/// within five minutes; nothing is written without one. Its label, its
/// allowlist, its mounts and reading the list do not.
#[tokio::test]
async fn where_a_token_can_go_changes_only_with_a_fresh_step_up() {
    let api = Api::new();
    api.host("host-a", 1);
    let id = api.create("linear").await;
    let path = format!("/api/mcp/connections/{id}");
    let stale = api.session(5 * 60);
    let before = api.store.connection(&id).unwrap();
    let refused = [
        ("POST", "/api/mcp/connections".to_string(), Some(api.new_body("other"))),
        ("DELETE", path.clone(), None),
        ("PUT", format!("{path}/credential"), Some(json!({ "token": "tok" }))),
        (
            "PATCH",
            path.clone(),
            Some(json!({ "url": "https://elsewhere.example/" })),
        ),
        ("PATCH", path.clone(), Some(json!({ "cred_kind": "none" }))),
        ("PATCH", path.clone(), Some(json!({ "internal_network": true }))),
        ("PATCH", path.clone(), Some(json!({ "static_header": "X-API-Key" }))),
        ("PATCH", path.clone(), Some(json!({ "static_prefix": "" }))),
        // A connection that is not there: refused before it is looked up.
        (
            "PATCH",
            "/api/mcp/connections/conn-0000000000000000".to_string(),
            Some(json!({ "url": "https://elsewhere.example/" })),
        ),
        // Named with the same value it has, still: the check is on what is
        // named, before anything is read.
        (
            "PATCH",
            path.clone(),
            Some(json!({ "label": "x", "url": "https://mcp.linear.example/mcp" })),
        ),
    ];
    for (method, path, body) in &refused {
        let (status, answer) = api.send(&stale, method, path, body.as_ref()).await;
        assert_eq!(
            (status, code(&answer)),
            (StatusCode::FORBIDDEN, "step_up_required"),
            "{method} {path} {body:?}"
        );
    }
    assert_eq!(api.store.connection(&id).unwrap(), before);
    assert_eq!(api.store.list().unwrap().len(), 1);
    assert!(!api.store.has_ciphertext().unwrap());

    let free = [
        ("GET", "/api/mcp/connections".to_string(), None),
        ("PATCH", path.clone(), Some(json!({ "label": "Renamed" }))),
        ("PATCH", path.clone(), Some(json!({ "tool_allowlist": ["search"] }))),
        ("PUT", format!("{path}/mounts"), Some(json!({ "host_ids": ["host-a"] }))),
    ];
    for (method, path, body) in &free {
        let (status, answer) = api.send(&stale, method, path, body.as_ref()).await;
        assert_eq!(status, StatusCode::OK, "{method} {path}: {answer}");
    }

    let fresh = api.session(0);
    let accepted = [
        (
            "PATCH",
            path.clone(),
            Some(json!({ "static_header": "X-API-Key" })),
            StatusCode::OK,
        ),
        (
            "PUT",
            format!("{path}/credential"),
            Some(json!({ "token": "tok" })),
            StatusCode::NO_CONTENT,
        ),
        (
            "POST",
            "/api/mcp/connections".to_string(),
            Some(api.new_body("other")),
            StatusCode::CREATED,
        ),
        ("DELETE", path.clone(), None, StatusCode::NO_CONTENT),
    ];
    for (method, path, body, expected) in &accepted {
        let (status, answer) = api.send(&fresh, method, path, body.as_ref()).await;
        assert_eq!(status, *expected, "{method} {path}: {answer}");
    }
}

#[tokio::test]
async fn every_route_needs_the_operators_session_and_origin() {
    let api = Api::new();
    let id = api.create("linear").await;
    for (method, path) in [
        ("GET", "/api/mcp/connections".to_string()),
        ("POST", "/api/mcp/connections".to_string()),
        ("PATCH", format!("/api/mcp/connections/{id}")),
        ("DELETE", format!("/api/mcp/connections/{id}")),
        ("PUT", format!("/api/mcp/connections/{id}/mounts")),
        ("PUT", format!("/api/mcp/connections/{id}/credential")),
    ] {
        let (status, body) = api.send("not-a-session", method, &path, Some(&json!({}))).await;
        assert_eq!(
            (status, code(&body)),
            (StatusCode::UNAUTHORIZED, "unauthenticated"),
            "{method} {path}"
        );
        if method != "GET" {
            let req = Request::builder()
                .method(method)
                .uri(&path)
                .header("origin", "https://evil.example")
                .header("cookie", format!("hennery_session={}", api.session(0)))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap();
            let resp = api.app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{method} {path}");
        }
    }
    assert_eq!(api.store.list().unwrap().len(), 1);
}

#[tokio::test]
async fn refusals_answer_with_their_code_and_write_nothing() {
    let api = Api::new();
    let s = api.session(0);
    let id = api.create("linear").await;
    let path = format!("/api/mcp/connections/{id}");
    let mut none = api.new_body("public");
    none["cred_kind"] = json!("none");
    let (_, public) = api.send(&s, "POST", "/api/mcp/connections", Some(&none)).await;
    let public = public["id"].as_str().unwrap().to_string();
    let mut oauth = api.new_body("oauth");
    oauth["cred_kind"] = json!("oauth_dcr");
    let mut bad_slug = api.new_body("Bad Slug");
    bad_slug["slug"] = json!("Bad Slug");
    let mut http = api.new_body("lan");
    http["url"] = json!("http://mcp.example/");
    let mut extra = api.new_body("extra");
    extra["owner_id"] = json!("owner-x");
    let cases = [
        (
            "POST",
            "/api/mcp/connections".to_string(),
            json!(api.new_body("linear")),
            409,
            "slug_taken",
        ),
        (
            "POST",
            "/api/mcp/connections".to_string(),
            oauth,
            400,
            "unsupported_cred_kind",
        ),
        ("POST", "/api/mcp/connections".to_string(), bad_slug, 400, "invalid"),
        ("POST", "/api/mcp/connections".to_string(), http, 400, "invalid"),
        ("POST", "/api/mcp/connections".to_string(), extra, 422, "invalid_body"),
        (
            "PATCH",
            path.clone(),
            json!({ "cred_kind": "oauth_client" }),
            400,
            "unsupported_cred_kind",
        ),
        // The slug and the hat cannot change: naming them is refused.
        ("PATCH", path.clone(), json!({ "slug": "renamed" }), 422, "invalid_body"),
        (
            "PATCH",
            path.clone(),
            json!({ "hat_id": api.hat() }),
            422,
            "invalid_body",
        ),
        ("PATCH", path.clone(), json!({ "label": "" }), 400, "invalid"),
        (
            "PUT",
            format!("{path}/mounts"),
            json!({ "host_ids": ["host-x"] }),
            400,
            "invalid",
        ),
        (
            "PUT",
            format!("{path}/credential"),
            json!({ "token": "two words" }),
            400,
            "invalid",
        ),
        (
            "PUT",
            format!("/api/mcp/connections/{public}/credential"),
            json!({ "token": "t" }),
            409,
            "wrong_cred_kind",
        ),
    ];
    for (method, path, body, status, expected) in cases {
        let (got, answer) = api.send(&s, method, &path, Some(&body)).await;
        assert_eq!(
            (got.as_u16(), code(&answer)),
            (status, expected),
            "{method} {path} {body}"
        );
    }
    assert_eq!(api.store.list().unwrap().len(), 2);
    assert!(!api.store.has_ciphertext().unwrap());
    // A body past the limit is refused before it is read.
    let huge = json!({ "token": "x".repeat(300 * 1024) });
    let (status, answer) = api.send(&s, "PUT", &format!("{path}/credential"), Some(&huge)).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::PAYLOAD_TOO_LARGE, "body_too_large")
    );
}

#[tokio::test]
async fn a_static_credential_is_write_only() {
    let api = Api::new();
    let s = api.session(0);
    let id = api.create("linear").await;
    let (status, body) = api
        .send(
            &s,
            "PUT",
            &format!("/api/mcp/connections/{id}/credential"),
            Some(&json!({ "token": "the-token" })),
        )
        .await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    assert_eq!(
        api.store
            .static_credential(&id, &api.key)
            .unwrap()
            .unwrap()
            .token
            .as_str(),
        "the-token"
    );
    let (_, list) = api.send(&s, "GET", "/api/mcp/connections", None).await;
    assert_eq!(list[0]["has_credential"], true);
    assert!(!list.to_string().contains("the-token"), "{list}");
    // Another origin deletes it (gateway spec §4.6).
    let (_, moved) = api
        .send(
            &s,
            "PATCH",
            &format!("/api/mcp/connections/{id}"),
            Some(&json!({ "url": "https://elsewhere.example/mcp" })),
        )
        .await;
    assert_eq!(moved["has_credential"], false);
    assert_eq!(api.store.static_credential(&id, &api.key).unwrap(), None);
}

#[tokio::test]
async fn mounts_are_replaced_and_revoked_hosts_left_out() {
    let api = Api::new();
    api.host("host-a", 1);
    api.host("host-b", 2);
    let s = api.session(0);
    let id = api.create("linear").await;
    let path = format!("/api/mcp/connections/{id}/mounts");
    let (status, item) = api
        .send(&s, "PUT", &path, Some(&json!({ "host_ids": ["host-b", "host-a"] })))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item["mounts"], json!(["host-a", "host-b"]));
    api.hosts.revoke("host-b", unix_now()).unwrap();
    let (_, list) = api.send(&s, "GET", "/api/mcp/connections", None).await;
    assert_eq!(list[0]["mounts"], json!(["host-a"]));
    let (status, answer) = api
        .send(&s, "PUT", &path, Some(&json!({ "host_ids": ["host-b"] })))
        .await;
    assert_eq!((status, code(&answer)), (StatusCode::BAD_REQUEST, "invalid"));
}

/// The review's O6: no answer of the gateway's is kept by a cache, the
/// list and the refusals alike.
#[tokio::test]
async fn answers_are_never_cached() {
    let api = Api::new();
    let s = api.session(0);
    for path in [
        "/api/mcp/connections",
        "/api/mcp/connections/conn-0000000000000000/mounts",
    ] {
        let method = if path.ends_with("mounts") { "PUT" } else { "GET" };
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", format!("hennery_session={s}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"host_ids":[]}"#))
            .unwrap();
        let resp = api.app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.headers().get("cache-control").map(|v| v.to_str().unwrap()),
            Some("no-store"),
            "{method} {path}"
        );
    }
    // The refusals of the session and the browser rules too (the
    // re-confirmation's finding 4): no session (401), another origin (403).
    for (origin, cookie, status) in [
        (ORIGIN, None, StatusCode::UNAUTHORIZED),
        ("https://elsewhere.example", Some(&s), StatusCode::FORBIDDEN),
    ] {
        let mut req = Request::builder()
            .method("GET")
            .uri("/api/mcp/connections")
            .header("origin", origin);
        if let Some(cookie) = cookie {
            req = req.header("cookie", format!("hennery_session={cookie}"));
        }
        let resp = api.app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), status, "{origin}");
        assert_eq!(
            resp.headers().get("cache-control").map(|v| v.to_str().unwrap()),
            Some("no-store"),
            "{status}"
        );
    }
}

/// The operator's decision of 2026-10-02 (through the gateway lane): a
/// connection marked `internal_network` may use plain `http`, set on
/// create or by one `PATCH` naming both, behind step-up; clearing the mark
/// while the URL is `http` is refused and changes nothing (plan 8a
/// decision 9).
#[tokio::test]
async fn an_internal_connection_may_use_http_and_keeps_its_mark_while_it_does() {
    let api = Api::new();
    let s = api.session(0);
    let id = api.create("lan").await;
    let path = format!("/api/mcp/connections/{id}");
    let lan = "http://10.0.0.7:8080/mcp";
    let (status, body) = api.send(&s, "PATCH", &path, Some(&json!({ "url": lan }))).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid"), "{body}");
    let stale = api.session(5 * 60);
    let both = json!({ "url": lan, "internal_network": true });
    let (status, body) = api.send(&stale, "PATCH", &path, Some(&both)).await;
    assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, "step_up_required"));
    let (status, item) = api.send(&s, "PATCH", &path, Some(&both)).await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert_eq!(
        (item["url"].as_str(), item["internal_network"].as_bool()),
        (Some(lan), Some(true))
    );
    let (status, body) = api
        .send(&s, "PATCH", &path, Some(&json!({ "internal_network": false })))
        .await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid"), "{body}");
    let (_, list) = api.send(&s, "GET", "/api/mcp/connections", None).await;
    assert_eq!(
        (list[0]["url"].as_str(), list[0]["internal_network"].as_bool()),
        (Some(lan), Some(true))
    );
    let mut marked = api.new_body("lan-2");
    marked["url"] = json!("http://192.168.1.9/mcp");
    marked["internal_network"] = json!(true);
    let (status, item) = api.send(&s, "POST", "/api/mcp/connections", Some(&marked)).await;
    assert_eq!(status, StatusCode::CREATED, "{item}");
}

/// The fleet parent's ruling (2026-10-02): a gateway error is the shared
/// `ApiError`, `{code, message}` and nothing else, over HTTP: a 404, a 400
/// and the step-up 403.
#[tokio::test]
async fn an_error_is_the_shared_api_error_and_nothing_more() {
    let api = Api::new();
    let id = api.create("linear").await;
    let path = format!("/api/mcp/connections/{id}");
    let fresh = api.session(0);
    let stale = api.session(5 * 60);
    for (session, path, body, status, expected) in [
        (
            &fresh,
            "/api/mcp/connections/conn-0000000000000000",
            json!({ "label": "Linear" }),
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            &fresh,
            path.as_str(),
            json!({ "label": "" }),
            StatusCode::BAD_REQUEST,
            "invalid",
        ),
        (
            &stale,
            path.as_str(),
            json!({ "url": "https://mcp.example/" }),
            StatusCode::FORBIDDEN,
            "step_up_required",
        ),
    ] {
        let (got, answer) = api.send(session, "PATCH", path, Some(&body)).await;
        assert_eq!(got, status, "{answer}");
        let mut keys: Vec<&str> = answer
            .as_object()
            .expect("a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["code", "message"], "{answer}");
        assert_eq!(answer["code"], expected, "{answer}");
        assert!(answer["message"].as_str().is_some_and(|m| !m.is_empty()), "{answer}");
    }
}

/// Plan 8e (api-8e-8f A1): a stdio set is read without step-up and
/// replaced with it; its values are never answered, only their names.
#[tokio::test]
async fn a_stdio_set_is_read_freely_and_replaced_behind_step_up() {
    let api = Api::new();
    api.host("host-a", 1);
    let hat = api.hat();
    let path = format!("/api/mcp/stdio-servers?host_id=host-a&hat_id={hat}");
    let fresh = api.session(0);
    let stale = api.session(600);
    let (status, set) = api.send(&stale, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(set, json!({"host_id": "host-a", "hat_id": hat, "servers": []}));
    let body = json!({"servers": [{
        "name": "files", "command": "files-mcp", "args": ["--root", "/srv"],
        "env": [{"name": "FILES_KEY", "value": "s3cr3t-stdio-value"}, {"name": "EMPTY", "value": ""}]
    }]});
    let (status, refused) = api.send(&stale, "PUT", &path, Some(&body)).await;
    assert_eq!((status, code(&refused)), (StatusCode::FORBIDDEN, "step_up_required"));
    let (status, _) = api.send(&stale, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, set) = api.send(&fresh, "PUT", &path, Some(&body)).await;
    assert_eq!(status, StatusCode::OK, "{set}");
    let server = &set["servers"][0];
    assert_eq!(server["name"], "files");
    assert_eq!(server["args"], json!(["--root", "/srv"]));
    assert_eq!(
        server["env"],
        json!([{"name": "FILES_KEY", "has_value": true}, {"name": "EMPTY", "has_value": true}])
    );
    assert!(server["created_at"].as_str().unwrap().ends_with('Z'), "{set}");
    let (_, read) = api.send(&stale, "GET", &path, None).await;
    assert_eq!(read, set);
    assert!(!read.to_string().contains("s3cr3t-stdio-value"), "{read}");
}

#[tokio::test]
async fn the_stdio_routes_answer_their_codes() {
    let api = Api::new();
    api.host("host-a", 1);
    api.host("host-gone", 2);
    let hat = api.hat();
    let s = api.session(0);
    let at = |host: &str, hat: &str| format!("/api/mcp/stdio-servers?host_id={host}&hat_id={hat}");
    let long = "h".repeat(65);
    for path in [
        "/api/mcp/stdio-servers".to_string(),
        format!("/api/mcp/stdio-servers?host_id=host-a"),
        format!("/api/mcp/stdio-servers?hat_id={hat}"),
        at(&long, &hat),
        at("host-a", &long),
        at("", &hat),
    ] {
        let (status, body) = api.send(&s, "GET", &path, None).await;
        assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid"), "{path}");
        assert!(!body.to_string().contains(&long), "{body}");
    }
    let (status, body) = api.send(&s, "GET", &at("host-x", &hat), None).await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "not_found"));
    let (status, body) = api.send(&s, "GET", &at("host-a", "hat-x"), None).await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "not_found"));
    let one = |name: &str, env: Value| json!({"servers": [{"name": name, "command": "c", "env": env}]});
    let (api, s) = (&api, &s);
    let put = |path: String, body: Value| async move { api.send(s, "PUT", &path, Some(&body)).await };
    let (status, body) = put(at("host-a", &hat), one("Bad", json!([]))).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid"));
    let (status, body) = put(at("host-a", &hat), one("files", json!([{"name": "K"}]))).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "env_value_missing"));
    api.create("linear").await;
    let (status, body) = put(at("host-a", &hat), one("linear", json!([]))).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "slug_taken"));
    let many: Vec<Value> = (0..33).map(|i| json!({"name": format!("s{i}"), "command": "c"})).collect();
    let (status, body) = put(at("host-a", &hat), json!({ "servers": many })).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "too_many_stdio_servers"));
    let (status, body) = put(at("host-a", &hat), json!({"servers": [], "extra": 1})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    api.hosts.revoke("host-gone", unix_now()).unwrap();
    let (status, body) = put(at("host-gone", &hat), json!({"servers": []})).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "host_revoked"));
    let (status, body) = api.send(s, "GET", &at("host-gone", &hat), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // And the other way: a connection may not take a stdio server's name.
    let (status, _) = put(at("host-a", &hat), one("files", json!([]))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = api
        .send(s, "POST", "/api/mcp/connections", Some(&api.new_body("files")))
        .await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "slug_taken"));
}
