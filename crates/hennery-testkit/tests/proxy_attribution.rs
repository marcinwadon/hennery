//! Dedicated proxy header changes abuse accounting only, across every limiter.
use axum::{Extension, middleware};
use hennery_kernel::client_address::{ProxyPolicy, resolve_request};
use hennery_kernel::passkeys::MAX_LOGIN_CEREMONIES_PER_CLIENT;
use hennery_kernel::{hosts::Hosts, operator::Operator, ratelimit::Policy, secret::unix_now};
use hennery_sessions::{AppState, store::Store};
use hennery_testkit::{OWNER_PASSWORD, PUBLIC_URL};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Instant;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

struct Collector {
    addr: SocketAddr,
    state: AppState,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Collector {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Collector {
    async fn start(trusted: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open_in_memory().unwrap(),
            Hosts::open_in_memory().unwrap(),
            Operator::open_in_memory().unwrap(),
        );
        hennery_testkit::operator_client(&state.operator);
        let policy = ProxyPolicy::new(if trusted {
            vec!["127.0.0.1".parse().unwrap()]
        } else {
            vec![]
        })
        .unwrap();
        let router = hennery_sessions::router(state.clone())
            .layer(middleware::from_fn(resolve_request))
            .layer(Extension(policy));
        let task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .unwrap();
        });
        Self { addr, state, task }
    }
    fn post(&self, ip: Option<&str>, path: &str) -> reqwest::RequestBuilder {
        let req = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("http://{}{path}", self.addr))
            .header("origin", PUBLIC_URL);
        ip.map_or(req.try_clone().unwrap(), |ip| req.header("hennery-client-ip", ip))
    }
    fn session(&self) -> String {
        self.state
            .operator
            .open_session(
                "test",
                &hennery_testkit::owner_phc(&self.state.operator),
                unix_now() - 600,
            )
            .unwrap()
            .unwrap()
    }
    async fn password(&self, ip: &str, path: &str, cookie: &str, password: &str) -> u16 {
        self.post(Some(ip), path)
            .header("cookie", format!("__Host-hennery_session={cookie}"))
            .json(&json!({"password":password}))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }
    /// Begin a passkey login from `ip`: the status and the answer.
    async fn passkey_start(&self, ip: &str) -> (u16, Value) {
        let resp = self
            .post(Some(ip), "/api/auth/passkeys/login/start")
            .send()
            .await
            .unwrap();
        (resp.status().as_u16(), resp.json().await.unwrap())
    }
    /// Begin a passkey login from `ip`, counted, and give it up with an
    /// answer that does not verify: its place is free again.
    async fn abandoned_passkey_start(&self, ip: &str) {
        let (status, start) = self.passkey_start(ip).await;
        assert_eq!(status, 200);
        let refused = self
            .post(Some(ip), "/api/auth/passkeys/login/finish")
            .json(&json!({"ceremony_id": start["ceremony_id"], "credential": {}}))
            .send()
            .await
            .unwrap();
        assert!(refused.status().is_client_error(), "{}", refused.status());
    }
    async fn enroll(&self, ip: &str, code: &str, byte: u8) -> u16 {
        self.post(Some(ip),"/api/hosts/enroll").json(&json!({"code":code,"public_key":hennery_host::identity::HostKey::from_seed([byte;32]).public_key_hex(),"name":"fixture","host_version":"0","platform":"test"})).send().await.unwrap().status().as_u16()
    }
}
const A: &str = "192.0.2.1";
const B: &str = "192.0.2.2";

#[tokio::test]
async fn login_and_password_step_up_charge_and_clear_only_the_effective_client() {
    for path in ["/api/auth/login", "/api/auth/step-up/password"] {
        let c = Collector::start(true).await;
        let session = c.session();
        for _ in 0..Policy::LOGIN.free_failures {
            assert_eq!(c.password(A, path, &session, "wrong").await, 401);
        }
        assert_eq!(c.password(A, path, &session, OWNER_PASSWORD).await, 429);
        for _ in 0..Policy::LOGIN.free_failures - 1 {
            assert_eq!(c.password(B, path, &session, "wrong").await, 401);
        }
        assert_eq!(c.password(B, path, &session, OWNER_PASSWORD).await, 204);
        for _ in 0..Policy::LOGIN.free_failures {
            assert_eq!(c.password(B, path, &session, "wrong").await, 401);
        }
        assert_eq!(c.password(B, path, &session, OWNER_PASSWORD).await, 429);
        assert_eq!(c.password(A, path, &session, OWNER_PASSWORD).await, 429);
        assert_eq!(c.password("2001:db8:1::1", path, &session, OWNER_PASSWORD).await, 204);
    }
}

