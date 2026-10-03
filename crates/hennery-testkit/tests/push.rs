//! Push over HTTP (kernel spec §6, §8; plan 10a): the VAPID key, the
//! owner's subscriptions, each hat's policy and the push contact in the
//! settings. Subscribing needs a fresh step-up (`step_up.rs` covers the
//! refusal).

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::push::MAX_SUBSCRIPTIONS;
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{
    ApiError, HatItem, PushKeys, PushPolicyItem, PushPolicyRequest, PushRotateRequest, PushSubscribeRequest,
    PushSubscriptionItem, PushUnsubscribeRequest, SettingsResponse, VapidKeyResponse,
};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::PUBLIC_URL;
use std::net::SocketAddr;

const P256DH: &str = "BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY";
const AUTH: &str = "_ordMnz7uTCmrpBTeUV4Bw";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Collector {
    /// A collector on one database file, set up.
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        hennery_testkit::operator_client(&state.operator);
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// A new signed-in client of the owner's, stepped up.
    fn client(&self) -> reqwest::Client {
        hennery_testkit::operator_client(&self.state.operator)
    }

    /// A client whose session's last password check was an hour ago.
    fn stale_client(&self) -> reqwest::Client {
        let phc = hennery_testkit::owner_phc(&self.state.operator);
        let token = self
            .state
            .operator
            .open_session("test", &phc, unix_now() - 3600)
            .unwrap()
            .unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("cookie", format!("hennery_session={token}").parse().unwrap());
        headers.insert("origin", PUBLIC_URL.parse().unwrap());
        reqwest::Client::builder().default_headers(headers).build().unwrap()
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self._dir.path().join("hennery.db")).unwrap()
    }

    async fn subscribe(&self, client: &reqwest::Client, endpoint: &str) -> reqwest::Response {
        client
            .post(self.url("/api/push/subscriptions"))
            .json(&subscription(endpoint))
            .send()
            .await
            .unwrap()
    }

    async fn subscriptions(&self, client: &reqwest::Client) -> Vec<PushSubscriptionItem> {
        let resp = client.get(self.url("/api/push/subscriptions")).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        resp.json().await.unwrap()
    }
}

fn subscription(endpoint: &str) -> PushSubscribeRequest {
    PushSubscribeRequest {
        endpoint: endpoint.into(),
        expiration_time: None,
        keys: PushKeys {
            p256dh: P256DH.into(),
            auth: AUTH.into(),
        },
        device_label: None,
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap().code)
}

#[tokio::test]
async fn the_vapid_key_is_the_collectors() {
    let c = Collector::start().await;
    let resp = c.client().get(c.url("/api/push/vapid")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let key: VapidKeyResponse = resp.json().await.unwrap();
    assert_eq!(key.public_key, c.state.vapid.public_key());
}

#[tokio::test]
async fn every_push_route_needs_the_owners_session() {
    let c = Collector::start().await;
    let anonymous = reqwest::Client::new();
    for (method, path) in [
        ("GET", "/api/push/vapid"),
        ("GET", "/api/push/subscriptions"),
        ("POST", "/api/push/subscriptions"),
        ("DELETE", "/api/push/subscriptions"),
        ("POST", "/api/push/subscriptions/rotate"),
        ("DELETE", "/api/push/subscriptions/push-1"),
        ("GET", "/api/push/policies"),
        ("PUT", "/api/push/policies/hat-1"),
        ("GET", "/api/settings"),
        ("PATCH", "/api/settings"),
    ] {
        let resp = anonymous
            .request(method.parse().unwrap(), c.url(path))
            .header("origin", PUBLIC_URL)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401, "{method} {path}");
    }
}

