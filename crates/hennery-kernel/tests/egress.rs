//! The outbound HTTP policy (kernel spec §7.1, plan 8b) against local
//! listeners. Every listener here is on loopback, so a connection that is
//! meant to go through uses `Allowance::InternalNetwork`; a test that one is
//! refused uses `PublicOnly` and checks the listener saw nothing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hennery_kernel::egress::{Allowance, Egress, EgressError, Method, Refused, Request, StatusCode, Timeouts, Url};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What a fake server does with each request it reads.
#[derive(Clone, Copy)]
enum Answer {
    /// `200 ok`, keeping the connection open for the next request.
    Ok,
    /// `302` to the given port on 127.0.0.1.
    Redirect(u16),
    /// Never answers.
    Silent,
    /// Headers and a first chunk at once, the last chunk after the delay.
    SlowBody(Duration),
}

/// A listener on 127.0.0.1 counting the connections it accepted and keeping
/// the request heads it read.
struct Server {
    port: u16,
    accepted: Arc<AtomicUsize>,
    heads: Arc<Mutex<Vec<String>>>,
}

impl Server {
    async fn start(answer: Answer) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let heads = Arc::new(Mutex::new(Vec::new()));
        let (count, kept) = (accepted.clone(), heads.clone());
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(stream, answer, kept.clone()));
            }
        });
        Server { port, accepted, heads }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }

    fn url(&self, host: &str) -> Url {
        Url::parse(&format!("http://{host}:{}/x", self.port)).unwrap()
    }
}

async fn serve(mut stream: TcpStream, answer: Answer, heads: Arc<Mutex<Vec<String>>>) {
    let mut buf = Vec::new();
    loop {
        // Read one request head (the tests send no bodies).
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let mut chunk = [0u8; 1024];
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        heads.lock().unwrap().push(String::from_utf8_lossy(&buf).into_owned());
        buf.clear();
        match answer {
            Answer::Ok => {
                let reply = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok";
                if stream.write_all(reply).await.is_err() {
                    return;
                }
            }
            Answer::Redirect(port) => {
                let reply = format!(
                    "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:{port}/next\r\ncontent-length: 0\r\n\r\n"
                );
                if stream.write_all(reply.as_bytes()).await.is_err() {
                    return;
                }
            }
            Answer::Silent => {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                return;
            }
            Answer::SlowBody(delay) => {
                let head = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nfirst\r\n";
                if stream.write_all(head).await.is_err() {
                    return;
                }
                tokio::time::sleep(delay).await;
                let _ = stream.write_all(b"4\r\nlast\r\n0\r\n\r\n").await;
                return;
            }
        }
    }
}

fn egress() -> Egress {
    Egress::new(Timeouts::DEFAULT).unwrap()
}

fn get(url: Url) -> Request {
    Request::new(Method::GET, url)
}

/// Bounds a call that a broken policy would leave hanging, so the test fails
/// instead of hanging.
async fn within<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("the request hung")
}

/// Plan 8b (b): reqwest never asks the resolver about an IP-literal host,
/// so the URL check itself refuses a non-public literal, on both send paths,
/// before anything connects.
#[tokio::test]
async fn a_non_public_literal_is_refused_before_connecting() {
    let server = Server::start(Answer::Ok).await;
    let client = egress().client(Allowance::PublicOnly);
    // Each passes the scheme rule (http to loopback; https), so only the
    // address check can refuse it. Unchecked, the IPv4-mapped one would
    // connect to the listener over IPv6.
    let port = server.port;
    for url in [
        format!("http://127.0.0.1:{port}/x"),
        format!("https://[::ffff:127.0.0.1]:{port}/x"),
    ] {
        let host = url.clone();
        let url = Url::parse(&url).unwrap();
        let err = within(client.send(get(url.clone()))).await.unwrap_err();
        assert!(
            matches!(err, EgressError::Refused(Refused::Address(_))),
            "{host}: {err:?}"
        );
        let err = within(client.send_streaming(get(url))).await.unwrap_err();
        assert!(
            matches!(err, EgressError::Refused(Refused::Address(_))),
            "{host} streaming: {err:?}"
        );
    }
    assert_eq!(server.accepted(), 0, "nothing reached the listener");
}

/// Plan 8b (d): a name is resolved by hennery and refused when any address
/// it resolves to is non-public; the refusal reaches the caller as
/// `Refused`, not as a transport error. `localhost` is answered without the
/// system resolver; there is no portable name that resolves inward through
/// it, so the system answer's check is the unit test on `checked`.
#[tokio::test]
async fn a_name_resolving_to_a_non_public_address_is_refused() {
    let server = Server::start(Answer::Ok).await;
    let client = egress().client(Allowance::PublicOnly);
    for name in ["localhost", "localhost."] {
        let err = within(client.send(get(server.url(name)))).await.unwrap_err();
        let EgressError::Refused(Refused::Resolved { host, .. }) = err else {
            panic!("{name}: not refused by the resolver: {err:?}");
        };
        assert_eq!(host, name);
    }
    assert_eq!(server.accepted(), 0, "nothing reached the listener");
}

