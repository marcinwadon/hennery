//! The proxy (gateway spec §5, §11) against a fake streamable-HTTP MCP
//! upstream, both on loopback, the connection marked `internal_network`
//! (lane L7). Every test drives the proxy over real TCP, so streaming is
//! what a client sees.

mod support;

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use hennery_gateway::model::{CredKind, NewConnection};
use hennery_gateway::proxy::Limits;
use serde_json::{Value, json};
use std::time::Duration;
use support::upstream::{FakeUpstream, Harness, call, event, first_then_block, json, list, listed, sse};

/// One hat, one host, one connection to `upstream` mounted there, and a
/// session token for it.
struct Setup {
    h: Harness,
    upstream: FakeUpstream,
    id: String,
    token: String,
}

async fn setup(kind: CredKind, allowlist: Option<&[&str]>) -> Setup {
    setup_with(Harness::new().await, kind, allowlist).await
}

async fn setup_with(h: Harness, kind: CredKind, allowlist: Option<&[&str]>) -> Setup {
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let hat = h.hat();
    let id = h.connection_in("linear", &upstream.url("/mcp"), kind, &hat, allowlist);
    h.mount(&id, &["host-a"]);
    if kind == CredKind::Static {
        h.set_token(&id, "upstream-secret-token");
    }
    let token = h.mint("s1", "host-a", &hat);
    Setup { h, upstream, id, token }
}

fn ping(id: i64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "ping" })
}

/// Gateway spec §5.2: the request goes up with the allowed headers and the
/// static credential, never the client's token, cookie or anything else;
/// the answer comes down with the allowed headers and `nosniff`.
#[tokio::test]
async fn a_request_goes_up_with_the_credential_and_the_allowed_headers_only() {
    let s = setup(CredKind::Static, None).await;
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", "upstream-session-1")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::SET_COOKIE, "tracker=1")
            .header(header::WWW_AUTHENTICATE, "Bearer realm=x")
            .header("x-upstream-internal", "1")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#))
            .unwrap()
    });
    let resp =
        s.h.client
            .post(format!("{}?leak=1", s.h.url("linear")))
            .bearer_auth(&s.token)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", "upstream-session-1")
            .header("mcp-protocol-version", "2025-06-18")
            .header("last-event-id", "41")
            .header(header::COOKIE, "hennery_session=browser-cookie")
            .header(header::ORIGIN, "https://evil.example")
            .header("x-forwarded-for", "10.0.0.1")
            .header(header::ACCEPT_ENCODING, "gzip, br")
            .header(header::HOST, "evil.example")
            .body(ping(1).to_string())
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let headers = resp.headers().clone();
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(headers["mcp-session-id"], "upstream-session-1");
    // Plan 8d decision 19: never the upstream's caching, always `no-store`.
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    for dropped in [
        "set-cookie",
        "www-authenticate",
        "x-upstream-internal",
        "content-encoding",
    ] {
        assert!(!headers.contains_key(dropped), "{dropped} came down");
    }
    assert_eq!(resp.text().await.unwrap(), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);

    let seen = s.upstream.seen();
    assert_eq!(seen.len(), 1);
    let up = &seen[0];
    assert_eq!(up.method, "POST");
    // The connection's URL, not the client's query.
    assert_eq!(up.uri, "/mcp");
    assert_eq!(up.header("authorization"), Some("Bearer upstream-secret-token"));
    assert_eq!(up.header("content-type"), Some("application/json"));
    assert_eq!(up.header("accept"), Some("application/json, text/event-stream"));
    assert_eq!(up.header("mcp-session-id"), Some("upstream-session-1"));
    assert_eq!(up.header("mcp-protocol-version"), Some("2025-06-18"));
    assert_eq!(up.header("last-event-id"), Some("41"));
    assert_eq!(up.header("accept-encoding"), Some("identity"));
    for dropped in ["cookie", "origin", "x-forwarded-for"] {
        assert_eq!(up.header(dropped), None, "{dropped} went up");
    }
    // The stored URL's authority, verbatim, never the client's `Host` (plan
    // 8b-ii's obligations to the proxy).
    let authority = s.upstream.url("/mcp");
    let authority = authority.trim_start_matches("http://").trim_end_matches("/mcp");
    assert_eq!(up.header("host"), Some(authority));
    assert_eq!(up.json(), ping(1));
    let everything = format!("{:?} {}", up.headers, String::from_utf8_lossy(&up.body));
    assert!(!everything.contains(&s.token), "the session token went upstream");
}

