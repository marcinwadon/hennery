//! Step-up (kernel spec §3.4, §11) and the signed-in sessions (§3.2):
//! minting a pairing code, changing or revoking a host, replacing its path
//! rules, changing a hat (plan 5a decisions 7 and 8), re-assigning a
//! session to another hat (plan 5d decision 2) and revoking a session need
//! a password check within the last five minutes, and are refused without
//! one, accepted within five minutes, and refused after.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{ApiError, AuthSessionItem, StepUpRequest};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::{OWNER_PASSWORD, PUBLIC_URL};
use std::net::SocketAddr;
use std::time::Duration;

struct Collector {
    addr: SocketAddr,
    state: AppState,
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
        // Sets the collector up.
        hennery_testkit::operator_client(&state.operator);
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state }
    }

    /// A session whose last password check was `age` seconds ago.
    fn session(&self, age: i64) -> String {
        let phc = hennery_testkit::owner_phc(&self.state.operator);
        self.state
            .operator
            .open_session("test", &phc, unix_now() - age)
            .unwrap()
            .unwrap()
    }

    fn request(&self, session: &str, method: &str, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method.parse().unwrap(), format!("http://{}{path}", self.addr))
            .header("origin", PUBLIC_URL)
            .header("cookie", format!("hennery_session={session}"))
    }

    async fn step_up(&self, session: &str, password: &str) -> reqwest::Response {
        self.request(session, "POST", "/api/auth/step-up/password")
            .json(&StepUpRequest {
                password: password.into(),
            })
            .send()
            .await
            .unwrap()
    }

    fn id_of(&self, session: &str) -> String {
        self.state
            .operator
            .authenticate(session, unix_now())
            .unwrap()
            .unwrap()
            .session_id
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (
        status,
        resp.json::<ApiError>().await.map(|e| e.code).unwrap_or_default(),
    )
}

