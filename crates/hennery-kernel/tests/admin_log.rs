//! The admin socket's log (kernel spec §4.2), in a test binary of its own:
//! `tracing`'s thread-local subscriber must not race other tests.

use hennery_kernel::admin::{ADMIN_SOCKET, Admin, AdminRequest, AdminResponse, bind, request, serve};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use std::sync::Arc;

/// A writer for a `tracing` subscriber that keeps what it is given.
#[derive(Clone, Default)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `hennery admin` checks the socket before it prompts by connecting and
/// closing without a request. The collector takes that quietly: no warning
/// in its log, and it keeps serving.
#[tokio::test]
async fn a_connection_closed_without_a_request_is_not_warned_about() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let dir = tempfile::tempdir().unwrap();
    let operator = Arc::new(Operator::open_in_memory().unwrap());
    let hosts = Arc::new(Hosts::open_in_memory().unwrap());
    let admin = Admin {
        operator,
        hosts,
        dir: dir.path().to_path_buf(),
        base_url: "http://localhost:7117".into(),
    };
    let (_stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(serve(
        bind(dir.path()).unwrap().expect("a short path"),
        admin,
        async move {
            let _ = stopped.await;
        },
    ));
    let socket = dir.path().join(ADMIN_SOCKET);
    drop(tokio::net::UnixStream::connect(&socket).await.unwrap());
    // Answered after the closed one: the server is still serving, and the
    // capture works (a command is logged at `info`).
    assert!(matches!(
        request(&socket, &AdminRequest::SetupUrl).await.unwrap(),
        AdminResponse::SetupUrl { .. }
    ));
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("admin command"), "{log}");
    assert!(!log.contains("WARN"), "{log}");
}