/// Maintainer decision 6d: a static token under another header, with its
/// prefix; then no `Authorization` goes up at all.
#[tokio::test]
async fn a_static_token_under_its_own_header_and_no_authorization_upstream() {
    let h = Harness::new().await;
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let id = h.connection_with(NewConnection {
        slug: "keyed".into(),
        label: "Keyed".into(),
        url: upstream.url("/mcp"),
        hat_id: h.hat(),
        cred_kind: CredKind::Static,
        static_header: Some("X-API-Key".into()),
        static_prefix: Some("key=".into()),
        tool_allowlist: None,
        internal_network: true,
    });
    h.mount(&id, &["host-a"]);
    h.set_token(&id, "k123");
    let token = h.mint("s1", "host-a", &h.hat());
    let resp = h.post("keyed", &token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let up = &upstream.seen()[0];
    assert_eq!(up.header("x-api-key"), Some("key=k123"));
    assert_eq!(up.header("authorization"), None);
}

#[tokio::test]
async fn a_none_connection_sends_no_credential() {
    let s = setup(CredKind::None, None).await;
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let up = &s.upstream.seen()[0];
    assert_eq!(up.header("authorization"), None);
    assert!(!format!("{:?}", up.headers).contains(&s.token));
    // A client that sends no `Accept` (by hand: reqwest always sends one)
    // gets the streamable-HTTP default.
    let body = ping(2).to_string();
    let mut stream = tokio::net::TcpStream::connect(s.h.addr).await.unwrap();
    let request = format!(
        "POST /mcp/linear HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        s.h.addr,
        s.token,
        body.len()
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .unwrap();
    let mut answer = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stream, &mut answer)
        .await
        .unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let up = &s.upstream.seen()[1];
    assert_eq!(up.header("accept"), Some("application/json, text/event-stream"));
    // The review's O6: what goes up is typed as what the gateway checked.
    assert_eq!(up.header("content-type"), Some("application/json"));
    s.h.client
        .post(s.h.url("linear"))
        .bearer_auth(&s.token)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-16")
        .body(ping(3).to_string())
        .send()
        .await
        .unwrap();
    let up = &s.upstream.seen()[2];
    let types: Vec<_> = up.headers.get_all("content-type").iter().collect();
    assert_eq!(types, ["application/json"]);
}

/// Gateway spec §3.1, §11's scope negative tests: every way out of scope
/// is the same 404, and the upstream sees nothing.
#[tokio::test]
async fn out_of_scope_is_one_404() {
    let s = setup(CredKind::None, None).await;
    let h = &s.h;
    h.host("host-b", 2);
    let hat = h.hat();
    let work = h.other_hat();
    let theirs = h.connection_in("theirs", &s.upstream.url("/mcp"), CredKind::None, &work, None);
    h.mount(&theirs, &["host-a"]);
    let elsewhere = h.connection("elsewhere", &s.upstream.url("/mcp"), CredKind::None);
    h.mount(&elsewhere, &["host-b"]);
    let revoked = h.mint("s2", "host-a", &hat);
    h.revoke("s2");
    let superseded = h.mint("s3", "host-a", &hat);
    h.mint("s3", "host-a", &hat);
    let on_b = h.mint("s4", "host-b", &hat);
    let unknown = format!("hnry_session_{}", "0".repeat(64));

    let ask = async |slug: &str, auth: Option<String>| {
        let mut req = h
            .client
            .post(h.url(slug))
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(auth) = auth {
            req = req.header(header::AUTHORIZATION, auth);
        }
        let resp = req.body(ping(1).to_string()).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{slug}");
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
        assert_eq!(resp.headers()["cache-control"], "no-store");
        assert!(!resp.headers().contains_key("www-authenticate"));
        resp.text().await.unwrap()
    };
    let bearer = |t: &str| Some(format!("Bearer {t}"));
    let bodies = vec![
        ask("linear", None).await,
        ask("linear", Some(format!("Basic {}", s.token))).await,
        ask("linear", Some(s.token.clone())).await,
        ask("linear", bearer(&unknown)).await,
        ask("linear", bearer(&revoked)).await,
        ask("linear", bearer(&superseded)).await,
        // Another hat's connection, mounted on this host.
        ask("theirs", bearer(&s.token)).await,
        // This hat's, mounted on another host; and this one from that host.
        ask("elsewhere", bearer(&s.token)).await,
        ask("linear", bearer(&on_b)).await,
        ask("nothing", bearer(&s.token)).await,
        // A slug that is not UTF-8 (the whole-branch review).
        ask("%FF", bearer(&s.token)).await,
    ];
    // Two `Authorization` headers, the first a live token: none is taken.
    let doubled = h
        .client
        .post(h.url("linear"))
        .header(header::AUTHORIZATION, format!("Bearer {}", s.token))
        .header(header::AUTHORIZATION, format!("Bearer {unknown}"))
        .body(ping(1).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(doubled.status(), StatusCode::NOT_FOUND);
    assert_eq!(doubled.text().await.unwrap(), bodies[0]);
    // The review's O7: anything deeper under a slug is the same 404, a
    // bare trailing slash too (the Task 2 review's finding 2).
    for path in ["/mcp/linear/", "/mcp/linear/extra", "/mcp/linear/a/b"] {
        let deeper = h
            .client
            .get(format!("http://{}{path}", h.addr))
            .bearer_auth(&s.token)
            .send()
            .await
            .unwrap();
        assert_eq!(deeper.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(deeper.headers()["x-content-type-options"], "nosniff");
        assert_eq!(deeper.text().await.unwrap(), bodies[0], "{path}");
    }
    // Any other method, with a live token, is the same 404; `HEAD` too,
    // which axum would hand to the `GET` handler (the Task 2 review's
    // finding 1). Nothing goes up, so nothing moves the status.
    for method in ["PUT", "PATCH", "OPTIONS"] {
        let other = h
            .client
            .request(method.parse().unwrap(), h.url("linear"))
            .bearer_auth(&s.token)
            .send()
            .await
            .unwrap();
        assert_eq!(other.status(), StatusCode::NOT_FOUND, "{method}");
        assert_eq!(other.text().await.unwrap(), bodies[0], "{method}");
    }
    let head = h
        .client
        .head(h.url("linear"))
        .bearer_auth(&s.token)
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::NOT_FOUND);
    assert_eq!(h.status(&s.id), "not_connected");
    assert!(bodies.iter().all(|b| b == &bodies[0]), "{bodies:?}");
    let body: Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(body["code"], "not_found");
    // Unmounted now, and a host revoked: refused at once.
    h.mount(&s.id, &[]);
    assert_eq!(ask("linear", bearer(&s.token)).await, bodies[0]);
    h.mount(&s.id, &["host-a"]);
    h.hosts.revoke("host-a", hennery_kernel::secret::unix_now()).unwrap();
    assert_eq!(ask("linear", bearer(&s.token)).await, bodies[0]);
    assert!(s.upstream.seen().is_empty(), "the upstream was reached");
}

/// Plan 8d decision 3: a static connection without its token sends nothing.
#[tokio::test]
async fn a_static_connection_without_a_token_is_502_and_sends_nothing() {
    let h = Harness::new().await;
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let id = h.connection("linear", &upstream.url("/mcp"), CredKind::Static);
    h.mount(&id, &["host-a"]);
    let token = h.mint("s1", "host-a", &h.hat());
    let resp = h.post("linear", &token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_auth");
    assert!(upstream.seen().is_empty());
}

/// Gateway spec §5.4: a 401 is never passed on, nor its
/// `WWW-Authenticate`: 502 `upstream_auth` naming the connection, and the
/// connection needs sign-in again. A 2xx later sets it `ok` (§7).
#[tokio::test]
async fn an_upstream_401_is_502_upstream_auth_and_needs_auth() {
    for kind in [CredKind::Static, CredKind::None] {
        let s = setup(kind, None).await;
        s.upstream.reply(|_, _| {
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(
                    header::WWW_AUTHENTICATE,
                    r#"Bearer resource_metadata="https://x/.well-known""#,
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap()
        });
        let resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY, "{kind:?}");
        assert!(!resp.headers().contains_key("www-authenticate"));
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["code"], "upstream_auth");
        assert_eq!(
            body["message"],
            "connection Label linear needs re-authorization in hennery"
        );
        assert_eq!(s.h.status(&s.id), "needs_auth", "{kind:?}");
        // Sent once: there is nothing to refresh.
        assert_eq!(s.upstream.seen().len(), 1);

        s.upstream
            .reply(|_, _| json(StatusCode::OK, &json!({"jsonrpc": "2.0", "id": 1, "result": {}})));
        let resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(s.h.status(&s.id), "ok", "{kind:?}");
    }
}

/// Gateway spec §7: a 2xx through a connection that was not `ok` sets it,
/// and other answers leave it.
#[tokio::test]
async fn live_traffic_sets_ok_on_a_2xx_only() {
    let s = setup(CredKind::None, None).await;
    assert_eq!(s.h.status(&s.id), "not_connected");
    s.upstream
        .reply(|_, _| json(StatusCode::INTERNAL_SERVER_ERROR, &json!({})));
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    // Non-401 errors pass through (§11).
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(s.h.status(&s.id), "not_connected");
    // The review's O5: a body-less 2xx proves nothing.
    s.upstream.reply(|_, _| {
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::empty())
            .unwrap()
    });
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert_eq!(s.h.post("linear", &s.token, &note).await.status(), StatusCode::ACCEPTED);
    assert_eq!(s.h.status(&s.id), "not_connected");
    // The same without a `Content-Length` (chunked, ending at once).
    s.upstream.reply(|_, _| {
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::from_stream(futures::stream::empty::<
                Result<Vec<u8>, std::io::Error>,
            >()))
            .unwrap()
    });
    assert_eq!(s.h.post("linear", &s.token, &note).await.status(), StatusCode::ACCEPTED);
    assert_eq!(s.h.status(&s.id), "not_connected");
    // Nor does an empty body under a JSON type (the Task 2 review).
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, "0")
            .body(Body::empty())
            .unwrap()
    });
    assert_eq!(s.h.post("linear", &s.token, &ping(1)).await.status(), StatusCode::OK);
    assert_eq!(s.h.status(&s.id), "not_connected");
    s.upstream.reply(|_, _| json(StatusCode::OK, &json!({})));
    s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(s.h.status(&s.id), "ok");
}

