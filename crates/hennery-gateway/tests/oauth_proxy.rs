//! The proxy's OAuth path (gateway spec §4.4, §4.5, §5.4, §11; plan 8f):
//! an upstream 401 is one single-flight refresh and one retry; concurrent
//! 401s share one refresh, so a rotating refresh token with reuse detection
//! survives; a refresh runs to completion whatever the caller does; and each
//! refresh outcome answers its own way. Against the fake authorization
//! server, whose `/mcp` takes only its live access tokens.

mod support;

use hennery_gateway::model::{CredKind, Status};
use hennery_gateway::notify::Alert;
use hennery_gateway::proxy::Limits;
use hennery_gateway::store::RefreshedGrant;
use hennery_kernel::secret::unix_now;
use std::time::Duration;
use support::oauth::{Config, FakeAs, seed_grant};
use support::upstream::{Harness, list};

/// A harness, a fake, an OAuth connection mounted on `host-a` with a grant
/// from the fake, and a session token of that host.
async fn setup(config: Config, expires_at: Option<i64>) -> (Harness, FakeAs, String, String) {
    setup_with(config, expires_at, |r| r).await
}

async fn setup_with(
    config: Config,
    expires_at: Option<i64>,
    adjust: impl FnOnce(hennery_gateway::runtime::Runtime) -> hennery_gateway::runtime::Runtime,
) -> (Harness, FakeAs, String, String) {
    let h = Harness::with(
        Limits::new(64, 8, Duration::from_secs(10), Duration::from_secs(2)),
        adjust,
    )
    .await;
    let fake = FakeAs::start(config).await;
    h.host("host-a", 1);
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    h.mount(&id, &["host-a"]);
    let (access, refresh) = fake.issue();
    seed_grant(&h, &id, &fake, &access, Some(&refresh), expires_at);
    let token = h.mint("sess-1", "host-a", &h.hat());
    (h, fake, id, token)
}

fn grant(h: &Harness, id: &str) -> hennery_gateway::model::OauthCredential {
    h.store.oauth_credential(id, &h.key).unwrap().unwrap()
}

/// Requests that reached the token endpoint, held at its gate or not.
fn token_requests(fake: &FakeAs) -> usize {
    fake.paths().iter().filter(|p| *p == "/token").count()
}

/// Wait until `check` holds, at most 10 s: a positive signal, never a
/// sleep (fleet rules: Linux races).
async fn until(mut check: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never");
}

/// A live grant: the request goes with its access token, no refresh.
#[tokio::test]
async fn a_live_grant_is_sent_as_a_bearer_token() {
    let (h, fake, id, token) = setup(Config::default(), None).await;
    let url = h.store.connection(&id).unwrap().unwrap().url;
    h.proxy_store
        .record_status(&id, &url, Status::Error, Some("down"), 1)
        .unwrap();
    let response = h.post("linear", &token, &list(1)).await;
    assert_eq!(response.status(), 200);
    assert_eq!(fake.with(|r| r.refreshes), 0);
    assert_eq!(h.status(&id), "ok", "live traffic sets ok");
    assert_eq!(h.alerts.alerts(), [(id, Alert::Recovered)]);
}

/// Gateway spec §5.4: a 401 is a refresh and one retry; the refresh sends
/// `resource` and the grant's scopes (G-10); the new tokens are stored.
#[tokio::test]
async fn a_401_is_refreshed_and_retried_once() {
    let (h, fake, id, token) = setup(Config::default(), None).await;
    let before = grant(&h, &id);
    fake.expire_access();
    let response = h.post("linear", &token, &list(1)).await;
    assert_eq!(response.status(), 200, "{:?}", response.text().await);
    assert_eq!(fake.with(|r| r.refreshes), 1);
    assert_eq!(
        fake.with(|r| r.token_resources.clone()),
        [("refresh_token".to_string(), Some(fake.mcp_url()))]
    );
    assert_eq!(
        fake.with(|r| r.refresh_scopes.clone()),
        [Some("read write".to_string())]
    );
    let after = grant(&h, &id);
    assert_ne!(after.tokens.access_token, before.tokens.access_token);
    assert_ne!(after.tokens.refresh_token, before.tokens.refresh_token, "rotated");
    assert!(fake.is_live(&after.tokens.access_token));
}

