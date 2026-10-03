//! The health probe and the `Notifier` (gateway spec §7; plan 8f): a real
//! MCP handshake through the proxy's forwarding path, each verdict's
//! status, the background loop's panic isolation and per-tick timeout, and
//! alerts on transitions only, a problem at startup announced once.

mod support;

use hennery_gateway::model::{CredKind, Status, StatusChange};
use hennery_gateway::notify::{self, Alert, ConnectionAlert, GENERIC_TITLE, Notifier};
use hennery_gateway::probe::{self, Verdict};
use hennery_gateway::proxy::Limits;
use hennery_kernel::push::Urgency;
use hennery_kernel::secret::unix_now;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use support::oauth::{Config, FakeAs, seed_grant};
use support::upstream::Harness;

async fn setup(config: Config) -> (Harness, FakeAs, String) {
    let h = Harness::with(Limits::default(), |r| r).await;
    let fake = FakeAs::start(config).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (access, refresh) = fake.issue();
    seed_grant(&h, &id, &fake, &access, Some(&refresh), None);
    (h, fake, id)
}

async fn probe(h: &Harness, id: &str) -> Verdict {
    let connection = h.proxy_store.connection_by_id(id).unwrap().unwrap();
    probe::probe_now(&h.runtime, &connection).await
}

fn set_status(h: &Harness, id: &str, status: Status) {
    let url = h.store.connection(id).unwrap().unwrap().url;
    h.proxy_store
        .record_status(id, &url, status, Some("before"), 1)
        .unwrap();
}

/// Gateway spec §7 (G-21): `initialize`, `notifications/initialized` and
/// `tools/list` on that session, then `DELETE`; all 2xx is `ok`.
#[tokio::test]
async fn a_healthy_connection_probes_ok_with_a_whole_handshake() {
    // A stateful server: `tools/list` without the session is 400 (G-21).
    let (h, fake, id) = setup(Config {
        stateful: true,
        ..Config::default()
    })
    .await;
    set_status(&h, &id, Status::Error);
    assert_eq!(probe(&h, &id).await, Verdict::Ok);
    let mcp: Vec<String> = fake
        .with(|r| r.requests.clone())
        .into_iter()
        .filter(|(_, path)| path == "/mcp")
        .map(|(method, _)| method)
        .collect();
    assert_eq!(mcp, ["POST", "POST", "POST", "DELETE"]);
    let record = h.store.connection(&id).unwrap().unwrap();
    assert_eq!(record.status, "ok");
    assert_eq!(record.status_note, None);
    assert!(record.checked_at.unwrap() >= unix_now() - 5);
    assert_eq!(h.alerts.alerts(), [(id, Alert::Recovered)]);
}

/// Gateway spec §7: a 401 after one single-flight refresh is `needs_auth`.
#[tokio::test]
async fn a_refused_credential_probes_needs_auth_after_one_refresh() {
    let (h, fake, id) = setup(Config {
        mcp_status: Some(401),
        ..Config::default()
    })
    .await;
    assert_eq!(probe(&h, &id).await, Verdict::NeedsAuth);
    assert_eq!(fake.with(|r| r.refreshes), 1);
    assert_eq!(h.status(&id), "needs_auth");
    assert!(h.store.connection(&id).unwrap().unwrap().checked_at.is_some());
    assert_eq!(h.alerts.alerts(), [(id, Alert::NeedsAuth)]);
}

/// Gateway spec §7: a 5xx is `error`, with a note, and its alert never asks
/// to reconnect (G-22).
#[tokio::test]
async fn a_failing_upstream_probes_error() {
    let (h, _fake, id) = setup(Config {
        mcp_status: Some(503),
        ..Config::default()
    })
    .await;
    set_status(&h, &id, Status::Ok);
    assert_eq!(
        probe(&h, &id).await,
        Verdict::Error("the upstream answered HTTP 503".into())
    );
    let record = h.store.connection(&id).unwrap().unwrap();
    assert_eq!(record.status, "error");
    assert_eq!(record.status_note.as_deref(), Some("the upstream answered HTTP 503"));
    assert_eq!(h.alerts.alerts(), [(id, Alert::Failing)]);
}

/// Gateway spec §7: a transport failure is `error`.
#[tokio::test]
async fn an_unreachable_upstream_probes_error() {
    let (h, fake, id) = setup(Config::default()).await;
    let url = fake.mcp_url();
    drop(fake);
    // The grant's URL is the fake's; nothing listens there now.
    assert_eq!(h.store.connection(&id).unwrap().unwrap().url, url);
    assert_eq!(
        probe(&h, &id).await,
        Verdict::Error("the upstream could not be reached".into())
    );
    assert_eq!(h.status(&id), "error");
}