/// Gateway spec §5.7: a redirect is never followed, nor passed on.
#[tokio::test]
async fn a_redirect_is_502_and_never_followed() {
    let s = setup(CredKind::Static, None).await;
    let target = FakeUpstream::start().await;
    let location = target.url("/stolen");
    s.upstream.reply(move |_, _| {
        Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, location.clone())
            .body(Body::empty())
            .unwrap()
    });
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert!(!resp.headers().contains_key("location"));
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_redirect");
    assert!(target.seen().is_empty(), "the redirect was followed");
}

/// Gateway spec §5.2: only JSON and event streams pass, uncompressed; a
/// body-less 202 or 204 has no type to judge (plan 8d decision 4).
#[tokio::test]
async fn only_json_and_event_streams_pass() {
    let s = setup(CredKind::None, None).await;
    let refused = [
        ("text/html", None, StatusCode::OK),
        ("text/plain", None, StatusCode::NOT_FOUND),
        ("application/json", Some("gzip"), StatusCode::OK),
        ("", None, StatusCode::OK),
    ];
    for (content_type, encoding, status) in refused {
        s.upstream.reply(move |_, _| {
            let mut resp = Response::builder().status(status);
            if !content_type.is_empty() {
                resp = resp.header(header::CONTENT_TYPE, content_type);
            }
            if let Some(encoding) = encoding {
                resp = resp.header(header::CONTENT_ENCODING, encoding);
            }
            resp.body(Body::from("<script>alert(1)</script>")).unwrap()
        });
        let resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY, "{content_type} {encoding:?}");
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["code"], "upstream_content_type");
    }
    // A notification's 202, without a body or a type.
    s.upstream.reply(|_, _| {
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::empty())
            .unwrap()
    });
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let resp = s.h.post("linear", &s.token, &note).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    // A `DELETE` ends the upstream session (G-16): forwarded, 204 back.
    s.upstream.reply(|_, _| {
        Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap()
    });
    let resp =
        s.h.client
            .delete(s.h.url("linear"))
            .bearer_auth(&s.token)
            .header("mcp-session-id", "upstream-session-9")
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let deleted = s.upstream.seen().pop().unwrap();
    assert_eq!(deleted.method, "DELETE");
    assert_eq!(deleted.header("mcp-session-id"), Some("upstream-session-9"));
}