#[tokio::test]
async fn a_browser_subscribes_and_sees_itself_as_this_device() {
    let c = Collector::start().await;
    let phone = c.client();
    let laptop = c.client();
    let resp = c.subscribe(&phone, "https://web.push.apple.com/QAbc").await;
    assert_eq!(resp.status(), 201);
    let body = resp.text().await.unwrap();
    // The endpoint is a capability URL: never sent back.
    assert!(!body.contains("QAbc"), "{body}");
    let created: PushSubscriptionItem = serde_json::from_str(&body).unwrap();
    assert_eq!(created.endpoint_host, "web.push.apple.com");
    assert_eq!(created.device_label, "web.push.apple.com");
    assert!(created.this_device);
    assert!(!created.signed_out);
    assert_eq!(created.last_success_at, None);
    assert_eq!(c.subscriptions(&phone).await, std::slice::from_ref(&created));
    let seen_elsewhere = c.subscriptions(&laptop).await;
    assert!(!seen_elsewhere[0].this_device);

    // The same endpoint again, from the laptop's session: replaced, and
    // now the laptop's.
    let mut again = subscription("https://web.push.apple.com/QAbc");
    again.device_label = Some("My phone".into());
    let expires_at = unix_now() + 3600;
    again.expiration_time = Some(expires_at * 1000 + 999);
    let resp = laptop
        .post(c.url("/api/push/subscriptions"))
        .json(&again)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let replaced: PushSubscriptionItem = resp.json().await.unwrap();
    assert_eq!(
        (replaced.id.as_str(), replaced.device_label.as_str()),
        (created.id.as_str(), "My phone")
    );
    assert!(replaced.this_device);
    let stored = c.state.hosts.subscriptions().unwrap();
    assert_eq!(stored[0].expires_at, Some(expires_at));
}

#[tokio::test]
async fn a_bad_subscription_is_refused() {
    let c = Collector::start().await;
    let client = c.client();
    let mut short_auth = subscription("https://fcm.googleapis.com/a");
    short_auth.keys.auth = "AAAA".into();
    let mut expired = subscription("https://fcm.googleapis.com/b");
    expired.expiration_time = Some(1000);
    for refused in [
        subscription("https://169.254.169.254/latest"),
        subscription("http://fcm.googleapis.com/a"),
        short_auth,
        expired,
    ] {
        let resp = client
            .post(c.url("/api/push/subscriptions"))
            .json(&refused)
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(resp).await, (400, "invalid".into()), "{refused:?}");
    }
    assert!(c.state.hosts.subscriptions().unwrap().is_empty());
}

#[tokio::test]
async fn one_subscription_too_many_is_a_conflict() {
    let c = Collector::start().await;
    let client = c.client();
    for n in 0..MAX_SUBSCRIPTIONS {
        let endpoint = format!("https://fcm.googleapis.com/fcm/send/{n}");
        assert_eq!(c.subscribe(&client, &endpoint).await.status(), 201);
    }
    let resp = c.subscribe(&client, "https://fcm.googleapis.com/fcm/send/more").await;
    assert_eq!(code_of(resp).await, (409, "too_many_subscriptions".into()));
}

#[tokio::test]
async fn a_subscription_is_removed_by_its_browser_or_from_settings() {
    let c = Collector::start().await;
    let client = c.client();
    let created: PushSubscriptionItem = c
        .subscribe(&client, "https://fcm.googleapis.com/a")
        .await
        .json()
        .await
        .unwrap();
    c.subscribe(&client, "https://fcm.googleapis.com/b").await;

    let by_id = format!("/api/push/subscriptions/{}", created.id);
    assert_eq!(client.delete(c.url(&by_id)).send().await.unwrap().status(), 204);
    assert_eq!(
        code_of(client.delete(c.url(&by_id)).send().await.unwrap()).await,
        (404, "not_found".into())
    );
    let mine = PushUnsubscribeRequest {
        endpoint: "https://fcm.googleapis.com/b".into(),
    };
    let unsubscribe = || client.delete(c.url("/api/push/subscriptions")).json(&mine).send();
    assert_eq!(unsubscribe().await.unwrap().status(), 204);
    assert_eq!(code_of(unsubscribe().await.unwrap()).await, (404, "not_found".into()));
    assert!(c.subscriptions(&client).await.is_empty());
}

/// Plan 10a decision 4, through the routes: signing out ends that
/// browser's subscription, not another's.
#[tokio::test]
async fn signing_out_ends_the_browsers_subscription() {
    let c = Collector::start().await;
    let phone = c.client();
    let laptop = c.client();
    c.subscribe(&phone, "https://web.push.apple.com/phone").await;
    c.subscribe(&laptop, "https://fcm.googleapis.com/laptop").await;
    let resp = phone.post(c.url("/api/auth/logout")).send().await.unwrap();
    assert_eq!(resp.status(), 204);
    let left = c.subscriptions(&laptop).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].endpoint_host, "fcm.googleapis.com");
}

