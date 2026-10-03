//! `deployment` on the admin socket (plan 4d-B3): kernel spec §10's facts,
//! by count, for `hennery doctor`; and the client's errors, told apart by
//! type (`admin::Why`), with the text `hennery admin` has always printed.

use hennery_kernel::admin::{
    ADMIN_SOCKET, Admin, AdminRequest, AdminResponse, Why, bind, request, request_within, serve, why,
};
use hennery_kernel::deployment::Deployment;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// An admin socket in `dir` answering from `deployment`, until the returned
/// sender is dropped.
fn start(dir: &Path, deployment: Deployment) -> tokio::sync::oneshot::Sender<()> {
    let admin = Admin {
        operator: Arc::new(Operator::open_in_memory().unwrap()),
        hosts: Arc::new(Hosts::open_in_memory().unwrap()),
        dir: dir.to_path_buf(),
        base_url: "http://localhost:7117".into(),
        deployment,
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(serve(bind(dir).unwrap().expect("a short path"), admin, async move {
        let _ = stopped.await;
    }));
    stop
}

/// The facts as the collector has them, a count and no hat; a read-only
/// question, logged at `info` like `list_hosts`.
#[tokio::test]
async fn deployment_answers_the_facts_by_count() {
    assert!(!AdminRequest::Deployment.changes_state());
    assert_eq!(AdminRequest::Deployment.name(), "deployment");
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path(), Deployment::new(true, || Ok(2)));
    let answer = request(&dir.path().join(ADMIN_SOCKET), &AdminRequest::Deployment)
        .await
        .unwrap();
    assert_eq!(
        answer,
        AdminResponse::Deployment {
            beside_host: true,
            hats: 2
        }
    );
    assert_eq!(
        serde_json::to_string(&answer).unwrap(),
        r#"{"result":"deployment","beside_host":true,"hats":2}"#
    );
}

/// A count that cannot be read is `failed`, with why.
#[tokio::test]
async fn a_count_that_cannot_be_read_fails() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(
        dir.path(),
        Deployment::new(true, || anyhow::bail!("the gateway's store is gone")),
    );
    let answer = request(&dir.path().join(ADMIN_SOCKET), &AdminRequest::Deployment)
        .await
        .unwrap();
    assert_eq!(
        answer,
        AdminResponse::Failed {
            message: "the gateway's store is gone".into()
        }
    );
}

/// Each way of getting no answer is told by its type, and its text is the
/// one `hennery admin` printed before.
#[tokio::test]
async fn each_way_of_getting_no_answer_is_told_apart() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join(ADMIN_SOCKET);
    let ask = |timeout: Duration| {
        let socket = socket.clone();
        async move {
            request_within(&socket, &AdminRequest::Deployment, timeout)
                .await
                .unwrap_err()
        }
    };

    // No socket.
    let err = ask(Duration::from_secs(5)).await;
    assert_eq!(why(&err), Some(Why::NoSocket));
    assert_eq!(
        format!("{err}"),
        format!("connect to {} (is the collector running?)", socket.display())
    );

    // A socket nobody serves: a collector that was killed left it.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    let err = ask(Duration::from_secs(5)).await;
    assert_eq!(why(&err), Some(Why::Stale));
    std::fs::remove_file(&socket).unwrap();

    // Served, but closed without a byte of answer. The request is read
    // first: on Linux, a socket closed with bytes unread resets the
    // connection, and the client would read an error, not the end.
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let closing = tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = tokio::io::BufReader::new(stream);
        let mut request = String::new();
        reader.read_line(&mut request).await.unwrap();
        drop(reader);
        listener
    });
    let err = ask(Duration::from_secs(5)).await;
    assert_eq!(why(&err), Some(Why::Unanswered));
    assert!(format!("{err}").starts_with("the collector closed the connection without answering"));

    // Served, and never answered.
    let listener = closing.await.unwrap();
    let holding = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(stream);
    });
    let err = ask(Duration::from_millis(300)).await;
    assert_eq!(why(&err), Some(Why::TimedOut));
    assert!(format!("{err}").contains("did not answer within"), "{err}");
    holding.abort();

    // A path too long for a Unix socket.
    let long = dir.path().join("d".repeat(120)).join(ADMIN_SOCKET);
    let err = request_within(&long, &AdminRequest::Deployment, Duration::from_secs(5))
        .await
        .unwrap_err();
    assert_eq!(why(&err), Some(Why::PathTooLong));

    // A directory this user may not search: refused, unless the tests run
    // as root, whom nothing refuses (and who cannot see this case).
    // SAFETY: geteuid(2) cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        use std::os::unix::fs::PermissionsExt;
        let closed = dir.path().join("closed");
        std::fs::create_dir(&closed).unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = request_within(
            &closed.join(ADMIN_SOCKET),
            &AdminRequest::Deployment,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(why(&err), Some(Why::Denied), "{err:#}");
    }

    // Anything else has no `Why`.
    assert_eq!(why(&anyhow::anyhow!("something else")), None);
}
