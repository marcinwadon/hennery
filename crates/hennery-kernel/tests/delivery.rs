//! Web Push delivery (kernel spec §6; plan 10b-ii), through a fake
//! transport: what reaches the push service (encrypted to the browser,
//! padded, signed), what the hat's policy lets through, and what each answer
//! does to the subscription. Nothing here opens a connection.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hennery_kernel::delivery::{Delivery, Outcome, PAD_TO, PushRequest, RetryPolicy, Sent, Transport};
use hennery_kernel::hats::HatChange;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::push::{NewSubscription, Notice, Push, PushPolicy, Subscribed, Urgency, VapidKey};
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::PushPayload;
use p256::ecdsa::signature::Verifier;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A push service per endpoint: answers from a script (201 once it runs
/// out), and every request kept. An endpoint `silent` never answers; one
/// `panicking` fails the task sending to it. `on_send` runs once, as the
/// first request is sent.
#[derive(Clone, Default)]
struct Fake {
    script: Arc<Mutex<HashMap<String, VecDeque<Sent>>>>,
    silent: Arc<Mutex<Vec<String>>>,
    panicking: Arc<Mutex<Vec<String>>>,
    on_send: Arc<Mutex<Option<OnSend>>>,
    sent: Arc<Mutex<Vec<PushRequest>>>,
}

type OnSend = Box<dyn FnOnce() + Send>;

impl Fake {
    fn answer(&self, endpoint: &str, answers: &[Sent]) {
        self.script
            .lock()
            .unwrap()
            .insert(endpoint.into(), answers.iter().cloned().collect());
    }

    fn sent(&self) -> Vec<PushRequest> {
        self.sent.lock().unwrap().clone()
    }

    fn silent(&self, endpoint: &str) {
        self.silent.lock().unwrap().push(endpoint.into());
    }

    fn panicking(&self, endpoint: &str) {
        self.panicking.lock().unwrap().push(endpoint.into());
    }

    fn on_send(&self, then: impl FnOnce() + Send + 'static) {
        *self.on_send.lock().unwrap() = Some(Box::new(then));
    }
}

impl Transport for Fake {
    fn send(&self, request: PushRequest) -> Pin<Box<dyn Future<Output = Sent> + Send + '_>> {
        let answer = self
            .script
            .lock()
            .unwrap()
            .get_mut(request.endpoint.as_str())
            .and_then(VecDeque::pop_front)
            .unwrap_or(Sent::Status {
                code: 201,
                retry_after: None,
            });
        let silent = self.silent.lock().unwrap().contains(&request.endpoint.to_string());
        let panics = self.panicking.lock().unwrap().contains(&request.endpoint.to_string());
        self.sent.lock().unwrap().push(request);
        let then = self.on_send.lock().unwrap().take();
        if let Some(then) = then {
            then();
        }
        Box::pin(async move {
            if silent {
                std::future::pending::<()>().await;
            }
            assert!(!panics, "the push task fails");
            answer
        })
    }
}

/// A browser's subscription keys, and what it decrypts with.
struct Browser {
    secret: p256::SecretKey,
    auth: [u8; 16],
}

impl Browser {
    fn new(seed: u8) -> Self {
        let secret = p256::SecretKey::from_bytes(&[seed; 32].into()).unwrap();
        Self {
            secret,
            auth: [seed; 16],
        }
    }

    fn p256dh(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.secret.public_key().to_encoded_point(false).as_bytes())
    }

    fn auth(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.auth)
    }

    /// The payload, as the browser reads it: decrypted, then parsed.
    fn read(&self, request: &PushRequest) -> PushPayload {
        let auth = web_push_native::Auth::clone_from_slice(&self.auth);
        let plain = web_push_native::decrypt(request.body.clone(), &self.secret, &auth).unwrap();
        assert_eq!(plain.len(), PAD_TO, "padded to its bucket");
        serde_json::from_slice(&plain).unwrap()
    }
}

struct Setup {
    hosts: Arc<Hosts>,
    operator: Arc<Operator>,
    vapid: Arc<VapidKey>,
    fake: Fake,
    session: String,
    _dir: tempfile::TempDir,
}

