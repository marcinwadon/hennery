//! Passkeys over HTTP (kernel spec §3.2–§3.4, §8, §11; plan 3c): register,
//! sign in, step up and remove, with a software passkey in place of the
//! browser (decision 10), behind the browser rules and the cookie.

use hennery_kernel::auth_api::MAX_BODY_BYTES;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::ratelimit::Policy;
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{
    ApiError, PasskeyCeremony, PasskeyFinishRequest, PasskeyItem, PasskeyRegisterRequest, SetupRequest, StepUpRequest,
};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::{OWNER_PASSWORD, PUBLIC_URL};
use std::net::SocketAddr;
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

type Passkey = WebauthnAuthenticator<SoftPasskey>;

fn authenticator() -> Passkey {
    WebauthnAuthenticator::new(SoftPasskey::new(true))
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
}

impl Collector {
    /// A collector, not set up.
    async fn bare() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open_in_memory().unwrap(),
            Hosts::open_in_memory().unwrap(),
            Operator::open_in_memory().unwrap(),
        );
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state }
    }

    /// A collector set up at `PUBLIC_URL`.
    async fn start() -> Self {
        let c = Self::bare().await;
        hennery_testkit::operator_client(&c.state.operator);
        c
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

    /// A `POST` from `PUBLIC_URL`, with `session`'s cookie if any.
    fn post(&self, session: Option<&str>, path: &str) -> reqwest::RequestBuilder {
        self.request(session, "POST", path)
    }

    fn request(&self, session: Option<&str>, method: &str, path: &str) -> reqwest::RequestBuilder {
        let req = reqwest::Client::new()
            .request(method.parse().unwrap(), format!("http://{}{path}", self.addr))
            .header("origin", PUBLIC_URL);
        match session {
            Some(session) => req.header("cookie", format!("hennery_session={session}")),
            None => req,
        }
    }

    /// Start the ceremony at `path`: its id and options, or the error.
    async fn start_ceremony(
        &self,
        session: Option<&str>,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<PasskeyCeremony, (u16, String)> {
        let mut req = self.post(session, path);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.unwrap();
        if resp.status() == 200 {
            Ok(resp.json().await.unwrap())
        } else {
            Err(code_of(resp).await)
        }
    }

    async fn finish(
        &self,
        session: Option<&str>,
        path: &str,
        ceremony_id: &str,
        credential: serde_json::Value,
    ) -> reqwest::Response {
        self.post(session, path)
            .json(&PasskeyFinishRequest {
                ceremony_id: ceremony_id.into(),
                credential,
            })
            .send()
            .await
            .unwrap()
    }

    /// Register a passkey labelled `label` from `session` (stepped up).
    async fn register(&self, session: &str, passkey: &mut Passkey, label: &str) -> PasskeyItem {
        let ceremony = self
            .start_ceremony(
                Some(session),
                "/api/auth/passkeys/register/start",
                Some(serde_json::to_value(PasskeyRegisterRequest { label: label.into() }).unwrap()),
            )
            .await
            .expect("the registration began");
        let options: CreationChallengeResponse = serde_json::from_value(ceremony.options).unwrap();
        let credential = passkey
            .do_registration(Url::parse(PUBLIC_URL).unwrap(), options)
            .unwrap();
        let resp = self
            .finish(
                Some(session),
                "/api/auth/passkeys/register/finish",
                &ceremony.ceremony_id,
                serde_json::to_value(credential).unwrap(),
            )
            .await;
        assert_eq!(resp.status(), 201);
        resp.json().await.unwrap()
    }

    /// Begin a login, and the passkey's answer made at `origin`.
    async fn begin_login(&self, passkey: &mut Passkey, origin: &str) -> (String, serde_json::Value) {
        let ceremony = self
            .start_ceremony(None, "/api/auth/passkeys/login/start", None)
            .await
            .expect("the login began");
        (ceremony.ceremony_id, answer(passkey, origin, ceremony.options))
    }

    /// Sign in with `passkey`: the response to the finish.
    async fn passkey_login(&self, passkey: &mut Passkey) -> reqwest::Response {
        let (ceremony_id, assertion) = self.begin_login(passkey, PUBLIC_URL).await;
        self.finish(None, "/api/auth/passkeys/login/finish", &ceremony_id, assertion)
            .await
    }

    /// Step `session` up with `passkey`: the response to the finish.
    async fn passkey_step_up(&self, session: &str, passkey: &mut Passkey) -> reqwest::Response {
        let ceremony = self
            .start_ceremony(Some(session), "/api/auth/step-up/passkey/start", None)
            .await
            .expect("the step-up began");
        let assertion = answer(passkey, PUBLIC_URL, ceremony.options);
        self.finish(
            Some(session),
            "/api/auth/step-up/passkey/finish",
            &ceremony.ceremony_id,
            assertion,
        )
        .await
    }

    async fn password_login(&self, password: &str) -> reqwest::Response {
        self.post(None, "/api/auth/login")
            .json(&serde_json::json!({ "password": password }))
            .send()
            .await
            .unwrap()
    }

    async fn list(&self, session: &str) -> reqwest::Response {
        self.request(Some(session), "GET", "/api/auth/passkeys")
            .send()
            .await
            .unwrap()
    }
}

/// The passkey's answer to a login or step-up's `options`, made at
/// `origin`.
fn answer(passkey: &mut Passkey, origin: &str, options: serde_json::Value) -> serde_json::Value {
    let options: RequestChallengeResponse = serde_json::from_value(options).unwrap();
    let assertion = passkey.do_authentication(Url::parse(origin).unwrap(), options).unwrap();
    serde_json::to_value(assertion).unwrap()
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (
        status,
        resp.json::<ApiError>().await.map(|e| e.code).unwrap_or_default(),
    )
}

/// The session token a response's `Set-Cookie` carries.
fn session_token(resp: &reqwest::Response) -> String {
    let cookie = resp.headers()["set-cookie"].to_str().unwrap();
    cookie
        .strip_prefix("hennery_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// A logged finish shows which ceremony it names, never the credential.
#[test]
fn a_passkey_finish_does_not_show_its_credential_in_debug() {
    let shown = format!(
        "{:?}",
        PasskeyFinishRequest {
            ceremony_id: "ceremony-0123".into(),
            credential: serde_json::json!({ "response": { "signature": "sig-sentinel-4567" } }),
        }
    );
    assert!(!shown.contains("sig-sentinel-4567"), "{shown}");
    assert!(shown.contains("ceremony-0123"), "{shown}");
}

/// Kernel spec §3.1: setup "then offers passkey registration". The session
/// setup opens is stepped up, so it registers a passkey at once; the
/// passkey then signs in, steps a stale session up, and is removed.
#[tokio::test]
async fn a_passkey_registered_at_setup_signs_in_steps_up_and_is_removed() {
    let c = Collector::bare().await;
    let token = c.state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
    let resp = c
        .post(None, "/api/setup")
        .json(&SetupRequest {
            token,
            password: OWNER_PASSWORD.into(),
            public_url: PUBLIC_URL.into(),
            default_hat_name: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let setup_session = session_token(&resp);
    let mut passkey = authenticator();
    let item = c.register(&setup_session, &mut passkey, "laptop").await;
    assert_eq!(item.label, "laptop");
    assert_eq!(item.last_used_at, None);
    let listed: Vec<PasskeyItem> = c.list(&setup_session).await.json().await.unwrap();
    assert_eq!(listed, vec![item.clone()]);

    let resp = c.passkey_login(&mut passkey).await;
    assert_eq!(resp.status(), 204);
    let session = session_token(&resp);
    assert!(
        resp.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .ends_with("; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000; Secure")
    );
    let listed: Vec<PasskeyItem> = c.list(&session).await.json().await.unwrap();
    assert!(listed[0].last_used_at.is_some());

    let stale = c.session(5 * 60);
    let remove = |session: String| {
        c.request(Some(&session), "DELETE", &format!("/api/auth/passkeys/{}", item.id))
            .send()
    };
    assert_eq!(
        code_of(remove(stale.clone()).await.unwrap()).await,
        (403, "step_up_required".into())
    );
    assert_eq!(c.passkey_step_up(&stale, &mut passkey).await.status(), 204);
    assert_eq!(remove(stale.clone()).await.unwrap().status(), 204);
    assert_eq!(code_of(remove(stale).await.unwrap()).await, (404, "not_found".into()));
    assert!(c.state.operator.passkeys().unwrap().is_empty());
}

/// Decision 7, kernel spec §3.4: registering and removing a passkey need a
/// password or passkey check within five minutes; listing does not.
#[tokio::test]
async fn registering_and_removing_a_passkey_need_a_fresh_check() {
    let c = Collector::start().await;
    let fresh = c.session(0);
    let mut passkey = authenticator();
    let item = c.register(&fresh, &mut passkey, "laptop").await;
    let ceremony = c
        .start_ceremony(
            Some(&fresh),
            "/api/auth/passkeys/register/start",
            Some(serde_json::json!({ "label": "phone" })),
        )
        .await
        .unwrap();

    let stale = c.session(5 * 60);
    assert_eq!(
        c.start_ceremony(
            Some(&stale),
            "/api/auth/passkeys/register/start",
            Some(serde_json::json!({ "label": "phone" })),
        )
        .await
        .unwrap_err(),
        (403, "step_up_required".into())
    );
    assert_eq!(
        code_of(
            c.finish(
                Some(&stale),
                "/api/auth/passkeys/register/finish",
                &ceremony.ceremony_id,
                serde_json::json!({}),
            )
            .await
        )
        .await,
        (403, "step_up_required".into())
    );
    assert_eq!(
        code_of(
            c.request(Some(&stale), "DELETE", &format!("/api/auth/passkeys/{}", item.id))
                .send()
                .await
                .unwrap()
        )
        .await,
        (403, "step_up_required".into())
    );
    assert_eq!(c.list(&stale).await.status(), 200);
    assert_eq!(c.state.operator.passkeys().unwrap().len(), 1);
    // A password step-up is enough.
    let resp = c
        .post(Some(&stale), "/api/auth/step-up/password")
        .json(&StepUpRequest {
            password: OWNER_PASSWORD.into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    c.register(&stale, &mut authenticator(), "phone").await;
}

/// Kernel spec §3.3: the passkey routes are browser routes. Login needs
/// the `public_url`'s `Origin` and JSON; the rest need the cookie too.
#[tokio::test]
async fn passkey_routes_follow_the_browser_rules() {
    let c = Collector::bare().await;
    assert_eq!(
        c.start_ceremony(None, "/api/auth/passkeys/login/start", None)
            .await
            .unwrap_err(),
        (403, "setup_required".into())
    );
    hennery_testkit::operator_client(&c.state.operator);
    let session = c.session(0);
    let item = c.register(&session, &mut authenticator(), "laptop").await;
    let url = |path: &str| format!("http://{}{path}", c.addr);
    let login_start = url("/api/auth/passkeys/login/start");

    let resp = reqwest::Client::new().post(&login_start).send().await.unwrap();
    assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()));
    // 3c review, O3: the login's finish, and a removal with a fresh
    // session, without `Origin`.
    let resp = reqwest::Client::new()
        .post(url("/api/auth/passkeys/login/finish"))
        .json(&serde_json::json!({ "ceremony_id": "x", "credential": {} }))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()));
    let resp = reqwest::Client::new()
        .delete(url(&format!("/api/auth/passkeys/{}", item.id)))
        .header("cookie", format!("hennery_session={session}"))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()));
    assert_eq!(c.state.operator.passkeys().unwrap().len(), 1);
    let resp = reqwest::Client::new()
        .post(&login_start)
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()));
    let resp = c
        .post(None, "/api/auth/passkeys/login/start")
        .header("content-type", "text/plain")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (415, "unsupported_media_type".into()));
    let resp = c
        .post(None, "/api/auth/passkeys/login/finish")
        .header("content-type", "text/plain")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (415, "unsupported_media_type".into()));

    for path in [
        "/api/auth/passkeys/register/start",
        "/api/auth/passkeys/register/finish",
        "/api/auth/step-up/passkey/start",
        "/api/auth/step-up/passkey/finish",
    ] {
        let resp = c.post(None, path).json(&serde_json::json!({})).send().await.unwrap();
        assert_eq!(code_of(resp).await, (401, "unauthenticated".into()), "{path}");
        let resp = reqwest::Client::new()
            .post(url(path))
            .header("cookie", format!("hennery_session={session}"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()), "{path}");
    }
    let resp = c.request(None, "GET", "/api/auth/passkeys").send().await.unwrap();
    assert_eq!(code_of(resp).await, (401, "unauthenticated".into()));
    let resp = c
        .request(Some(&session), "GET", "/api/auth/passkeys")
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (403, "cross_site".into()));
}

