//! Web Push's store (kernel spec §6; plan 10a): the owner's subscriptions,
//! which end with the signed-in session that made them, each hat's push
//! policy, and the owner's contact.

use hennery_kernel::hats::HatChange;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::{Operator, Reset, SetupOutcome};
use hennery_kernel::push::{MAX_SUBSCRIPTIONS, NewSubscription, PushPolicy, Subscribed};

const NOW: i64 = 1_800_000_000;

/// A real browser's keys (web-push-native's example subscription).
const P256DH: &str = "BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY";
const AUTH: &str = "_ordMnz7uTCmrpBTeUV4Bw";

fn new(endpoint: &str) -> NewSubscription<'_> {
    NewSubscription {
        endpoint,
        p256dh: P256DH,
        auth: AUTH,
        device_label: None,
        expires_at: None,
    }
}

/// The operator, set up, and the registry, on one database file; the
/// stored PHC string, which sessions are opened on.
fn stores(dir: &tempfile::TempDir) -> (Operator, Hosts, String) {
    let path = dir.path().join("hennery.db");
    let operator = Operator::open(&path).unwrap();
    let hosts = Hosts::open(&path).unwrap();
    let token = operator.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = operator
        .set_up(&token, "correct horse battery", "https://hennery.example", NOW)
        .unwrap()
    else {
        panic!("setup failed");
    };
    (operator, hosts, phc)
}

/// A signed-in session's id (`Authenticated::session_id`).
fn session(operator: &Operator, phc: &str) -> String {
    let token = operator.open_session("browser", phc, NOW).unwrap().unwrap();
    operator.authenticate(&token, NOW).unwrap().unwrap().session_id
}

/// The fake sessions the store tests subscribe with, live in the file.
const SESSIONS: [&str; 5] = ["s", "s1", "s2", "session-1", "session-2"];

/// The registry on a file of its own, its owner's `SESSIONS` signed in
/// (a subscription needs a live session, Task 1's review).
fn registry() -> (Hosts, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    let hosts = Hosts::open(&path).unwrap();
    for id in SESSIONS {
        sign_in(&path, hosts.owner_id(), id);
    }
    (hosts, dir)
}

/// A live session `id_hash` of `owner`, written straight into the file.
fn sign_in(path: &std::path::Path, owner: &str, id_hash: &str) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, expires_at)
             VALUES (?1, ?2, 'test', ?3, ?3, ?4)",
            rusqlite::params![id_hash, owner, NOW, NOW + 3600],
        )
        .unwrap();
}

/// The session `id_hash` gone from the file, as an expired one is pruned.
fn prune(dir: &tempfile::TempDir, id_hash: &str) {
    rusqlite::Connection::open(dir.path().join("hennery.db"))
        .unwrap()
        .execute("DELETE FROM auth_sessions WHERE id_hash = ?1", [id_hash])
        .unwrap();
}

fn endpoints(hosts: &Hosts) -> Vec<String> {
    hosts.subscriptions().unwrap().into_iter().map(|s| s.endpoint).collect()
}

#[test]
fn a_subscription_is_stored_and_listed_with_its_host_as_its_label() {
    let (hosts, _dir) = registry();
    let Subscribed::Created(sub) = hosts
        .subscribe(&new("https://fcm.googleapis.com/fcm/send/a"), "session-1", NOW)
        .unwrap()
    else {
        panic!("created");
    };
    assert!(sub.id.starts_with("push-"));
    assert_eq!(sub.device_label, "fcm.googleapis.com");
    assert_eq!(sub.auth_session, "session-1");
    assert_eq!((sub.created_at, sub.expires_at, sub.last_success_at), (NOW, None, None));
    assert_eq!(hosts.subscriptions().unwrap(), std::slice::from_ref(&sub));
    // The endpoint is a capability URL: never printed.
    assert!(!format!("{sub:?}").contains("/fcm/send/a"));
}

