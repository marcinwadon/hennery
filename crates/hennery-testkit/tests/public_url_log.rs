//! Plan 4d-B4's review, A2: a `public_url` changed over the API is logged
//! at `warn` with the origin left, the origin taken and what the change
//! ended. In a test binary of its own, with a global subscriber: the
//! collector's tasks run on the runtime's threads, not the test's.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::PUBLIC_URL;
use std::sync::{Arc, Mutex};

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

#[tokio::test]
async fn a_change_over_the_api_is_logged_with_both_origins() {
    use tracing_subscriber::util::SubscriberInitExt;
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing_subscriber::filter::LevelFilter::WARN)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish()
        .init();

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(
        Store::open(&db).unwrap(),
        Hosts::open(&db).unwrap(),
        Operator::open(&db).unwrap(),
    );
    let client = hennery_testkit::operator_client(&state.operator);
    tokio::spawn(hennery_sessions::serve(listener, state.clone()));
    let resp = client
        .patch(format!("http://{addr}/api/settings"))
        .json(&serde_json::json!({ "public_url": "https://moved.example" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let line = logged
        .lines()
        .find(|line| line.contains("public_url changed over the API"))
        .unwrap_or_else(|| panic!("not logged:\n{logged}"));
    assert!(line.contains(" WARN "), "{line}");
    // `Debug`-quoted, as every string field is.
    assert!(line.contains(&format!("from=\"{PUBLIC_URL}\"")), "{line}");
    assert!(line.contains("to=\"https://moved.example\""), "{line}");
    assert!(line.contains("sessions_ended=1 passkeys_removed=0"), "{line}");
}