/// Gateway spec §5.4: a second 401 after the refresh is `needs_auth` and
/// 502 `upstream_auth`, and the `Notifier` hears of it.
#[tokio::test]
async fn a_second_401_is_needs_auth() {
    let (h, fake, id, token) = setup(
        Config {
            mcp_status: Some(401),
            ..Config::default()
        },
        None,
    )
    .await;
    let response = h.post("linear", &token, &list(1)).await;
    assert_eq!(response.status(), 502);
    assert!(response.headers().get("www-authenticate").is_none());
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "upstream_auth");
    assert_eq!(fake.with(|r| r.refreshes), 1, "one refresh, one retry");
    assert_eq!(h.status(&id), "needs_auth");
    assert_eq!(h.alerts.alerts(), [(id, Alert::NeedsAuth)]);
}

/// Gateway spec §4.5 and §11: concurrent 401s share one refresh, so a
/// rotating refresh token with reuse detection is never presented twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_401s_share_one_refresh() {
    let (h, fake, id, token) = setup(Config::default(), None).await;
    fake.expire_access();
    let mut calls = Vec::new();
    for i in 0..8 {
        let h = &h;
        let token = token.clone();
        calls.push(async move { h.post("linear", &token, &list(i)).await.status() });
    }
    let statuses = futures::future::join_all(calls).await;
    assert!(statuses.iter().all(|s| *s == 200), "{statuses:?}");
    assert_eq!(fake.with(|r| r.refreshes), 1);
    assert!(!fake.with(|r| r.reuse_detected));
    assert_eq!(h.status(&id), "ok");
}

/// Gateway spec §4.4: a refresh without a new refresh token keeps the old
/// one, and the grant goes on refreshing with it.
#[tokio::test]
async fn a_refresh_without_a_new_refresh_token_keeps_the_old_one() {
    let (h, fake, id, token) = setup(
        Config {
            refresh_keeps_token: true,
            ..Config::default()
        },
        None,
    )
    .await;
    let before = grant(&h, &id);
    fake.expire_access();
    assert_eq!(h.post("linear", &token, &list(1)).await.status(), 200);
    let after = grant(&h, &id);
    assert_eq!(after.tokens.refresh_token, before.tokens.refresh_token);
    fake.expire_access();
    assert_eq!(h.post("linear", &token, &list(2)).await.status(), 200);
    assert_eq!(fake.with(|r| r.refreshes), 2);
}

/// Gateway spec §4.5: the vendor's refusal of a refresh is `needs_auth`.
#[tokio::test]
async fn a_refused_refresh_is_needs_auth() {
    let (h, fake, id, token) = setup(Config::default(), None).await;
    // The vendor revoked the grant: its refresh token no longer works.
    let stale = grant(&h, &id);
    fake.expire_access();
    assert_eq!(h.post("linear", &token, &list(1)).await.status(), 200);
    // Presenting the spent one again is reuse: the family is revoked.
    let refreshed = RefreshedGrant {
        tokens: &stale.tokens,
        expires_at: None,
        resource_param_accepted: true,
    };
    h.store
        .store_refreshed(&id, &grant(&h, &id), &refreshed, &h.key, unix_now())
        .unwrap();
    fake.expire_access();
    let response = h.post("linear", &token, &list(2)).await;
    assert_eq!(response.status(), 502);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "upstream_auth");
    assert!(fake.with(|r| r.reuse_detected));
    assert_eq!(h.status(&id), "needs_auth");
}