#[test]
fn the_same_endpoint_again_replaces_its_keys_and_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    let hosts = Hosts::open(&path).unwrap();
    sign_in(&path, hosts.owner_id(), "session-1");
    sign_in(&path, hosts.owner_id(), "session-2");
    let endpoint = "https://web.push.apple.com/QAbc";
    let Subscribed::Created(first) = hosts.subscribe(&new(endpoint), "session-1", NOW).unwrap() else {
        panic!("created");
    };
    // A failed delivery (plan 10b) is forgotten once the browser subscribes
    // again: its keys are new.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE push_subscriptions SET last_error = 'HTTP 403'", [])
        .unwrap();
    let mut again = new(endpoint);
    again.device_label = Some("  iPhone  ");
    // Padded keys are stored as checked: unpadded (Task 1's review).
    let (p256dh, auth) = (format!("{P256DH}="), format!("{AUTH}=="));
    again.p256dh = &p256dh;
    again.auth = &auth;
    again.expires_at = Some(NOW + 3600);
    let Subscribed::Replaced(second) = hosts.subscribe(&again, "session-2", NOW + 5).unwrap() else {
        panic!("replaced");
    };
    assert_eq!(second.id, first.id);
    assert_eq!(second.created_at, NOW);
    assert_eq!(second.device_label, "iPhone");
    assert_eq!(second.auth_session, "session-2");
    assert_eq!(second.expires_at, Some(NOW + 3600));
    assert_eq!(second.last_error, None);
    assert_eq!((second.p256dh.as_str(), second.auth.as_str()), (P256DH, AUTH));
    // Again with no label: the one given is kept.
    let Subscribed::Replaced(third) = hosts.subscribe(&new(endpoint), "session-2", NOW + 6).unwrap() else {
        panic!("replaced");
    };
    assert_eq!(third.device_label, "iPhone");
    assert_eq!(hosts.subscriptions().unwrap().len(), 1);
}

#[test]
fn a_bad_subscription_is_refused_and_nothing_is_stored() {
    let (hosts, _dir) = registry();
    let mut bad_keys = new("https://fcm.googleapis.com/a");
    bad_keys.auth = "AAAA";
    let mut bad_label = new("https://fcm.googleapis.com/b");
    bad_label.device_label = Some("   ");
    let mut expired = new("https://fcm.googleapis.com/c");
    expired.expires_at = Some(NOW);
    for refused in [new("https://127.0.0.1/push"), bad_keys, bad_label, expired] {
        assert!(
            matches!(hosts.subscribe(&refused, "s", NOW).unwrap(), Subscribed::Invalid(_)),
            "{refused:?}"
        );
    }
    assert!(hosts.subscriptions().unwrap().is_empty());
}

#[test]
fn an_owner_has_at_most_max_subscriptions() {
    let (hosts, _dir) = registry();
    let endpoints: Vec<String> = (0..=MAX_SUBSCRIPTIONS)
        .map(|n| format!("https://fcm.googleapis.com/fcm/send/{n}"))
        .collect();
    for endpoint in &endpoints[..MAX_SUBSCRIPTIONS] {
        assert!(matches!(
            hosts.subscribe(&new(endpoint), "s", NOW).unwrap(),
            Subscribed::Created(_)
        ));
    }
    assert_eq!(
        hosts.subscribe(&new(&endpoints[MAX_SUBSCRIPTIONS]), "s", NOW).unwrap(),
        Subscribed::TooMany
    );
    // One already there is still replaced: it is not one more.
    assert!(matches!(
        hosts.subscribe(&new(&endpoints[0]), "s", NOW).unwrap(),
        Subscribed::Replaced(_)
    ));
}

#[test]
fn a_subscription_is_removed_by_id_or_by_endpoint() {
    let (hosts, _dir) = registry();
    let Subscribed::Created(a) = hosts.subscribe(&new("https://fcm.googleapis.com/a"), "s", NOW).unwrap() else {
        panic!("created");
    };
    hosts.subscribe(&new("https://fcm.googleapis.com/b"), "s", NOW).unwrap();
    assert!(hosts.unsubscribe(&a.id).unwrap());
    assert!(!hosts.unsubscribe(&a.id).unwrap());
    assert!(hosts.unsubscribe_endpoint("https://fcm.googleapis.com/b").unwrap());
    assert!(!hosts.unsubscribe_endpoint("https://fcm.googleapis.com/b").unwrap());
    assert!(hosts.subscriptions().unwrap().is_empty());
}

/// Decision 4: signing out, or revoking the device, ends what that
/// session subscribed, and only that.
#[test]
fn a_revoked_session_takes_its_subscriptions_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let (operator, hosts, phc) = stores(&dir);
    let phone = session(&operator, &phc);
    let laptop = session(&operator, &phc);
    hosts
        .subscribe(&new("https://web.push.apple.com/phone"), &phone, NOW)
        .unwrap();
    hosts
        .subscribe(&new("https://fcm.googleapis.com/laptop"), &laptop, NOW)
        .unwrap();
    assert!(operator.revoke_session(&phone, NOW).unwrap());
    assert_eq!(endpoints(&hosts), ["https://fcm.googleapis.com/laptop"]);
    // A session that was not there removes nothing.
    assert!(!operator.revoke_session(&phone, NOW).unwrap());
    assert_eq!(endpoints(&hosts), ["https://fcm.googleapis.com/laptop"]);
}