#[tokio::test]
async fn each_hat_has_a_policy_that_can_be_set() {
    let c = Collector::start().await;
    let client = c.client();
    let hats: Vec<HatItem> = client
        .get(c.url("/api/hats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let hat_id = hats[0].id.clone();
    let policies: Vec<PushPolicyItem> = client
        .get(c.url("/api/push/policies"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        policies,
        [PushPolicyItem {
            hat_id: hat_id.clone(),
            muted: false,
            details: false,
            generic_title: false,
        }]
    );
    let quiet = PushPolicyRequest {
        muted: true,
        details: false,
        generic_title: true,
    };
    let resp = client
        .put(c.url(&format!("/api/push/policies/{hat_id}")))
        .json(&quiet)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let set: PushPolicyItem = resp.json().await.unwrap();
    assert_eq!((set.muted, set.details, set.generic_title), (true, false, true));
    assert!(c.state.hosts.push_policy(&hat_id).unwrap().muted);

    let resp = client
        .put(c.url("/api/push/policies/hat-nope"))
        .json(&quiet)
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (404, "not_found".into()));
}

#[tokio::test]
async fn the_push_contact_is_set_in_the_settings() {
    let c = Collector::start().await;
    let client = c.client();
    let get = || async {
        let resp = client.get(c.url("/api/settings")).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        resp.json::<SettingsResponse>().await.unwrap()
    };
    assert_eq!(
        get().await,
        SettingsResponse {
            public_url: PUBLIC_URL.into(),
            contact: None,
            deployment_warning: false,
        }
    );
    let patch = |body: serde_json::Value| client.patch(c.url("/api/settings")).json(&body).send();
    let resp = patch(serde_json::json!({ "contact": " me@example.com " }))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<SettingsResponse>().await.unwrap().contact.as_deref(),
        Some("me@example.com")
    );
    let resp = patch(serde_json::json!({ "contact": "me@example.com?subject=x" }))
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (400, "invalid".into()));
    // Anything else is refused (`public_url` is `public_url.rs`'s).
    let resp = patch(serde_json::json!({ "name": "x" })).await.unwrap();
    assert_eq!(code_of(resp).await, (422, "invalid_body".into()));
    // Nothing in the body changes nothing.
    assert_eq!(patch(serde_json::json!({})).await.unwrap().status(), 200);
    assert_eq!(get().await.contact.as_deref(), Some("me@example.com"));
    // Empty clears it.
    let resp = patch(serde_json::json!({ "contact": "" })).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(get().await.contact, None);
}

/// The review's A1: a push service rotating the subscription, reported by
/// the service worker with no page to step up on.
#[tokio::test]
async fn a_rotation_needs_no_step_up_but_only_moves_the_owners_own() {
    let c = Collector::start().await;
    let created: PushSubscriptionItem = c
        .subscribe(&c.client(), "https://updates.push.services.mozilla.com/wpush/v2/old")
        .await
        .json()
        .await
        .unwrap();
    let stale = c.stale_client();
    let rotate = |old: &str| {
        stale
            .post(c.url("/api/push/subscriptions/rotate"))
            .json(&PushRotateRequest {
                old_endpoint: old.into(),
                subscription: subscription("https://updates.push.services.mozilla.com/wpush/v2/new"),
            })
            .send()
    };
    let resp = rotate("https://updates.push.services.mozilla.com/wpush/v2/old")
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let rotated: PushSubscriptionItem = resp.json().await.unwrap();
    assert_eq!(rotated.id, created.id);
    // Still the subscribing session's, not the rotating one's.
    assert!(!rotated.this_device);
    assert_eq!(
        c.state.hosts.subscriptions().unwrap()[0].endpoint,
        "https://updates.push.services.mozilla.com/wpush/v2/new"
    );
    // Its age and label are kept; the old endpoint is gone.
    assert_eq!(
        (rotated.created_at.as_str(), rotated.device_label.as_str()),
        (created.created_at.as_str(), created.device_label.as_str())
    );
    let gone = PushUnsubscribeRequest {
        endpoint: "https://updates.push.services.mozilla.com/wpush/v2/old".into(),
    };
    let resp = stale
        .delete(c.url("/api/push/subscriptions"))
        .json(&gone)
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (404, "not_found".into()));
    let resp = rotate("https://updates.push.services.mozilla.com/wpush/v2/unknown")
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (404, "not_found".into()));
    // From another origin, refused like any other change: it is the one
    // push route with no step-up.
    let resp = stale
        .post(c.url("/api/push/subscriptions/rotate"))
        .header("origin", "https://elsewhere.example")
        .json(&PushRotateRequest {
            old_endpoint: "https://updates.push.services.mozilla.com/wpush/v2/new".into(),
            subscription: subscription("https://updates.push.services.mozilla.com/wpush/v2/newer"),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()));
    // Subscribing afresh still needs the step-up.
    let resp = c.subscribe(&stale, "https://fcm.googleapis.com/fcm/send/new").await;
    assert_eq!(code_of(resp).await, (403, "step_up_required".into()));
}