impl Setup {
    /// The owner set up at `public_url`, signed in once.
    fn new(public_url: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hennery.db");
        let operator = Operator::open(&path).unwrap();
        let hosts = Hosts::open(&path).unwrap();
        let now = unix_now();
        let token = operator.issue_setup_token(now).unwrap().unwrap();
        let SetupOutcome::Done { phc, .. } = operator
            .set_up(&token, "correct horse battery", public_url, now)
            .unwrap()
        else {
            panic!("setup failed");
        };
        let cookie = operator.open_session("browser", &phc, now).unwrap().unwrap();
        let session = operator.authenticate(&cookie, now).unwrap().unwrap().session_id;
        Self {
            hosts: Arc::new(hosts),
            operator: Arc::new(operator),
            vapid: Arc::new(VapidKey::generate()),
            fake: Fake::default(),
            session,
            _dir: dir,
        }
    }

    fn subscribe(&self, endpoint: &str, browser: &Browser, expires_at: Option<i64>) -> String {
        let (p256dh, auth) = (browser.p256dh(), browser.auth());
        let new = NewSubscription {
            endpoint,
            p256dh: &p256dh,
            auth: &auth,
            device_label: None,
            expires_at,
        };
        match self.hosts.subscribe(&new, &self.session, unix_now()).unwrap() {
            Subscribed::Created(sub) => sub.id,
            other => panic!("{other:?}"),
        }
    }

    fn delivery(&self, retry: RetryPolicy) -> Arc<Delivery<Fake>> {
        Delivery::new(
            self.hosts.clone(),
            self.operator.clone(),
            self.vapid.clone(),
            self.fake.clone(),
            retry,
        )
    }

    fn hat(&self) -> String {
        self.hosts.default_hat_for_new_hosts().unwrap()
    }
}

/// No waits between tries, and time enough for all of them.
fn at_once() -> RetryPolicy {
    RetryPolicy {
        delays: vec![Duration::ZERO, Duration::ZERO],
        max_retry_after: Duration::ZERO,
        attempt_timeout: Duration::from_secs(60),
        budget: Duration::from_secs(3600),
    }
}

fn notice(hat_id: &str) -> Notice {
    Notice {
        hat_id: hat_id.into(),
        urgency: Urgency::High,
        title: "Fix the flaky test".into(),
        generic_title: "Session needs your answer".into(),
        body: "needs your answer".into(),
        detail: Some("Run cargo test".into()),
        url: "/sessions/s1".into(),
        tag: "s1".into(),
    }
}

fn header<'a>(request: &'a PushRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// The VAPID header's claims, verified with the key it names: a JWS parse
/// and p256's verify, independent of the signing code.
fn vapid_claims(authorization: &str, vapid: &VapidKey) -> Value {
    let (t, k) = authorization.strip_prefix("vapid ").unwrap().split_once(", ").unwrap();
    let k = k.strip_prefix("k=").unwrap();
    assert_eq!(k, vapid.public_key());
    let parts: Vec<&str> = t.strip_prefix("t=").unwrap().split('.').collect();
    let signature = p256::ecdsa::Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
    p256::ecdsa::VerifyingKey::from_sec1_bytes(&URL_SAFE_NO_PAD.decode(k).unwrap())
        .unwrap()
        .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
        .unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap()
}

