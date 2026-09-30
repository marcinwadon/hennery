//! The admin socket (kernel spec §4.2): private to its user, one JSON line
//! each way, every command against the collector's own operator and host
//! registry, and one collector per data directory.

use hennery_kernel::admin::{
    ADMIN_SOCKET, Admin, AdminRequest, AdminResponse, MAX_REQUEST_BYTES, bind, read_answer, request, serve,
};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

const PASSWORD: &str = "correct horse battery";
const BASE_URL: &str = "http://localhost:7117";

/// An admin socket served in `dir` on `operator` and `hosts`, until the
/// returned sender is dropped.
fn start(dir: &Path, operator: &Arc<Operator>, hosts: &Arc<Hosts>) -> tokio::sync::oneshot::Sender<()> {
    let socket = bind(dir).unwrap().expect("a short path");
    let admin = Admin {
        operator: operator.clone(),
        hosts: hosts.clone(),
        dir: dir.to_path_buf(),
        base_url: BASE_URL.into(),
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(serve(socket, admin, async move {
        let _ = stopped.await;
    }));
    stop
}

fn set_up(operator: &Operator) {
    let now = unix_now();
    let token = operator.issue_setup_token(now).unwrap().unwrap();
    operator
        .set_up(&token, PASSWORD, "https://hennery.example", now)
        .unwrap();
}

#[tokio::test]
async fn the_admin_socket_is_private_and_carries_out_each_command() {
    let dir = tempfile::tempdir().unwrap();
    let operator = Arc::new(Operator::open_in_memory().unwrap());
    let hosts = Arc::new(Hosts::open_in_memory().unwrap());
    let _stop = start(dir.path(), &operator, &hosts);
    let socket = dir.path().join(ADMIN_SOCKET);
    let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let ask = |req: AdminRequest| {
        let socket = socket.clone();
        async move { request(&socket, &req).await }
    };

    // Before setup.
    let AdminResponse::SetupUrl { url } = ask(AdminRequest::SetupUrl).await.unwrap() else {
        panic!("no setup link");
    };
    assert!(url.starts_with(&format!("{BASE_URL}/setup#")), "{url}");
    assert_eq!(
        ask(AdminRequest::SetupUrl).await.unwrap(),
        AdminResponse::SetupUrl { url: url.clone() },
        "the live link was not the one shown"
    );
    let reset = AdminRequest::ResetPassword {
        password: "a new long password".into(),
    };
    assert_eq!(ask(reset.clone()).await.unwrap(), AdminResponse::NotSetUp);

    set_up(&operator);
    assert_eq!(ask(AdminRequest::SetupUrl).await.unwrap(), AdminResponse::AlreadySetUp);
    operator.open_session("browser", unix_now()).unwrap().unwrap();
    assert_eq!(
        ask(reset).await.unwrap(),
        AdminResponse::PasswordReset { sessions_ended: 1 }
    );
    assert!(operator.verify_password("a new long password").unwrap());
    assert!(matches!(
        ask(AdminRequest::ResetPassword {
            password: "short".into()
        })
        .await
        .unwrap(),
        AdminResponse::Refused { .. }
    ));

    // The router's own operator: its cached origin changes at once.
    assert_eq!(
        ask(AdminRequest::ResetPublicUrl {
            public_url: "https://Moved.Example".into()
        })
        .await
        .unwrap(),
        AdminResponse::PublicUrlReset {
            public_url: "https://moved.example".into(),
            sessions_ended: 0
        }
    );
    assert_eq!(operator.public_url().unwrap().origin(), "https://moved.example");

    assert_eq!(
        ask(AdminRequest::ListHosts).await.unwrap(),
        AdminResponse::Hosts { hosts: Vec::new() }
    );
    let AdminResponse::PairingCode { code, expires_at } = ask(AdminRequest::MintPairingCode).await.unwrap() else {
        panic!("no pairing code");
    };
    assert!(expires_at > unix_now());
    let enrollment = Enrollment {
        public_key: hex_key(),
        name: "laptop".into(),
        host_version: "0.0.0".into(),
        platform: "test".into(),
    };
    assert!(matches!(
        hosts.enroll(&code, &enrollment, unix_now()).unwrap(),
        hennery_kernel::hosts::EnrollOutcome::Enrolled { .. }
    ));
    let AdminResponse::Hosts { hosts: listed } = ask(AdminRequest::ListHosts).await.unwrap() else {
        panic!("no host list");
    };
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "laptop");
}