/// O1 of the review: a device whose session ended still receives until it
/// is removed, and the list says so.
#[tokio::test]
async fn a_subscription_whose_session_ended_is_listed_as_signed_out() {
    let c = Collector::start().await;
    let laptop = c.client();
    c.subscribe(&laptop, "https://fcm.googleapis.com/laptop").await;
    // That session, and only it, expired (a revoke would have removed the
    // subscription).
    let auth_session = c.state.hosts.subscriptions().unwrap()[0].auth_session.clone();
    c.db()
        .execute(
            "UPDATE auth_sessions SET expires_at = 0 WHERE id_hash = ?1",
            [&auth_session],
        )
        .unwrap();
    let phone = c.client();
    let listed = c.subscriptions(&phone).await;
    assert!(listed[0].signed_out);
    assert!(!listed[0].this_device);
}

/// The review's A7: one browser, one endpoint, one account.
#[tokio::test]
async fn an_endpoint_another_account_has_is_a_conflict() {
    let c = Collector::start().await;
    c.db()
        .execute_batch(&format!(
            "INSERT INTO owners(id, created_at) VALUES ('owner-other', {later});
             INSERT INTO push_subscriptions(id, owner_id, endpoint, p256dh, auth, device_label, auth_session, created_at)
                 VALUES ('push-theirs', 'owner-other', 'https://fcm.googleapis.com/theirs', '{P256DH}', '{AUTH}', 'x',
                         's', 0);",
            later = unix_now() + 1_000_000
        ))
        .unwrap();
    let client = c.client();
    let resp = c.subscribe(&client, "https://fcm.googleapis.com/theirs").await;
    assert_eq!(code_of(resp).await, (409, "endpoint_taken".into()));
    // Nor can it be rotated onto, or from.
    c.subscribe(&client, "https://fcm.googleapis.com/mine").await;
    let rotate = |old: &str, new: &str| {
        client
            .post(c.url("/api/push/subscriptions/rotate"))
            .json(&PushRotateRequest {
                old_endpoint: old.into(),
                subscription: subscription(new),
            })
            .send()
    };
    let resp = rotate("https://fcm.googleapis.com/mine", "https://fcm.googleapis.com/theirs")
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (409, "endpoint_taken".into()));
    let resp = rotate("https://fcm.googleapis.com/theirs", "https://fcm.googleapis.com/x")
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (404, "not_found".into()));
}

/// Task 2's review: the body a browser sends, as raw JSON
/// (`PushSubscription.toJSON()`), not through the Rust type, so the field
/// names are the browser's.
#[tokio::test]
async fn a_browsers_own_subscription_json_is_accepted() {
    let c = Collector::start().await;
    let client = c.client();
    let expires_ms = (unix_now() + 3600) * 1000;
    for (endpoint, expiration) in [
        ("https://fcm.googleapis.com/fcm/send/raw1", serde_json::Value::Null),
        (
            "https://fcm.googleapis.com/fcm/send/raw2",
            serde_json::json!(expires_ms),
        ),
    ] {
        let body = serde_json::json!({
            "endpoint": endpoint,
            "expirationTime": expiration,
            "keys": { "p256dh": P256DH, "auth": AUTH },
        });
        let resp = client
            .post(c.url("/api/push/subscriptions"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201, "{body}");
    }
    // Made in the same second: listed by id, so sorted here.
    let mut stored = c.state.hosts.subscriptions().unwrap();
    stored.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
    assert_eq!(
        stored.iter().map(|s| s.expires_at).collect::<Vec<_>>(),
        [None, Some(expires_ms / 1000)]
    );
}

/// The review's A8: the push routes read 16 KiB at most.
#[tokio::test]
async fn a_large_body_is_refused_before_it_is_read() {
    let c = Collector::start().await;
    let mut big = subscription("https://fcm.googleapis.com/a");
    big.device_label = Some("x".repeat(hennery_kernel::auth_api::MAX_BODY_BYTES));
    let resp = c
        .client()
        .post(c.url("/api/push/subscriptions"))
        .json(&big)
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (413, "body_too_large".into()));
}