/// Plan 8b (d): the internal-network allowance lets both a literal and a
/// name reach a non-public address.
#[tokio::test]
async fn the_internal_network_allowance_reaches_local_addresses() {
    let server = Server::start(Answer::Ok).await;
    let client = egress().client(Allowance::InternalNetwork);
    for host in ["127.0.0.1", "localhost"] {
        let response = within(client.send(get(server.url(host)))).await.unwrap();
        assert_eq!(response.status(), 200, "{host}");
        assert_eq!(response.text().await.unwrap(), "ok");
    }
}

/// Plan 8b: each allowance has its own client, so its own connection pool.
/// A public-only request never rides a connection an internal-network
/// request opened (the pool would skip the resolver's check).
#[tokio::test]
async fn a_public_only_request_never_reuses_an_internal_network_connection() {
    let server = Server::start(Answer::Ok).await;
    let egress = egress();
    let url = server.url("localhost");
    let response = within(egress.client(Allowance::InternalNetwork).send(get(url.clone())))
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "ok");
    // Let the finished connection go back to its pool, so a shared pool
    // would offer it (as in `clones_share_the_policy`).
    tokio::time::sleep(Duration::from_millis(200)).await;
    let err = within(egress.client(Allowance::PublicOnly).send(get(url)))
        .await
        .unwrap_err();
    assert!(matches!(err, EgressError::Refused(Refused::Resolved { .. })), "{err:?}");
    assert_eq!(server.accepted(), 1, "only the internal-network request connected");
}

/// Plan 8b (c): a redirect is returned to the caller, never followed.
#[tokio::test]
async fn a_redirect_is_returned_not_followed() {
    let next = Server::start(Answer::Ok).await;
    let server = Server::start(Answer::Redirect(next.port)).await;
    let client = egress().client(Allowance::InternalNetwork);
    let response = within(client.send(get(server.url("127.0.0.1")))).await.unwrap();
    assert_eq!(response.status(), 302);
    assert_eq!(
        response.headers()["location"],
        format!("http://127.0.0.1:{}/next", next.port).as_str()
    );
    // Let a follow-up request, were one made, arrive.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(next.accepted(), 0, "the redirect was not followed");
}

/// Plan 8b (e), 8b-ii: https, or plain http to loopback; under
/// `InternalNetwork` also plain http to an internal address, never to a
/// public one. The scheme is refused before anything is resolved.
#[tokio::test]
async fn plain_http_is_refused_except_to_loopback_or_an_internal_address() {
    let egress = egress();
    for (allowance, url) in [
        (Allowance::PublicOnly, "http://hennery-egress-test.invalid/"),
        (Allowance::PublicOnly, "http://10.0.0.1/"),
        (Allowance::InternalNetwork, "http://8.8.8.8/"),
        (Allowance::InternalNetwork, "http://[::ffff:10.0.0.1]/"),
        (Allowance::InternalNetwork, "ftp://example.com/"),
    ] {
        let client = egress.client(allowance);
        for streaming in [false, true] {
            let request = get(Url::parse(url).unwrap());
            let err = if streaming {
                within(client.send_streaming(request)).await.unwrap_err()
            } else {
                within(client.send(request)).await.unwrap_err()
            };
            assert!(
                matches!(err, EgressError::Refused(Refused::Scheme(_))),
                "{url} {allowance:?}: {err:?}"
            );
        }
    }
}

/// Plan 8b: a URL carrying credentials is refused; reqwest would send them
/// as basic auth, and they would land in every log line naming the URL.
#[tokio::test]
async fn a_url_with_credentials_is_refused() {
    let client = egress().client(Allowance::InternalNetwork);
    for url in [
        "https://user:secret@hennery-egress-test.invalid/",
        "https://user@hennery-egress-test.invalid/",
    ] {
        let err = within(client.send(get(Url::parse(url).unwrap()))).await.unwrap_err();
        assert!(
            matches!(err, EgressError::Refused(Refused::Credentials)),
            "{url}: {err:?}"
        );
    }
}

/// Plan 8b: an error never carries the URL, whose query may hold a secret,
/// so a caller can log it as it is.
#[tokio::test]
async fn an_error_does_not_carry_the_url() {
    // A port nothing listens on: bound, then closed.
    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let client = egress().client(Allowance::InternalNetwork);
    let url = Url::parse(&format!("http://127.0.0.1:{port}/x?code=s3cr3t")).unwrap();
    let err = within(client.send(get(url))).await.unwrap_err();
    assert!(matches!(err, EgressError::Http(_)), "{err:?}");
    let shown = format!("{err} {err:?}");
    assert!(!shown.contains("s3cr3t"), "{shown}");
}