/// Gateway spec §4.5: a token endpoint that does not answer in time is no
/// reason to sign in again: 502 `upstream_unreachable`, status untouched.
#[tokio::test]
async fn an_unreachable_token_endpoint_is_not_needs_auth() {
    let (h, fake, id, token) = setup_with(
        Config {
            gate_token: true,
            ..Config::default()
        },
        None,
        |r| r.with_refresh_timeout(Duration::from_millis(300)),
    )
    .await;
    fake.expire_access();
    let started = std::time::Instant::now();
    let response = h.post("linear", &token, &list(1)).await;
    // The refresh's own bound ended it, not a request's 15 s timeout.
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(response.status(), 502);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "upstream_unreachable");
    assert_ne!(h.status(&id), "needs_auth");
}

/// Gateway spec §4.4, §4.5: concurrent requests with a token about to
/// expire refresh it once; the ones that waited for the lock find it fresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_near_expiry_refresh_once() {
    let (h, fake, id, token) = setup(Config::default(), Some(unix_now() + 60)).await;
    let mut calls = Vec::new();
    for i in 0..8 {
        let h = &h;
        let token = token.clone();
        calls.push(async move { h.post("linear", &token, &list(i)).await.status() });
    }
    let statuses = futures::future::join_all(calls).await;
    assert!(statuses.iter().all(|s| *s == 200), "{statuses:?}");
    assert_eq!(fake.with(|r| r.refreshes), 1);
    assert!(!fake.with(|r| r.reuse_detected));
    assert!(fake.is_live(&grant(&h, &id).tokens.access_token));
}

/// Gateway spec §4.5: a refresh the vendor answered but that could not be
/// stored is 502 `credential_unsaved`, without `needs_auth`.
#[tokio::test]
async fn an_unsaved_refresh_is_not_needs_auth() {
    let (h, fake, id, token) = setup(Config::default(), None).await;
    h.raw()
        .execute_batch(
            "CREATE TRIGGER no_refresh BEFORE UPDATE ON gw_credentials
             BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
        )
        .unwrap();
    fake.expire_access();
    let response = h.post("linear", &token, &list(1)).await;
    assert_eq!(response.status(), 502);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "credential_unsaved");
    assert_ne!(h.status(&id), "needs_auth");
}

/// Gateway spec §4.5 and §11 (G-11, G-12): a caller that goes away
/// mid-refresh leaves the refresh running; it completes, persists the
/// rotated grant, and sets no `needs_auth`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_cancelled_mid_refresh_leaves_it_to_complete() {
    let (h, fake, id, token) = setup(
        Config {
            gate_token: true,
            ..Config::default()
        },
        None,
    )
    .await;
    let before = grant(&h, &id);
    fake.expire_access();
    let body = list(1);
    let request = h.post("linear", &token, &body);
    // The refresh has reached the token endpoint, and waits there.
    let waiting = async {
        until(|| token_requests(&fake) == 1).await;
    };
    tokio::select! {
        _ = request => panic!("the request answered while the refresh was held"),
        () = waiting => {}
    }
    // The caller went away (its future dropped); the vendor answers now.
    fake.open_gate();
    until(|| grant(&h, &id).tokens.refresh_token != before.tokens.refresh_token).await;
    assert!(fake.is_live(&grant(&h, &id).tokens.access_token));
    assert_ne!(h.status(&id), "needs_auth");
}

/// Gateway spec §4.4: an access token within 5 minutes of its expiry is
/// refreshed before it is used, single-flight.
#[tokio::test]
async fn a_token_about_to_expire_is_refreshed_first() {
    let (h, fake, id, token) = setup(Config::default(), Some(unix_now() + 60)).await;
    let before = grant(&h, &id);
    assert_eq!(h.post("linear", &token, &list(1)).await.status(), 200);
    assert_eq!(fake.with(|r| r.refreshes), 1);
    let unauthorized = fake.paths().iter().filter(|p| *p == "/mcp").count();
    assert_eq!(unauthorized, 1, "one request, sent with the fresh token");
    assert_ne!(grant(&h, &id).tokens.access_token, before.tokens.access_token);
    // Not again: the new one is good for an hour.
    assert_eq!(h.post("linear", &token, &list(2)).await.status(), 200);
    assert_eq!(fake.with(|r| r.refreshes), 1);
}