/// Gateway spec §5.3: the first chunk reaches the client before the
/// upstream finishes. The upstream sends one chunk and blocks; a buffering
/// proxy fails this within seconds, it does not hang.
#[tokio::test]
async fn the_first_chunk_arrives_before_the_upstream_finishes() {
    for (content_type, first) in [
        ("application/json", r#"{"jsonrpc":"2.0","#.to_string()),
        (
            "text/event-stream",
            event(&json!({"jsonrpc": "2.0", "method": "notifications/progress"})),
        ),
    ] {
        let s = setup(CredKind::None, None).await;
        let chunk = first.clone();
        s.upstream
            .reply(move |_, hold| first_then_block(content_type, &chunk, hold));
        let resp = tokio::time::timeout(Duration::from_secs(5), s.h.post("linear", &s.token, &ping(1)))
            .await
            .expect("the head was held");
        assert_eq!(resp.status(), StatusCode::OK);
        let mut body = resp.bytes_stream();
        let got = tokio::time::timeout(Duration::from_secs(5), futures::StreamExt::next(&mut body))
            .await
            .unwrap_or_else(|_| panic!("{content_type}: the first chunk was held until the upstream finished"))
            .unwrap()
            .unwrap();
        assert_eq!(got, first.as_bytes(), "{content_type}");
    }
}

/// Gateway spec §5.2: `GET`, the server-to-client channel, is forwarded and
/// streamed; open streams are capped per connection (§5.7), and a closed
/// one frees its place.
#[tokio::test]
async fn get_streams_are_forwarded_and_capped() {
    let s = setup_with(
        Harness::with_limits(Limits::new(8, 1, Duration::from_secs(10), Duration::from_secs(2))).await,
        CredKind::None,
        None,
    )
    .await;
    let note = event(&json!({"jsonrpc": "2.0", "method": "notifications/message"}));
    let chunk = note.clone();
    s.upstream
        .reply(move |_, hold| first_then_block("text/event-stream", &chunk, hold));
    let open = || {
        s.h.client
            .get(s.h.url("linear"))
            .bearer_auth(&s.token)
            .header(header::ACCEPT, "text/event-stream")
            .send()
    };
    let first = open().await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()["content-type"], "text/event-stream");
    // No body, so no content type, goes up with a `GET`.
    assert_eq!(s.upstream.seen()[0].header("content-type"), None);
    let mut body = first.bytes_stream();
    let got = futures::StreamExt::next(&mut body).await.unwrap().unwrap();
    assert_eq!(got, note.as_bytes());
    assert_eq!(s.upstream.seen()[0].method, "GET");
    let second = open().await.unwrap();
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    let refused: Value = second.json().await.unwrap();
    assert_eq!(refused["code"], "busy");
    assert_eq!(s.upstream.seen().len(), 1, "the refused stream went upstream");
    drop(body);
    // The place frees once the proxy sees the client gone.
    let mut status = StatusCode::SERVICE_UNAVAILABLE;
    for _ in 0..100 {
        status = open().await.unwrap().status();
        if status == StatusCode::OK {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(status, StatusCode::OK);
}

/// Gateway spec §5.7: requests in flight are capped per connection, held
/// until their body ends.
#[tokio::test]
async fn requests_past_the_cap_are_503() {
    let s = setup_with(
        Harness::with_limits(Limits::new(1, 8, Duration::from_secs(10), Duration::from_secs(2))).await,
        CredKind::None,
        None,
    )
    .await;
    s.upstream
        .reply(|_, hold| first_then_block("application/json", "{", hold));
    let first = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(first.status(), StatusCode::OK);
    let second = s.h.post("linear", &s.token, &ping(2)).await;
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(s.upstream.seen().len(), 1);
    drop(first);
    let mut status = StatusCode::SERVICE_UNAVAILABLE;
    s.upstream.reply(|_, _| json(StatusCode::OK, &json!({})));
    for _ in 0..100 {
        status = s.h.post("linear", &s.token, &ping(3)).await.status();
        if status == StatusCode::OK {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(status, StatusCode::OK);
}

/// Gateway spec §5.1: a request body over 4 MiB is 413, whether declared
/// or streamed, and nothing goes up.
#[tokio::test]
async fn a_body_over_4_mib_is_413() {
    let s = setup(CredKind::None, None).await;
    let big = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"pad":"{}"}}}}"#,
        "x".repeat(4 * 1024 * 1024)
    );
    let resp =
        s.h.client
            .post(s.h.url("linear"))
            .bearer_auth(&s.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(big.clone())
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        big.as_bytes().chunks(64 * 1024).map(|c| Ok(c.to_vec())).collect();
    let resp =
        s.h.client
            .post(s.h.url("linear"))
            .bearer_auth(&s.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(reqwest::Body::wrap_stream(futures::stream::iter(chunks)))
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "body_too_large");
    assert!(s.upstream.seen().is_empty());
    // Just under the cap passes.
    let fits = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"pad":"{}"}}}}"#,
        "x".repeat(4 * 1024 * 1024 - 100)
    );
    let resp =
        s.h.client
            .post(s.h.url("linear"))
            .bearer_auth(&s.token)
            .body(fits)
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Plan 8d decision 6: a body that is not JSON, or has a key twice, is
/// refused before anything goes up.
#[tokio::test]
async fn a_body_that_is_not_json_or_has_a_key_twice_is_400() {
    let s = setup(CredKind::None, Some(&["search"])).await;
    for body in [
        "not json",
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"ping","params":{"name":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","name":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"ping",}"#,
        // A key the gateway reads, spelt otherwise: a decoder that ignores
        // case (Go's) reads `delete` (the whole-branch review).
        r#"{"jsonrpc":"2.0","id":1,"METHOD":"tools/call","params":{"name":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","NAME":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","Params":{"name":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","na_me":"delete"}}"#,
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{\"\u{17f}ampling\":{}}}}",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"Capabilities":{"sampling":{}}}}"#,
    ] {
        let resp =
            s.h.client
                .post(s.h.url("linear"))
                .bearer_auth(&s.token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
        let answer: Value = resp.json().await.unwrap();
        assert_eq!(answer["code"], "invalid_request");
    }
    assert!(s.upstream.seen().is_empty());
}

/// Gateway spec §5.5 (G-18): a `tools/call` outside the allowlist is
/// answered by the gateway, -32602, and never reaches the upstream; one
/// inside it does.
#[tokio::test]
async fn a_tools_call_outside_the_allowlist_never_reaches_the_upstream() {
    let s = setup(CredKind::Static, Some(&["search"])).await;
    let resp = s.h.post("linear", &s.token, &call(5, "delete_everything")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let answer: Value = resp.json().await.unwrap();
    assert_eq!(answer["id"], 5);
    assert_eq!(answer["error"]["code"], -32602);
    assert_eq!(answer["error"]["message"], "tool not available through hennery");
    // In a batch too.
    let batch = json!([call(6, "search"), call(7, "delete_everything")]);
    let resp = s.h.post("linear", &s.token, &batch).await;
    let answers: Value = resp.json().await.unwrap();
    assert_eq!(answers[1]["error"]["code"], -32602);
    // A batch with nothing to answer, its refused call a notification:
    // 202, no body (decision 7).
    let note = json!([{"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "delete_everything"}}]);
    let resp = s.h.post("linear", &s.token, &note).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(resp.text().await.unwrap(), "");
    assert!(s.upstream.seen().is_empty(), "a refused call reached the upstream");
    s.upstream.reply(|_, _| {
        json(
            StatusCode::OK,
            &json!({"jsonrpc": "2.0", "id": 8, "result": {"content": []}}),
        )
    });
    let resp = s.h.post("linear", &s.token, &call(8, "search")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(s.upstream.seen().len(), 1);
}

/// Gateway spec §5.5: `tools/list` filtered to the allowlist in JSON, in a
/// JSON batch, and in an event stream (other events byte for byte); a list
/// matching nothing is `[]`.
#[tokio::test]
async fn tools_list_is_filtered_in_json_batches_and_event_streams() {
    let s = setup(CredKind::None, Some(&["search"])).await;
    s.upstream
        .reply(|_, _| json(StatusCode::OK, &listed(1, &["search", "delete", "admin"])));
    let resp = s.h.post("linear", &s.token, &list(1)).await;
    let body: Value = resp.json().await.unwrap();
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["search"]);

    s.upstream.reply(|_, _| {
        json(
            StatusCode::OK,
            &json!([listed(1, &["delete"]), {"jsonrpc": "2.0", "id": 2, "result": {}}]),
        )
    });
    let resp = s.h.post("linear", &s.token, &json!([list(1), ping(2)])).await;
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body[0]["result"]["tools"], json!([]));

    // The review's O1: an answer to id 1 written as 1.0 is filtered too.
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":1.0,"result":{"tools":[{"name":"delete"}]}}"#,
            ))
            .unwrap()
    });
    assert_eq!(body[1], json!({"jsonrpc": "2.0", "id": 2, "result": {}}));
    let renumbered: Value = s.h.post("linear", &s.token, &list(1)).await.json().await.unwrap();
    assert_eq!(renumbered["result"]["tools"], json!([]));

    let progress = "id: 1\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"p\":1}}\n\n";
    let answer = format!("id: 2\n{}", event(&listed(3, &["admin", "search"])));
    let events = [progress.to_string(), answer];
    s.upstream.reply(move |_, _| sse(&events));
    let resp = s.h.post("linear", &s.token, &list(3)).await;
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let text = resp.text().await.unwrap();
    assert!(text.starts_with(progress), "{text}");
    let data = text[progress.len()..]
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap();
    let filtered: Value = serde_json::from_str(data).unwrap();
    assert_eq!(filtered["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["result"]["tools"][0]["name"], "search");
    assert!(text[progress.len()..].starts_with("id: 2\n"), "{text}");
}

/// Without an allowlist nothing is filtered: the bytes as they came.
#[tokio::test]
async fn without_an_allowlist_tools_list_passes_as_it_came() {
    let s = setup(CredKind::None, None).await;
    let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"delete"}]}}"#;
    s.upstream.reply(move |_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(raw))
            .unwrap()
    });
    let resp = s.h.post("linear", &s.token, &list(1)).await;
    assert_eq!(resp.text().await.unwrap(), raw);
}

