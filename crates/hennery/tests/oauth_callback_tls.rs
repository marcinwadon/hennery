//! The real CLI serves HTTPS without trusting a CA on the test machine.
use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Receiver(Child);
impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn https_callback_serves_real_tls_preserves_redirect_and_stops_with_pending_handshakes() {
    let binary = env!("CARGO_BIN_EXE_hennery");
    hennery_testkit::first_exec(binary);
    let dir = tempfile::tempdir().unwrap();
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let local = format!("https://127.0.0.1:{port}/callback");
    let log = dir.path().join("receiver.log");
    let mut receiver = Receiver(
        Command::new(binary)
            .env_clear()
            .env("HOME", dir.path())
            .env("XDG_DATA_HOME", dir.path().join("data"))
            .env("XDG_CONFIG_HOME", dir.path().join("config"))
            .env("XDG_CACHE_HOME", dir.path().join("cache"))
            .env("XDG_STATE_HOME", dir.path().join("state"))
            .args([
                "oauth-callback",
                "--collector",
                "https://collector.example",
                "--redirect-uri",
                &local,
                "--timeout-secs",
                "30",
            ])
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if std::fs::read_to_string(&log)
                .unwrap()
                .contains("Local OAuth callback ready")
            {
                break;
            }
            assert!(
                receiver.0.try_wait().unwrap().is_none(),
                "{}",
                std::fs::read_to_string(&log).unwrap()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    // A connection that sends no ClientHello must not hold up the browser.
    let stalled = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let strict = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    assert!(
        strict.get(&local).send().await.is_err(),
        "an ephemeral certificate is never globally trusted"
    );
    let prepare = reqwest::Client::builder()
        .no_proxy()
        .tls_info(true)
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let ready = prepare.get(&local).send().await.unwrap();
    assert_eq!(ready.status(), 200);
    assert!(ready.headers().get("location").is_none());
    let certificate = ready
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .unwrap()
        .peer_certificate()
        .unwrap();
    // Scope trust to this one client and verify the generated SAN for the IP.
    let browser = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(reqwest::Certificate::from_der(certificate).unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    assert!(ready.text().await.unwrap().contains("Local OAuth callback ready"));
    let query = "code=https-code-canary&state=https-state-canary&extra=%2520&state=duplicate";
    let response = browser.get(format!("{local}?{query}")).send().await.unwrap();
    assert_eq!(response.status(), 302);
    assert_eq!(
        response.headers()["location"],
        format!("https://collector.example/api/mcp/oauth/callback?{query}")
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(
        browser
            .get(&local)
            .header("Host", "foreign.example")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(browser.post(&local).send().await.unwrap().status(), 405);
    // Only our receiver receives the signal; Drop also reaps it on failure.
    assert_eq!(unsafe { libc::kill(receiver.0.id() as i32, libc::SIGINT) }, 0);
    tokio::time::timeout(Duration::from_secs(5), async {
        while receiver.0.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(receiver.0.wait().unwrap().success());
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err());
    let diagnostic = std::fs::read_to_string(&log).unwrap();
    assert!(diagnostic.contains("temporary self-signed certificate"));
    assert!(!diagnostic.contains("https-code-canary"));
    assert!(!diagnostic.contains("https-state-canary"));
    assert!(
        dir.path()
            .read_dir()
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == "receiver.log"),
        "no certificate, key or data files are persisted"
    );
    drop(stalled);
}