#[tokio::test]
async fn a_push_reaches_the_browser_encrypted_padded_and_signed() {
    let setup = Setup::new("https://hennery.example");
    setup.operator.set_contact(Some("me@example.com")).unwrap().unwrap();
    let phone = Browser::new(7);
    let id = setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
    let outcomes = setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
    assert_eq!(outcomes, [(id.clone(), Outcome::Delivered)]);

    let sent = setup.fake.sent();
    assert_eq!(sent.len(), 1);
    let request = &sent[0];
    assert_eq!(
        phone.read(request),
        PushPayload {
            title: "Fix the flaky test".into(),
            body: "needs your answer".into(),
            url: "/sessions/s1".into(),
            tag: "s1".into(),
        }
    );
    assert_eq!(header(request, "TTL"), Some("3600"));
    assert_eq!(header(request, "Urgency"), Some("high"));
    assert_eq!(header(request, "Content-Encoding"), Some("aes128gcm"));
    assert_eq!(header(request, "Content-Type"), Some("application/octet-stream"));
    assert_eq!(header(request, "Topic"), None, "the push service would see a topic");
    let claims = vapid_claims(header(request, "Authorization").unwrap(), &setup.vapid);
    assert_eq!(claims["aud"], "https://fcm.googleapis.com");
    assert_eq!(claims["sub"], "mailto:me@example.com");
    let lifetime = claims["exp"].as_i64().unwrap() - unix_now();
    assert!(lifetime > 0 && lifetime <= 24 * 60 * 60);
    // Nothing of the title in the clear: the body is ciphertext.
    assert!(!String::from_utf8_lossy(&request.body).contains("flaky"));

    let stored = &setup.hosts.subscriptions().unwrap()[0];
    assert!(stored.last_success_at.is_some());
    assert_eq!(stored.last_error, None);

    // Anything but "needs your answer" is normal (kernel spec §6).
    let finished = Notice {
        urgency: Urgency::Normal,
        ..notice(&setup.hat())
    };
    setup.delivery(at_once()).deliver(&finished).await.unwrap();
    assert_eq!(header(&setup.fake.sent()[1], "Urgency"), Some("normal"));
}

#[tokio::test]
async fn the_hats_policy_decides_what_is_sent() {
    let setup = Setup::new("https://hennery.example");
    let phone = Browser::new(7);
    setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
    let hat = setup.hat();
    let delivery = setup.delivery(at_once());
    let policy = |muted, details, generic_title| PushPolicy {
        muted,
        details,
        generic_title,
    };
    setup.hosts.set_push_policy(&hat, policy(false, true, true)).unwrap();
    delivery.deliver(&notice(&hat)).await.unwrap();
    let shown = phone.read(&setup.fake.sent()[0]);
    assert_eq!(
        (shown.title.as_str(), shown.body.as_str()),
        ("Session needs your answer", "Run cargo test")
    );
    // Muted: nothing at all.
    setup.hosts.set_push_policy(&hat, policy(true, false, false)).unwrap();
    assert!(delivery.deliver(&notice(&hat)).await.unwrap().is_empty());
    assert_eq!(setup.fake.sent().len(), 1);
    // Another hat's policy is its own: a hat with none set has the default.
    let HatChange::Done(work) = setup.hosts.create_hat("Work", None, unix_now()).unwrap() else {
        panic!("a hat");
    };
    delivery.deliver(&notice(&work.id)).await.unwrap();
    let shown = phone.read(&setup.fake.sent()[1]);
    assert_eq!(
        (shown.title.as_str(), shown.body.as_str()),
        ("Fix the flaky test", "needs your answer")
    );
}

#[tokio::test]
async fn a_subscription_the_service_says_is_gone_is_removed() {
    let setup = Setup::new("https://hennery.example");
    let (a, b, c) = (Browser::new(1), Browser::new(2), Browser::new(3));
    let gone = setup.subscribe("https://fcm.googleapis.com/fcm/send/gone", &a, None);
    let missing = setup.subscribe("https://fcm.googleapis.com/fcm/send/missing", &b, None);
    let fine = setup.subscribe("https://fcm.googleapis.com/fcm/send/fine", &c, None);
    setup.fake.answer(
        "https://fcm.googleapis.com/fcm/send/gone",
        &[Sent::Status {
            code: 410,
            retry_after: None,
        }],
    );
    setup.fake.answer(
        "https://fcm.googleapis.com/fcm/send/missing",
        &[Sent::Status {
            code: 404,
            retry_after: None,
        }],
    );
    let mut outcomes = setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
    outcomes.sort_by(|a, b| a.0.cmp(&b.0));
    let mut expected = vec![
        (gone, Outcome::Removed),
        (missing, Outcome::Removed),
        (fine.clone(), Outcome::Delivered),
    ];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(outcomes, expected);
    let left: Vec<String> = setup.hosts.subscriptions().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(left, [fine]);
}