/// Gateway spec §5.3: a `tools/list` answer read whole to filter it is at
/// most 8 MiB, and an error past it, never truncated.
#[tokio::test]
async fn a_filtered_tools_list_over_8_mib_is_502() {
    let s = setup(CredKind::None, Some(&["search"])).await;
    s.upstream.reply(|_, _| {
        let pad = "x".repeat(8 * 1024 * 1024);
        json(
            StatusCode::OK,
            &json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": [{"name": "search", "description": pad}]}}),
        )
    });
    let resp = s.h.post("linear", &s.token, &list(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_too_large");
}

/// Gateway spec §5.6 (G-19): `initialize` goes up without `sampling`,
/// `elicitation` and `roots`.
#[tokio::test]
async fn initialize_goes_up_without_the_capabilities_not_forwarded() {
    let s = setup(CredKind::None, None).await;
    let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18",
        "capabilities": {"sampling": {}, "elicitation": {}, "roots": {"listChanged": true}},
        "clientInfo": {"name": "claude-code", "version": "1"}}});
    s.h.post("linear", &s.token, &init).await;
    let up = s.upstream.seen()[0].json();
    assert_eq!(up["params"]["capabilities"], json!({}));
    assert_eq!(up["params"]["clientInfo"]["name"], "claude-code");
}

