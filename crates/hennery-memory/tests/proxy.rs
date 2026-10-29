//! Reverse-proxy attribution for the served memory router: two clients behind
//! one trusted peer keep separate sign-in budgets; nothing else is trusted.
use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use hennery_kernel::{client_address::ProxyPolicy, operator::Operator};
use hennery_memory::{serve, store::Store, ui};
use std::{net::SocketAddr, sync::Arc};
use tower::ServiceExt;
const ORIGIN: &str = "https://memory.example.test";
const PASSWORD: &str = "synthetic proxy password";
const PROXY: &str = "127.0.0.1";
struct World {
    _dir: tempfile::TempDir,
    app: Router,
}
impl World {
    fn new(peers: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("memory.db")).unwrap());
        ui::bootstrap(&store, dir.path(), "fixture-owner", ORIGIN, PASSWORD).unwrap();
        let operator = Arc::new(Operator::open(&dir.path().join("memory-auth.db")).unwrap());
        let policy = ProxyPolicy::new(peers.iter().map(|p| p.parse().unwrap()).collect()).unwrap();
        Self {
            app: serve::app(store, Some(operator), policy).unwrap(),
            _dir: dir,
        }
    }
    async fn send(&self, path: &str, peer: &str, clients: &[&str], password: Option<&str>) -> StatusCode {
        let (content_type, body) = match (path, password) {
            ("/login", Some(p)) => (
                "application/x-www-form-urlencoded",
                format!("password={}", p.replace(' ', "+")),
            ),
            (_, Some(p)) => ("application/json", format!(r#"{{"password":"{p}"}}"#)),
            (_, None) => ("application/json", String::new()),
        };
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header("origin", ORIGIN)
            .header("content-type", content_type)
            .extension(ConnectInfo(format!("{peer}:40000").parse::<SocketAddr>().unwrap()));
        for client in clients {
            request = request.header("hennery-client-ip", *client);
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        response.status()
    }
    async fn login(&self, path: &str, peer: &str, client: Option<&str>, password: &str) -> StatusCode {
        self.send(path, peer, client.as_slice(), Some(password)).await
    }
}
fn signed_in(status: StatusCode) -> bool {
    status == StatusCode::SEE_OTHER || status == StatusCode::NO_CONTENT || status == StatusCode::OK
}

#[tokio::test]
async fn trusted_proxy_keeps_two_clients_password_budgets_and_clearing_separate() {
    for path in ["/login", "/api/auth/login"] {
        let w = World::new(&[PROXY]);
        let (a, b) = (Some("192.0.2.1"), Some("2001:db8::2"));
        for _ in 0..5 {
            assert_eq!(
                w.login(path, PROXY, a, "wrong").await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
        assert_eq!(
            w.login(path, PROXY, a, "wrong").await,
            StatusCode::TOO_MANY_REQUESTS,
            "{path}"
        );
        assert!(
            signed_in(w.login(path, PROXY, b, PASSWORD).await),
            "{path}: B shares A's budget"
        );
        // B's success clears only B: A stays locked out, even with the password.
        assert_eq!(
            w.login(path, PROXY, a, PASSWORD).await,
            StatusCode::TOO_MANY_REQUESTS,
            "{path}"
        );
        for _ in 0..4 {
            assert_eq!(
                w.login(path, PROXY, b, "wrong").await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
        assert!(signed_in(w.login(path, PROXY, b, PASSWORD).await), "{path}");
        for _ in 0..5 {
            assert_eq!(
                w.login(path, PROXY, b, "wrong").await,
                StatusCode::UNAUTHORIZED,
                "{path}: not cleared"
            );
        }
    }
}

#[tokio::test]
async fn trusted_proxy_keeps_two_clients_passkey_budgets_separate() {
    const START: &str = "/api/auth/passkeys/login/start";
    let w = World::new(&[PROXY]);
    let (a, b) = ("192.0.2.1", "192.0.2.2");
    for _ in 0..5 {
        assert_eq!(
            w.login("/login", PROXY, Some(b), "wrong").await,
            StatusCode::UNAUTHORIZED
        );
    }
    // No passkey is registered: each start is refused, yet counts.
    for _ in 0..30 {
        assert_eq!(w.send(START, PROXY, &[a], None).await, StatusCode::CONFLICT);
    }
    assert_eq!(w.send(START, PROXY, &[a], None).await, StatusCode::TOO_MANY_REQUESTS);
    // B's password failures and A's passkey starts are separate budgets.
    assert_eq!(w.send(START, PROXY, &[b], None).await, StatusCode::CONFLICT);
}

#[tokio::test]
async fn without_configured_peers_the_header_is_ignored_even_from_loopback() {
    let w = World::new(&[]);
    for _ in 0..5 {
        assert_eq!(
            w.login("/login", PROXY, Some("192.0.2.1"), "wrong").await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        w.login("/login", PROXY, Some("192.0.2.2"), PASSWORD).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn untrusted_peers_cannot_spoof_and_trusted_peers_cannot_send_malformed_headers() {
    let w = World::new(&[PROXY]);
    let direct = "198.51.100.7";
    for n in 1..=5 {
        let client = format!("192.0.2.{n}");
        assert_eq!(
            w.login("/login", direct, Some(&client), "wrong").await,
            StatusCode::UNAUTHORIZED
        );
    }
    // Keyed by the direct peer, whatever it claims.
    assert_eq!(
        w.login("/login", direct, Some("192.0.2.200"), PASSWORD).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    for clients in [
        &["192.0.2.1", "192.0.2.2"][..],
        &["192.0.2.1, 192.0.2.2"],
        &["not-an-address"],
        &["0.0.0.0"],
        &["192.0.2.1:443"],
    ] {
        assert_eq!(
            w.send("/login", PROXY, clients, Some(PASSWORD)).await,
            StatusCode::BAD_REQUEST,
            "{clients:?}"
        );
    }
    // A trusted peer without the header falls back to its own address.
    for _ in 0..5 {
        assert_eq!(w.login("/login", PROXY, None, "wrong").await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        w.login("/login", PROXY, None, PASSWORD).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert!(signed_in(w.login("/login", PROXY, Some("192.0.2.9"), PASSWORD).await));
}

#[tokio::test]
async fn mcp_is_served_unchanged_behind_the_attribution_layer() {
    let w = World::new(&[PROXY]);
    for peer in [PROXY, "198.51.100.7"] {
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("authorization", "Bearer hnry_plugin_unknown")
            .header("content-type", "application/json")
            .header("hennery-client-ip", "192.0.2.1")
            .extension(ConnectInfo(format!("{peer}:40000").parse::<SocketAddr>().unwrap()))
            .body(Body::from("{}"))
            .unwrap();
        let response = w.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