/// Gateway spec §5.4: OAuth without a refresh token answers a 401 with
/// `needs_auth` and 502, and sends no refresh.
#[tokio::test]
async fn a_grant_without_a_refresh_token_is_needs_auth_on_401() {
    let (h, fake, id, token) = setup(Config::default(), None).await;
    let (access, _) = fake.issue();
    seed_grant(&h, &id, &fake, &access, None, None);
    fake.expire_access();
    let response = h.post("linear", &token, &list(1)).await;
    assert_eq!(response.status(), 502);
    assert_eq!(fake.with(|r| r.refreshes), 0);
    assert_eq!(h.status(&id), "needs_auth");
}

/// The review's decision 9 (G-13, G-14): a refresh that finishes after an
/// edit deleted the grant never writes it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_refresh_never_resurrects_a_deleted_grant() {
    let (h, fake, id, token) = setup(
        Config {
            gate_token: true,
            ..Config::default()
        },
        None,
    )
    .await;
    fake.expire_access();
    let request = tokio::spawn({
        let client = h.client.clone();
        let url = h.url("linear");
        let token = token.clone();
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(list(1).to_string())
                .send()
                .await
        }
    });
    until(|| token_requests(&fake) == 1).await;
    // An edit to another origin deletes the grant, in its own transaction
    // (the API would wait for the lock; the store's compare-and-swap is
    // what stops a writer that cannot).
    h.store
        .update(
            &id,
            &hennery_gateway::model::ConnectionPatch {
                url: Some("https://elsewhere.example/mcp".into()),
                ..Default::default()
            },
            unix_now(),
        )
        .unwrap();
    fake.open_gate();
    let _ = request.await;
    assert!(h.store.oauth_credential(&id, &h.key).unwrap().is_none());
    assert!(!h.store.connection(&id).unwrap().unwrap().has_credential);
    assert_eq!(Status::parse(&h.status(&id)), Some(Status::NotConnected));
}

/// The 8d seam's every outcome, by what the proxy answered: retried (200),
/// not refreshable and refused (502 `upstream_auth`, `needs_auth`),
/// unavailable (502 `upstream_unreachable`) and unsaved (502
/// `credential_unsaved`) are each pinned above; this pins that a static
/// connection's 401 is still `needs_auth` without a refresh.
#[tokio::test]
async fn a_static_connection_s_401_is_not_refreshed() {
    let (h, fake, _, token) = setup(Config::default(), None).await;
    let id = h.connection("static", &fake.mcp_url(), CredKind::Static);
    h.mount(&id, &["host-a"]);
    h.set_token(&id, "not-a-live-token");
    let response = h.post("static", &token, &list(1)).await;
    assert_eq!(response.status(), 502);
    assert_eq!(fake.with(|r| r.refreshes), 0);
    assert_eq!(h.status(&id), "needs_auth");
}

/// Gateway spec §4.3, §4.4: a refresh the server refuses for `resource`
/// is sent once more without it; that is recorded, and the next refresh
/// omits it.
#[tokio::test]
async fn a_refresh_is_retried_once_without_resource() {
    let (h, fake, id, token) = setup(
        Config {
            refuse_resource_at_token: true,
            ..Config::default()
        },
        None,
    )
    .await;
    fake.expire_access();
    assert_eq!(h.post("linear", &token, &list(1)).await.status(), 200);
    assert!(!grant(&h, &id).resource_param_accepted);
    fake.expire_access();
    assert_eq!(h.post("linear", &token, &list(2)).await.status(), 200);
    assert_eq!(
        fake.with(|r| r.token_resources.clone()),
        [
            ("refresh_token".to_string(), Some(fake.mcp_url())),
            ("refresh_token".to_string(), None),
            ("refresh_token".to_string(), None),
        ]
    );
}