/// Gateway spec §7: any other 4xx changes nothing, but `checked_at` moves.
#[tokio::test]
async fn another_4xx_changes_nothing() {
    let (h, _fake, id) = setup(Config {
        mcp_status: Some(404),
        ..Config::default()
    })
    .await;
    set_status(&h, &id, Status::Ok);
    let before = h.store.connection(&id).unwrap().unwrap();
    assert_eq!(probe(&h, &id).await, Verdict::NoChange);
    let after = h.store.connection(&id).unwrap().unwrap();
    assert_eq!((after.status.as_str(), after.status_at), ("ok", before.status_at));
    assert!(after.checked_at > before.checked_at);
    assert!(h.alerts.alerts().is_empty());
}

/// Each remaining verdict: a redirect is `error` (the proxy would answer
/// 502 too); a 401 whose refresh cannot reach the authorization server is
/// `error`, not `needs_auth`; a probe past its bound is `error`.
#[tokio::test]
async fn a_redirect_an_unreachable_refresh_or_a_timeout_probes_error() {
    let (h, _fake, id) = setup(Config {
        mcp_status: Some(302),
        ..Config::default()
    })
    .await;
    assert_eq!(
        probe(&h, &id).await,
        Verdict::Error("the upstream answered with a redirect".into())
    );
    let (h, fake, id) = setup(Config {
        token_status: Some(503),
        ..Config::default()
    })
    .await;
    fake.expire_access();
    assert_eq!(
        probe(&h, &id).await,
        Verdict::Error("the authorization server could not be reached to refresh the grant".into())
    );
    assert_eq!(h.status(&id), "error");
    let h = Harness::with(Limits::default(), |r| r.with_probe_timeout(Duration::from_millis(200))).await;
    let fake = FakeAs::start(Config {
        mcp_hang: true,
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (access, refresh) = fake.issue();
    seed_grant(&h, &id, &fake, &access, Some(&refresh), None);
    assert_eq!(probe(&h, &id).await, Verdict::Error("the probe timed out".into()));
    assert_eq!(h.status(&id), "error");
}

/// A connection without a credential sends nothing.
#[tokio::test]
async fn a_connection_without_a_credential_is_not_probed() {
    let h = Harness::with(Limits::default(), |r| r).await;
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    assert_eq!(probe(&h, &id).await, Verdict::NoCredential);
    assert!(fake.paths().is_empty());
}

/// Gateway spec §7: the background probe takes OAuth connections with a
/// grant only.
#[tokio::test]
async fn the_background_probe_takes_oauth_connections_with_a_grant_only() {
    let (h, fake, id) = setup(Config::default()).await;
    let bare = h.connection("bare", &fake.mcp_url(), CredKind::OauthClient);
    let fixed = h.connection("fixed", &fake.mcp_url(), CredKind::Static);
    h.set_token(&fixed, "tok");
    let public = h.connection("public", &fake.mcp_url(), CredKind::None);
    let targets: Vec<String> = h
        .proxy_store
        .probe_targets()
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(targets, std::slice::from_ref(&id));
    for other in [bare, fixed, public] {
        assert!(!targets.contains(&other));
    }
    probe::tick(&h.runtime).await;
    assert_eq!(h.status(&id), "ok");
}

/// api-8e-8f B6: single-flight, and within 10 s a completed probe's result
/// stands without a new request.
#[tokio::test]
async fn a_probe_within_ten_seconds_reuses_the_last() {
    let (h, fake, id) = setup(Config::default()).await;
    assert_eq!(probe(&h, &id).await, Verdict::Ok);
    let sent = fake.paths().len();
    let (a, b) = tokio::join!(probe(&h, &id), probe(&h, &id));
    assert_eq!((a, b), (Verdict::Ok, Verdict::Ok));
    assert_eq!(fake.paths().len(), sent, "no new request");
}

/// Gateway spec §7: a tick that hangs past its timeout is abandoned, and
/// the loop goes on to the next.
#[tokio::test]
async fn a_hung_tick_is_abandoned_and_the_loop_goes_on() {
    let (h, fake, _id) = setup(Config {
        mcp_hang: true,
        ..Config::default()
    })
    .await;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let loop_task = tokio::spawn(probe::run(
        h.runtime.clone(),
        Duration::from_millis(20),
        Duration::from_millis(200),
        async move {
            let _ = stopped.await;
        },
    ));
    let initializes = || fake.with(|r| r.requests.iter().filter(|(m, p)| m == "POST" && p == "/mcp").count());
    for _ in 0..500 {
        if initializes() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(initializes() >= 2, "a second tick ran");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), loop_task)
        .await
        .unwrap()
        .unwrap();
}

/// A `Notifier` that panics: the tick that called it fails, not the loop.
struct Panics(AtomicUsize);

impl Notifier for Panics {
    fn notify(&self, _alert: &ConnectionAlert) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("a notifier that panics");
    }
}

