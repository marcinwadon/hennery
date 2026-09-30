//! The one-time setup over HTTP (kernel spec §3.1, §3.3): the token from
//! the setup link creates the owner and signs them in, once, from the
//! origin being stored as `public_url`.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_proto::rest::{ApiError, SetupRequest, SetupResponse};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;

const PASSWORD: &str = "correct horse battery";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    token: String,
}

impl Collector {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open_in_memory().unwrap(),
            Hosts::open_in_memory().unwrap(),
            Operator::open_in_memory().unwrap(),
        );
        let token = state
            .operator
            .issue_setup_token(hennery_kernel::secret::unix_now())
            .unwrap()
            .unwrap();
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, token }
    }

    async fn setup(&self, origin: Option<&str>, token: &str, password: &str, public_url: &str) -> reqwest::Response {
        let mut req = reqwest::Client::new()
            .post(format!("http://{}/api/setup", self.addr))
            .json(&SetupRequest {
                token: token.into(),
                password: password.into(),
                public_url: public_url.into(),
            });
        if let Some(origin) = origin {
            req = req.header("origin", origin);
        }
        req.send().await.unwrap()
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap().code)
}

fn session_cookie(resp: &reqwest::Response) -> String {
    resp.headers()["set-cookie"].to_str().unwrap().to_string()
}

#[tokio::test]
async fn setup_creates_the_owner_signs_them_in_and_happens_once() {
    let c = Collector::start().await;
    let origin = "https://hennery.example";
    let resp = c
        .setup(Some(origin), &c.token, PASSWORD, "https://Hennery.Example:443/")
        .await;
    assert_eq!(resp.status(), 201);
    let cookie = session_cookie(&resp);
    assert!(
        cookie.ends_with("; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000; Secure"),
        "{cookie}"
    );
    let body: SetupResponse = resp.json().await.unwrap();
    assert_eq!(body.public_url, origin);
    // The cookie is a live session, stepped up by the password just set.
    let token = cookie
        .strip_prefix("hennery_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let session = c
        .state
        .operator
        .authenticate(token, hennery_kernel::secret::unix_now())
        .unwrap()
        .unwrap();
    assert!(session.stepped_up(hennery_kernel::secret::unix_now()));
    assert_eq!(c.state.operator.public_url().unwrap().origin(), origin);

    let again = c.setup(Some(origin), &c.token, PASSWORD, origin).await;
    assert_eq!(code_of(again).await, (409, "already_set_up".into()));
}

/// Kernel spec §3.3: setup is a state-changing browser route. Before there
/// is a `public_url`, `Origin` must be the one being stored.
#[tokio::test]
async fn setup_needs_the_origin_of_the_public_url_it_stores() {
    let c = Collector::start().await;
    let url = "https://hennery.example";
    for origin in [
        None,
        Some("https://evil.example"),
        Some("null"),
        Some("http://hennery.example"),
    ] {
        let resp = c.setup(origin, &c.token, PASSWORD, url).await;
        assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()), "{origin:?}");
    }
    // None of that used the token up.
    assert_eq!(c.setup(Some(url), &c.token, PASSWORD, url).await.status(), 201);
}

#[tokio::test]
async fn a_wrong_token_bad_input_or_a_form_post_is_refused_and_keeps_the_token() {
    let c = Collector::start().await;
    let url = "http://127.0.0.1:7117";
    let wrong = c.setup(Some(url), &"0".repeat(64), PASSWORD, url).await;
    assert_eq!(code_of(wrong).await, (401, "invalid_setup_token".into()));
    let short = c.setup(Some(url), &c.token, "short", url).await;
    assert_eq!(code_of(short).await, (400, "invalid".into()));
    let remote_http = c
        .setup(
            Some("http://hennery.example"),
            &c.token,
            PASSWORD,
            "http://hennery.example",
        )
        .await;
    assert_eq!(code_of(remote_http).await, (400, "invalid".into()));
    // A form post (what a cross-site page can send without a preflight) is
    // not JSON and is refused before anything is looked at.
    let form = reqwest::Client::new()
        .post(format!("http://{}/api/setup", c.addr))
        .header("origin", url)
        .header("content-type", "text/plain")
        .body(serde_json::json!({"token": c.token, "password": PASSWORD, "public_url": url}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(form.status(), 415);

    let resp = c.setup(Some(url), &c.token, PASSWORD, url).await;
    assert_eq!(resp.status(), 201);
    // Loopback `http://`: the cookie cannot be `Secure`.
    let cookie = session_cookie(&resp);
    assert!(!cookie.contains("Secure"), "{cookie}");
}

fn assert_private(resp: &reqwest::Response) {
    assert_eq!(resp.headers()["referrer-policy"], "no-referrer");
    assert_eq!(resp.headers()["cache-control"], "no-store");
}

/// Every setup response is uncached and sends no `Referer` onwards, the
/// refusals as well as the success.
#[tokio::test]
async fn setup_responses_are_never_cached_nor_referred() {
    let c = Collector::start().await;
    let url = "https://hennery.example";
    let refused = c.setup(None, &c.token, PASSWORD, url).await;
    assert_private(&refused);
    let done = c.setup(Some(url), &c.token, PASSWORD, url).await;
    assert_eq!(done.status(), 201);
    assert_private(&done);
}

/// 3b decision 16: the link is `/setup#<token>`. The page and its script
/// are static; the script reads the token from the fragment and sends it
/// only in the `POST /api/setup` body. The page runs no inline script.
#[tokio::test]
async fn the_setup_page_is_static_and_reads_the_token_from_the_fragment() {
    let c = Collector::start().await;
    let page = reqwest::get(format!("http://{}/setup", c.addr)).await.unwrap();
    assert_eq!(page.status(), 200);
    assert_private(&page);
    let csp = page.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(
        csp.starts_with("script-src 'self';") && csp.contains("frame-ancestors 'none'"),
        "{csp}"
    );
    let html = page.text().await.unwrap();
    assert!(html.contains(r#"<script src="/setup.js" defer></script>"#), "{html}");
    assert_eq!(html.matches("<script").count(), 1, "an inline script: {html}");

    let script = reqwest::get(format!("http://{}/setup.js", c.addr)).await.unwrap();
    assert_eq!(script.status(), 200);
    assert_private(&script);
    assert!(
        script.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    let js = script.text().await.unwrap();
    assert!(
        js.contains("location.hash") && js.contains(r#"fetch("/api/setup""#),
        "{js}"
    );
}

/// A logged request must never show the password.
#[test]
fn a_setup_request_does_not_show_its_password_in_debug() {
    let req = SetupRequest {
        token: "t".into(),
        password: PASSWORD.into(),
        public_url: "https://hennery.example".into(),
    };
    let shown = format!("{req:?}");
    assert!(
        !shown.contains(PASSWORD) && shown.contains("hennery.example"),
        "{shown}"
    );
}