/// The review's decision 9 (G-13): a refresh that finishes after a newer
/// grant was stored leaves the newer one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_refresh_never_overwrites_a_newer_grant() {
    let (h, fake, id, token) = setup(
        Config {
            gate_token: true,
            ..Config::default()
        },
        None,
    )
    .await;
    fake.expire_access();
    let request = tokio::spawn({
        let client = h.client.clone();
        let url = h.url("linear");
        let token = token.clone();
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(list(1).to_string())
                .send()
                .await
        }
    });
    until(|| token_requests(&fake) == 1).await;
    // A Connect completed meanwhile (without the lock: the store's
    // compare-and-swap is what stops the late write).
    seed_grant(&h, &id, &fake, "newer-access", Some("newer-refresh"), None);
    fake.open_gate();
    let _ = request.await;
    assert_eq!(grant(&h, &id).tokens.access_token.as_str(), "newer-access");
}

/// Gateway spec §4.6: a path-only edit keeps the grant, so a refresh that
/// finishes after one keeps the rotated token (else the spent one would be
/// all that is left).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_across_a_path_edit_keeps_the_rotated_token() {
    let (h, fake, id, token) = setup(
        Config {
            gate_token: true,
            ..Config::default()
        },
        None,
    )
    .await;
    let before = grant(&h, &id);
    fake.expire_access();
    let request = tokio::spawn({
        let client = h.client.clone();
        let url = h.url("linear");
        let token = token.clone();
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(list(1).to_string())
                .send()
                .await
        }
    });
    until(|| token_requests(&fake) == 1).await;
    h.store
        .update(
            &id,
            &hennery_gateway::model::ConnectionPatch {
                url: Some(format!("{}/mcp/", fake.origin())),
                ..Default::default()
            },
            unix_now(),
        )
        .unwrap();
    fake.open_gate();
    let _ = request.await;
    let after = grant(&h, &id);
    assert_ne!(after.tokens.refresh_token, before.tokens.refresh_token);
    assert!(fake.is_live(&after.tokens.access_token));
}

/// Gateway spec §4.5: a token endpoint answering 5xx is unavailable, not a
/// refusal.
#[tokio::test]
async fn a_failing_token_endpoint_is_not_needs_auth() {
    // A 5xx, and a 429 (rate limited: not the vendor refusing the grant).
    for status in [503, 429] {
        let (h, fake, id, token) = setup(
            Config {
                token_status: Some(status),
                ..Config::default()
            },
            None,
        )
        .await;
        fake.expire_access();
        let response = h.post("linear", &token, &list(1)).await;
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["code"], "upstream_unreachable", "{status}");
        assert_ne!(h.status(&id), "needs_auth", "{status}");
    }
}

/// Move `id` to fake `b`'s origin behind the API (its grant is deleted:
/// gateway spec §4.6), and give it a grant from `b`: what an origin edit
/// and a completed Connect leave while a refresh runs. `b`'s access token.
fn moved_to(h: &Harness, id: &str, b: &FakeAs) -> String {
    h.store
        .update(
            id,
            &hennery_gateway::model::ConnectionPatch {
                url: Some(b.mcp_url()),
                ..Default::default()
            },
            unix_now(),
        )
        .unwrap();
    let (access, refresh) = b.issue();
    seed_grant(h, id, b, &access, Some(&refresh), None);
    access
}