/// Task 1's review: a session that ended (or expired) before its
/// subscription was stored gets none, or it would outlive its session.
#[test]
fn a_session_that_has_ended_cannot_subscribe() {
    let dir = tempfile::tempdir().unwrap();
    let (operator, hosts, phc) = stores(&dir);
    let phone = session(&operator, &phc);
    assert!(operator.revoke_session(&phone, NOW).unwrap());
    let laptop = session(&operator, &phc);
    for (session, at) in [
        (phone.as_str(), NOW),
        (laptop.as_str(), NOW + 3600 * 24 * 31),
        ("never", NOW),
    ] {
        assert_eq!(
            hosts
                .subscribe(&new("https://web.push.apple.com/phone"), session, at)
                .unwrap(),
            Subscribed::SignedOut
        );
    }
    assert!(endpoints(&hosts).is_empty());
}

/// Decision 4: an expired session's device keeps its notifications; only
/// an ended one loses them.
#[test]
fn a_session_that_expires_leaves_its_subscriptions() {
    let dir = tempfile::tempdir().unwrap();
    let (operator, hosts, phc) = stores(&dir);
    let phone = session(&operator, &phc);
    hosts
        .subscribe(&new("https://web.push.apple.com/phone"), &phone, NOW)
        .unwrap();
    let later = NOW + hennery_kernel::operator::SESSION_TTL_SECS + 1;
    // A later login prunes the expired session.
    operator.open_session("browser", &phc, later).unwrap().unwrap();
    assert!(!operator.revoke_session(&phone, later).unwrap());
    assert_eq!(endpoints(&hosts), ["https://web.push.apple.com/phone"]);
}

#[tokio::test]
async fn a_password_reset_ends_every_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let (operator, hosts, phc) = stores(&dir);
    let phone = session(&operator, &phc);
    hosts
        .subscribe(&new("https://web.push.apple.com/phone"), &phone, NOW)
        .unwrap();
    // One whose session has since been pruned goes too.
    let laptop = session(&operator, &phc);
    hosts
        .subscribe(&new("https://fcm.googleapis.com/old"), &laptop, NOW)
        .unwrap();
    prune(&dir, &laptop);
    let operator = std::sync::Arc::new(operator);
    let reset = operator
        .reset_password("a new long password".into(), NOW)
        .await
        .unwrap();
    assert!(matches!(reset, Reset::Done { .. }));
    assert!(endpoints(&hosts).is_empty());
}

#[test]
fn a_public_url_reset_ends_every_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let (operator, hosts, phc) = stores(&dir);
    let phone = session(&operator, &phc);
    hosts
        .subscribe(&new("https://web.push.apple.com/phone"), &phone, NOW)
        .unwrap();
    // One whose session has since been pruned goes too.
    let laptop = session(&operator, &phc);
    hosts
        .subscribe(&new("https://fcm.googleapis.com/old"), &laptop, NOW)
        .unwrap();
    prune(&dir, &laptop);
    assert!(matches!(
        operator.reset_public_url("https://moved.example").unwrap(),
        Reset::Done { .. }
    ));
    assert!(endpoints(&hosts).is_empty());
}

#[test]
fn every_hat_has_the_default_policy_until_one_is_set() {
    let (hosts, _dir) = registry();
    let HatChange::Done(work) = hosts.create_hat("Work", None, NOW).unwrap() else {
        panic!("a hat");
    };
    let policies = hosts.push_policies().unwrap();
    assert_eq!(policies.len(), 2);
    assert!(policies.iter().all(|(_, policy)| *policy == PushPolicy::default()));
    assert_eq!(policies[1].0, work.id);

    let quiet = PushPolicy {
        muted: true,
        details: false,
        generic_title: true,
    };
    assert!(hosts.set_push_policy(&work.id, quiet).unwrap());
    assert_eq!(hosts.push_policy(&work.id).unwrap(), quiet);
    assert_eq!(hosts.push_policies().unwrap()[1], (work.id.clone(), quiet));
    let open = PushPolicy {
        muted: false,
        details: true,
        generic_title: false,
    };
    assert!(hosts.set_push_policy(&work.id, open).unwrap());
    assert_eq!(hosts.push_policy(&work.id).unwrap(), open);
    // The other hat is untouched.
    assert_eq!(hosts.push_policies().unwrap()[0].1, PushPolicy::default());
}