#[tokio::test]
async fn a_busy_or_failing_service_is_tried_again_then_given_up() {
    let setup = Setup::new("https://hennery.example");
    let phone = Browser::new(7);
    let endpoint = "https://fcm.googleapis.com/fcm/send/phone";
    setup.subscribe(endpoint, &phone, None);
    let delivery = setup.delivery(at_once());
    // A 503 then a timeout, then taken: delivered on the third try.
    setup.fake.answer(
        endpoint,
        &[
            Sent::Status {
                code: 503,
                retry_after: None,
            },
            Sent::Timeout,
        ],
    );
    let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
    assert_eq!(outcomes[0].1, Outcome::Delivered);
    assert_eq!(setup.fake.sent().len(), 3);
    // Three failures: given up, the reason kept, without the endpoint.
    setup.fake.answer(
        endpoint,
        &[
            Sent::Status {
                code: 500,
                retry_after: None,
            },
            Sent::Failed,
            Sent::Status {
                code: 429,
                retry_after: None,
            },
        ],
    );
    let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
    assert!(
        matches!(&outcomes[0].1, Outcome::Failed(why) if why.contains("429")),
        "{outcomes:?}"
    );
    assert_eq!(setup.fake.sent().len(), 6);
    let stored = &setup.hosts.subscriptions().unwrap()[0];
    let error = stored.last_error.clone().unwrap();
    assert!(error.contains("429") && !error.contains("fcm"), "{error}");
    // A later success clears it.
    delivery.deliver(&notice(&setup.hat())).await.unwrap();
    assert_eq!(setup.hosts.subscriptions().unwrap()[0].last_error, None);
}

#[tokio::test]
async fn a_refusal_is_final() {
    let setup = Setup::new("https://hennery.example");
    let phone = Browser::new(7);
    let endpoint = "https://fcm.googleapis.com/fcm/send/phone";
    setup.subscribe(endpoint, &phone, None);
    let delivery = setup.delivery(at_once());
    setup.fake.answer(
        endpoint,
        &[Sent::Status {
            code: 403,
            retry_after: None,
        }],
    );
    let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
    assert!(matches!(&outcomes[0].1, Outcome::Failed(why) if why.contains("403")));
    assert_eq!(setup.fake.sent().len(), 1, "a 403 is not tried again");
    // The egress policy refusing the address: not tried again either, and
    // the subscription kept (a DNS answer can change).
    setup.fake.answer(endpoint, &[Sent::Refused]);
    let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
    assert!(matches!(&outcomes[0].1, Outcome::Failed(why) if why.contains("public address")));
    assert_eq!(setup.fake.sent().len(), 2);
    assert_eq!(setup.hosts.subscriptions().unwrap().len(), 1);
}

/// `Retry-After` is honoured, up to its cap; the clock is paused, so the
/// waits are counted, not slept.
#[tokio::test(start_paused = true)]
async fn retry_after_is_honoured_up_to_its_cap() {
    let setup = Setup::new("https://hennery.example");
    let phone = Browser::new(7);
    let endpoint = "https://fcm.googleapis.com/fcm/send/phone";
    setup.subscribe(endpoint, &phone, None);
    let delivery = setup.delivery(RetryPolicy {
        delays: vec![Duration::from_secs(1), Duration::from_secs(4), Duration::from_secs(4)],
        max_retry_after: Duration::from_secs(60),
        attempt_timeout: Duration::from_secs(10),
        budget: Duration::from_secs(3600),
    });
    setup.fake.answer(
        endpoint,
        &[
            Sent::Status {
                code: 429,
                retry_after: Some(Duration::ZERO),
            },
            Sent::Status {
                code: 429,
                retry_after: Some(Duration::from_secs(30)),
            },
            Sent::Status {
                code: 503,
                retry_after: Some(Duration::from_secs(3600)),
            },
        ],
    );
    let started = tokio::time::Instant::now();
    let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
    assert_eq!(outcomes[0].1, Outcome::Delivered);
    // The 1 s scheduled rather than the none asked, 30 s as asked, then
    // 60 s for the hour asked.
    assert_eq!(started.elapsed(), Duration::from_secs(91));
}