/// Gateway spec §4.5, §5.4: the retry after a 401 goes only where the
/// token it carries was granted for. A refresh that loses its
/// compare-and-swap reads the grant there is now; when that grant is for
/// another URL (an origin edit and a new Connect meanwhile), its token is
/// never sent to the old one: 502 `upstream_changed`, no `needs_auth`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_never_sends_another_url_s_token() {
    let (h, a, id, token) = setup(
        Config {
            gate_token: true,
            ..Config::default()
        },
        None,
    )
    .await;
    // Its tokens are named apart from `a`'s.
    let b = FakeAs::start(Config {
        prefix: "b-".into(),
        ..Config::default()
    })
    .await;
    a.expire_access();
    let request = tokio::spawn({
        let client = h.client.clone();
        let url = h.url("linear");
        let token = token.clone();
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(list(1).to_string())
                .send()
                .await
                .unwrap()
        }
    });
    // The 401's refresh is at `a`'s token endpoint, held there.
    until(|| token_requests(&a) == 1).await;
    let b_access = moved_to(&h, &id, &b);
    a.open_gate();
    let response = request.await.unwrap();
    assert_eq!(response.status(), 502);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "upstream_changed", "{body}");
    assert!(!a.with(|r| r.bearers.contains(&b_access)), "b's token went to a");
    assert_ne!(h.status(&id), "needs_auth");
}

/// The same for a refresh before use (gateway spec §4.4): the request goes
/// with the URL and the token read together after the refresh, never the
/// URL read before it with the token read after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_before_use_never_sends_another_url_s_token() {
    let (h, a, id, token) = setup(
        Config {
            gate_token: true,
            ..Config::default()
        },
        Some(unix_now() + 60),
    )
    .await;
    // Its tokens are named apart from `a`'s.
    let b = FakeAs::start(Config {
        prefix: "b-".into(),
        ..Config::default()
    })
    .await;
    let request = tokio::spawn({
        let client = h.client.clone();
        let url = h.url("linear");
        let token = token.clone();
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(list(1).to_string())
                .send()
                .await
                .unwrap()
        }
    });
    until(|| token_requests(&a) == 1).await;
    let b_access = moved_to(&h, &id, &b);
    a.open_gate();
    let response = request.await.unwrap();
    assert!(!a.with(|r| r.bearers.contains(&b_access)), "b's token went to a");
    // It went to `b`, with `b`'s token: what a request after the edit does.
    assert_eq!(response.status(), 200);
    assert_eq!(b.with(|r| r.bearers.clone()), [b_access]);
}

/// Start a request whose 401 refresh waits for the connection's lock,
/// which the test holds; `change` runs once the upstream has answered 401.
async fn while_a_401_waits(h: &Harness, a: &FakeAs, token: &str, change: impl FnOnce()) -> reqwest::Response {
    let id = h.store.list().unwrap()[0].id.clone();
    let held = h.runtime.lock(&id).await;
    a.expire_access();
    let request = tokio::spawn({
        let client = h.client.clone();
        let url = h.url("linear");
        let token = token.to_string();
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(list(1).to_string())
                .send()
                .await
                .unwrap()
        }
    });
    until(|| a.with(|r| !r.bearers.is_empty())).await;
    change();
    drop(held);
    request.await.unwrap()
}

/// A grant gone by the time the refresh holds the lock: nothing to
/// refresh, so the 401 stands: `needs_auth`, 502 `upstream_auth`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grant_gone_before_its_refresh_is_needs_auth() {
    let (h, a, id, token) = setup(Config::default(), None).await;
    let response = while_a_401_waits(&h, &a, &token, || {
        h.raw()
            .execute("DELETE FROM gw_credentials WHERE connection_id = ?1", [&id])
            .unwrap();
    })
    .await;
    assert_eq!(response.status(), 502);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "upstream_auth");
    assert_eq!(a.with(|r| r.refreshes), 0);
    assert_eq!(h.status(&id), "needs_auth");
}

