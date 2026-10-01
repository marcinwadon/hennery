//! 3c review, O4: a refused passkey answer's reason is logged
//! `Debug`-escaped, so a client cannot forge a log line with a newline in
//! its answer. In a test binary of its own, and driven through the router
//! in-process on the test's own task: `tracing`'s thread-local subscriber
//! then sees every event, whatever other tests run.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const ORIGIN: &str = "https://hennery.example";

/// A `tracing` writer into a shared buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A `POST` of `body` to `path` from the public URL's origin, signed in.
fn post(path: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(path)
        .header("origin", ORIGIN)
        .header("cookie", format!("hennery_session={token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn a_refused_passkeys_reason_is_logged_escaped() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let operator = Arc::new(Operator::open_in_memory().unwrap());
    let now = unix_now();
    let setup = operator.issue_setup_token(now).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap() else {
        panic!("setup failed");
    };
    // Just signed in, so stepped up.
    let token = operator.open_session("test", &phc, now).unwrap().unwrap();
    let app = hennery_kernel::auth_api::router(operator);

    let start = app
        .clone()
        .oneshot(post(
            "/api/auth/passkeys/register/start",
            &token,
            serde_json::json!({ "label": "laptop" }),
        ))
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::OK);
    let start: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(start.into_body(), 1 << 20).await.unwrap()).unwrap();
    let ceremony_id = start["ceremony_id"].as_str().unwrap();
    // serde quotes an unknown variant as it was sent, newline included.
    let finish = app
        .oneshot(post(
            "/api/auth/passkeys/register/finish",
            &token,
            serde_json::json!({
                "ceremony_id": ceremony_id,
                "credential": { "extensions": { "credProtect": "x\nforged log line" } },
            }),
        ))
        .await
        .unwrap();
    assert_eq!(finish.status(), StatusCode::UNAUTHORIZED);

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let refused: Vec<&str> = log.lines().filter(|l| l.contains("passkey refused")).collect();
    assert_eq!(refused.len(), 1, "{log}");
    assert!(refused[0].contains(r"x\nforged log line"), "{log}");
    assert!(!log.lines().any(|l| l.starts_with("forged log line")), "{log}");
}