#[tokio::test]
async fn an_expired_subscription_is_removed_and_sent_nothing() {
    let setup = Setup::new("https://hennery.example");
    let phone = Browser::new(7);
    let laptop = Browser::new(8);
    setup.subscribe(
        "https://fcm.googleapis.com/fcm/send/phone",
        &phone,
        Some(unix_now() + 2),
    );
    let laptop_id = setup.subscribe("https://fcm.googleapis.com/fcm/send/laptop", &laptop, None);
    rusqlite::Connection::open(setup._dir.path().join("hennery.db"))
        .unwrap()
        .execute(
            "UPDATE push_subscriptions SET expires_at = 1 WHERE expires_at IS NOT NULL",
            [],
        )
        .unwrap();
    let outcomes = setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
    assert_eq!(outcomes, [(laptop_id, Outcome::Delivered)]);
    assert_eq!(setup.fake.sent().len(), 1);
    assert_eq!(setup.hosts.subscriptions().unwrap().len(), 1);
}

/// Plan 10a's re-confirmation, N1: with no contact and an `http`
/// `public_url`, the token names no subject.
#[tokio::test]
async fn without_a_contact_a_loopback_collector_names_no_subject() {
    let setup = Setup::new("http://localhost:7117");
    let phone = Browser::new(7);
    setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
    setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
    let claims = vapid_claims(header(&setup.fake.sent()[0], "Authorization").unwrap(), &setup.vapid);
    assert_eq!(claims.get("sub"), None);
    // An https one is named.
    let setup = Setup::new("https://hennery.example");
    setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
    setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
    let claims = vapid_claims(header(&setup.fake.sent()[0], "Authorization").unwrap(), &setup.vapid);
    assert_eq!(claims["sub"], json!("https://hennery.example"));
}

/// The token is made once per push service and reused (the
/// re-confirmation's N2), one per service.
#[tokio::test]
async fn a_token_is_reused_per_push_service() {
    let setup = Setup::new("https://hennery.example");
    setup.subscribe("https://fcm.googleapis.com/fcm/send/a", &Browser::new(1), None);
    setup.subscribe("https://fcm.googleapis.com/fcm/send/b", &Browser::new(2), None);
    setup.subscribe("https://web.push.apple.com/c", &Browser::new(3), None);
    let delivery = setup.delivery(at_once());
    delivery.deliver(&notice(&setup.hat())).await.unwrap();
    // A second later a fresh token would differ (its `exp`); the signature
    // is deterministic (RFC 6979), so only a later one tells reuse apart.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    delivery.deliver(&notice(&setup.hat())).await.unwrap();
    let mut by_service: HashMap<String, Vec<String>> = HashMap::new();
    for request in setup.fake.sent() {
        by_service
            .entry(request.endpoint.host_str().unwrap().into())
            .or_default()
            .push(header(&request, "Authorization").unwrap().into());
    }
    let fcm = &by_service["fcm.googleapis.com"];
    assert_eq!(fcm.len(), 4);
    assert!(fcm.iter().all(|h| h == &fcm[0]));
    let apple = &by_service["web.push.apple.com"];
    assert_ne!(apple[0], fcm[0]);
    assert_eq!(
        vapid_claims(&apple[0], &setup.vapid)["aud"],
        "https://web.push.apple.com"
    );
}

/// The queue's notices are delivered as they come.
#[tokio::test]
async fn delivery_drains_the_queue() {
    let setup = Setup::new("https://hennery.example");
    let phone = Browser::new(7);
    setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
    let (push, notices) = Push::new();
    tokio::spawn(setup.delivery(at_once()).run(notices));
    push.notify(notice(&setup.hat()));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while setup.fake.sent().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "nothing delivered");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(phone.read(&setup.fake.sent()[0]).tag, "s1");
}

