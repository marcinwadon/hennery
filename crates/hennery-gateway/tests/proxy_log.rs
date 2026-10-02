//! Token and URL hygiene through the proxy (gateway spec §3.1, §11; lane
//! L8, L11): the session token, the static credential and the secret parts
//! of an upstream URL (its path and query) appear in no log line at any
//! level, no answer to the client and no `Debug` the proxy prints, whatever
//! the request comes to. A binary of its own: the subscriber is the
//! process's, so the server's tasks on every worker thread log into it.

mod support;

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use hennery_gateway::model::CredKind;
use hennery_gateway::scope::MountPolicy;
use serde_json::json;
use std::sync::{Arc, Mutex};
use support::upstream::{FakeUpstream, Harness, event, first_then_fail, json, sse};

const PATH_SECRET: &str = "pathcanary0123456789";
const QUERY_SECRET: &str = "querycanary9876543210";
const CREDENTIAL: &str = "credcanary-5f3e2d1c0b";

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
async fn no_token_credential_or_url_secret_is_logged_or_answered() {
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

    let h = Harness::new().await;
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let secret_url = upstream.url(&format!("/mcp/{PATH_SECRET}?key={QUERY_SECRET}"));
    let id = h.connection("linear", &secret_url, CredKind::Static);
    h.mount(&id, &["host-a"]);
    h.set_token(&id, CREDENTIAL);
    // A connection to a port nothing listens on, its URL secret too.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let gone = h.connection(
        "gone",
        &format!("http://127.0.0.1:{closed}/mcp/{PATH_SECRET}?key={QUERY_SECRET}"),
        CredKind::Static,
    );
    h.mount(&gone, &["host-a"]);
    h.set_token(&gone, CREDENTIAL);
    let token = h.mint("s1", "host-a", &h.hat());

    let mut answers = String::new();
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    let mut exchange = async |slug: &str, token: &str| {
        let resp = h.post(slug, token, &ping).await;
        answers.push_str(&format!("{} {:?} ", resp.status(), resp.headers()));
        answers.push_str(&resp.text().await.unwrap_or_default());
    };
    let replies: Vec<Box<dyn Fn() -> Response + Send + Sync>> = vec![
        Box::new(|| json(StatusCode::OK, &json!({"jsonrpc": "2.0", "id": 1, "result": {}}))),
        Box::new(|| {
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Bearer")
                .body(Body::empty())
                .unwrap()
        }),
        Box::new(|| {
            Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, "http://127.0.0.1:1/x")
                .body(Body::empty())
                .unwrap()
        }),
        Box::new(|| {
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html")
                .body(Body::from("<p>"))
                .unwrap()
        }),
        Box::new(|| first_then_fail("application/json", "{")),
        Box::new(|| first_then_fail("text/event-stream", "data: {}\n\n")),
        Box::new(|| sse(&[event(&json!({"jsonrpc": "2.0", "id": "x", "method": "roots/list"}))])),
    ];
    for reply in replies {
        let reply = Arc::new(reply);
        upstream.reply(move |_, _| reply());
        exchange("linear", &token).await;
    }
    exchange("gone", &token).await;
    exchange("linear", "hnry_session_0000").await;
    exchange("nothing", &token).await;

    // A scoped connection's `Debug`, as a log line would print it.
    let scoped = h
        .proxy_store
        .connection(
            &hennery_gateway::scope::Principal {
                hat_id: h.hat(),
                kind: hennery_gateway::scope::PrincipalKind::Session {
                    session_id: "s1".into(),
                    host_id: "host-a".into(),
                },
            },
            "linear",
        )
        .unwrap()
        .unwrap();
    let debug = format!("{scoped:?} {scoped:#?}");
    // The upstream answering the refused `roots/list` in the background.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    // The capture works: the proxy's lines are in it, with the connection
    // and the upstream's origin.
    assert!(logs.contains(&id), "{logs}");
    assert!(
        logs.contains("gateway proxy: the upstream refused the credential"),
        "{logs}"
    );
    assert!(logs.contains("gateway proxy: the upstream was not reached"), "{logs}");
    // The cut bodies' errors were logged, so their lines were checked too.
    assert!(logs.contains("gateway proxy: the upstream body failed"), "{logs}");
    assert!(logs.contains("gateway proxy: the upstream stream failed"), "{logs}");
    assert!(logs.contains(&format!("http://127.0.0.1:{closed}")), "{logs}");
    assert!(answers.contains("upstream_unreachable"), "{answers}");
    assert!(debug.contains("linear"), "{debug}");
    for secret in [PATH_SECRET, QUERY_SECRET, CREDENTIAL, token.as_str()] {
        assert!(!logs.contains(secret), "{secret} logged:\n{logs}");
        assert!(!answers.contains(secret), "{secret} answered:\n{answers}");
        assert!(!debug.contains(secret), "{secret} in a Debug:\n{debug}");
    }
}
