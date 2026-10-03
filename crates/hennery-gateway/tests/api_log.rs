//! Token hygiene (gateway spec §11, kernel spec §7, lane L8): a static
//! token sent to the API appears in no log line, at any level, in no
//! answer, and nowhere in `hennery.db` as written, whether the request is
//! taken or refused. In a test binary of its own, driven in-process on the
//! test's own task, so `tracing`'s thread-local subscriber sees every
//! event.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hennery_gateway::api::{GatewayState, router};
use hennery_gateway::key::MasterKey;
use hennery_gateway::notify::Silent;
use hennery_gateway::runtime::Runtime;
use hennery_gateway::scope::ProxyStore;
use hennery_gateway::store::GatewayStore;
use hennery_kernel::egress::{Egress, Timeouts};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::McpCredentialRequest;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const ORIGIN: &str = "https://hennery.example";
const SECRET: &str = "hnry-test-SECRET-0123456789abcdef";

/// A `tracing` writer into a shared buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// The gateway's runtime on `db`, as the collector makes it.
fn runtime(db: &std::path::Path, store: Arc<GatewayStore>, key: Arc<MasterKey>) -> Arc<Runtime> {
    Arc::new(Runtime::new(
        store,
        Arc::new(ProxyStore::open(db).unwrap()),
        key,
        Egress::new(Timeouts::DEFAULT).unwrap(),
        Arc::new(Silent),
    ))
}