fn short() -> Egress {
    Egress::new(Timeouts {
        connect: Duration::from_secs(2),
        request: Duration::from_secs(1),
    })
    .unwrap()
}

/// Plan 8b (g): a request with no timeout of its own gets the default one.
#[tokio::test]
async fn a_request_times_out_by_default() {
    let server = Server::start(Answer::Silent).await;
    let client = short().client(Allowance::InternalNetwork);
    let err = within(client.send(get(server.url("127.0.0.1")))).await.unwrap_err();
    assert!(matches!(err, EgressError::Timeout), "{err:?}");
}

/// Plan 8b (g): a streaming request must get its response head within the
/// deadline…
#[tokio::test]
async fn a_streaming_request_times_out_waiting_for_its_head() {
    let server = Server::start(Answer::Silent).await;
    let client = short().client(Allowance::InternalNetwork);
    let err = within(client.send_streaming(get(server.url("127.0.0.1"))))
        .await
        .unwrap_err();
    assert!(matches!(err, EgressError::Timeout), "{err:?}");
}

/// …but its body may take as long as it takes: the caller reads it and
/// decides how long is too idle.
#[tokio::test]
async fn a_streaming_body_outlives_the_deadline() {
    let server = Server::start(Answer::SlowBody(Duration::from_millis(2500))).await;
    let client = short().client(Allowance::InternalNetwork);
    let mut response = within(client.send_streaming(get(server.url("127.0.0.1"))))
        .await
        .unwrap();
    let mut body = Vec::new();
    while let Some(chunk) = within(response.chunk()).await.unwrap() {
        body.extend_from_slice(&chunk);
    }
    assert_eq!(body, b"firstlast");
}

/// Plan 8b (g): `send`'s deadline bounds the whole exchange, body included:
/// a body still arriving after it fails to read, as a timeout.
#[tokio::test]
async fn send_s_deadline_covers_the_body() {
    let server = Server::start(Answer::SlowBody(Duration::from_millis(2500))).await;
    let client = short().client(Allowance::InternalNetwork);
    let response = within(client.send(get(server.url("127.0.0.1")))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let err = within(response.bytes()).await.unwrap_err();
    assert!(err.is_timeout(), "{err:?}");
}

/// Plan 8b (g): a request's own timeout wins over the default, on both
/// send paths (a caller asks for less, or for more).
#[tokio::test]
async fn a_request_s_own_timeout_wins_over_the_default() {
    let server = Server::start(Answer::Silent).await;
    let client = Egress::new(Timeouts {
        connect: Duration::from_secs(2),
        request: Duration::from_secs(8),
    })
    .unwrap()
    .client(Allowance::InternalNetwork);
    for streaming in [false, true] {
        let mut request = get(server.url("127.0.0.1"));
        *request.timeout_mut() = Some(Duration::from_millis(200));
        let started = Instant::now();
        let sent = if streaming {
            within(client.send_streaming(request)).await
        } else {
            within(client.send(request)).await
        };
        assert!(
            matches!(sent, Err(EgressError::Timeout)),
            "streaming {streaming}: {sent:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "streaming {streaming}: waited {:?}, the default's 8 s",
            started.elapsed()
        );
    }
}

/// Plan 8b decision 11: every request says it is hennery.
#[tokio::test]
async fn requests_say_they_are_hennery() {
    let server = Server::start(Answer::Ok).await;
    let client = egress().client(Allowance::InternalNetwork);
    within(client.send(get(server.url("127.0.0.1")))).await.unwrap();
    let heads = server.heads();
    assert_eq!(heads.len(), 1);
    let expected = concat!("user-agent: hennery/", env!("CARGO_PKG_VERSION"), "\r\n");
    assert!(heads[0].to_ascii_lowercase().contains(expected), "{}", heads[0]);
}

/// An `Egress` is cheap to clone: a clone shares its clients, and so their
/// connection pools (L9: the binary builds one and hands out clones).
#[tokio::test]
async fn clones_share_the_policy() {
    let server = Server::start(Answer::Ok).await;
    let egress = egress();
    let clone = egress.clone();
    let url = server.url("localhost");
    let response = within(egress.client(Allowance::InternalNetwork).send(get(url.clone())))
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "ok");
    drop(egress);
    // The finished connection goes back to the shared pool from a task of
    // its own; give it a moment.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let response = within(clone.client(Allowance::InternalNetwork).send(get(url)))
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "ok");
    assert_eq!(server.accepted(), 1, "the clone reused the original's connection");
    assert_eq!(clone.client(Allowance::PublicOnly).allowance(), Allowance::PublicOnly);
}
