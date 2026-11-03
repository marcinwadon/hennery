use futures::{SinkExt, StreamExt};
use hennery_host::transport::{CollectorEndpoint, CollectorTransport, CollectorTrust, Stage, TransportFailure};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

struct Fixture {
    url: String,
    pem: String,
    seen: Arc<AtomicUsize>,
    verdict: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn fixture(names: Vec<String>, nonce: &'static str) -> Fixture {
    let key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(vec!["fixture-ca".into()]).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca.self_signed(&key).unwrap();
    let mut signer_params = CertificateParams::new(vec!["fixture-ca".into()]).unwrap();
    signer_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let signer = rcgen::Issuer::from_params(&signer_params, key);
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(names)
        .unwrap()
        .signed_by(&leaf_key, &signer)
        .unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(leaf_key.serialize_der()).into(),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://localhost:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(AtomicUsize::new(0));
    let capture = seen.clone();
    let verdict = Arc::new(AtomicUsize::new(0));
    let reply = verdict.clone();
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let capture = capture.clone();
            let reply = reply.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(stream).await {
                    let ws = tokio_tungstenite::accept_hdr_async(tls, NonceCallback { capture, nonce }).await;
                    if let Ok(mut ws) = ws {
                        while let Some(Ok(msg)) = ws.next().await {
                            if msg.is_close() {
                                break;
                            }
                            let reply = match reply.load(Ordering::SeqCst) {
                                0 => "{\"type\":\"hello_ack\",\"protocol_version\":\"1.0\",\"collector_version\":\"fixture\",\"committed\":{}}".to_string(),
                                4 => "{\"type\":\"sensitive-hello-body-canary\"}".to_string(),
                                n => format!("{{\"type\":\"hello_error\",\"code\":\"{}\",\"message\":\"fixture\"}}", match n {1=>"already_connected",2=>"revoked",_=>"bad_proof"}),
                            };
                            ws.send(Message::Text(reply.into())).await.ok();
                        }
                    }
                }
            });
        }
    });
    Fixture {
        url,
        pem: ca.pem(),
        seen,
        verdict,
        task,
    }
}
#[tokio::test]
async fn private_ca_wss_and_https_share_the_same_verifier() {
    let f = fixture(
        vec!["localhost".into()],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .await;
    let transport = CollectorTransport::new(
        CollectorEndpoint::public(&f.url).unwrap(),
        CollectorTrust::CaBundle { pem: f.pem.clone() },
    )
    .unwrap();
    let (mut ws, nonce) = transport.websocket(WebSocketConfig::default()).await.unwrap();
    assert_eq!(nonce, vec![0xaa; 32]);
    ws.close(None).await.unwrap();
    // The fixture speaks WS only: a normal HTTP request still reaches the
    // same verified TLS listener, then fails at HTTP rather than TLS.
    let error = transport.health().await.unwrap_err();
    assert!(!hennery_host::transport::classify(error.as_ref()).is_tls());
    assert_eq!(f.seen.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn failed_name_verification_sends_no_websocket_or_http_request() {
    let f = fixture(
        vec!["wrong.example".into()],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .await;
    let transport = CollectorTransport::new(
        CollectorEndpoint::public(&f.url).unwrap(),
        CollectorTrust::CaBundle { pem: f.pem.clone() },
    )
    .unwrap();
    let err = transport.websocket(WebSocketConfig::default()).await.err().unwrap();
    assert_eq!(
        err.downcast_ref::<TransportFailure>().unwrap().0,
        Stage::TlsNameMismatch
    );
    let err = transport.health().await.unwrap_err();
    assert_eq!(hennery_host::transport::classify(err.as_ref()), Stage::TlsNameMismatch);
    assert_eq!(f.seen.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn invalid_nonce_is_refused_before_any_hello() {
    let f = fixture(vec!["localhost".into()], "bad-nonce").await;
    let transport = CollectorTransport::new(
        CollectorEndpoint::public(&f.url).unwrap(),
        CollectorTrust::CaBundle { pem: f.pem.clone() },
    )
    .unwrap();
    let err = transport.websocket(WebSocketConfig::default()).await.err().unwrap();
    assert_eq!(err.downcast_ref::<TransportFailure>().unwrap().0, Stage::MissingNonce);
}

#[tokio::test]
async fn malformed_hello_diagnostics_never_reflect_response_bodies() {
    let f = fixture(
        vec!["localhost".into()],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .await;
    f.verdict.store(4, Ordering::SeqCst);
    let transport = CollectorTransport::new(
        CollectorEndpoint::public(&f.url).unwrap(),
        CollectorTrust::CaBundle { pem: f.pem.clone() },
    )
    .unwrap();
    let error = hennery_host::connection::probe_transport(
        &transport,
        "host-0123456789abcdef",
        &hennery_host::identity::HostKey::from_seed([126; 32]),
    )
    .await
    .unwrap_err();
    assert!(!format!("{error:#}").contains("sensitive-hello-body-canary"));
    assert!(error.to_string().contains("invalid collector hello response"));
}

struct NonceCallback {
    capture: Arc<AtomicUsize>,
    nonce: &'static str,
}
impl tokio_tungstenite::tungstenite::handshake::server::Callback for NonceCallback {
    fn on_request(
        self,
        _: &tokio_tungstenite::tungstenite::handshake::server::Request,
        mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
    ) -> Result<
        tokio_tungstenite::tungstenite::handshake::server::Response,
        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
    > {
        self.capture.fetch_add(1, Ordering::SeqCst);
        response
            .headers_mut()
            .insert(hennery_proto::HELLO_NONCE_HEADER, self.nonce.parse().unwrap());
        Ok(response)
    }
}

#[tokio::test]
#[cfg(feature = "test-hooks")]
async fn transport_move_preserves_identity_outbox_and_configuration_and_reports_fsync() {
    use hennery_host::{
        identity::{DurabilityUncertain, HostKey, Paired, storage_hooks},
        pairing::set_transport,
    };
    let fixture = fixture(
        vec!["localhost".into()],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let paired = Paired {
        collector_url: "ws://localhost:7117/api/hosts/ws".into(),
        trust: CollectorTrust::System,
        host_id: "host-0123456789abcdef".into(),
        key: HostKey::from_seed([31; 32]),
        workspace_roots: vec!["/srv/projects".into()],
    };
    paired.save(dir.path()).unwrap();
    std::fs::write(dir.path().join("outbox.db"), b"preserved").unwrap();
    let path = dir.path().join("host.toml");
    let mut table: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    table.insert(
        "cli".into(),
        toml::Value::Table(toml::Table::from_iter([(
            "claude".into(),
            toml::Value::String("/opt/fixture".into()),
        )])),
    );
    std::fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
    let original = std::fs::read(&path).unwrap();
    let key = std::fs::read(dir.path().join("host.key")).unwrap();
    assert!(set_transport(dir.path(), &fixture.url, None).await.is_err());
    assert_eq!(fixture.seen.load(Ordering::SeqCst), 0);
    let trust = CollectorTrust::CaBundle {
        pem: fixture.pem.clone(),
    };
    for verdict in [2, 3] {
        fixture.verdict.store(verdict, Ordering::SeqCst);
        assert!(
            set_transport(dir.path(), &fixture.url, Some(trust.clone()))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    fixture.verdict.store(1, Ordering::SeqCst);
    assert_eq!(
        set_transport(dir.path(), &fixture.url, Some(trust.clone()))
            .await
            .unwrap(),
        hennery_host::connection::Standing::Connected
    );
    assert_eq!(std::fs::read(dir.path().join("host.key")).unwrap(), key);
    std::fs::write(&path, &original).unwrap();
    fixture.verdict.store(0, Ordering::SeqCst);
    storage_hooks::fail_once(path.clone(), "before_rename");
    assert!(
        set_transport(dir.path(), &fixture.url, Some(trust.clone()))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    storage_hooks::fail_once(path.clone(), "directory_sync");
    let error = set_transport(dir.path(), &fixture.url, Some(trust.clone()))
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<DurabilityUncertain>().is_some(), "{error:#}");
    assert!(error.to_string().contains("read-back"));
    assert_ne!(std::fs::read(&path).unwrap(), original);
    set_transport(dir.path(), &fixture.url, None).await.unwrap();
    let loaded = Paired::read(dir.path()).unwrap().unwrap();
    assert_eq!(loaded.host_id, paired.host_id);
    assert_eq!(loaded.trust, trust);
    assert_eq!(loaded.workspace_roots, paired.workspace_roots);
    assert_eq!(std::fs::read(dir.path().join("host.key")).unwrap(), key);
    assert_eq!(std::fs::read(dir.path().join("outbox.db")).unwrap(), b"preserved");
    let table: toml::Table = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(table["cli"]["claude"].as_str(), Some("/opt/fixture"));
}

#[tokio::test]
#[cfg(feature = "test-hooks")]
async fn injected_system_roots_share_the_wss_and_http_policy_without_fallback() {
    use rustls::pki_types::pem::PemObject;
    let fixture = fixture(
        vec!["localhost".into()],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .await;
    let endpoint = CollectorEndpoint::public(&fixture.url).unwrap();
    assert!(CollectorTransport::with_native_roots(endpoint.clone(), CollectorTrust::System, Default::default).is_err());
    let cert = rustls::pki_types::CertificateDer::from_pem_slice(fixture.pem.as_bytes()).unwrap();
    let transport = CollectorTransport::with_native_roots(endpoint.clone(), CollectorTrust::System, || {
        let mut result = rustls_native_certs::CertificateResult::default();
        result.certs = vec![cert, vec![1, 2, 3].into()];
        result
    })
    .unwrap();
    assert_eq!(transport.root_warnings, 1);
    transport.preflight().await.unwrap();
    let error = transport.health().await.unwrap_err();
    assert!(!hennery_host::transport::classify(error.as_ref()).is_tls());
    let explicit = CollectorTransport::with_native_roots(
        endpoint,
        CollectorTrust::CaBundle {
            pem: fixture.pem.clone(),
        },
        || panic!("explicit CA must ignore native roots and SSL overrides"),
    )
    .unwrap();
    explicit.preflight().await.unwrap();
    let plain = CollectorTransport::with_native_roots(
        CollectorEndpoint::public("http://localhost:7117").unwrap(),
        CollectorTrust::System,
        || panic!("plaintext must not load roots"),
    );
    assert!(plain.is_ok());
}

#[test]
fn actual_native_roots_load_without_bundled_fallback() {
    let native = rustls_native_certs::load_native_certs();
    let any_usable = native.certs.iter().any(|cert| {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.clone()).is_ok()
    });
    let loaded = CollectorTransport::new(
        CollectorEndpoint::public("https://localhost").unwrap(),
        CollectorTrust::System,
    );
    assert_eq!(loaded.is_ok(), any_usable);
}