#[tokio::test]
async fn enrollment_separates_clients_and_success_clears_only_its_own_counter() {
    let c = Collector::start(true).await;
    for _ in 0..Policy::ENROLL.free_failures {
        assert_eq!(c.enroll(A, "wrong", 1).await, 401);
    }
    assert_eq!(c.enroll(A, "wrong", 1).await, 429);
    for _ in 0..Policy::ENROLL.free_failures - 1 {
        assert_eq!(c.enroll(B, "wrong", 2).await, 401);
    }
    let code = c.state.hosts.mint_pairing_code(unix_now()).unwrap().code;
    // Use a valid Ed25519 key, not arbitrary bytes.
    let key = hennery_host::identity::HostKey::generate();
    let response = c.post(Some(B),"/api/hosts/enroll").json(&json!({"code":code,"public_key":key.public_key_hex(),"name":"fixture","host_version":"0","platform":"test"})).send().await.unwrap();
    assert_eq!(response.status(), 201);
    for _ in 0..Policy::ENROLL.free_failures {
        assert_eq!(c.enroll(B, "wrong", 2).await, 401);
    }
    assert_eq!(c.enroll(B, "wrong", 2).await, 429);
    assert_eq!(c.enroll(A, "wrong", 1).await, 429);
}

#[tokio::test]
async fn malformed_trusted_headers_fail_before_body_auth_or_limiter_work_without_reflection() {
    let c = Collector::start(true).await;
    for path in [
        "/api/hosts/enroll",
        "/api/auth/login",
        "/api/auth/step-up/password",
        "/api/auth/passkeys/login/start",
        "/api/auth/passkeys/login/finish",
    ] {
        let response = c
            .post(Some("private-canary:443"), path)
            .body("invalid json")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{path}");
        assert!(!response.text().await.unwrap().contains("private-canary"));
    }
    let duplicate = c
        .post(Some(A), "/api/auth/login")
        .header("hennery-client-ip", B)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 400);
    for limiter in [
        &c.state.enroll_limiter,
        &c.state.operator.login_limiter,
        &c.state.operator.step_up_limiter,
        &c.state.operator.passkey_limiter,
    ] {
        assert_eq!(limiter.attempt("127.0.0.1".parse().unwrap(), Instant::now()), Ok(()));
        limiter.succeeded("127.0.0.1".parse().unwrap());
    }
}