/// Gateway spec §5.6: a server-to-client request for sampling, in a
/// stream, is answered by the gateway with an error on the same upstream
/// session, and never reaches the client.
#[tokio::test]
async fn a_server_request_for_sampling_is_answered_and_not_passed_on() {
    let s = setup(CredKind::Static, None).await;
    let sampling = event(&json!({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage", "params": {}}));
    let result = event(&json!({"jsonrpc": "2.0", "id": 1, "result": {"content": []}}));
    let events = [sampling, result.clone()];
    s.upstream.reply(move |seen, _| {
        if seen.body.windows(5).any(|w| w == b"error") {
            Response::builder()
                .status(StatusCode::ACCEPTED)
                .body(Body::empty())
                .unwrap()
        } else {
            let mut resp = sse(&events);
            resp.headers_mut().insert("mcp-session-id", "up-7".parse().unwrap());
            resp
        }
    });
    let resp = s.h.post("linear", &s.token, &call(1, "search")).await;
    assert_eq!(resp.text().await.unwrap(), result);
    let mut answered = None;
    for _ in 0..100 {
        answered = s
            .upstream
            .seen()
            .into_iter()
            .find(|seen| seen.json().get("error").is_some());
        if answered.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let answered = answered.expect("the sampling request was not answered");
    assert_eq!(answered.method, "POST");
    assert_eq!(answered.json()["id"], "srv-1");
    assert_eq!(answered.json()["error"]["code"], -32601);
    assert_eq!(answered.header("mcp-session-id"), Some("up-7"));
    assert_eq!(answered.header("authorization"), Some("Bearer upstream-secret-token"));
}

/// Plan 8d decision 9: the egress allowance is the connection's stored
/// `internal_network` flag, read at every request, and nothing the request
/// carries. Not marked, a loopback upstream is refused before any
/// connection is opened; marked by the operator, the next request goes.
#[tokio::test]
async fn the_egress_allowance_is_the_connection_s_stored_flag() {
    let h = Harness::new().await;
    h.host("host-a", 1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = accepted.clone();
    let accepting = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            drop(socket);
        }
    });
    let id = h.connection_with(NewConnection {
        slug: "lan".into(),
        label: "LAN".into(),
        url: format!("https://127.0.0.1:{port}/mcp"),
        hat_id: h.hat(),
        cred_kind: CredKind::None,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: false,
    });
    h.mount(&id, &["host-a"]);
    let token = h.mint("s1", "host-a", &h.hat());
    let ask = async || {
        h.client
            .post(h.url("lan"))
            .bearer_auth(&token)
            .header(header::CONTENT_TYPE, "application/json")
            // Nothing a request says moves it to the internal network.
            .header("x-internal-network", "true")
            .header("x-hennery-allowance", "internal_network")
            .body(ping(1).to_string())
            .send()
            .await
            .unwrap()
    };
    let resp = ask().await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_unreachable");
    assert_eq!(
        accepted.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a public-only request connected"
    );
    let patch = hennery_gateway::model::ConnectionPatch {
        internal_network: Some(true),
        ..Default::default()
    };
    h.store.update(&id, &patch, hennery_kernel::secret::unix_now()).unwrap();
    // Plain TCP behind an https URL: the handshake fails, but it connected.
    assert_eq!(ask().await.status(), StatusCode::BAD_GATEWAY);
    assert!(
        accepted.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the marked connection never connected"
    );
    accepting.abort();
}

/// Gateway spec §5.3: an event stream is passed on event by event, and one
/// event is at most 8 MiB: past that without its end, the stream ends.
#[tokio::test]
async fn an_event_over_8_mib_ends_the_stream() {
    let s = setup(CredKind::None, None).await;
    s.upstream.reply(|_, hold| {
        let big = format!("data: {}", "x".repeat(8 * 1024 * 1024 + 1));
        first_then_block("text/event-stream", &big, hold)
    });
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let read = tokio::time::timeout(Duration::from_secs(10), resp.bytes())
        .await
        .expect("the stream went on past 8 MiB without an event's end");
    assert!(read.is_err(), "the stream ended cleanly");
}

/// The review's B1: the answer's type is the one the gateway judged it by,
/// exactly; a parameter cannot smuggle another, and two types are refused.
#[tokio::test]
async fn the_answer_s_type_is_the_one_the_gateway_judged() {
    let s = setup(CredKind::None, None).await;
    for (sent, seen) in [
        ("application/json; charset=utf-8", "application/json"),
        ("application/json; x=text/event-stream", "application/json"),
        ("Text/Event-Stream; charset=utf-8", "text/event-stream"),
    ] {
        s.upstream.reply(move |_, _| {
            Response::builder()
                .header(header::CONTENT_TYPE, sent)
                .body(Body::from("{}"))
                .unwrap()
        });
        let resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.status(), StatusCode::OK, "{sent}");
        let types: Vec<_> = resp.headers().get_all("content-type").iter().collect();
        assert_eq!(types, [seen], "{sent}");
    }
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from("{}"))
            .unwrap()
    });
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_content_type");
}

