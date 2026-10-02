//! A revoke ends what is open on its token (plan 8e decision 12, the fleet
//! parent's ruling): an event stream already flowing is cut, a request
//! still waiting answers 404, and another token's stream goes on. Every
//! wait has a positive signal; the bounds are failure bounds, not sleeps.

mod support;

use axum::body::{Body, Bytes};
use axum::http::{StatusCode, header};
use axum::response::Response;
use futures::StreamExt;
use hennery_gateway::model::CredKind;
use hennery_gateway::proxy::Limits;
use hennery_gateway::session::{GatewayMcp, SessionMcp};
use std::time::Duration;
use support::upstream::{FakeUpstream, Harness};
use tokio::sync::watch;

/// How long anything here may take before the test fails.
const BOUND: Duration = Duration::from_secs(10);

/// An event stream that sends one event, then a second once `next` is
/// signalled, then holds until the fake is dropped.
fn two_events(next: watch::Receiver<bool>, hold: &watch::Receiver<()>) -> Response {
    let hold = hold.clone();
    let stream = futures::stream::unfold(0, move |step| {
        let mut next = next.clone();
        let mut hold = hold.clone();
        async move {
            match step {
                0 => Some((Ok::<_, std::io::Error>(Bytes::from("data: {\"n\":1}\n\n")), 1)),
                1 => {
                    while !*next.borrow_and_update() {
                        if next.changed().await.is_err() {
                            return None;
                        }
                    }
                    Some((Ok(Bytes::from("data: {\"n\":2}\n\n")), 2))
                }
                _ => {
                    while hold.changed().await.is_ok() {}
                    None
                }
            }
        }
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

struct Open {
    chunks: futures::stream::BoxStream<'static, reqwest::Result<Bytes>>,
}

impl Open {
    /// The next chunk, or `None` once the stream ended or broke.
    async fn next(&mut self) -> Option<Bytes> {
        match tokio::time::timeout(BOUND, self.chunks.next()).await {
            Ok(Some(Ok(bytes))) => Some(bytes),
            Ok(Some(Err(_)) | None) => None,
            Err(_) => panic!("neither a chunk nor an end within {BOUND:?}"),
        }
    }
}

async fn open(h: &Harness, token: &str) -> Open {
    let response = h
        .client
        .get(h.url("linear"))
        .bearer_auth(token)
        .header(header::ACCEPT, "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    Open {
        chunks: response.bytes_stream().boxed(),
    }
}

/// A connection `linear` on `host-a` in the default hat, answered by
/// `upstream`.
fn mounted(h: &Harness, upstream: &FakeUpstream) {
    h.host("host-a", 1);
    let id = h.connection("linear", &upstream.url("/mcp"), CredKind::None);
    h.mount(&id, &["host-a"]);
}

fn revoke(h: &Harness, session: &str) {
    let mcp = GatewayMcp::new(&h.gateway());
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    let cut = mcp.revoke_in(&tx, session).unwrap();
    tx.commit().unwrap();
    mcp.cut(cut);
}

#[tokio::test]
async fn a_revoke_cuts_the_tokens_open_stream_and_not_another_tokens() {
    let upstream = FakeUpstream::start().await;
    let (next, wait) = watch::channel(false);
    upstream.reply(move |_, hold| two_events(wait.clone(), hold));
    let h = Harness::new().await;
    mounted(&h, &upstream);
    let hat = h.hat();
    let mine = h.mint("s1", "host-a", &hat);
    let theirs = h.mint("s2", "host-a", &hat);
    let mut cut = open(&h, &mine).await;
    let mut kept = open(&h, &theirs).await;
    assert!(cut.next().await.is_some(), "the first event");
    assert!(kept.next().await.is_some(), "the first event");
    revoke(&h, "s1");
    // Cut: the stream ends (broken) without its second event.
    assert_eq!(cut.next().await, None);
    // The other token's stream still flows: its second event arrives.
    next.send(true).unwrap();
    let second = kept.next().await.expect("the other stream goes on");
    assert!(String::from_utf8_lossy(&second).contains("\"n\":2"), "{second:?}");
}

/// A request on the token still being read when the revoke commits ends at
/// once with the same 404 as an unknown token, not at its body's timeout.
#[tokio::test]
async fn a_revoke_ends_a_request_not_yet_answered() {
    let upstream = FakeUpstream::start().await;
    let h = Harness::with_limits(Limits::new(8, 8, Duration::from_secs(60), Duration::from_secs(60))).await;
    mounted(&h, &upstream);
    let token = h.mint("s1", "host-a", &h.hat());
    // A body that never comes: the request waits in the proxy.
    let (_keep, never) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    let body = reqwest::Body::wrap_stream(tokio_stream_from(never));
    let request = h
        .client
        .post(h.url("linear"))
        .bearer_auth(&token)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .send();
    let request = tokio::spawn(request);
    // Positive signal: the proxy is watching the token.
    let deadline = tokio::time::Instant::now() + BOUND;
    while h.revocations.watched() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "the request never reached the proxy");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    revoke(&h, "s1");
    let response = tokio::time::timeout(BOUND, request).await.unwrap().unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(upstream.seen().is_empty(), "nothing went upstream");
}

/// The watch goes when the request does: nothing is kept per token once
/// its answers have ended.
#[tokio::test]
async fn a_finished_request_leaves_no_watch() {
    let upstream = FakeUpstream::start().await;
    let h = Harness::new().await;
    mounted(&h, &upstream);
    let token = h.mint("s1", "host-a", &h.hat());
    let response = h.post("linear", &token, &support::upstream::list(1)).await;
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    assert_eq!(h.revocations.watched(), 0);
    // An unknown or malformed token is never watched.
    let response = h.post("linear", "not-a-token", &support::upstream::list(1)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(h.revocations.watched(), 0);
}

fn tokio_stream_from(
    mut rx: tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}