#[tokio::test]
async fn untrusted_spoof_and_missing_header_use_the_tcp_budget() {
    for trusted in [false, true] {
        let c = Collector::start(trusted).await;
        for _ in 0..Policy::LOGIN.free_failures {
            let response = c
                .post(if trusted { None } else { Some("bad, spoof") }, "/api/auth/login")
                .header("forwarded", "for=192.0.2.1")
                .header("x-forwarded-for", B)
                .json(&json!({"password":"wrong"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 401);
        }
        assert_eq!(
            c.post(None, "/api/auth/login")
                .json(&json!({"password":OWNER_PASSWORD}))
                .send()
                .await
                .unwrap()
                .status(),
            429
        );
    }
}

#[tokio::test]
async fn forwarded_ipv6_grouping_and_mapped_ipv4_keep_the_existing_budgets() {
    let c = Collector::start(true).await;
    for _ in 0..Policy::ENROLL.free_failures {
        assert_eq!(c.enroll("2001:db8:1::1", "wrong", 1).await, 401);
    }
    assert_eq!(c.enroll("2001:db8:1::ffff", "wrong", 1).await, 429);
    assert_eq!(c.enroll("2001:db8:2::1", "wrong", 1).await, 401);
    for _ in 0..Policy::ENROLL.free_failures {
        assert_eq!(c.enroll(A, "wrong", 1).await, 401);
    }
    assert_eq!(c.enroll("::ffff:192.0.2.1", "wrong", 1).await, 429);
}

/// A collector behind the trusted proxy with a passkey registered, and
/// its authenticator.
async fn with_passkey() -> (Collector, WebauthnAuthenticator<SoftPasskey>) {
    let c = Collector::start(true).await;
    let mut passkey = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let session = c.session();
    // Registration needs fresh step-up.
    c.state
        .operator
        .step_up(
            &c.state
                .operator
                .authenticate(&session, unix_now())
                .unwrap()
                .unwrap()
                .session_id,
            unix_now(),
        )
        .unwrap();
    let start: Value = c
        .post(Some(B), "/api/auth/passkeys/register/start")
        .header("cookie", format!("__Host-hennery_session={session}"))
        .json(&json!({"label":"fixture"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let options: CreationChallengeResponse = serde_json::from_value(start["options"].clone()).unwrap();
    let credential = passkey
        .do_registration(Url::parse(PUBLIC_URL).unwrap(), options)
        .unwrap();
    assert_eq!(
        c.post(Some(B), "/api/auth/passkeys/register/finish")
            .header("cookie", format!("__Host-hennery_session={session}"))
            .json(&json!({"ceremony_id":start["ceremony_id"],"credential":credential}))
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
    (c, passkey)
}

#[tokio::test]
async fn passkey_start_and_success_use_the_effective_client() {
    let (c, mut passkey) = with_passkey().await;
    // Each start counts; giving its ceremony up keeps the key under its
    // cap of live logins, so the budget is what runs out.
    for _ in 0..Policy::PASSKEY_LOGIN.free_failures {
        c.abandoned_passkey_start(A).await;
    }
    assert_eq!(
        c.post(Some(A), "/api/auth/passkeys/login/start")
            .send()
            .await
            .unwrap()
            .status(),
        429
    );
    // B's starts are its own budget.
    for _ in 0..Policy::PASSKEY_LOGIN.free_failures - 2 {
        c.abandoned_passkey_start(B).await;
    }
    let (status, start) = c.passkey_start(B).await;
    assert_eq!(status, 200);
    let options: RequestChallengeResponse = serde_json::from_value(start["options"].clone()).unwrap();
    let credential = passkey
        .do_authentication(Url::parse(PUBLIC_URL).unwrap(), options)
        .unwrap();
    assert_eq!(
        c.post(Some(B), "/api/auth/passkeys/login/finish")
            .json(&json!({"ceremony_id":start["ceremony_id"],"credential":credential}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    for _ in 0..Policy::PASSKEY_LOGIN.free_failures {
        c.abandoned_passkey_start(B).await;
    }
    assert_eq!(
        c.post(Some(B), "/api/auth/passkeys/login/start")
            .send()
            .await
            .unwrap()
            .status(),
        429
    );
    assert_eq!(
        c.post(Some(A), "/api/auth/passkeys/login/start")
            .send()
            .await
            .unwrap()
            .status(),
        429
    );
}

/// Browsers behind one NAT share a rate-limit key (here, two IPv6
/// addresses of one /64). Another browser there starting logins past the
/// key's cap of live ceremonies is refused 503 `auth_busy`, uncounted, and
/// never pushes out the owner's challenge, which still signs in.
#[tokio::test]
async fn a_neighbour_behind_the_same_key_cannot_end_the_owners_passkey_login() {
    let (c, mut passkey) = with_passkey().await;
    const OWNER: &str = "2001:db8:5:1::10";
    const NEIGHBOUR: &str = "2001:db8:5:1::20";
    let (status, start) = c.passkey_start(OWNER).await;
    assert_eq!(status, 200);
    let mut begun = 1;
    let mut busy = 0;
    for _ in 0..MAX_LOGIN_CEREMONIES_PER_CLIENT + 8 {
        let (status, answer) = c.passkey_start(NEIGHBOUR).await;
        match status {
            200 => begun += 1,
            503 => {
                assert_eq!(answer["code"], "auth_busy");
                busy += 1;
            }
            other => panic!("a start answered {other}"),
        }
    }
    assert_eq!((begun, busy), (MAX_LOGIN_CEREMONIES_PER_CLIENT, 9));
    // Only the begun starts were counted against the shared key's budget.
    let key: std::net::IpAddr = NEIGHBOUR.parse().unwrap();
    for _ in 0..Policy::PASSKEY_LOGIN.free_failures - begun as u32 {
        assert!(c.state.operator.passkey_limiter.attempt(key, Instant::now()).is_ok());
    }
    assert!(c.state.operator.passkey_limiter.attempt(key, Instant::now()).is_err());
    let options: RequestChallengeResponse = serde_json::from_value(start["options"].clone()).unwrap();
    let credential = passkey
        .do_authentication(Url::parse(PUBLIC_URL).unwrap(), options)
        .unwrap();
    assert_eq!(
        c.post(Some(OWNER), "/api/auth/passkeys/login/finish")
            .json(&json!({"ceremony_id":start["ceremony_id"],"credential":credential}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
}
