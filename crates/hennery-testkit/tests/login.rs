//! Login and logout (kernel spec §3.2, §3.3, §11): the owner's password
//! opens a session, rate limited per client address with one password
//! check per attempt, and only from the `public_url`'s origin.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{ApiError, LoginRequest};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;

const PASSWORD: &str = "correct horse battery";
const ORIGIN: &str = "https://hennery.example";

struct Collector {
    addr: SocketAddr,
    state: AppState,
}

impl Collector {
    async fn start(set_up: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open_in_memory().unwrap(),
            Hosts::open_in_memory().unwrap(),
            Operator::open_in_memory().unwrap(),
        );
        if set_up {
            let token = state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
            state.operator.set_up(&token, PASSWORD, ORIGIN, unix_now()).unwrap();
        }
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state }
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!("http://{}{path}", self.addr))
            .header("origin", ORIGIN)
    }

    async fn login(&self, password: &str) -> reqwest::Response {
        self.post("/api/auth/login")
            .json(&LoginRequest {
                password: password.into(),
            })
            .send()
            .await
            .unwrap()
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap().code)
}

/// The session token a response's `Set-Cookie` carries.
fn cookie_token(resp: &reqwest::Response) -> String {
    let cookie = resp.headers()["set-cookie"].to_str().unwrap();
    cookie
        .strip_prefix("hennery_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn the_owners_password_opens_a_session_and_logout_ends_it() {
    let c = Collector::start(true).await;
    let resp = c.login(PASSWORD).await;
    assert_eq!(resp.status(), 204);
    assert!(
        resp.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .ends_with("; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000; Secure")
    );
    let token = cookie_token(&resp);
    assert!(c.state.operator.authenticate(&token, unix_now()).unwrap().is_some());

    let out = c
        .post("/api/auth/logout")
        .header("cookie", format!("hennery_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), 204);
    assert_eq!(cookie_token(&out), "");
    assert!(c.state.operator.authenticate(&token, unix_now()).unwrap().is_none());
    // Logging out again, or without a session, is harmless.
    let again = c.post("/api/auth/logout").send().await.unwrap();
    assert_eq!(again.status(), 204);
}

/// Kernel spec §3.2: 5 wrong passwords a minute per address, then backoff,
/// and every attempt that gets in runs exactly one password check (the
/// constant-time failure path); a refused one runs none, and a right
/// password is refused too while the address is locked out.
#[tokio::test]
async fn wrong_passwords_lock_the_address_out_and_each_attempt_checks_once() {
    let c = Collector::start(true).await;
    for n in 1..=5 {
        assert_eq!(
            code_of(c.login("wrong password").await).await,
            (401, "invalid_password".into())
        );
        assert_eq!(c.state.operator.verifications(), n);
    }
    let locked = c.login(PASSWORD).await;
    assert_eq!(locked.status(), 429);
    let retry_after: u64 = locked.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(code_of(locked).await, (429, "rate_limited".into()));
    assert_eq!(
        c.state.operator.verifications(),
        5,
        "a locked-out attempt checked the password"
    );
}

#[tokio::test]
async fn a_right_password_clears_the_count() {
    let c = Collector::start(true).await;
    for _ in 0..4 {
        c.login("wrong password").await;
    }
    assert_eq!(c.login(PASSWORD).await.status(), 204);
    for _ in 0..4 {
        assert_eq!(c.login("wrong password").await.status(), 401);
    }
    assert_eq!(c.login(PASSWORD).await.status(), 204);
}

/// Kernel spec §3.3 on the login route itself: from the `public_url`'s
/// origin only, as JSON, and not before setup.
#[tokio::test]
async fn login_needs_the_public_urls_origin_json_and_setup() {
    let c = Collector::start(true).await;
    let body = serde_json::json!({ "password": PASSWORD }).to_string();
    let url = format!("http://{}/api/auth/login", c.addr);
    let client = reqwest::Client::new();
    let missing = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(missing).await, (403, "origin_mismatch".into()));
    for origin in [
        "https://evil.example",
        "http://hennery.example",
        "https://hennery.example:444",
    ] {
        let resp = client
            .post(&url)
            .header("origin", origin)
            .header("content-type", "application/json")
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()), "{origin}");
    }
    for content_type in [Some("text/plain"), Some("application/x-www-form-urlencoded"), None] {
        let mut req = c.post("/api/auth/login").body(body.clone());
        if let Some(content_type) = content_type {
            req = req.header("content-type", content_type);
        }
        let resp = req.send().await.unwrap();
        assert_eq!(
            code_of(resp).await,
            (415, "unsupported_media_type".into()),
            "{content_type:?}"
        );
    }
    // None of those reached the password check.
    assert_eq!(c.state.operator.verifications(), 0);

    let unset = Collector::start(false).await;
    assert_eq!(
        code_of(unset.login(PASSWORD).await).await,
        (403, "setup_required".into())
    );
}

/// A body far larger than any password is refused with 413 before it is
/// parsed, on setup and on login alike.
#[tokio::test]
async fn an_oversized_body_is_refused_before_it_is_parsed() {
    let unset = Collector::start(false).await;
    let token = unset.state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
    let huge = "x".repeat(hennery_kernel::auth_api::MAX_BODY_BYTES + 1);
    let setup = unset
        .post("/api/setup")
        .json(&serde_json::json!({ "token": token, "password": huge, "public_url": ORIGIN }))
        .send()
        .await
        .unwrap();
    assert_eq!(setup.status(), 413);
    assert!(!unset.state.operator.is_set_up().unwrap());

    let c = Collector::start(true).await;
    assert_eq!(c.login(&huge).await.status(), 413);
    assert_eq!(c.state.operator.verifications(), 0);
}

/// A logged request must never show the password.
#[test]
fn a_login_request_does_not_show_its_password_in_debug() {
    let shown = format!(
        "{:?}",
        LoginRequest {
            password: PASSWORD.into()
        }
    );
    assert!(!shown.contains(PASSWORD), "{shown}");
}