#[test]
fn a_policy_for_a_hat_that_does_not_exist_is_refused_and_reads_as_the_default() {
    let (hosts, _dir) = registry();
    let muted = PushPolicy {
        muted: true,
        ..PushPolicy::default()
    };
    assert!(!hosts.set_push_policy("hat-nope", muted).unwrap());
    assert_eq!(hosts.push_policy("hat-nope").unwrap(), PushPolicy::default());
    // A session from before hats has none.
    assert_eq!(hosts.push_policy("").unwrap(), PushPolicy::default());
}

/// Migration 9's cascade: a hat's policy goes with its hat.
#[test]
fn a_policy_goes_with_its_hat() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    let hosts = Hosts::open(&path).unwrap();
    let HatChange::Done(work) = hosts.create_hat("Work", None, NOW).unwrap() else {
        panic!("a hat");
    };
    let muted = PushPolicy {
        muted: true,
        ..PushPolicy::default()
    };
    assert!(hosts.set_push_policy(&work.id, muted).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute("DELETE FROM hats WHERE id = ?1", [&work.id]).unwrap();
    let left: i64 = conn
        .query_row("SELECT count(*) FROM hat_push_policies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
}

/// Another owner's subscriptions and policies, written straight into the
/// file, are neither listed, counted, replaced nor removed.
#[test]
fn another_owners_subscriptions_are_invisible_and_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    let hosts = Hosts::open(&path).unwrap();
    sign_in(&path, hosts.owner_id(), "s");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "
        PRAGMA foreign_keys = ON;
        INSERT INTO owners(id, created_at) VALUES ('owner-other', {later});
        INSERT INTO hats(id, owner_id, name, colour, created_at)
            VALUES ('hat-theirs', 'owner-other', 'Theirs', '#000000', {NOW});
        INSERT INTO hat_push_policies(hat_id, owner_id, muted, details, generic_title)
            VALUES ('hat-theirs', 'owner-other', 1, 1, 1);
        INSERT INTO push_subscriptions(id, owner_id, endpoint, p256dh, auth, device_label, auth_session, created_at)
            VALUES ('push-theirs', 'owner-other', 'https://fcm.googleapis.com/theirs', '{P256DH}', '{AUTH}', 'x', 's',
                    {NOW});
        ",
        later = NOW + 1_000_000
    ))
    .unwrap();
    assert!(hosts.subscriptions().unwrap().is_empty());
    assert!(!hosts.unsubscribe("push-theirs").unwrap());
    assert!(!hosts.unsubscribe_endpoint("https://fcm.googleapis.com/theirs").unwrap());
    assert_eq!(hosts.push_policy("hat-theirs").unwrap(), PushPolicy::default());
    assert!(!hosts.set_push_policy("hat-theirs", PushPolicy::default()).unwrap());
    assert!(hosts.push_policies().unwrap().iter().all(|(id, _)| id != "hat-theirs"));
    // Their endpoint is not the owner's to take over, nor to rotate to or
    // from (the review's A7).
    assert_eq!(
        hosts
            .subscribe(&new("https://fcm.googleapis.com/theirs"), "s", NOW)
            .unwrap(),
        Subscribed::Conflict
    );
    assert_eq!(
        hosts
            .rotate(
                "https://fcm.googleapis.com/theirs",
                &new("https://fcm.googleapis.com/x"),
                NOW
            )
            .unwrap(),
        Subscribed::NotFound
    );
    hosts
        .subscribe(&new("https://fcm.googleapis.com/mine"), "s", NOW)
        .unwrap();
    assert_eq!(
        hosts
            .rotate(
                "https://fcm.googleapis.com/mine",
                &new("https://fcm.googleapis.com/theirs"),
                NOW
            )
            .unwrap(),
        Subscribed::Conflict
    );
    let theirs: (i64, i64) = conn
        .query_row(
            "SELECT (SELECT count(*) FROM push_subscriptions WHERE owner_id = 'owner-other'),
                    (SELECT muted FROM hat_push_policies WHERE hat_id = 'hat-theirs')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(theirs, (1, 1));
}

/// The review's A2: the endpoint is stored as checked, so one browser's
/// endpoint written two ways is one subscription.
#[test]
fn an_endpoint_is_stored_in_its_canonical_form() {
    let (hosts, _dir) = registry();
    let Subscribed::Created(sub) = hosts
        .subscribe(&new(" https://FCM.googleapis.com/fcm/send/a"), "s", NOW)
        .unwrap()
    else {
        panic!("created");
    };
    assert_eq!(sub.endpoint, "https://fcm.googleapis.com/fcm/send/a");
    assert!(matches!(
        hosts
            .subscribe(&new("https://fcm.googleapis.com/fcm/send/a"), "s", NOW)
            .unwrap(),
        Subscribed::Replaced(_)
    ));
    assert!(
        hosts
            .unsubscribe_endpoint("https://Fcm.Googleapis.com/fcm/send/a")
            .unwrap()
    );
    assert!(!hosts.unsubscribe_endpoint("not a url").unwrap());
}