/// Kernel spec §3.4 and §11: every listed action refused without a fresh
/// check, accepted within five minutes, and refused after.
#[tokio::test]
async fn minting_changing_revoking_a_host_and_revoking_a_session_need_a_fresh_password_check() {
    let c = Collector::start().await;
    let other = c.session(0);
    let actions = [
        ("POST", "/api/hosts/pairing-codes".to_string(), None, 201),
        ("PATCH", "/api/hosts/host-9".to_string(), Some(r#"{"name":"x"}"#), 404),
        (
            "PUT",
            "/api/hosts/host-9/path-rules".to_string(),
            Some(r#"{"rules":[]}"#),
            404,
        ),
        ("PATCH", "/api/hats/hat-9".to_string(), Some(r#"{"name":"x"}"#), 404),
        (
            "PATCH",
            "/api/sessions/s-9".to_string(),
            Some(r#"{"hat_id":"hat-9"}"#),
            404,
        ),
        ("DELETE", "/api/hosts/host-9".to_string(), None, 404),
        ("DELETE", format!("/api/auth/sessions/{}", c.id_of(&other)), None, 204),
    ];
    let send = |session: &str, method: &str, path: &str, body: Option<&str>| {
        let req = c.request(session, method, path);
        match body {
            Some(body) => req.header("content-type", "application/json").body(body.to_string()),
            None => req,
        }
        .send()
    };
    // Five minutes after the last check: refused, and nothing happens.
    let stale = c.session(5 * 60);
    for (method, path, body, _) in &actions {
        let resp = send(&stale, method, path, *body).await.unwrap();
        assert_eq!(code_of(resp).await, (403, "step_up_required".into()), "{method} {path}");
    }
    // A PATCH that names no hat needs no step-up (plan 5d decision 2).
    let resp = send(&stale, "PATCH", "/api/sessions/s-9", Some("{}")).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_some());
    // A wrong password does not step up.
    assert_eq!(
        code_of(c.step_up(&stale, "wrong password").await).await,
        (401, "invalid_password".into())
    );
    assert_eq!(
        code_of(
            c.request(&stale, "POST", "/api/hosts/pairing-codes")
                .send()
                .await
                .unwrap()
        )
        .await,
        (403, "step_up_required".into())
    );
    // The right one does, for this session.
    assert_eq!(c.step_up(&stale, OWNER_PASSWORD).await.status(), 204);
    for (method, path, body, status) in &actions {
        let resp = send(&stale, method, path, *body).await.unwrap();
        assert_eq!(resp.status().as_u16(), *status, "{method} {path}");
    }
    assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_none());
    // Just inside the five minutes still counts.
    let recent = c.session(5 * 60 - 5);
    let resp = c
        .request(&recent, "POST", "/api/hosts/pairing-codes")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
}

impl Collector {
    async fn login(&self, password: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("http://{}/api/auth/login", self.addr))
            .header("origin", PUBLIC_URL)
            .json(&serde_json::json!({ "password": password }))
            .send()
            .await
            .unwrap()
    }
}

/// 3b decision 10: step-up has a budget of its own. Five wrong step-ups
/// lock out step-up from that address, even with the right password, and
/// leave login alone.
#[tokio::test]
async fn wrong_step_ups_lock_out_step_up_only() {
    let c = Collector::start().await;
    let session = c.session(0);
    for _ in 0..5 {
        assert_eq!(c.step_up(&session, "wrong password").await.status(), 401);
    }
    // A locked-out step-up runs no password check.
    let checked = c.state.operator.verifications();
    assert_eq!(
        code_of(c.step_up(&session, OWNER_PASSWORD).await).await,
        (429, "rate_limited".into())
    );
    assert_eq!(c.state.operator.verifications(), checked);
    assert_eq!(c.login(OWNER_PASSWORD).await.status(), 204);
}

/// The other way round: a login flood from the owner's address (a shared
/// proxy, say) does not stop a signed-in owner stepping up.
#[tokio::test]
async fn a_locked_out_login_does_not_block_step_up() {
    let c = Collector::start().await;
    for _ in 0..5 {
        assert_eq!(c.login("wrong password").await.status(), 401);
    }
    assert_eq!(
        code_of(c.login(OWNER_PASSWORD).await).await,
        (429, "rate_limited".into())
    );
    let session = c.session(0);
    assert_eq!(c.step_up(&session, OWNER_PASSWORD).await.status(), 204);
}

/// Open `GET /api/stream/sessions/s-1` with `session`.
async fn open_stream(c: &Collector, session: &str) -> reqwest::Response {
    let resp = c
        .request(session, "GET", "/api/stream/sessions/s-1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp
}

/// Whether the SSE body of `stream` ends within `within`.
async fn ends(stream: reqwest::Response, within: Duration) -> bool {
    use futures::StreamExt;
    let mut body = stream.bytes_stream();
    tokio::time::timeout(within, async { while let Some(Ok(_)) = body.next().await {} })
        .await
        .is_ok()
}

/// 3b decision 7: a stream a session opened ends when that session does,
/// by a revoke or a logout; another session's stream stays open.
#[tokio::test]
async fn ending_a_session_ends_its_open_streams() {
    let c = Collector::start().await;
    let owner = c.session(0);
    let kept = open_stream(&c, &owner).await;

    let revoked = c.session(0);
    let stream = open_stream(&c, &revoked).await;
    let resp = c
        .request(&owner, "DELETE", &format!("/api/auth/sessions/{}", c.id_of(&revoked)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    assert!(
        ends(stream, Duration::from_secs(1)).await,
        "a revoked session's stream stayed open"
    );

    let signed_out = c.session(0);
    let stream = open_stream(&c, &signed_out).await;
    let resp = c.request(&signed_out, "POST", "/api/auth/logout").send().await.unwrap();
    assert_eq!(resp.status(), 204);
    assert!(
        ends(stream, Duration::from_secs(1)).await,
        "a signed-out session's stream stayed open"
    );

    // Control: the owner's own stream is still open.
    assert!(
        !ends(kept, Duration::from_millis(200)).await,
        "an unrelated stream ended"
    );
}

/// The admin socket's resets (kernel spec §4.2) end every session, and so
/// every stream a session holds open: a password reset, then a
/// `public_url` reset.
#[tokio::test]
async fn a_reset_ends_every_session_and_its_streams() {
    let c = Collector::start().await;
    let streams = [
        open_stream(&c, &c.session(0)).await,
        open_stream(&c, &c.session(0)).await,
    ];
    let reset = c
        .state
        .operator
        .reset_password("a new long password".into(), unix_now())
        .await
        .unwrap();
    assert_eq!(
        reset,
        hennery_kernel::operator::Reset::Done {
            sessions_ended: 3,
            passkeys_removed: 0
        }
    );
    for stream in streams {
        assert!(
            ends(stream, Duration::from_secs(1)).await,
            "a stream outlived the password reset"
        );
    }

    // A session opened on the new password.
    let phc = c
        .state
        .operator
        .verify_password("a new long password")
        .unwrap()
        .unwrap();
    let session = c
        .state
        .operator
        .open_session("test", &phc, unix_now())
        .unwrap()
        .unwrap();
    let stream = open_stream(&c, &session).await;
    let reset = c.state.operator.reset_public_url(PUBLIC_URL).unwrap();
    assert_eq!(
        reset,
        hennery_kernel::operator::Reset::Done {
            sessions_ended: 1,
            passkeys_removed: 0
        }
    );
    assert!(
        ends(stream, Duration::from_secs(1)).await,
        "a stream outlived the public_url reset"
    );
}

/// A logged request must never show the password.
#[test]
fn a_step_up_request_does_not_show_its_password_in_debug() {
    let shown = format!(
        "{:?}",
        StepUpRequest {
            password: OWNER_PASSWORD.into()
        }
    );
    assert!(!shown.contains(OWNER_PASSWORD), "{shown}");
}

#[tokio::test]
async fn the_session_list_marks_the_current_one_and_revoking_it_signs_out() {
    let c = Collector::start().await;
    let mine = c.session(0);
    let listed: Vec<AuthSessionItem> = c
        .request(&mine, "GET", "/api/auth/sessions")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // `operator_client`'s session and this one.
    assert_eq!(listed.len(), 2);
    let current: Vec<_> = listed.iter().filter(|s| s.current).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, c.id_of(&mine));
    assert_eq!(current[0].user_agent, "test");
    assert!(!listed.iter().any(|s| s.id == mine), "a token was listed");

    let unknown = c
        .request(&mine, "DELETE", "/api/auth/sessions/0000")
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(unknown).await, (404, "not_found".into()));
    let own = c
        .request(&mine, "DELETE", &format!("/api/auth/sessions/{}", current[0].id))
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 204);
    let cookie = own.headers()["set-cookie"].to_str().unwrap();
    assert!(
        cookie.starts_with("hennery_session=;") && cookie.contains("Max-Age=0"),
        "{cookie}"
    );
    let after = c.request(&mine, "GET", "/api/auth/sessions").send().await.unwrap();
    assert_eq!(code_of(after).await, (401, "unauthenticated".into()));
}

/// A request that slides its session's expiry and revokes that same
/// session gets one `Set-Cookie`, the clearing one: the session layer must
/// not send the dead token again after it, or the browser keeps that.
#[tokio::test]
async fn revoking_the_own_session_on_a_sliding_request_sends_only_the_clearing_cookie() {
    let c = Collector::start().await;
    // Last seen two minutes ago (so this request slides) and stepped up
    // then (so it is still fresh). The id is computed, not looked up:
    // `id_of` authenticates, which would slide the session already.
    let mine = c.session(120);
    let id = hennery_kernel::secret::sha256_hex(mine.as_bytes());
    let resp = c
        .request(&mine, "DELETE", &format!("/api/auth/sessions/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    let cookies: Vec<_> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    assert!(
        cookies[0].starts_with("hennery_session=;") && cookies[0].contains("Max-Age=0"),
        "{cookies:?}"
    );
}
