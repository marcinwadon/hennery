//! Kernel spec §10's deployment warning in Settings (plan 4d-B3):
//! `deployment_warning` on `GET /api/settings` and on both `PATCH` answers,
//! drawn from the collector's `Deployment`, read before anything is written.

use hennery_kernel::deployment::Deployment;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{ApiError, SettingsResponse, SettingsUpdateResponse};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::{OWNER_PASSWORD, PUBLIC_URL};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

struct Collector {
    addr: SocketAddr,
    state: AppState,
    db: PathBuf,
}

impl Collector {
    /// A collector on `db`, set up at `PUBLIC_URL`, drawing its warning
    /// from `deployment`.
    async fn on(db: &Path, deployment: Deployment) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut state = AppState::new(
            Store::open(db).unwrap(),
            Hosts::open(db).unwrap(),
            Operator::open(db).unwrap(),
        );
        state.deployment = deployment;
        let token = state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
        state
            .operator
            .set_up(&token, OWNER_PASSWORD, PUBLIC_URL, unix_now())
            .unwrap();
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            addr,
            state,
            db: db.to_path_buf(),
        }
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

    fn request(&self, session: &str, method: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method.parse().unwrap(), format!("http://{}/api/settings", self.addr))
            .header("origin", PUBLIC_URL)
            .header("cookie", format!("hennery_session={session}"))
    }

    async fn get(&self, session: &str) -> reqwest::Response {
        self.request(session, "GET").send().await.unwrap()
    }

    async fn patch(&self, session: &str, body: &str) -> reqwest::Response {
        self.request(session, "PATCH")
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap()
    }

    /// The owner's settings and sessions, as text: what a `PATCH` writes.
    fn dump(&self) -> Vec<String> {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        let mut rows = Vec::new();
        for table in ["owners", "settings", "auth_sessions"] {
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
            let columns = stmt.column_count();
            let found = stmt
                .query_map([], |r| {
                    Ok((0..columns)
                        .map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap()))
                        .collect::<Vec<_>>()
                        .join("|"))
                })
                .unwrap();
            rows.extend(found.map(|row| format!("{table}: {}", row.unwrap())));
        }
        rows
    }
}

/// Every answer that carries the settings carries the verdict: `GET`, a
/// `PATCH` of the contact, and a `PATCH` that moves `public_url`. True only
/// beside `up`'s host child with credentials for more than one hat.
#[tokio::test]
async fn every_settings_answer_carries_the_deployment_warning() {
    for (beside_host, hats, warning) in [(true, 2, true), (true, 1, false), (false, 3, false)] {
        let case = format!("beside_host {beside_host}, {hats} hats");
        let dir = tempfile::tempdir().unwrap();
        let c = Collector::on(
            &dir.path().join("hennery.db"),
            Deployment::new(beside_host, move || Ok(hats)),
        )
        .await;
        let session = c.session(0);

        let resp = c.get(&session).await;
        assert_eq!(resp.status(), 200, "{case}");
        let got: SettingsResponse = resp.json().await.unwrap();
        assert_eq!(got.deployment_warning, warning, "GET, {case}");

        let resp = c.patch(&session, r#"{"contact":"me@example.com"}"#).await;
        assert_eq!(resp.status(), 200, "{case}");
        let got: SettingsUpdateResponse = resp.json().await.unwrap();
        assert_eq!(got.deployment_warning, warning, "PATCH contact, {case}");

        let resp = c.patch(&session, r#"{"public_url":"https://moved.example"}"#).await;
        assert_eq!(resp.status(), 200, "{case}");
        let got: SettingsUpdateResponse = resp.json().await.unwrap();
        assert!(got.public_url_changed.is_some(), "{case}");
        assert_eq!(got.deployment_warning, warning, "PATCH public_url, {case}");
    }
}

/// A verdict that cannot be read is a 500, never a quiet `false`, and a
/// `PATCH` reads it before it writes anything (the review's A4): nothing
/// changes. It is read after the step-up check, so a stale session still
/// hears that it needs one.
#[tokio::test]
async fn a_verdict_that_cannot_be_read_is_a_500_that_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let c = Collector::on(
        &dir.path().join("hennery.db"),
        Deployment::new(true, || anyhow::bail!("the gateway's store is gone")),
    )
    .await;
    let session = c.session(0);
    let stale = c.session(5 * 60);
    // Slid now, so no request below changes a session's row.
    for s in [&session, &stale] {
        assert_eq!(c.get(s).await.status(), 500);
    }
    let before = c.dump();

    let resp = c.patch(&session, r#"{"contact":"me@example.com"}"#).await;
    assert_eq!(resp.status(), 500);
    assert_eq!(c.dump(), before, "a contact was stored");

    let resp = c.patch(&session, r#"{"public_url":"https://moved.example"}"#).await;
    assert_eq!(resp.status(), 500);
    assert_eq!(c.dump(), before, "public_url moved");

    let resp = c.patch(&stale, r#"{"public_url":"https://moved.example"}"#).await;
    assert_eq!(resp.status(), 403);
    assert_eq!(resp.json::<ApiError>().await.unwrap().code, "step_up_required");
    assert_eq!(c.dump(), before);
}