#[tokio::test]
async fn a_static_token_is_never_logged_answered_or_stored_in_clear() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let operator = Arc::new(Operator::open(&db).unwrap());
    let now = unix_now();
    let setup = operator.issue_setup_token(now).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap() else {
        panic!("setup failed");
    };
    let session = operator.open_session("test", &phc, now).unwrap().unwrap();
    let hat = Hosts::open(&db).unwrap().default_hat_for_new_hosts().unwrap();
    let store = Arc::new(GatewayStore::open(&db).unwrap());
    let key = Arc::new(MasterKey::from_bytes([5; 32]));
    let app = router(GatewayState {
        runtime: runtime(&db, store.clone(), key.clone()),
        operator,
    });
    let send = |method: &str, path: &str, body: String| {
        Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", format!("hennery_session={session}"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    let mut answers = Vec::new();
    let mut call = async |method: &str, path: &str, body: serde_json::Value| {
        let resp = app.clone().oneshot(send(method, path, body.to_string())).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        answers.extend_from_slice(&bytes);
        (status, bytes)
    };

    let mut ids = Vec::new();
    for (slug, kind) in [("linear", "static"), ("public", "none")] {
        let (status, bytes) = call(
            "POST",
            "/api/mcp/connections",
            json!({ "slug": slug, "label": "L", "url": "https://mcp.example/", "hat_id": hat, "cred_kind": kind }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let item: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        ids.push(item["id"].as_str().unwrap().to_string());
    }
    let credential = |id: &str| format!("/api/mcp/connections/{id}/credential");
    // Refused: the wrong type, invalid characters, the wrong kind, an
    // unknown field beside it, an unknown connection.
    for (path, body, status) in [
        (
            credential(&ids[0]),
            json!({ "token": [SECRET] }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            credential(&ids[0]),
            json!({ "token": format!("{SECRET} x") }),
            StatusCode::BAD_REQUEST,
        ),
        (
            credential(&ids[0]),
            json!({ "token": format!("{SECRET}\n") }),
            StatusCode::BAD_REQUEST,
        ),
        (credential(&ids[1]), json!({ "token": SECRET }), StatusCode::CONFLICT),
        (
            credential(&ids[0]),
            json!({ "token": SECRET, "extra": SECRET }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            credential("conn-0000000000000000"),
            json!({ "token": SECRET }),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(call("PUT", &path, body.clone()).await.0, status, "{body}");
    }
    assert!(!store.has_ciphertext().unwrap());
    // Taken.
    let (status, _) = call("PUT", &credential(&ids[0]), json!({ "token": SECRET })).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        store.static_credential(&ids[0], &key).unwrap().unwrap().token.as_str(),
        SECRET
    );
    let (status, _) = call("GET", "/api/mcp/connections", json!(null)).await;
    assert_eq!(status, StatusCode::OK);

    let log = captured.0.lock().unwrap().clone();
    assert!(!log.is_empty(), "nothing was captured");
    assert!(contains(&log, &ids[0]), "the credential's connection is not logged");
    assert!(!contains(&log, SECRET), "{}", String::from_utf8_lossy(&log));
    assert!(!contains(&answers, SECRET), "{}", String::from_utf8_lossy(&answers));
    for file in ["hennery.db", "hennery.db-wal"] {
        let path = dir.path().join(file);
        if let Ok(bytes) = std::fs::read(&path) {
            assert!(!contains(&bytes, SECRET), "{file} holds the token in clear");
        }
    }
    let shown = format!("{:?}", McpCredentialRequest { token: SECRET.into() });
    assert!(!shown.contains(SECRET), "{shown}");
}

/// Plan 8a decision 19 (lane L11): some vendors put a secret in the URL's
/// path or query. A full upstream URL is in no log line and no error body,
/// and no `Debug` of a connection type shows more than its origin; the
/// list still answers the URL as stored, to its owner.
#[tokio::test]
async fn an_upstream_url_is_logged_and_shown_only_as_its_origin() {
    use hennery_proto::rest::{CreateMcpConnectionRequest, McpConnectionItem, McpCredKind, UpdateMcpConnectionRequest};
    const CANARY: &str = "c4n4ry-url-secret";
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let operator = Arc::new(Operator::open(&db).unwrap());
    let now = unix_now();
    let setup = operator.issue_setup_token(now).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap() else {
        panic!("setup failed");
    };
    let session = operator.open_session("test", &phc, now).unwrap().unwrap();
    let hat = Hosts::open(&db).unwrap().default_hat_for_new_hosts().unwrap();
    let app = router(GatewayState {
        runtime: runtime(
            &db,
            Arc::new(GatewayStore::open(&db).unwrap()),
            Arc::new(MasterKey::from_bytes([5; 32])),
        ),
        operator,
    });
    let url = format!("https://mcp.vendor.example:8443/s/{CANARY}/mcp?key={CANARY}");
    let mut refusals = Vec::new();
    let mut call = async |method: &str, path: &str, body: serde_json::Value| {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", format!("hennery_session={session}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        if !status.is_success() {
            refusals.extend_from_slice(&bytes);
        }
        (status, bytes)
    };
    let create = json!({ "slug": "vendor", "label": "V", "url": url, "hat_id": hat, "cred_kind": "static" });
    let (status, created) = call("POST", "/api/mcp/connections", create.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = serde_json::from_slice::<serde_json::Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/mcp/connections/{id}");
    for (method, path, body, expected) in [
        (
            "POST",
            "/api/mcp/connections".to_string(),
            create.clone(),
            StatusCode::CONFLICT,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "label": "Vendor", "url": url }),
            StatusCode::OK,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "url": url.replace("https", "http") }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "label": "", "url": url }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "url": format!("{url}#{CANARY}") }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "PUT",
            format!("{path}/mounts"),
            json!({ "host_ids": ["host-x"] }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "internal_network": url }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        assert_eq!(call(method, &path, body.clone()).await.0, expected, "{method} {body}");
    }
    let (status, list) = call("GET", "/api/mcp/connections", json!(null)).await;
    assert_eq!(status, StatusCode::OK);
    let items: Vec<McpConnectionItem> = serde_json::from_slice(&list).unwrap();
    assert_eq!(items[0].url, url, "the owner's list keeps the URL as stored");

    let log = captured.0.lock().unwrap().clone();
    assert!(
        contains(&log, "https://mcp.vendor.example:8443"),
        "the origin is not logged"
    );
    assert!(!contains(&log, CANARY), "{}", String::from_utf8_lossy(&log));
    assert!(!refusals.is_empty());
    assert!(!contains(&refusals, CANARY), "{}", String::from_utf8_lossy(&refusals));
    let create: CreateMcpConnectionRequest = serde_json::from_value(create).unwrap();
    let update = UpdateMcpConnectionRequest {
        label: None,
        url: Some(url.clone()),
        cred_kind: Some(McpCredKind::Static),
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: None,
    };
    for shown in [format!("{:?}", items[0]), format!("{create:?}"), format!("{update:?}")] {
        assert!(!shown.contains(CANARY), "{shown}");
        assert!(shown.contains("https://mcp.vendor.example:8443"), "{shown}");
    }
    assert_eq!(
        hennery_proto::rest::url_origin("https://user:pw@h.example:1/x?y#z"),
        "https://h.example:1"
    );
    // Raw input, as a request holds it before any check (the
    // re-confirmation's finding 1): its `Debug` shows an origin or nothing.
    for raw in [
        format!("https://user:{CANARY}/x@h.example/"),
        format!("{CANARY} https://h.example/p"),
        format!("https://h.example\\{CANARY}"),
        format!("https://{CANARY}@h.example/"),
        format!("https://h.example/{CANARY}%2F@x"),
        format!("mailto:{CANARY}@h.example"),
        CANARY.to_string(),
    ] {
        let create = CreateMcpConnectionRequest {
            url: raw.clone(),
            ..create.clone()
        };
        let update = UpdateMcpConnectionRequest {
            url: Some(raw.clone()),
            ..update.clone()
        };
        for shown in [
            format!("{create:?}"),
            format!("{update:?}"),
            hennery_proto::rest::url_origin(&raw),
        ] {
            assert!(!shown.contains(CANARY), "{raw}: {shown}");
        }
    }
}

/// Plan 8a's follow-up (8f's O6): a stored status the API does not know is
/// shown as `not_connected` and logged, never silently taken for one. The
/// schema's CHECK keeps any other out; the test steps around it.
#[tokio::test]
async fn an_unknown_stored_status_is_logged() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let operator = Arc::new(Operator::open(&db).unwrap());
    let now = unix_now();
    let setup = operator.issue_setup_token(now).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap() else {
        panic!("setup failed");
    };
    let session = operator.open_session("test", &phc, now).unwrap().unwrap();
    let hat = Hosts::open(&db).unwrap().default_hat_for_new_hosts().unwrap();
    let store = Arc::new(GatewayStore::open(&db).unwrap());
    let key = Arc::new(MasterKey::from_bytes([5; 32]));
    let hennery_gateway::model::Change::Done(record) = store
        .create(
            &hennery_gateway::model::NewConnection {
                slug: "linear".into(),
                label: "L".into(),
                url: "https://mcp.example/".into(),
                hat_id: hat,
                cred_kind: hennery_gateway::model::CredKind::None,
                static_header: None,
                static_prefix: None,
                tool_allowlist: None,
                internal_network: false,
            },
            now,
        )
        .unwrap()
    else {
        panic!("not created");
    };
    let raw = rusqlite::Connection::open(&db).unwrap();
    raw.execute_batch("PRAGMA ignore_check_constraints = ON;").unwrap();
    raw.execute("UPDATE gw_connections SET status = 'weird' WHERE id = ?1", [&record.id])
        .unwrap();
    let app = router(GatewayState {
        runtime: runtime(&db, store, key),
        operator,
    });
    let resp = app
        .oneshot(
            Request::get("/api/mcp/connections")
                .header("origin", ORIGIN)
                .header("cookie", format!("hennery_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(list[0]["status"], "not_connected");
    let log = captured.0.lock().unwrap().clone();
    assert!(
        contains(&log, "a stored status the API does not know") && contains(&log, &record.id),
        "{}",
        String::from_utf8_lossy(&log)
    );
}