/// The review's B2: an event stream fails closed. A refused server request
/// behind a byte-order mark, in data serde_json cannot read, or in a last
/// event the stream never ends, never reaches the client.
#[tokio::test]
async fn an_event_stream_fails_closed_on_what_it_cannot_read() {
    let s = setup(CredKind::None, None).await;
    let sampling = r#"{"jsonrpc":"2.0","id":"srv-1","method":"sampling/createMessage","params":{}}"#;
    let surrogate = r#"{"jsonrpc":"2.0","id":"srv-2","method":"sampling/createMessage","params":{"x":"\ud800"}}"#;
    let result = event(&json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
    let streams = [
        format!("\u{feff}data: {sampling}\n\n{result}"),
        format!("data: {surrogate}\n\n{result}"),
        format!("{result}data: {sampling}\n"),
        // The re-confirmation's note 1: a mark that would start the client's
        // stream, behind a stripped mark or a dropped event.
        format!("\u{feff}\u{feff}data: {sampling}\n\n{result}"),
        format!("data: not json\n\n\u{feff}data: {sampling}\n\n{result}"),
        // A key spelt otherwise: serde_json reads no method, a client that
        // ignores case reads sampling (the whole-branch review).
        format!(
            "data: {{\"jsonrpc\":\"2.0\",\"id\":\"srv-4\",\"METHOD\":\"sampling/createMessage\",\"params\":{{}}}}\n\n{result}"
        ),
        // A key twice: serde_json reads `ping`, a client may read sampling
        // (the Task 2 review's finding 3).
        format!(
            "data: {{\"jsonrpc\":\"2.0\",\"id\":\"srv-3\",\"method\":\"sampling/createMessage\",\"method\":\"ping\",\"params\":{{}}}}\n\n{result}"
        ),
    ];
    for body in streams {
        let sent = body.clone();
        s.upstream.reply(move |_, _| {
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(sent.clone()))
                .unwrap()
        });
        let text = s.h.post("linear", &s.token, &ping(1)).await.text().await.unwrap();
        assert_eq!(text, result, "{body:?}");
    }
    // The one behind the mark was read, and answered.
    let mut answered = false;
    for _ in 0..100 {
        answered = s
            .upstream
            .seen()
            .iter()
            .any(|seen| seen.body.windows(5).any(|w| w == b"srv-1"));
        if answered {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(answered, "the request behind the mark was not answered");
}

/// The review's O3: a request body has `body_timeout` to arrive, and a
/// stalled one is 408; nothing goes up.
#[tokio::test]
async fn a_body_that_does_not_arrive_in_time_is_408() {
    let s = setup(CredKind::None, None).await;
    let mut stream = tokio::net::TcpStream::connect(s.h.addr).await.unwrap();
    let request = format!(
        "POST /mcp/linear HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{\"jsonrpc\"",
        s.h.addr, s.token
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .unwrap();
    let mut answer = vec![0u8; 4096];
    let read = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::io::AsyncReadExt::read(&mut stream, &mut answer),
    )
    .await
    .expect("no answer to a stalled body")
    .unwrap();
    let answer = String::from_utf8_lossy(&answer[..read]);
    assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
    assert!(answer.contains("request_timeout"), "{answer}");
    assert!(s.upstream.seen().is_empty());
}

/// The review's O3: a `GET` stream takes a stream permit, not a request
/// one, so open channels never starve a connection's requests.
#[tokio::test]
async fn a_get_stream_takes_no_request_permit() {
    let s = setup_with(
        Harness::with_limits(Limits::new(1, 1, Duration::from_secs(10), Duration::from_secs(2))).await,
        CredKind::None,
        None,
    )
    .await;
    s.upstream.reply(|seen, hold| {
        if seen.method == "GET" {
            first_then_block("text/event-stream", ": open\n\n", hold)
        } else {
            json(StatusCode::OK, &json!({"jsonrpc": "2.0", "id": 1, "result": {}}))
        }
    });
    let stream =
        s.h.client
            .get(s.h.url("linear"))
            .bearer_auth(&s.token)
            .send()
            .await
            .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(s.h.post("linear", &s.token, &ping(1)).await.status(), StatusCode::OK);
    drop(stream);
}

/// Plan 8d decision 10: the response head has `head_timeout` to arrive (300
/// s by default, past the egress client's own deadline); an upstream that
/// takes the request and never answers is 502 `upstream_unreachable` then
/// (the Task 2 review's finding 4).
#[tokio::test]
async fn an_upstream_that_never_answers_is_502_after_the_head_timeout() {
    let h = Harness::with_limits(Limits::new(8, 8, Duration::from_millis(300), Duration::from_secs(2))).await;
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = silent.local_addr().unwrap();
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        loop {
            let (conn, _) = silent.accept().await.unwrap();
            open.push(conn);
        }
    });
    h.host("host-a", 1);
    let hat = h.hat();
    let id = h.connection_in("silent", &format!("http://{addr}/mcp"), CredKind::None, &hat, None);
    h.mount(&id, &["host-a"]);
    let token = h.mint("s1", "host-a", &hat);
    let resp = tokio::time::timeout(Duration::from_secs(5), h.post("silent", &token, &ping(1)))
        .await
        .expect("no answer within 5 s: the head timeout did not fire");
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_unreachable");
    held.abort();
}