/// Decision 8, 3b-i's obligation: passkey login has a budget of its own. A
/// password lockout does not stop it; its own starts are limited; and a
/// passkey login takes nothing from the password's budget.
#[tokio::test]
async fn passkey_login_has_a_budget_of_its_own() {
    let c = Collector::start().await;
    let mut passkey = authenticator();
    c.register(&c.session(0), &mut passkey, "laptop").await;
    for _ in 0..5 {
        assert_eq!(c.password_login("wrong password").await.status(), 401);
    }
    assert_eq!(
        code_of(c.password_login(OWNER_PASSWORD).await).await,
        (429, "rate_limited".into())
    );
    assert_eq!(c.passkey_login(&mut passkey).await.status(), 204);

    let c = Collector::start().await;
    c.register(&c.session(0), &mut passkey, "laptop").await;
    for _ in 0..Policy::PASSKEY_LOGIN.free_failures {
        c.start_ceremony(None, "/api/auth/passkeys/login/start", None)
            .await
            .unwrap();
    }
    assert_eq!(
        c.start_ceremony(None, "/api/auth/passkeys/login/start", None)
            .await
            .unwrap_err(),
        (429, "rate_limited".into())
    );
    assert_eq!(c.password_login(OWNER_PASSWORD).await.status(), 204);
}