/// The review's A1: a push service rotating a browser's subscription.
#[test]
fn a_rotation_replaces_the_old_endpoints_subscription_in_place() {
    let (hosts, _dir) = registry();
    let mut first = new("https://updates.push.services.mozilla.com/wpush/v2/old");
    first.device_label = Some("Firefox");
    let Subscribed::Created(old) = hosts.subscribe(&first, "session-1", NOW).unwrap() else {
        panic!("created");
    };
    let mut fresh = new("https://updates.push.services.mozilla.com/wpush/v2/new");
    fresh.expires_at = Some(NOW + 60);
    let Subscribed::Replaced(rotated) = hosts
        .rotate(
            "https://updates.push.services.mozilla.com/wpush/v2/old",
            &fresh,
            NOW + 5,
        )
        .unwrap()
    else {
        panic!("rotated");
    };
    assert_eq!(rotated.id, old.id);
    assert_eq!(
        rotated.endpoint,
        "https://updates.push.services.mozilla.com/wpush/v2/new"
    );
    // The session that made it, its label and its age stay.
    assert_eq!(
        (
            rotated.auth_session.as_str(),
            rotated.device_label.as_str(),
            rotated.created_at
        ),
        ("session-1", "Firefox", NOW)
    );
    assert_eq!(rotated.expires_at, Some(NOW + 60));
    assert_eq!(hosts.subscriptions().unwrap(), std::slice::from_ref(&rotated));
    // The old endpoint is gone: rotating from it again finds nothing.
    assert_eq!(
        hosts
            .rotate(
                "https://updates.push.services.mozilla.com/wpush/v2/old",
                &fresh,
                NOW + 5
            )
            .unwrap(),
        Subscribed::NotFound
    );
}

#[test]
fn a_rotation_onto_an_endpoint_already_stored_keeps_one_subscription() {
    let (hosts, _dir) = registry();
    let Subscribed::Created(old) = hosts
        .subscribe(&new("https://fcm.googleapis.com/old"), "s1", NOW)
        .unwrap()
    else {
        panic!("created");
    };
    hosts
        .subscribe(&new("https://fcm.googleapis.com/new"), "s2", NOW)
        .unwrap();
    let Subscribed::Replaced(rotated) = hosts
        .rotate(
            "https://fcm.googleapis.com/old",
            &new("https://fcm.googleapis.com/new"),
            NOW,
        )
        .unwrap()
    else {
        panic!("rotated");
    };
    assert_eq!(rotated.id, old.id);
    assert_eq!(hosts.subscriptions().unwrap(), [rotated]);
}

#[test]
fn a_bad_rotation_changes_nothing() {
    let (hosts, _dir) = registry();
    hosts
        .subscribe(&new("https://fcm.googleapis.com/old"), "s", NOW)
        .unwrap();
    let before = hosts.subscriptions().unwrap();
    for (old, to) in [
        ("https://fcm.googleapis.com/old", "https://10.0.0.1/push"),
        ("not a url", "https://fcm.googleapis.com/new"),
        ("https://fcm.googleapis.com/other", "https://fcm.googleapis.com/new"),
    ] {
        assert!(!matches!(
            hosts.rotate(old, &new(to), NOW).unwrap(),
            Subscribed::Replaced(_)
        ));
    }
    assert_eq!(hosts.subscriptions().unwrap(), before);
}

#[test]
fn the_contact_is_set_cleared_and_checked() {
    let operator = Operator::open_in_memory().unwrap();
    assert_eq!(operator.contact().unwrap(), None);
    assert_eq!(operator.set_contact(Some("me@example.com")).unwrap(), Ok(()));
    assert_eq!(operator.contact().unwrap().as_deref(), Some("me@example.com"));
    assert!(operator.set_contact(Some("me@example.com?cc=x")).unwrap().is_err());
    assert_eq!(operator.contact().unwrap().as_deref(), Some("me@example.com"));
    assert_eq!(operator.set_contact(None).unwrap(), Ok(()));
    assert_eq!(operator.contact().unwrap(), None);
}

/// Task 1's review: a subscription on its way in is not printed whole
/// either: its endpoint is a capability URL and `auth` a secret.
#[test]
fn a_new_subscription_prints_only_its_host() {
    let shown = format!("{:?}", new("https://fcm.googleapis.com/fcm/send/capability"));
    assert!(shown.contains("fcm.googleapis.com"), "{shown}");
    assert!(!shown.contains("capability") && !shown.contains(AUTH), "{shown}");
}