/// A grant that no longer opens when the refresh reads it: the gateway's
/// failure, not the vendor's: 502 `credential_unsaved`, no `needs_auth`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grant_that_does_not_open_at_its_refresh_is_not_needs_auth() {
    let (h, a, id, token) = setup(Config::default(), None).await;
    let response = while_a_401_waits(&h, &a, &token, || {
        h.raw()
            .execute(
                "UPDATE gw_credentials SET ciphertext = x'00' WHERE connection_id = ?1",
                [&id],
            )
            .unwrap();
    })
    .await;
    assert_eq!(response.status(), 502);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "credential_unsaved");
    assert_eq!(a.with(|r| r.refreshes), 0);
    assert_ne!(h.status(&id), "needs_auth");
}

/// A grant due for refresh without a refresh token is used as it is, and
/// its requests do not queue on the connection's lock for a refresh that
/// cannot happen (gateway spec §4.4).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unrefreshable_grant_does_not_wait_for_the_lock() {
    let (h, a, id, token) = setup(Config::default(), None).await;
    let (access, _) = a.issue();
    seed_grant(&h, &id, &a, &access, None, Some(unix_now() + 60));
    let _held = h.runtime.lock(&id).await;
    let response = tokio::time::timeout(Duration::from_secs(5), h.post("linear", &token, &list(1)))
        .await
        .expect("the request waited for the lock");
    assert_eq!(response.status(), 200);
    assert_eq!(a.with(|r| r.refreshes), 0);
}

/// The same for the allowance (plan 8d decision 9): a connection unmarked
/// "internal network" and connected again while its refresh ran is not
/// retried with the internal network's client.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_keeps_the_allowance_it_was_read_with() {
    let (h, a, id, token) = setup(
        Config {
            gate_token: true,
            ..Config::default()
        },
        None,
    )
    .await;
    a.expire_access();
    let request = tokio::spawn({
        let client = h.client.clone();
        let url = h.url("linear");
        let token = token.clone();
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(list(1).to_string())
                .send()
                .await
                .unwrap()
        }
    });
    until(|| token_requests(&a) == 1).await;
    // The fake is on loopback, which the API keeps marked internal: the
    // test steps around that rule, as an edit to a public URL would not.
    let unmarked = h
        .raw()
        .execute("UPDATE gw_connections SET internal_network = 0 WHERE id = ?1", [&id])
        .unwrap();
    assert_eq!(unmarked, 1);
    let (access, refresh) = a.issue();
    seed_grant(&h, &id, &a, &access, Some(&refresh), None);
    a.open_gate();
    let response = request.await.unwrap();
    assert_eq!(response.status(), 502);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "upstream_changed", "{body}");
    assert_eq!(a.with(|r| r.bearers.len()), 1, "no retry");
}

/// Decision 13 on the path production takes: an origin edit and a new
/// Connect each take the connection's lock, so they land before the 401's
/// refresh holds it, never during. The refresh then finds another token
/// than the one that failed and sends nothing to the vendor; that token is
/// for another URL, so it is not retried: 502 `upstream_changed`, no
/// `needs_auth`, and the old upstream never sees it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_connected_elsewhere_before_the_refresh_is_not_retried() {
    let (h, a, id, token) = setup(Config::default(), None).await;
    // Its tokens are named apart from `a`'s.
    let b = FakeAs::start(Config {
        prefix: "b-".into(),
        ..Config::default()
    })
    .await;
    let mut b_access = String::new();
    let response = while_a_401_waits(&h, &a, &token, || b_access = moved_to(&h, &id, &b)).await;
    assert_eq!(response.status(), 502);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "upstream_changed", "{body}");
    assert_eq!(
        token_requests(&a),
        0,
        "the refresh sent nothing: another token was there"
    );
    assert!(!a.with(|r| r.bearers.contains(&b_access)), "b's token went to a");
    assert!(b.with(|r| r.bearers.is_empty()), "nothing was retried at b either");
    assert_ne!(h.status(&id), "needs_auth");
}