/// Gateway spec §7: panic isolation: a tick that panics is logged, and the
/// next tick runs.
#[tokio::test]
async fn a_tick_that_panics_does_not_end_the_loop() {
    let panics = Arc::new(Panics(AtomicUsize::new(0)));
    let notifier = panics.clone();
    let h = Harness::with(Limits::default(), move |r| {
        hennery_gateway::runtime::Runtime::new(
            r.store.clone(),
            r.statuses.clone(),
            r.key.clone(),
            r.egress.clone(),
            notifier,
        )
    })
    .await;
    let fake = FakeAs::start(Config {
        mcp_status: Some(503),
        ..Config::default()
    })
    .await;
    let id = h.connection("linear", &fake.mcp_url(), CredKind::OauthDcr);
    let (access, refresh) = fake.issue();
    seed_grant(&h, &id, &fake, &access, Some(&refresh), None);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let loop_task = tokio::spawn(probe::run(
        h.runtime.clone(),
        Duration::from_millis(20),
        Duration::from_secs(5),
        async move {
            let _ = stopped.await;
        },
    ));
    // The first tick's `error` transition panics in the notifier; the
    // probes after it must still run (and find no transition to tell).
    let posts = || fake.with(|r| r.requests.iter().filter(|(m, p)| m == "POST" && p == "/mcp").count());
    for _ in 0..500 {
        if panics.0.load(Ordering::SeqCst) >= 1 && posts() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(panics.0.load(Ordering::SeqCst) >= 1);
    assert!(posts() >= 2, "a tick ran after the one that panicked");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), loop_task)
        .await
        .unwrap()
        .unwrap();
}

fn change(from: Status, to: Status) -> StatusChange {
    StatusChange {
        connection_id: "conn-1".into(),
        hat_id: "hat-1".into(),
        label: "Linear".into(),
        from,
        to,
    }
}

/// Gateway spec §7 (G-22): alerts on transitions into or out of a problem
/// only; every outcome of the mapping.
#[test]
fn alerts_are_for_transitions_into_or_out_of_a_problem_only() {
    use Status::*;
    for (from, to, expected) in [
        (Ok, NeedsAuth, Some(Alert::NeedsAuth)),
        (NotConnected, NeedsAuth, Some(Alert::NeedsAuth)),
        (Error, NeedsAuth, Some(Alert::NeedsAuth)),
        (Ok, Error, Some(Alert::Failing)),
        (NeedsAuth, Error, Some(Alert::Failing)),
        (NeedsAuth, Ok, Some(Alert::Recovered)),
        (Error, Ok, Some(Alert::Recovered)),
        (NotConnected, Ok, None),
        (Ok, Ok, None),
        (Error, Error, None),
        (NeedsAuth, NeedsAuth, None),
        (Ok, NotConnected, None),
    ] {
        assert_eq!(Alert::of(&change(from, to)), expected, "{from:?} -> {to:?}");
    }
}

/// Lane L9: the push notice, as agreed with the push lane; `error`'s text
/// never asks to reconnect.
#[test]
fn the_push_notice_is_the_agreed_one() {
    for (alert, body) in [
        (Alert::NeedsAuth, "needs sign-in again"),
        (Alert::Failing, "is failing"),
        (Alert::Recovered, "is working again"),
    ] {
        let notice = notify::notice(&ConnectionAlert {
            connection_id: "conn-1".into(),
            hat_id: "hat-1".into(),
            label: "Linear".into(),
            alert,
        });
        assert_eq!(notice.hat_id, "hat-1");
        assert_eq!(notice.urgency, Urgency::Normal);
        assert_eq!(notice.title, "Linear");
        assert_eq!(notice.generic_title, GENERIC_TITLE);
        assert_eq!(notice.generic_title, "An MCP connection needs attention");
        assert_eq!(notice.body, body);
        assert_eq!(notice.detail, None);
        assert_eq!(notice.url, "/mcp");
        assert_eq!(notice.tag, "mcp-conn-1");
    }
    let failing = Alert::Failing.body().to_ascii_lowercase();
    assert!(!failing.contains("connect") && !failing.contains("sign"), "{failing}");
}