/// 10b-ii's review, A1: notices go out one after another, so one slow or
/// silent push service must not hold up the others. A service asking to
/// wait an hour is waited for no longer than the cap, and one that never
/// answers is given up within the budget (its three timed-out tries and
/// their waits would outlast it), while a healthy one is delivered.
#[tokio::test(start_paused = true)]
async fn a_slow_or_silent_service_is_given_up_within_the_budget() {
    let setup = Setup::new("https://hennery.example");
    let busy = setup.subscribe("https://fcm.googleapis.com/fcm/send/busy", &Browser::new(1), None);
    let silent = setup.subscribe("https://fcm.googleapis.com/fcm/send/silent", &Browser::new(2), None);
    let fine = setup.subscribe("https://fcm.googleapis.com/fcm/send/fine", &Browser::new(3), None);
    let wait_an_hour = Sent::Status {
        code: 503,
        retry_after: Some(Duration::from_secs(3600)),
    };
    setup.fake.answer(
        "https://fcm.googleapis.com/fcm/send/busy",
        &[wait_an_hour.clone(), wait_an_hour.clone(), wait_an_hour],
    );
    setup.fake.silent("https://fcm.googleapis.com/fcm/send/silent");
    let policy = RetryPolicy::default();
    let budget = policy.budget;
    let delivery = setup.delivery(policy);
    let started = tokio::time::Instant::now();
    let mut outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
    assert!(started.elapsed() <= budget, "{:?}", started.elapsed());
    outcomes.sort_by(|a, b| a.0.cmp(&b.0));
    let of = |id: &str| outcomes.iter().find(|(i, _)| i == id).unwrap().1.clone();
    assert_eq!(of(&fine), Outcome::Delivered);
    assert!(matches!(of(&busy), Outcome::Failed(_)), "{outcomes:?}");
    assert!(
        matches!(of(&silent), Outcome::Failed(why) if why.contains("in time")),
        "{outcomes:?}"
    );
    // The next notice is not held up either.
    let started = tokio::time::Instant::now();
    delivery.deliver(&notice(&setup.hat())).await.unwrap();
    assert!(started.elapsed() <= budget);
}

/// 10b-ii's review, A2: a push task that fails is logged, and the other
/// subscriptions' outcomes are still recorded.
#[tokio::test]
async fn a_failed_push_task_does_not_lose_the_others() {
    let setup = Setup::new("https://hennery.example");
    setup.subscribe("https://fcm.googleapis.com/fcm/send/broken", &Browser::new(1), None);
    let fine = setup.subscribe("https://fcm.googleapis.com/fcm/send/fine", &Browser::new(2), None);
    setup.fake.panicking("https://fcm.googleapis.com/fcm/send/broken");
    let outcomes = setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
    assert_eq!(outcomes, vec![(fine, Outcome::Delivered)]);
}

/// The whole-branch review's must-fix 1: a browser rotates its subscription
/// (same id, a new endpoint) while a push to the old one is on its way. The
/// old endpoint's answer, whatever it is, is not recorded on the new one,
/// and a "gone" does not remove it.
#[tokio::test]
async fn an_answer_for_a_rotated_endpoint_leaves_the_new_one_alone() {
    let (old, new) = (
        "https://fcm.googleapis.com/fcm/send/old",
        "https://fcm.googleapis.com/fcm/send/new",
    );
    for code in [410, 403, 201] {
        let setup = Setup::new("https://hennery.example");
        let browser = Browser::new(1);
        let id = setup.subscribe(old, &browser, None);
        setup.fake.answer(
            old,
            &[Sent::Status {
                code,
                retry_after: None,
            }],
        );
        let (hosts, p256dh, auth) = (setup.hosts.clone(), browser.p256dh(), browser.auth());
        setup.fake.on_send(move || {
            let rotated = NewSubscription {
                endpoint: new,
                p256dh: &p256dh,
                auth: &auth,
                device_label: None,
                expires_at: None,
            };
            let replaced = hosts.rotate(old, &rotated, unix_now()).unwrap();
            assert!(matches!(replaced, Subscribed::Replaced(_)), "{replaced:?}");
        });
        setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
        let subs = setup.hosts.subscriptions().unwrap();
        assert_eq!(subs.len(), 1, "{code}: the rotated subscription is kept");
        let (sub_id, endpoint) = (&subs[0].id, &subs[0].endpoint);
        assert_eq!((sub_id, endpoint.as_str()), (&id, new), "{code}");
        assert_eq!(subs[0].last_error, None, "{code}");
        assert_eq!(subs[0].last_success_at, None, "{code}");
    }
}