/// A successful passkey login gives its address its budget back.
#[tokio::test]
async fn a_passkey_login_clears_its_addresss_count() {
    let c = Collector::start().await;
    let mut passkey = authenticator();
    c.register(&c.session(0), &mut passkey, "laptop").await;
    for _ in 0..Policy::PASSKEY_LOGIN.free_failures - 1 {
        c.start_ceremony(None, "/api/auth/passkeys/login/start", None)
            .await
            .unwrap();
    }
    assert_eq!(c.passkey_login(&mut passkey).await.status(), 204);
    for _ in 0..Policy::PASSKEY_LOGIN.free_failures {
        c.start_ceremony(None, "/api/auth/passkeys/login/start", None)
            .await
            .unwrap();
    }
}

/// A passkey steps up while the password's step-up budget is locked out.
#[tokio::test]
async fn a_passkey_steps_up_while_password_step_up_is_locked_out() {
    let c = Collector::start().await;
    let mut passkey = authenticator();
    c.register(&c.session(0), &mut passkey, "laptop").await;
    let stale = c.session(5 * 60);
    for _ in 0..5 {
        let resp = c
            .post(Some(&stale), "/api/auth/step-up/password")
            .json(&StepUpRequest {
                password: "wrong password".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }
    assert_eq!(c.passkey_step_up(&stale, &mut passkey).await.status(), 204);
    assert!(
        c.state
            .operator
            .authenticate(&stale, unix_now())
            .unwrap()
            .unwrap()
            .stepped_up(unix_now())
    );
}

/// Decision 11: what a refused ceremony answers.
#[tokio::test]
async fn refused_ceremonies_answer_what_went_wrong() {
    let c = Collector::start().await;
    let session = c.session(0);
    assert_eq!(
        c.start_ceremony(None, "/api/auth/passkeys/login/start", None)
            .await
            .unwrap_err(),
        (409, "no_passkeys".into())
    );
    assert_eq!(
        c.start_ceremony(
            Some(&session),
            "/api/auth/passkeys/register/start",
            Some(serde_json::json!({ "label": "" })),
        )
        .await
        .unwrap_err(),
        (400, "invalid".into())
    );
    let mut passkey = authenticator();
    c.register(&session, &mut passkey, "laptop").await;

    // A finish used twice.
    let (ceremony_id, assertion) = c.begin_login(&mut passkey, PUBLIC_URL).await;
    let finish = || c.finish(None, "/api/auth/passkeys/login/finish", &ceremony_id, assertion.clone());
    assert_eq!(finish().await.status(), 204);
    assert_eq!(code_of(finish().await).await, (400, "invalid_ceremony".into()));
    // An answer made at another origin.
    let (ceremony_id, assertion) = c.begin_login(&mut passkey, "https://sub.hennery.example").await;
    let resp = c
        .finish(None, "/api/auth/passkeys/login/finish", &ceremony_id, assertion)
        .await;
    assert_eq!(code_of(resp).await, (401, "passkey_refused".into()));
    // A body that is not a finish.
    let resp = c
        .post(None, "/api/auth/passkeys/login/finish")
        .json(&serde_json::json!({ "ceremony_id": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);
}

/// The auth routes' body limit holds for passkeys too: a finish larger
/// than `MAX_BODY_BYTES` is refused with 413 before it is parsed.
#[tokio::test]
async fn a_finish_larger_than_the_body_limit_is_refused() {
    let c = Collector::start().await;
    let resp = c
        .finish(
            None,
            "/api/auth/passkeys/login/finish",
            "x",
            serde_json::json!({ "padding": "x".repeat(MAX_BODY_BYTES) }),
        )
        .await;
    assert_eq!(resp.status(), 413);
    let session = c.session(0);
    let resp = c
        .finish(
            Some(&session),
            "/api/auth/passkeys/register/finish",
            "x",
            serde_json::json!({ "padding": "x".repeat(MAX_BODY_BYTES) }),
        )
        .await;
    assert_eq!(resp.status(), 413);
}

/// Decision 1: set up at an IP address, passkeys are unavailable, and say
/// so.
#[tokio::test]
async fn passkeys_are_unavailable_at_an_ip_address() {
    let c = Collector::bare().await;
    let public_url = format!("http://{}", c.addr);
    let now = unix_now();
    let token = c.state.operator.issue_setup_token(now).unwrap().unwrap();
    c.state
        .operator
        .set_up(&token, OWNER_PASSWORD, &public_url, now)
        .unwrap();
    let resp = reqwest::Client::new()
        .post(format!("{public_url}/api/auth/passkeys/login/start"))
        .header("origin", &public_url)
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (409, "passkeys_unavailable".into()));
}
