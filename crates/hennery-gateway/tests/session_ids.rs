//! An upstream's `Mcp-Session-Id`, bound to the token that opened it (plan
//! 8e decision 13; gateway spec §5.2, §5.5). Every token on a connection
//! sends upstream with the connection's one credential, so a session id
//! that is not this token's on this connection is refused with the same
//! 404 as an unknown token, and nothing goes up.

mod support;

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use hennery_gateway::model::CredKind;
use reqwest::Method;
use serde_json::{Value, json};
use support::upstream::{FakeUpstream, Harness, json};

struct Setup {
    h: Harness,
    upstream: FakeUpstream,
    token: String,
    /// Another session's token, on the same host, hat and connection.
    other: String,
}

async fn setup() -> Setup {
    let h = Harness::new().await;
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let hat = h.hat();
    let id = h.connection_in("linear", &upstream.url("/mcp"), CredKind::None, &hat, None);
    h.mount(&id, &["host-a"]);
    let token = h.mint("s1", "host-a", &hat);
    let other = h.mint("s2", "host-a", &hat);
    Setup {
        h,
        upstream,
        token,
        other,
    }
}

fn ping() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })
}

/// `method` on `slug` with `token` and the session ids `ids`.
async fn send(h: &Harness, method: Method, slug: &str, token: &str, ids: &[&str]) -> reqwest::Response {
    let mut request = h
        .client
        .request(method.clone(), h.url(slug))
        .bearer_auth(token)
        .header(header::ACCEPT, "application/json, text/event-stream");
    for id in ids {
        request = request.header("mcp-session-id", *id);
    }
    if method == Method::POST {
        request = request
            .header(header::CONTENT_TYPE, "application/json")
            .body(ping().to_string());
    }
    request.send().await.unwrap()
}

async fn assert_refused(resp: reqwest::Response, what: &str) {
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{what}");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "not_found", "{what}");
}

/// The id comes down wrapped and goes up bare; a request with none goes up
/// with none.
#[tokio::test]
async fn a_session_id_comes_down_wrapped_and_goes_up_bare() {
    let s = setup().await;
    let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
    assert!(s.upstream.seen()[0].header("mcp-session-id").is_none());
    for method in [Method::POST, Method::GET, Method::DELETE] {
        let resp = send(&s.h, method.clone(), "linear", &s.token, &[&mine]).await;
        assert_eq!(resp.status(), StatusCode::OK, "{method}");
        let up = s.upstream.seen().pop().unwrap();
        assert_eq!(up.method, method.as_str());
        assert_eq!(up.header("mcp-session-id"), Some("up-1"), "{method}");
    }
}

/// The finding #99 left open: another session's token, on the same
/// connection and credential, cannot ride this session's upstream session.
#[tokio::test]
async fn another_tokens_session_id_is_404_and_nothing_goes_up() {
    let s = setup().await;
    let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
    let before = s.upstream.seen().len();
    for method in [Method::POST, Method::GET, Method::DELETE] {
        let resp = send(&s.h, method.clone(), "linear", &s.other, &[&mine]).await;
        assert_refused(resp, method.as_str()).await;
    }
    assert_eq!(s.upstream.seen().len(), before, "nothing went up");
}

/// Bound to the connection too: the id one connection gave is no id on
/// another, under the same token.
#[tokio::test]
async fn a_session_id_from_another_connection_is_404() {
    let s = setup().await;
    let second = FakeUpstream::start().await;
    let id =
        s.h.connection_in("github", &second.url("/mcp"), CredKind::None, &s.h.hat(), None);
    s.h.mount(&id, &["host-a"]);
    let linear = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
    assert_refused(
        send(&s.h, Method::POST, "github", &s.token, &[&linear]).await,
        "linear's id on github",
    )
    .await;
    assert!(second.seen().is_empty(), "nothing went up");
}

/// A bare upstream id, a forged or respelled tag, or two ids: the same 404,
/// nothing up.
#[tokio::test]
async fn a_bare_forged_or_doubled_session_id_is_404() {
    let s = setup().await;
    let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
    let before = s.upstream.seen().len();
    let tag = &mine["up-1.".len()..];
    let forged = format!("up-1.{}", "0".repeat(64));
    let upper = format!("up-1.{}", tag.to_uppercase());
    let moved = format!("up-2.{tag}");
    for (ids, what) in [
        (vec!["up-1"], "bare"),
        (vec![forged.as_str()], "forged"),
        (vec![upper.as_str()], "respelled"),
        (vec![moved.as_str()], "another upstream id"),
        (vec![mine.as_str(), mine.as_str()], "twice"),
    ] {
        assert_refused(send(&s.h, Method::POST, "linear", &s.token, &ids).await, what).await;
    }
    assert_eq!(s.upstream.seen().len(), before, "nothing went up");
}

/// The key is the process's own: after a restart every id is refused, and
/// the client initializes again (MCP: a 404 on a request with a session id).
#[tokio::test]
async fn a_restarted_proxy_refuses_the_ids_it_gave_before() {
    let mut s = setup().await;
    let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
    s.h.restart().await;
    assert_refused(
        send(&s.h, Method::POST, "linear", &s.token, &[&mine]).await,
        "after a restart",
    )
    .await;
    let again = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
    assert_ne!(again, mine);
    assert_eq!(
        send(&s.h, Method::POST, "linear", &s.token, &[&again]).await.status(),
        StatusCode::OK
    );
}

/// An upstream that answers with two session ids: no answer to pass on.
#[tokio::test]
async fn an_answer_with_two_session_ids_is_502() {
    let s = setup().await;
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", "up-1")
            .header("mcp-session-id", "up-2")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#))
            .unwrap()
    });
    let resp = send(&s.h, Method::POST, "linear", &s.token, &[]).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert!(resp.headers().get("mcp-session-id").is_none());
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_invalid");
    // One id is wrapped as ever.
    s.upstream.reply(|_, _| {
        let mut resp = json(StatusCode::OK, &json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
        resp.headers_mut().insert("mcp-session-id", "up-1".parse().unwrap());
        resp
    });
    let resp = send(&s.h, Method::POST, "linear", &s.token, &[]).await;
    assert!(resp.headers()["mcp-session-id"].to_str().unwrap().starts_with("up-1."));
}
