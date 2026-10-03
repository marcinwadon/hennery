//! `PATCH /api/settings {public_url}` (kernel spec §3.2, §3.4, §8; plan
//! 4d-B4): the API moves the collector exactly as `hennery admin
//! reset-public-url` does, through the same operator function, behind a
//! fresh step-up that only a body naming `public_url` needs.

use hennery_kernel::admin::{ADMIN_SOCKET, Admin, AdminRequest, AdminResponse};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::passkeys::Start;
use hennery_kernel::secret::{sha256_hex, unix_now};
use hennery_proto::rest::{ApiError, PublicUrlChanged, SettingsUpdateResponse};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::{OWNER_PASSWORD, PUBLIC_URL};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, Url};

/// A browser's subscription (web-push-native's example keys).
const SUBSCRIBE: &str = r#"{"endpoint":"https://fcm.googleapis.com/fcm/send/x","keys":{"p256dh":"BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY","auth":"_ordMnz7uTCmrpBTeUV4Bw"}}"#;

/// The kernel's tables a `public_url` change reads or writes.
const TABLES: &[&str] = &[
    "owners",
    "settings",
    "password_credentials",
    "auth_sessions",
    "push_subscriptions",
    "passkeys",
];

struct Collector {
    addr: SocketAddr,
    state: AppState,
    db: PathBuf,
}

impl Collector {
    /// A collector on `db`, set up at `PUBLIC_URL` with no session signed
    /// in, unless it was set up already (a copy).
    async fn on(db: &Path) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(db).unwrap(),
            Hosts::open(db).unwrap(),
            Operator::open(db).unwrap(),
        );
        if !state.operator.is_set_up().unwrap() {
            let token = state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
            state
                .operator
                .set_up(&token, OWNER_PASSWORD, PUBLIC_URL, unix_now())
                .unwrap();
            // The session `stream` follows: a stream is 404 for an unknown one.
            state
                .store
                .create_session("s-1", "host-9", "fake", "/tmp", "hat-9", None)
                .unwrap();
        }
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            addr,
            state,
            db: db.to_path_buf(),
        }
    }

    /// A session whose last password check was `age` seconds ago, last
    /// seen then too.
    fn session(&self, age: i64) -> String {
        let phc = hennery_testkit::owner_phc(&self.state.operator);
        self.state
            .operator
            .open_session("test", &phc, unix_now() - age)
            .unwrap()
            .unwrap()
    }

    fn request(&self, session: &str, origin: &str, method: &str, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method.parse().unwrap(), format!("http://{}{path}", self.addr))
            .header("origin", origin)
            .header("cookie", format!("hennery_session={session}"))
    }

    /// `PATCH /api/settings` with `body` as it is, from `origin`.
    async fn patch_from(&self, session: &str, origin: &str, body: &str) -> reqwest::Response {
        self.request(session, origin, "PATCH", "/api/settings")
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap()
    }

    async fn patch(&self, session: &str, body: &str) -> reqwest::Response {
        self.patch_from(session, PUBLIC_URL, body).await
    }

    /// Every row of the kernel's `TABLES`, as text.
    fn dump(&self) -> Vec<(String, Vec<String>)> {
        dump(&self.db)
    }

    /// Register a passkey at `PUBLIC_URL` from `session` (stepped up).
    fn register(&self, session: &str, label: &str) {
        let op = &self.state.operator;
        let id = sha256_hex(session.as_bytes());
        let Start::Begun { ceremony_id, options } = op.start_passkey_registration(&id, label, unix_now()).unwrap()
        else {
            panic!("the registration did not begin");
        };
        let options: CreationChallengeResponse = serde_json::from_value(options).unwrap();
        let credential = WebauthnAuthenticator::new(SoftPasskey::new(true))
            .do_registration(Url::parse(PUBLIC_URL).unwrap(), options)
            .unwrap();
        op.finish_passkey_registration(
            &id,
            &ceremony_id,
            &serde_json::to_value(credential).unwrap(),
            unix_now(),
        )
        .unwrap()
        .expect("registered");
    }

    /// Subscribe a browser to push from `session` (stepped up).
    async fn subscribe(&self, session: &str, endpoint: &str) {
        let body = SUBSCRIBE.replace("https://fcm.googleapis.com/fcm/send/x", endpoint);
        let resp = self
            .request(session, PUBLIC_URL, "POST", "/api/push/subscriptions")
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
    }

    /// Open the SSE stream of `s-1` with `session`.
    async fn stream(&self, session: &str) -> reqwest::Response {
        let resp = self
            .request(session, PUBLIC_URL, "GET", "/api/stream/sessions/s-1")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        resp
    }

    async fn login_from(&self, origin: &str) -> u16 {
        reqwest::Client::new()
            .post(format!("http://{}/api/auth/login", self.addr))
            .header("origin", origin)
            .json(&serde_json::json!({ "password": OWNER_PASSWORD }))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }
}