/// A valid Ed25519 public key in hex (the base point's encoding).
fn hex_key() -> String {
    "5866666666666666666666666666666666666666666666666666666666666666".into()
}

/// A request that is not one line of JSON, or runs past
/// `MAX_REQUEST_BYTES`, is refused with an answer and changes nothing.
#[tokio::test]
async fn a_malformed_or_oversized_request_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let operator = Arc::new(Operator::open_in_memory().unwrap());
    let hosts = Arc::new(Hosts::open_in_memory().unwrap());
    let _stop = start(dir.path(), &operator, &hosts);
    let socket = dir.path().join(ADMIN_SOCKET);
    let send = |bytes: Vec<u8>| {
        let socket = socket.clone();
        async move {
            let mut stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
            // Written whole even when the collector stops reading early.
            let writer = tokio::spawn(async move {
                let _ = stream.write_all(&bytes).await;
                stream
            });
            let mut stream = writer.await.unwrap();
            read_answer(&mut stream).await.unwrap()
        }
    };
    for (what, bytes) in [
        ("not JSON", b"reset everything\n".to_vec()),
        ("an unknown command", b"{\"command\":\"drop_tables\"}\n".to_vec()),
        ("too long", vec![b'x'; MAX_REQUEST_BYTES as usize + 10]),
    ] {
        let answer = send(bytes).await;
        assert!(matches!(answer, AdminResponse::Refused { .. }), "{what}: {answer:?}");
    }
    assert!(!operator.is_set_up().unwrap());
    assert!(hosts.list().unwrap().is_empty());
}

/// One collector per data directory: a socket that answers is refused; a
/// stale one (its collector killed) is replaced; the socket is removed at
/// shutdown.
#[tokio::test]
async fn a_live_socket_is_refused_and_a_stale_one_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let operator = Arc::new(Operator::open_in_memory().unwrap());
    let hosts = Arc::new(Hosts::open_in_memory().unwrap());
    let socket = dir.path().join(ADMIN_SOCKET);
    let stop = start(dir.path(), &operator, &hosts);
    let err = bind(dir.path()).err().expect("a second socket was bound");
    assert!(format!("{err:#}").contains("another collector"), "{err:#}");
    drop(stop);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while socket.exists() {
        assert!(std::time::Instant::now() < deadline, "the socket outlived its server");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // A killed collector's socket: still there, nobody listening.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    let _stop = start(dir.path(), &operator, &hosts);
    assert!(matches!(
        request(&socket, &AdminRequest::ListHosts).await.unwrap(),
        AdminResponse::Hosts { .. }
    ));
}

/// A data directory whose `admin.sock` path does not fit a Unix socket
/// address leaves the collector without one, rather than stopping it.
#[test]
fn a_path_too_long_for_a_socket_binds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let long = dir.path().join("d".repeat(120));
    std::fs::create_dir(&long).unwrap();
    assert!(bind(&long).unwrap().is_none());
    assert!(!long.join(ADMIN_SOCKET).exists());
}

#[test]
fn a_reset_request_does_not_show_its_password_in_debug() {
    let req = AdminRequest::ResetPassword {
        password: "hunter2-hunter2".into(),
    };
    let shown = format!("{req:?}");
    assert!(!shown.contains("hunter2"), "{shown}");
    assert!(shown.contains("ResetPassword"), "{shown}");
}
