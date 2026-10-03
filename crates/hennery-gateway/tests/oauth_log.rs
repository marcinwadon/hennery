//! OAuth hygiene (gateway spec §4.3, §5.8; lane L11; plan 8f): through a
//! whole Connect, a proxied request, a refresh, a refused refresh and
//! refused authorizes, at `TRACE`, no log line and no answer holds an
//! upstream or authorization-server URL's path or query, a code, a PKCE
//! verifier, `state`, an access or refresh token, a client secret, or the
//! token endpoint's refusal body. A binary of its own: the subscriber is
//! the process's, so the servers' tasks on every worker thread log into it.

mod support;

use axum::body::Body;
use axum::http::{Request, header};
use hennery_gateway::api::{GatewayState, router};
use hennery_gateway::model::{CredKind, NewConnection};
use hennery_gateway::proxy::{self, Limits, ProxyState};
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::oauth::{Config, FakeAs, PrAt};
use support::{Recorder, World};
use tower::ServiceExt;

const ORIGIN: &str = "https://hennery.example";
const PATH_CANARY: &str = "pathcanary0a1b2c3d";
const QUERY_CANARY: &str = "querycanary4e5f6a7b";
const SECRET_CANARY: &str = "secretcanary8c9d0e1f";
const BODY_CANARY: &str = "bodycanary2a3b4c5d";
const TOKEN_PREFIX: &str = "tokcanary6e7f8a9b-";

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_url_secret_code_verifier_token_or_client_secret_is_logged_or_answered() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();

    let world = World::new();
    let operator = Arc::new(Operator::open(&world.db).unwrap());
    let now = unix_now();
    let setup = operator.issue_setup_token(now).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap() else {
        panic!("setup failed");
    };
    let session = operator.open_session("test", &phc, now).unwrap().unwrap();
    let runtime = world.runtime(support::test_egress(), Arc::new(Recorder::default()));
    let api = router(GatewayState {
        runtime: runtime.clone(),
        operator: operator.clone(),
    });
    let mcp = proxy::router(ProxyState::for_sessions(runtime.clone(), Limits::default()));
    let fake = FakeAs::start(Config {
        pr_at: PrAt::Origin,
        registration: false,
        confidential: Some(("pre-1".into(), SECRET_CANARY.into())),
        refusal_body: Some(json!({ "error": "invalid_grant", "error_description": BODY_CANARY })),
        prefix: TOKEN_PREFIX.into(),
        ..Config::default()
    })
    .await;
    let url = format!("{}/s/{PATH_CANARY}/mcp?k={QUERY_CANARY}", fake.origin());
    fake.configure(|c| c.resource = Some(url.clone()));
    let id = world.connection_with(NewConnection {
        slug: "linear".into(),
        label: "Linear".into(),
        url: url.clone(),
        hat_id: world.hat(),
        cred_kind: CredKind::OauthClient,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: true,
    });
    world.host("host-a", 1);
    world.mount(&id, &["host-a"]);
    let session_token = world.mint("sess-1", "host-a", &world.hat());

    let answers = std::cell::RefCell::new(String::new());
    let call = async |method: &str, path: &str, body: Option<Value>, cookie: Option<String>| {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", cookie.unwrap_or_else(|| format!("hennery_session={session}")));
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let body = body.map_or(Body::empty(), |b| Body::from(b.to_string()));
        let resp = api.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let headers = format!("{:?}", resp.headers());
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        answers.borrow_mut().push_str(&text);
        (headers, text)
    };

    call(
        "PUT",
        &format!("/api/mcp/connections/{id}/oauth-client"),
        Some(json!({ "client_id": "pre-1", "client_secret": SECRET_CANARY })),
        None,
    )
    .await;
    let (headers, text) = call(
        "POST",
        &format!("/api/mcp/connections/{id}/authorize"),
        Some(json!({})),
        None,
    )
    .await;
    let answer: Value = serde_json::from_str(&text).unwrap();
    let consent = answer["consent_url"].as_str().unwrap().to_string();
    let state: String = url::Url::parse(&consent)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let flow = headers
        .split("set-cookie\": \"")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let location = fake.consent(&consent).await;
    let (_, page) = call("GET", location.strip_prefix(ORIGIN).unwrap(), None, Some(flow)).await;
    assert!(page.contains("data-result=\"connected\""), "{page}");

    // A proxied request, a 401, a refresh and a retry.
    fake.expire_access();
    let request = Request::builder()
        .method("POST")
        .uri("/mcp/linear")
        .header(header::AUTHORIZATION, format!("Bearer {session_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string(),
        ))
        .unwrap();
    let resp = mcp.clone().oneshot(request).await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    answers.borrow_mut().push_str(&String::from_utf8_lossy(&bytes));
    // A refused refresh: the token endpoint answers with the canary body.
    let held = world.store.oauth_credential(&id, &world.key).unwrap().unwrap();
    let mut stale = held.tokens.clone();
    stale.refresh_token = Some(zeroize::Zeroizing::new(format!("{TOKEN_PREFIX}unknown")));
    let refreshed = hennery_gateway::store::RefreshedGrant {
        tokens: &stale,
        expires_at: None,
        resource_param_accepted: true,
    };
    world
        .store
        .store_refreshed(&id, &held, &refreshed, &world.key, unix_now())
        .unwrap();
    fake.expire_access();
    let request = Request::builder()
        .method("POST")
        .uri("/mcp/linear")
        .header(header::AUTHORIZATION, format!("Bearer {session_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
        ))
        .unwrap();
    let resp = mcp.clone().oneshot(request).await.unwrap();
    assert_eq!(resp.status(), 502);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    answers.borrow_mut().push_str(&String::from_utf8_lossy(&bytes));
    // Refused authorizes, each naming the URL in its own way.
    fake.configure(|c| c.resource = Some(format!("{}/other/{PATH_CANARY}?q={QUERY_CANARY}", fake.origin())));
    call(
        "POST",
        &format!("/api/mcp/connections/{id}/authorize"),
        Some(json!({})),
        None,
    )
    .await;
    fake.configure(|c| c.resource = Some(format!("https://foreign.example/{PATH_CANARY}?q={QUERY_CANARY}")));
    call(
        "POST",
        &format!("/api/mcp/connections/{id}/authorize"),
        Some(json!({})),
        None,
    )
    .await;
    fake.configure(|c| c.token_endpoint = Some(format!("http://plain.example/{PATH_CANARY}?q={QUERY_CANARY}")));
    call(
        "POST",
        &format!("/api/mcp/connections/{id}/authorize"),
        Some(json!({})),
        None,
    )
    .await;
    // The item itself may show the URL whole (the owner's data): only the
    // log and the other answers are checked from here.
    let logs = String::from_utf8_lossy(&captured.0.lock().unwrap()).to_string();
    assert!(logs.contains(&id), "the log names the connection");
    assert!(!logs.is_empty());
    let mut canaries = vec![
        PATH_CANARY.to_string(),
        QUERY_CANARY.to_string(),
        SECRET_CANARY.to_string(),
        BODY_CANARY.to_string(),
        TOKEN_PREFIX.to_string(),
        state.clone(),
    ];
    canaries.extend(fake.with(|r| r.verifiers.clone()));
    canaries.extend(fake.with(|r| r.issued.clone()));
    assert!(fake.with(|r| r.verifiers.len()) == 1);
    for canary in &canaries {
        assert!(!logs.contains(canary.as_str()), "{canary} is in the log");
    }
    let answers = answers.into_inner();
    for canary in [
        SECRET_CANARY,
        BODY_CANARY,
        TOKEN_PREFIX,
        &fake.with(|r| r.verifiers[0].clone()),
    ] {
        assert!(!answers.contains(canary), "{canary} is in an answer");
    }
    // Error answers name origins only: no `{code, message}` holds the
    // URLs' canaries (the items, which show `url` whole, are not errors).
    for answer in answers.split("{\"code\":").skip(1) {
        let error = answer.split('}').next().unwrap();
        assert!(!error.contains(PATH_CANARY) && !error.contains(QUERY_CANARY), "{error}");
    }
}
