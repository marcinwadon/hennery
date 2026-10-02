//! Web Push delivery over HTTP (plan 10b-ii), through the egress client, to a
//! fake push service on loopback: what arrives on the wire, what the
//! service's answers do, and that the collector's own client, public only,
//! sends nothing to it. No test leaves this machine.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hennery_kernel::delivery::{Delivery, Outcome, PAD_TO, RetryPolicy, spawn};
use hennery_kernel::egress::{Allowance, Egress, Timeouts};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::push::{Notice, Urgency, VapidKey};
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::PushPayload;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One request as the push service received it.
#[derive(Clone, Debug)]
struct Received {
    name: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

/// A status, and the `Retry-After` it is sent with.
type Answer = (u16, Option<&'static str>);

/// A push service: per subscription, scripted (status, `Retry-After`)
/// answers, 201 once they run out.
#[derive(Clone, Default)]
struct Service {
    script: Arc<Mutex<HashMap<String, VecDeque<Answer>>>>,
    received: Arc<Mutex<Vec<Received>>>,
}

async fn push(State(service): State<Service>, Path(name): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    let answer = service
        .script
        .lock()
        .unwrap()
        .get_mut(&name)
        .and_then(VecDeque::pop_front)
        .unwrap_or((201, None));
    service.received.lock().unwrap().push(Received {
        name,
        headers,
        body: body.to_vec(),
    });
    let mut response = StatusCode::from_u16(answer.0).unwrap().into_response();
    if let Some(after) = answer.1 {
        response.headers_mut().insert("retry-after", after.parse().unwrap());
    }
    response
}

impl Service {
    async fn start() -> (Service, SocketAddr) {
        let service = Service::default();
        let app = Router::new()
            .route("/push/{name}", post(push))
            .with_state(service.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (service, addr)
    }

    fn answer(&self, name: &str, answers: &[Answer]) {
        self.script
            .lock()
            .unwrap()
            .insert(name.into(), answers.iter().copied().collect());
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

struct Browser {
    secret: p256::SecretKey,
    auth: [u8; 16],
}

impl Browser {
    fn new(seed: u8) -> Self {
        Self {
            secret: p256::SecretKey::from_bytes(&[seed; 32].into()).unwrap(),
            auth: [seed; 16],
        }
    }

    fn read(&self, body: &[u8]) -> PushPayload {
        let auth = web_push_native::Auth::clone_from_slice(&self.auth);
        let plain = web_push_native::decrypt(body.to_vec(), &self.secret, &auth).unwrap();
        serde_json::from_slice(&plain).unwrap()
    }
}

struct Setup {
    hosts: Arc<Hosts>,
    operator: Arc<Operator>,
    vapid: Arc<VapidKey>,
    dir: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hennery.db");
        let operator = Operator::open(&path).unwrap();
        let hosts = Hosts::open(&path).unwrap();
        let token = operator.issue_setup_token(unix_now()).unwrap().unwrap();
        let SetupOutcome::Done { .. } = operator
            .set_up(&token, "correct horse battery", "https://hennery.example", unix_now())
            .unwrap()
        else {
            panic!("setup failed");
        };
        Self {
            hosts: Arc::new(hosts),
            operator: Arc::new(operator),
            vapid: Arc::new(VapidKey::generate()),
            dir,
        }
    }

    /// A subscription at `endpoint`, written straight into the file: a
    /// loopback `http` endpoint is never one `subscribe` accepts.
    fn subscribe_at(&self, id: &str, endpoint: &str, browser: &Browser) {
        let p256dh = URL_SAFE_NO_PAD.encode(browser.secret.public_key().to_encoded_point(false).as_bytes());
        rusqlite::Connection::open(self.dir.path().join("hennery.db"))
            .unwrap()
            .execute(
                "INSERT INTO push_subscriptions(id, owner_id, endpoint, p256dh, auth, device_label, auth_session, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'test', 's', ?6)",
                rusqlite::params![id, self.hosts.owner_id(), endpoint, p256dh, URL_SAFE_NO_PAD.encode(browser.auth), unix_now()],
            )
            .unwrap();
    }

    fn last_error(&self, id: &str) -> Option<String> {
        self.hosts
            .subscriptions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == id)?
            .last_error
    }
}

fn notice(hat_id: &str) -> Notice {
    Notice {
        hat_id: hat_id.into(),
        urgency: Urgency::High,
        title: "Fix the flaky test".into(),
        generic_title: "Session needs your answer".into(),
        body: "needs your answer".into(),
        detail: None,
        url: "/sessions/s1".into(),
        tag: "s1".into(),
    }
}

fn egress() -> Egress {
    Egress::new(Timeouts::DEFAULT).unwrap()
}

fn quick() -> RetryPolicy {
    RetryPolicy {
        delays: vec![Duration::ZERO, Duration::ZERO],
        max_retry_after: Duration::from_secs(2),
        attempt_timeout: Duration::from_secs(10),
        budget: Duration::from_secs(60),
    }
}

#[tokio::test]
async fn a_push_arrives_on_the_wire_encrypted_and_signed() {
    let (service, addr) = Service::start().await;
    let setup = Setup::new();
    let phone = Browser::new(7);
    setup.subscribe_at("push-a", &format!("http://{addr}/push/a"), &phone);
    let delivery = Delivery::new(
        setup.hosts.clone(),
        setup.operator.clone(),
        setup.vapid.clone(),
        egress().client(Allowance::InternalNetwork),
        quick(),
    );
    let outcomes = delivery
        .deliver(&notice(&setup.hosts.default_hat_for_new_hosts().unwrap()))
        .await
        .unwrap();
    assert_eq!(outcomes, [("push-a".into(), Outcome::Delivered)]);
    let received = service.received();
    assert_eq!(received.len(), 1);
    let got = &received[0];
    let header = |name: &str| got.headers.get(name).map(|v| v.to_str().unwrap().to_string());
    assert_eq!(header("ttl").as_deref(), Some("3600"));
    assert_eq!(header("urgency").as_deref(), Some("high"));
    assert_eq!(header("content-encoding").as_deref(), Some("aes128gcm"));
    assert_eq!(header("topic"), None);
    assert!(header("authorization").unwrap().starts_with("vapid t="));
    assert_eq!(phone.read(&got.body).title, "Fix the flaky test");
    // The padding survives the real path: RFC 8291's 86-byte header, the
    // padded plaintext, the 16-byte tag and its delimiter (the review's A4).
    assert_eq!(got.body.len(), 86 + PAD_TO + 17);
    assert!(setup.hosts.subscriptions().unwrap()[0].last_success_at.is_some());
}

#[tokio::test]
async fn the_services_answers_over_http_decide_the_subscription() {
    let (service, addr) = Service::start().await;
    let setup = Setup::new();
    setup.subscribe_at("push-gone", &format!("http://{addr}/push/gone"), &Browser::new(1));
    setup.subscribe_at("push-busy", &format!("http://{addr}/push/busy"), &Browser::new(2));
    setup.subscribe_at("push-refused", &format!("http://{addr}/push/refused"), &Browser::new(3));
    service.answer("gone", &[(410, None)]);
    service.answer("busy", &[(503, Some("1")), (429, None)]);
    service.answer("refused", &[(403, None)]);
    let delivery = Delivery::new(
        setup.hosts.clone(),
        setup.operator.clone(),
        setup.vapid.clone(),
        egress().client(Allowance::InternalNetwork),
        quick(),
    );
    let started = std::time::Instant::now();
    let mut outcomes = delivery
        .deliver(&notice(&setup.hosts.default_hat_for_new_hosts().unwrap()))
        .await
        .unwrap();
    outcomes.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(outcomes[0], ("push-busy".into(), Outcome::Delivered));
    assert_eq!(outcomes[1], ("push-gone".into(), Outcome::Removed));
    assert!(matches!(&outcomes[2], (id, Outcome::Failed(why)) if id == "push-refused" && why.contains("403")));
    // The 503's `Retry-After: 1` was waited out.
    assert!(started.elapsed() >= Duration::from_secs(1));
    let tries = |name: &str| service.received().iter().filter(|r| r.name == name).count();
    assert_eq!((tries("busy"), tries("gone"), tries("refused")), (3, 1, 1));
    let ids: Vec<String> = setup.hosts.subscriptions().unwrap().into_iter().map(|s| s.id).collect();
    assert!(!ids.contains(&"push-gone".to_string()));
}

#[tokio::test]
async fn a_service_that_cannot_be_reached_is_tried_again_then_recorded() {
    // A port nothing listens on: bound, then closed.
    let addr = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let setup = Setup::new();
    setup.subscribe_at("push-a", &format!("http://{addr}/push/a"), &Browser::new(1));
    let delivery = Delivery::new(
        setup.hosts.clone(),
        setup.operator.clone(),
        setup.vapid.clone(),
        egress().client(Allowance::InternalNetwork),
        quick(),
    );
    let outcomes = delivery
        .deliver(&notice(&setup.hosts.default_hat_for_new_hosts().unwrap()))
        .await
        .unwrap();
    assert!(
        matches!(&outcomes[0].1, Outcome::Failed(why) if why.contains("could not be reached")),
        "{outcomes:?}"
    );
    let error = setup.last_error("push-a").unwrap();
    assert!(!error.contains("127.0.0.1"), "{error}");
}

/// The collector's delivery (`spawn`) is public only (kernel spec §7.1): a
/// subscription at a loopback address is refused before anything is sent,
/// and kept, with the reason.
#[tokio::test]
async fn the_collectors_delivery_never_reaches_a_private_address() {
    let (service, addr) = Service::start().await;
    let setup = Setup::new();
    setup.subscribe_at("push-a", &format!("http://{addr}/push/a"), &Browser::new(1));
    let push = spawn(
        setup.hosts.clone(),
        setup.operator.clone(),
        setup.vapid.clone(),
        &egress(),
    );
    push.notify(notice(&setup.hosts.default_hat_for_new_hosts().unwrap()));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let error = loop {
        if let Some(error) = setup.last_error("push-a") {
            break error;
        }
        assert!(tokio::time::Instant::now() < deadline, "nothing recorded");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(error.contains("public address"), "{error}");
    assert!(service.received().is_empty());
    assert_eq!(setup.hosts.subscriptions().unwrap().len(), 1);
}