fn dump(db: &Path) -> Vec<(String, Vec<String>)> {
    let conn = rusqlite::Connection::open(db).unwrap();
    TABLES
        .iter()
        .map(|table| {
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |r| {
                    let values: Vec<String> = (0..columns)
                        .map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap()))
                        .collect();
                    Ok(values.join("|"))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (table.to_string(), rows)
        })
        .collect()
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (
        status,
        resp.json::<ApiError>().await.map(|e| e.code).unwrap_or_default(),
    )
}

/// Whether the SSE body of `stream` ends within `within`.
async fn ends(stream: reqwest::Response, within: Duration) -> bool {
    use futures::StreamExt;
    let mut body = stream.bytes_stream();
    tokio::time::timeout(within, async { while let Some(Ok(_)) = body.next().await {} })
        .await
        .is_ok()
}

fn set_cookies(resp: &reqwest::Response) -> Vec<String> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

/// Decision 2 (kernel spec §3.4, §6): a body naming `public_url` needs a
/// fresh step-up, checked before anything is read or written, so a stale
/// session's `contact` beside it is not stored either. A body without one
/// needs none, and neither does a `null` one, which changes nothing. The
/// same bytes decoded another way never give another verdict: an escaped
/// key is the key, and a duplicate, another case or a stray space is no
/// key at all.
#[tokio::test]
async fn naming_public_url_needs_a_fresh_step_up_and_a_contact_alone_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let c = Collector::on(&dir.path().join("hennery.db")).await;
    let stale = c.session(5 * 60);
    // Slid now, so no request below changes the session's row.
    let resp = c
        .request(&stale, PUBLIC_URL, "GET", "/api/settings")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let before = c.dump();
    let escaped = format!(r#"{{"public{}u005furl":"https://moved.example"}}"#, '\\');
    assert_eq!(escaped.as_bytes()[8], b'\\', "{escaped}");
    for (body, expected) in [
        (r#"{"public_url":"https://moved.example"}"#, (403, "step_up_required")),
        (
            r#"{"contact":"me@example.com","public_url":"https://moved.example"}"#,
            (403, "step_up_required"),
        ),
        (r#"{"public_url":"not a url"}"#, (403, "step_up_required")),
        (r#"{"public_url":""}"#, (403, "step_up_required")),
        // `public\u005furl`, built so no tool on the way can decode it.
        (escaped.as_str(), (403, "step_up_required")),
        (
            r#"{"public_url":null,"public_url":"https://moved.example"}"#,
            (422, "invalid_body"),
        ),
        (
            r#"{"public_url":"https://moved.example","public_url":"https://other.example"}"#,
            (422, "invalid_body"),
        ),
        (r#"{"Public_url":"https://moved.example"}"#, (422, "invalid_body")),
        (r#"{"public_url ":"https://moved.example"}"#, (422, "invalid_body")),
        (r#"{"public_url":7117}"#, (422, "invalid_body")),
        (
            "\u{feff}{\"public_url\":\"https://moved.example\"}",
            (400, "invalid_body"),
        ),
    ] {
        let resp = c.patch(&stale, body).await;
        assert_eq!(code_of(resp).await, (expected.0, expected.1.into()), "{body}");
        assert_eq!(c.dump(), before, "{body}");
    }
    assert!(c.state.operator.authenticate(&stale, unix_now()).unwrap().is_some());

    let resp = c.patch(&stale, r#"{"public_url":null}"#).await;
    assert_eq!(resp.status(), 200);
    assert!(set_cookies(&resp).is_empty());
    assert_eq!(c.dump(), before);
    let resp = c
        .patch(&stale, r#"{"public_url":null,"contact":"me@example.com"}"#)
        .await;
    assert_eq!(resp.status(), 200);
    assert!(set_cookies(&resp).is_empty());
    assert_eq!(c.state.operator.contact().unwrap().as_deref(), Some("me@example.com"));
    let resp = c.patch(&stale, r#"{"contact":"you@example.com"}"#).await;
    assert_eq!(resp.status(), 200);
    assert!(set_cookies(&resp).is_empty());
    let settings: SettingsUpdateResponse = resp.json().await.unwrap();
    assert_eq!(
        settings,
        SettingsUpdateResponse {
            public_url: PUBLIC_URL.into(),
            contact: Some("you@example.com".into()),
            public_url_changed: None,
        }
    );
    assert!(c.state.operator.authenticate(&stale, unix_now()).unwrap().is_some());
}

/// Decisions 1, 3 and 4: a change ends every signed-in session, the
/// caller's with the others, and so their streams and push subscriptions,
/// and every passkey ceremony under way. The answer reports it as the
/// admin socket does, and clears the cookie once, on a request that slid
/// its session too. The old origin is refused from then on; the new one
/// signs in.
#[tokio::test]
async fn a_change_ends_every_session_stream_subscription_and_ceremony() {
    let dir = tempfile::tempdir().unwrap();
    let c = Collector::on(&dir.path().join("hennery.db")).await;
    // Last seen two minutes ago, so this request slides it.
    let mine = c.session(120);
    let other = c.session(0);
    c.subscribe(&mine, "https://fcm.googleapis.com/fcm/send/mine").await;
    c.subscribe(&other, "https://fcm.googleapis.com/fcm/send/other").await;
    let stream = c.stream(&other).await;
    assert!(matches!(
        c.state.operator.start_passkey_login(unix_now()).unwrap(),
        Start::NoPasskeys
    ));
    c.register(&other, "laptop");
    assert!(matches!(
        c.state.operator.start_passkey_login(unix_now()).unwrap(),
        Start::Begun { .. }
    ));
    assert!(!c.state.operator.ceremonies.is_empty());
    let mut ends_seen = c.state.operator.session_ends();
    ends_seen.borrow_and_update();

    let resp = c.patch(&mine, r#"{"public_url":"https://Moved.Example/"}"#).await;
    assert_eq!(resp.status(), 200);
    let cookies = set_cookies(&resp);
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    assert_eq!(
        cookies[0],
        "hennery_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0; Secure"
    );
    let settings: SettingsUpdateResponse = resp.json().await.unwrap();
    assert_eq!(
        settings,
        SettingsUpdateResponse {
            public_url: "https://moved.example".into(),
            contact: None,
            public_url_changed: Some(PublicUrlChanged {
                sessions_ended: 2,
                passkeys_removed: 1,
            }),
        }
    );
    assert!(ends_seen.has_changed().unwrap(), "the ending was not announced");
    assert!(
        ends(stream, Duration::from_secs(1)).await,
        "a stream outlived the change"
    );
    for session in [&mine, &other] {
        assert!(c.state.operator.authenticate(session, unix_now()).unwrap().is_none());
    }
    assert!(c.state.hosts.subscriptions().unwrap().is_empty());
    assert!(c.state.operator.passkeys().unwrap().is_empty());
    assert!(c.state.operator.ceremonies.is_empty());
    // Stored, not only cached: a restart keeps it.
    assert_eq!(
        Operator::open(&c.db).unwrap().public_url().unwrap().origin(),
        "https://moved.example"
    );

    // The old origin is refused, signed in or not; the new one is served.
    let fresh = c.session(0);
    let resp = c.patch(&fresh, r#"{"contact":"me@example.com"}"#).await;
    assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()));
    assert_eq!(c.login_from(PUBLIC_URL).await, 403);
    let resp = c
        .patch_from(&fresh, "https://moved.example", r#"{"contact":"me@example.com"}"#)
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(c.login_from("https://moved.example").await, 204);
}

/// Decision 4: the cookie is cleared as the old `public_url` set it, since
/// the answer goes to the old origin: `Secure` from an `https` one, though
/// the new one is loopback `http`, and not from loopback `http`.
#[tokio::test]
async fn the_cookie_is_cleared_as_the_old_public_url_set_it() {
    let dir = tempfile::tempdir().unwrap();
    let c = Collector::on(&dir.path().join("hennery.db")).await;
    let resp = c
        .patch(&c.session(0), r#"{"public_url":"http://localhost:7117"}"#)
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        set_cookies(&resp),
        ["hennery_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0; Secure"]
    );
    let resp = c
        .patch_from(
            &c.session(0),
            "http://localhost:7117",
            r#"{"public_url":"https://hennery.example"}"#,
        )
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        set_cookies(&resp),
        ["hennery_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"]
    );
}

/// Plan 3c decision 9, now over the API: a change of host name removes
/// every passkey; one of port keeps them.
#[tokio::test]
async fn a_host_change_removes_the_passkeys_and_a_port_change_keeps_them() {
    let dir = tempfile::tempdir().unwrap();
    let c = Collector::on(&dir.path().join("hennery.db")).await;
    let session = c.session(0);
    c.register(&session, "laptop");
    c.register(&session, "phone");
    let changed = |resp: SettingsUpdateResponse| resp.public_url_changed.unwrap();
    let resp = c
        .patch(&session, r#"{"public_url":"https://hennery.example:8443"}"#)
        .await;
    assert_eq!(
        changed(resp.json().await.unwrap()),
        PublicUrlChanged {
            sessions_ended: 1,
            passkeys_removed: 0
        }
    );
    assert_eq!(c.state.operator.passkeys().unwrap().len(), 2);
    let resp = c
        .patch_from(
            &c.session(0),
            "https://hennery.example:8443",
            r#"{"public_url":"https://moved.example"}"#,
        )
        .await;
    assert_eq!(
        changed(resp.json().await.unwrap()),
        PublicUrlChanged {
            sessions_ended: 1,
            passkeys_removed: 2
        }
    );
    assert!(c.state.operator.passkeys().unwrap().is_empty());
}

/// Decision 3: the same origin again is a change like any other, as the
/// admin socket's reset is: every session ends, the passkeys stay (the
/// host name is the same).
#[tokio::test]
async fn the_same_origin_again_ends_every_session_and_keeps_the_passkeys() {
    let dir = tempfile::tempdir().unwrap();
    let c = Collector::on(&dir.path().join("hennery.db")).await;
    let session = c.session(0);
    let other = c.session(0);
    c.register(&session, "laptop");
    let resp = c.patch(&session, &format!(r#"{{"public_url":"{PUBLIC_URL}"}}"#)).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(set_cookies(&resp).len(), 1);
    let settings: SettingsUpdateResponse = resp.json().await.unwrap();
    assert_eq!(settings.public_url, PUBLIC_URL);
    assert_eq!(
        settings.public_url_changed,
        Some(PublicUrlChanged {
            sessions_ended: 2,
            passkeys_removed: 0
        })
    );
    assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_none());
    assert_eq!(c.state.operator.passkeys().unwrap().len(), 1);
}

/// Every refusal of a body naming `public_url`, each with its code, and
/// none changes anything: no session, no cookie, a URL refused keeps the
/// contact beside it and a contact refused the URL (decision 1).
#[tokio::test]
async fn every_refusal_answers_its_code_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let c = Collector::on(&dir.path().join("hennery.db")).await;
    c.state.operator.set_contact(Some("old@example.com")).unwrap().unwrap();
    let session = c.session(0);
    let before = c.dump();
    let move_body = r#"{"public_url":"https://moved.example"}"#;
    let refusals = [
        // No cookie.
        (
            reqwest::Client::new()
                .patch(format!("http://{}/api/settings", c.addr))
                .header("origin", PUBLIC_URL)
                .json(&serde_json::json!({ "public_url": "https://moved.example" }))
                .send()
                .await
                .unwrap(),
            (401, "unauthenticated"),
        ),
        (
            c.patch_from(&session, "https://moved.example", move_body).await,
            (403, "origin_mismatch"),
        ),
        (
            c.request(&session, PUBLIC_URL, "PATCH", "/api/settings")
                .header("content-type", "text/plain")
                .body(move_body)
                .send()
                .await
                .unwrap(),
            (415, "unsupported_media_type"),
        ),
        (
            c.patch(
                &session,
                &format!(
                    r#"{{"public_url":"https://moved.example","contact":"{}"}}"#,
                    "a".repeat(hennery_kernel::auth_api::MAX_BODY_BYTES)
                ),
            )
            .await,
            (413, "body_too_large"),
        ),
        (
            c.patch(&session, r#"{"public_url":"https://moved.example","name":"x"}"#)
                .await,
            (422, "invalid_body"),
        ),
        (
            c.patch(&session, r#"{"public_url":"http://moved.example"}"#).await,
            (400, "invalid"),
        ),
        (
            c.patch(
                &session,
                r#"{"public_url":"https://moved.example/app","contact":"me@example.com"}"#,
            )
            .await,
            (400, "invalid"),
        ),
        (
            c.patch(
                &session,
                r#"{"public_url":"https://moved.example","contact":"me@example.com?cc=x"}"#,
            )
            .await,
            (400, "invalid"),
        ),
    ];
    for (resp, (status, code)) in refusals {
        assert!(set_cookies(&resp).is_empty(), "{code}");
        assert_eq!(code_of(resp).await, (status, code.into()));
        assert_eq!(c.dump(), before, "{code}");
    }
    assert_eq!(c.state.operator.public_url().unwrap().origin(), PUBLIC_URL);
    assert!(c.state.operator.authenticate(&session, unix_now()).unwrap().is_some());
}

/// The review's A1, over HTTP: a session revoked while its request's body
/// is still arriving, so after `require_operator` let it through (its
/// slide shows it ran), changes nothing. The answer is 401 and clears the
/// cookie (O2).
#[tokio::test]
async fn a_session_revoked_while_its_body_arrives_changes_nothing() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = tempfile::tempdir().unwrap();
    let c = Collector::on(&dir.path().join("hennery.db")).await;
    // Seen two minutes ago, so authenticating it slides its expiry.
    let token = c.session(120);
    let id = sha256_hex(token.as_bytes());
    let expiry = c.state.operator.session_expires_at(&id, unix_now()).unwrap().unwrap();
    let body = r#"{"public_url":"https://moved.example"}"#;
    let mut stream = tokio::net::TcpStream::connect(c.addr).await.unwrap();
    let head = format!(
        "PATCH /api/settings HTTP/1.1\r\nhost: {}\r\norigin: {PUBLIC_URL}\r\ncookie: hennery_session={token}\r\n\
         content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        c.addr,
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(&body.as_bytes()[..10]).await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while c.state.operator.session_expires_at(&id, unix_now()).unwrap() == Some(expiry) {
        assert!(std::time::Instant::now() < deadline, "the session never slid");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(c.state.operator.revoke_session(&id, unix_now()).unwrap());
    let before = c.dump();
    stream.write_all(&body.as_bytes()[10..]).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    let answer = answer.to_ascii_lowercase();
    assert!(answer.starts_with("http/1.1 401"), "{answer}");
    assert!(answer.contains(r#""code":"unauthenticated""#), "{answer}");
    assert!(
        answer.contains("set-cookie: hennery_session=; httponly; samesite=strict; path=/; max-age=0; secure\r\n"),
        "{answer}"
    );
    assert_eq!(c.dump(), before);
    assert_eq!(c.state.operator.public_url().unwrap().origin(), PUBLIC_URL);
}

/// The review's A1, the other way: a session whose step-up lapses while
/// its request's body is still arriving, after the handler's own check let
/// it through, changes nothing, the contact beside the URL included. The
/// answer is 403 `step_up_required`; the session lives on, so its cookie
/// is not cleared.
#[tokio::test]
async fn a_step_up_lapsed_while_its_body_arrives_changes_nothing() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = tempfile::tempdir().unwrap();
    let c = Collector::on(&dir.path().join("hennery.db")).await;
    c.state.operator.set_contact(Some("old@example.com")).unwrap().unwrap();
    // Checked two minutes ago: fresh, and authenticating it slides its
    // expiry.
    let token = c.session(120);
    let id = sha256_hex(token.as_bytes());
    let expiry = c.state.operator.session_expires_at(&id, unix_now()).unwrap().unwrap();
    let body = r#"{"public_url":"https://moved.example","contact":"me@example.com"}"#;
    let mut stream = tokio::net::TcpStream::connect(c.addr).await.unwrap();
    let head = format!(
        "PATCH /api/settings HTTP/1.1\r\nhost: {}\r\norigin: {PUBLIC_URL}\r\ncookie: hennery_session={token}\r\n\
         content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        c.addr,
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(&body.as_bytes()[..10]).await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while c.state.operator.session_expires_at(&id, unix_now()).unwrap() == Some(expiry) {
        assert!(std::time::Instant::now() < deadline, "the session never slid");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The last check is now exactly five minutes old: no longer fresh.
    let stepped_down = rusqlite::Connection::open(&c.db)
        .unwrap()
        .execute(
            "UPDATE auth_sessions SET last_step_up_at = ?1 WHERE id_hash = ?2",
            rusqlite::params![unix_now() - hennery_kernel::operator::STEP_UP_SECS, id],
        )
        .unwrap();
    assert_eq!(stepped_down, 1);
    let before = c.dump();
    stream.write_all(&body.as_bytes()[10..]).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    let answer = answer.to_ascii_lowercase();
    assert!(answer.starts_with("http/1.1 403"), "{answer}");
    assert!(answer.contains(r#""code":"step_up_required""#), "{answer}");
    // The slide's cookie, renewed; never a cleared one.
    assert!(!answer.contains("hennery_session=;"), "{answer}");
    assert!(!answer.contains("max-age=0"), "{answer}");
    assert_eq!(c.dump(), before);
    assert_eq!(c.state.operator.public_url().unwrap().origin(), PUBLIC_URL);
    assert_eq!(c.state.operator.contact().unwrap().as_deref(), Some("old@example.com"));
    assert!(c.state.operator.authenticate(&token, unix_now()).unwrap().is_some());
}

/// `Reset::NotSetUp` cannot be answered over HTTP: before setup the
/// browser rules refuse every state-changing request first.
#[tokio::test]
async fn before_setup_the_browser_rules_answer_first() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(
        Store::open(&db).unwrap(),
        Hosts::open(&db).unwrap(),
        Operator::open(&db).unwrap(),
    );
    tokio::spawn(hennery_sessions::serve(listener, state.clone()));
    let resp = reqwest::Client::new()
        .patch(format!("http://{addr}/api/settings"))
        .header("origin", PUBLIC_URL)
        .json(&serde_json::json!({ "public_url": PUBLIC_URL }))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (403, "setup_required".into()));
    assert_eq!(state.operator.public_url(), None);
}

/// Decision 1's parity: the API and the admin socket, given the same
/// database, leave it in the same state (the owner and its contact, the
/// settings, the password, sessions, subscriptions and passkeys), the
/// same cached origin and no ceremony, and both announce the ending. Once
/// for a host change, once for a port change, so both of decision 9's
/// branches are compared.
#[tokio::test]
async fn the_api_and_the_admin_socket_leave_the_same_state() {
    for target in ["https://moved.example", "https://hennery.example:8443"] {
        let dir = tempfile::tempdir().unwrap();
        let (api_dir, admin_dir) = (dir.path().join("api"), dir.path().join("admin"));
        std::fs::create_dir_all(&api_dir).unwrap();
        std::fs::create_dir_all(&admin_dir).unwrap();
        // Seeded once: a contact, two sessions, a subscription each, two
        // passkeys. Then copied whole, the WAL included.
        let api = Collector::on(&api_dir.join("hennery.db")).await;
        api.state.operator.set_contact(Some("me@example.com")).unwrap().unwrap();
        let caller = api.session(0);
        let other = api.session(0);
        api.subscribe(&caller, "https://fcm.googleapis.com/fcm/send/caller")
            .await;
        api.subscribe(&other, "https://fcm.googleapis.com/fcm/send/other").await;
        api.register(&caller, "laptop");
        api.register(&other, "phone");
        let copy = admin_dir.join("hennery.db");
        rusqlite::Connection::open(&api.db)
            .unwrap()
            .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
            .unwrap();
        let admin = Collector::on(&copy).await;
        assert_eq!(admin.dump(), api.dump(), "the copy differs");

        let mut announced = Vec::new();
        for c in [&api, &admin] {
            assert!(matches!(
                c.state.operator.start_passkey_login(unix_now()).unwrap(),
                Start::Begun { .. }
            ));
            let mut ends = c.state.operator.session_ends();
            ends.borrow_and_update();
            announced.push(ends);
        }

        let resp = api.patch(&caller, &format!(r#"{{"public_url":"{target}"}}"#)).await;
        assert_eq!(resp.status(), 200);
        let changed = resp
            .json::<SettingsUpdateResponse>()
            .await
            .unwrap()
            .public_url_changed
            .unwrap();

        let socket = admin_dir.join(ADMIN_SOCKET);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(hennery_kernel::admin::serve(
            hennery_kernel::admin::bind(&admin_dir).unwrap().expect("a short path"),
            Admin {
                operator: admin.state.operator.clone(),
                hosts: admin.state.hosts.clone(),
                dir: admin_dir.clone(),
                base_url: PUBLIC_URL.into(),
            },
            async move {
                let _ = stopped.await;
            },
        ));
        let answer = hennery_kernel::admin::request(
            &socket,
            &AdminRequest::ResetPublicUrl {
                public_url: target.into(),
            },
        )
        .await
        .unwrap();
        drop(stop);
        let AdminResponse::PublicUrlReset {
            public_url,
            sessions_ended,
            passkeys_removed,
        } = answer
        else {
            panic!("{answer:?}");
        };
        assert_eq!(
            (sessions_ended as u64, passkeys_removed as u64),
            (changed.sessions_ended, changed.passkeys_removed),
            "{target}"
        );
        assert_eq!(public_url, api.state.operator.public_url().unwrap().origin());

        assert_eq!(api.dump(), admin.dump(), "{target}");
        assert_eq!(
            api.state.operator.public_url(),
            admin.state.operator.public_url(),
            "{target}"
        );
        for (c, ends) in [&api, &admin].into_iter().zip(&announced) {
            assert!(c.state.operator.ceremonies.is_empty(), "{target}");
            assert!(ends.has_changed().unwrap(), "{target}: the ending was not announced");
        }
    }
}