/// Gateway spec §7: a problem present at startup is announced once; a
/// probe that finds it unchanged says nothing more.
#[tokio::test]
async fn a_problem_at_startup_is_announced_once() {
    let (h, _fake, id) = setup(Config {
        mcp_status: Some(503),
        ..Config::default()
    })
    .await;
    set_status(&h, &id, Status::Error);
    let ok = h.connection("fine", "http://127.0.0.1:9/mcp", CredKind::None);
    h.runtime.announce_startup().unwrap();
    assert_eq!(h.alerts.alerts(), [(id.clone(), Alert::Failing)]);
    assert!(!h.alerts.alerts().iter().any(|(c, _)| *c == ok));
    assert_eq!(
        probe(&h, &id).await,
        Verdict::Error("the upstream answered HTTP 503".into())
    );
    assert_eq!(h.alerts.alerts().len(), 1, "no re-post on an unchanged problem");
}

/// Gateway spec §7: what a startup announces is every connection in
/// `needs_auth` or `error`, and only those; an `ok` or `not_connected` one
/// is not a problem to hear of.
#[tokio::test]
async fn startup_problems_are_the_needs_auth_and_error_connections_only() {
    let (h, _fake, failing) = setup(Config::default()).await;
    set_status(&h, &failing, Status::Error);
    let refused = h.connection("refused", "http://127.0.0.1:9/a", CredKind::None);
    set_status(&h, &refused, Status::NeedsAuth);
    let ok = h.connection("fine", "http://127.0.0.1:9/b", CredKind::None);
    set_status(&h, &ok, Status::Ok);
    h.connection("fresh", "http://127.0.0.1:9/c", CredKind::None);
    // Created in the same second: their order is by id, which is random.
    let mut problems: Vec<(String, &str)> = h
        .proxy_store
        .problems()
        .unwrap()
        .into_iter()
        .map(|change| (change.connection_id, change.to.as_str()))
        .collect();
    problems.sort();
    let mut expected = vec![(failing, "error"), (refused, "needs_auth")];
    expected.sort();
    assert_eq!(problems, expected);
}

/// Gateway spec §7: a probe that fails in the gateway itself (a refreshed
/// grant it could not store) concludes nothing about the upstream: no
/// status change.
#[tokio::test]
async fn a_probe_the_gateway_fails_changes_nothing() {
    let (h, fake, id) = setup(Config::default()).await;
    set_status(&h, &id, Status::Ok);
    h.raw()
        .execute_batch(
            "CREATE TRIGGER no_refresh BEFORE UPDATE ON gw_credentials
             BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
        )
        .unwrap();
    fake.expire_access();
    assert_eq!(probe(&h, &id).await, Verdict::NoChange);
    assert_eq!(h.status(&id), "ok");
    assert!(h.alerts.alerts().is_empty());
}

/// Decision 13 for the probe: a grant for another URL found by the 401's
/// refresh is not sent, and concludes nothing about the upstream: no
/// status change, no alert, and the old upstream never sees its token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_probe_whose_connection_moved_during_its_refresh_changes_nothing() {
    let (h, a, id) = setup(Config::default()).await;
    set_status(&h, &id, Status::Ok);
    let b = FakeAs::start(Config {
        prefix: "b-".into(),
        ..Config::default()
    })
    .await;
    let held = h.runtime.lock(&id).await;
    a.expire_access();
    let probing = tokio::spawn({
        let runtime = h.runtime.clone();
        let connection = h.proxy_store.connection_by_id(&id).unwrap().unwrap();
        async move { probe::probe_now(&runtime, &connection).await }
    });
    // The upstream answered 401: the refresh now waits for the lock.
    for _ in 0..1000 {
        if a.with(|r| !r.bearers.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(a.with(|r| !r.bearers.is_empty()), "never sent");
    h.store
        .update(
            &id,
            &hennery_gateway::model::ConnectionPatch {
                url: Some(b.mcp_url()),
                ..Default::default()
            },
            unix_now(),
        )
        .unwrap();
    let (b_access, b_refresh) = b.issue();
    seed_grant(&h, &id, &b, &b_access, Some(&b_refresh), None);
    drop(held);
    assert_eq!(probing.await.unwrap(), Verdict::NoChange);
    assert!(!a.with(|r| r.bearers.contains(&b_access)), "b's token went to a");
    assert!(b.with(|r| r.bearers.is_empty()));
    assert!(h.alerts.alerts().is_empty());
}