/// Plan 8d decision 14: a refused server request's answer goes on the
/// request's own `Mcp-Session-Id` when the upstream's answer names none;
/// and at the connection's request cap it is skipped, not queued (the Task
/// 2 review's finding 5).
#[tokio::test]
async fn a_refused_server_request_s_answer_takes_the_request_s_session_and_a_permit() {
    for cap in [2, 1] {
        let s = setup_with(
            Harness::with_limits(Limits::new(cap, 8, Duration::from_secs(10), Duration::from_secs(2))).await,
            CredKind::None,
            None,
        )
        .await;
        let sampling =
            event(&json!({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage", "params": {}}));
        s.upstream.reply(move |seen, hold| {
            if seen.body.windows(5).any(|w| w == b"error") {
                Response::builder()
                    .status(StatusCode::ACCEPTED)
                    .body(Body::empty())
                    .unwrap()
            } else {
                // The sampling request, then nothing: the open stream holds
                // its request permit.
                first_then_block("text/event-stream", &sampling, hold)
            }
        });
        let resp =
            s.h.client
                .post(s.h.url("linear"))
                .bearer_auth(&s.token)
                .header(header::CONTENT_TYPE, "application/json")
                .header("mcp-session-id", "client-session-9")
                .body(ping(1).to_string())
                .send()
                .await
                .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let answered = || {
            s.upstream
                .seen()
                .into_iter()
                .find(|seen| seen.json().get("error").is_some())
        };
        let mut found = None;
        for _ in 0..50 {
            found = answered();
            if found.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if cap == 2 {
            let found = found.expect("the sampling request was not answered");
            assert_eq!(found.header("mcp-session-id"), Some("client-session-9"));
        } else {
            assert!(found.is_none(), "answered past the request cap");
        }
        drop(resp);
    }
}

/// Plan 8b-ii's obligation, nothing cached across a `PATCH` (the
/// whole-branch review): a refused server request's answer reads the
/// connection again, so a request that arrives on a stream opened before
/// the connection changed is left unanswered: its URL moved, its internal
/// marking went (written to the row: the API keeps an `http` URL marked),
/// it was unmounted, or another connection has its slug and URL now.
#[tokio::test]
async fn a_refused_server_request_after_its_connection_changed_is_left_unanswered() {
    for change in ["moved", "unmarked", "unmounted", "replaced"] {
        let s = setup(CredKind::None, None).await;
        let moved_to = FakeUpstream::start().await;
        let (go, wait) = tokio::sync::watch::channel(false);
        let sampling =
            event(&json!({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage", "params": {}}));
        let replies = move |seen: &support::upstream::Seen, _: &tokio::sync::watch::Receiver<()>| {
            if seen.body.windows(5).any(|w| w == b"error") {
                return Response::builder()
                    .status(StatusCode::ACCEPTED)
                    .body(Body::empty())
                    .unwrap();
            }
            // An open stream; the server request only once the test says so.
            let (wait, sampling) = (wait.clone(), sampling.clone());
            let body = futures::stream::unfold(0, move |step| {
                let (mut wait, sampling) = (wait.clone(), sampling.clone());
                async move {
                    match step {
                        0 => Some((Ok::<_, std::io::Error>(": open\n\n".to_string()), 1)),
                        1 => {
                            wait.wait_for(|go| *go).await.unwrap();
                            Some((Ok(sampling), 2))
                        }
                        _ => None,
                    }
                }
            });
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(body))
                .unwrap()
        };
        s.upstream.reply(replies);
        let mut resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.chunk().await.unwrap().unwrap(), ": open\n\n", "{change}");
        let now = hennery_kernel::secret::unix_now();
        match change {
            "moved" => {
                let patch = hennery_gateway::model::ConnectionPatch {
                    url: Some(moved_to.url("/mcp")),
                    ..Default::default()
                };
                s.h.store.update(&s.id, &patch, now).unwrap();
            }
            "unmarked" => {
                let changed =
                    s.h.raw()
                        .execute("UPDATE gw_connections SET internal_network = 0 WHERE id = ?1", [&s.id])
                        .unwrap();
                assert_eq!(changed, 1);
            }
            "unmounted" => s.h.mount(&s.id, &[]),
            _ => {
                assert!(s.h.store.delete(&s.id).unwrap());
                let again = s.h.connection("linear", &s.upstream.url("/mcp"), CredKind::None);
                s.h.mount(&again, &["host-a"]);
            }
        }
        go.send(true).unwrap();
        // The request is not passed on, and the stream ends.
        while resp.chunk().await.unwrap().is_some() {}
        tokio::time::sleep(Duration::from_millis(500)).await;
        let answered = |up: &FakeUpstream| up.seen().iter().any(|seen| seen.body.windows(5).any(|w| w == b"error"));
        assert!(!answered(&s.upstream), "{change}: answered at the stream's URL");
        assert!(!answered(&moved_to), "{change}: answered at the new URL");
    }
}

/// Gateway spec §5.1: a body declared over 4 MiB is 413 at once, before
/// any of it is read; a client that never sends it is not kept waiting for
/// the body timeout (408).
#[tokio::test]
async fn a_body_declared_over_4_mib_is_413_before_it_is_read() {
    let s = setup(CredKind::None, None).await;
    let mut stream = tokio::net::TcpStream::connect(s.h.addr).await.unwrap();
    let request = format!(
        "POST /mcp/linear HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        s.h.addr,
        s.token,
        4 * 1024 * 1024 + 1
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .unwrap();
    let mut answer = vec![0u8; 4096];
    let read = tokio::time::timeout(
        Duration::from_secs(1),
        tokio::io::AsyncReadExt::read(&mut stream, &mut answer),
    )
    .await
    .expect("no answer within 1 s: the body was waited for")
    .unwrap();
    let answer = String::from_utf8_lossy(&answer[..read]);
    assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");
    assert!(answer.contains("body_too_large"), "{answer}");
    assert!(s.upstream.seen().is_empty());
}

/// Plan 8d: a `tools/list` answer in JSON that must be filtered, and does
/// not parse, is 502 `upstream_invalid`; nothing of it is passed on.
#[tokio::test]
async fn a_filtered_tools_list_that_does_not_parse_is_502() {
    let s = setup(CredKind::None, Some(&["search"])).await;
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"delete"}"#,
            ))
            .unwrap()
    });
    let resp = s.h.post("linear", &s.token, &list(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_invalid");
}

/// Plan 8d decision 13: what the gateway cannot read of its own store is
/// 500 `internal`, an `ApiError`, and nothing goes up.
#[tokio::test]
async fn a_connection_the_store_holds_damaged_is_500_internal() {
    let s = setup(CredKind::None, None).await;
    let changed =
        s.h.raw()
            .execute("UPDATE gw_connections SET url = 'not a url' WHERE id = ?1", [&s.id])
            .unwrap();
    assert_eq!(changed, 1);
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "internal");
    assert!(s.upstream.seen().is_empty());
}
