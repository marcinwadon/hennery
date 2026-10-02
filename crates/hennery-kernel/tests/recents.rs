//! Project recents (kernel spec §1.1, §5.3; plan 6c decision 11).

use hennery_kernel::hats::{HatChange, NewRule};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::recents::{KEPT, Recent};

const NOW: i64 = 1_800_000_000;

fn paths(recents: &[Recent]) -> Vec<&str> {
    recents.iter().map(|r| r.path.as_str()).collect()
}

/// A registry with `host-1` and `host-2` paired, and the default hat.
fn registry() -> (Hosts, String) {
    let hosts = Hosts::open_in_memory().unwrap();
    for (n, id) in ["host-1", "host-2"].iter().enumerate() {
        let enrollment = Enrollment {
            public_key: hex::encode(
                ed25519_dalek::SigningKey::from_bytes(&[n as u8 + 1; 32])
                    .verifying_key()
                    .as_bytes(),
            ),
            name: "test".into(),
            host_version: "0".into(),
            platform: "test".into(),
        };
        hosts.register(id, &enrollment, NOW).unwrap();
    }
    let default_hat = hosts.host("host-1").unwrap().unwrap().default_hat_id;
    (hosts, default_hat)
}

#[test]
fn recents_are_per_host_newest_first_and_refreshed_when_used_again() {
    let (hosts, hat) = registry();
    assert!(hosts.remember("host-1", &hat, "/p/a", NOW).unwrap());
    assert!(hosts.remember("host-1", &hat, "/p/b", NOW + 1).unwrap());
    assert!(hosts.remember("host-2", &hat, "/p/c", NOW + 2).unwrap());
    assert_eq!(paths(&hosts.recents("host-1", &hat, 10).unwrap()), ["/p/b", "/p/a"]);
    assert!(hosts.remember("host-1", &hat, "/p/a", NOW + 3).unwrap());
    assert_eq!(
        hosts.recents("host-1", &hat, 10).unwrap(),
        [
            Recent {
                path: "/p/a".into(),
                last_used_at: NOW + 3
            },
            Recent {
                path: "/p/b".into(),
                last_used_at: NOW + 1
            }
        ]
    );
    assert_eq!(paths(&hosts.recents("host-1", &hat, 1).unwrap()), ["/p/a"]);
    assert_eq!(paths(&hosts.recents("host-2", &hat, 10).unwrap()), ["/p/c"]);
    assert!(hosts.recents("host-x", &hat, 10).unwrap().is_empty());
}

#[test]
fn a_cwd_that_is_not_canonical_or_cannot_be_shown_is_not_remembered() {
    let (hosts, hat) = registry();
    let long = format!("/{}", "x".repeat(4096));
    for bad in [
        "relative",
        "",
        "/p/",
        "/p/./q",
        "/p/../q",
        "/new\nline",
        "/lap\u{202E}top",
        long.as_str(),
    ] {
        assert!(!hosts.remember("host-1", &hat, bad, NOW).unwrap(), "{bad:?}");
    }
    assert!(hosts.recents("host-1", &hat, 10).unwrap().is_empty());
}

#[test]
fn only_the_newest_are_kept_per_host_and_hat() {
    let (hosts, hat) = registry();
    for n in 0..KEPT + 5 {
        hosts
            .remember("host-1", &hat, &format!("/p/{n:03}"), NOW + n as i64)
            .unwrap();
    }
    let kept = hosts.recents("host-1", &hat, 1000).unwrap();
    assert_eq!(kept.len(), KEPT);
    assert_eq!(kept[0].path, format!("/p/{:03}", KEPT + 4));
    assert_eq!(kept[KEPT - 1].path, "/p/005");
}

/// The review's A1: a recent is shown for the hat its path resolves to
/// now; a rule added since moves it, and a path stored under two hats is
/// shown once, with its latest use.
#[test]
fn recents_follow_the_hat_their_path_resolves_to_now() {
    let (hosts, default_hat) = registry();
    let HatChange::Done(work) = hosts.create_hat("Work", None, NOW).unwrap() else {
        panic!("a hat");
    };
    hosts.remember("host-1", &default_hat, "/p/acme", NOW).unwrap();
    hosts.remember("host-1", &default_hat, "/p/home", NOW + 1).unwrap();
    assert_eq!(
        paths(&hosts.recents("host-1", &default_hat, 10).unwrap()),
        ["/p/home", "/p/acme"]
    );
    assert!(hosts.recents("host-1", &work.id, 10).unwrap().is_empty());
    let rule = NewRule {
        prefix: "/p/acme".into(),
        hat_id: work.id.clone(),
        verified: true,
    };
    hosts.replace_path_rules("host-1", &[rule]).unwrap();
    assert_eq!(paths(&hosts.recents("host-1", &default_hat, 10).unwrap()), ["/p/home"]);
    assert_eq!(paths(&hosts.recents("host-1", &work.id, 10).unwrap()), ["/p/acme"]);
    // Remembered again under its new hat: one entry, the latest use.
    hosts.remember("host-1", &work.id, "/p/acme", NOW + 5).unwrap();
    assert_eq!(
        hosts.recents("host-1", &work.id, 10).unwrap(),
        [Recent {
            path: "/p/acme".into(),
            last_used_at: NOW + 5
        }]
    );
}

/// Task 6's review: a change of the host's default hat moves the recents
/// no rule claims, at once; and a use that reads earlier (a clock stepped
/// back) never makes a recent older.
#[test]
fn a_new_default_hat_takes_the_recents_no_rule_claims() {
    let (hosts, old_default) = registry();
    let HatChange::Done(work) = hosts.create_hat("Work", None, NOW).unwrap() else {
        panic!("a hat");
    };
    hosts.remember("host-1", &old_default, "/p/a", NOW + 5).unwrap();
    hosts.remember("host-1", &old_default, "/p/a", NOW).unwrap();
    assert_eq!(
        hosts.recents("host-1", &old_default, 10).unwrap()[0].last_used_at,
        NOW + 5
    );
    hosts.update_host("host-1", None, Some(&work.id)).unwrap();
    assert!(hosts.recents("host-1", &old_default, 10).unwrap().is_empty());
    assert_eq!(paths(&hosts.recents("host-1", &work.id, 10).unwrap()), ["/p/a"]);
}

/// Migration 8's cascade: recents go with their hat and their host.
#[test]
fn recents_go_with_their_hat_and_their_host() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    let hosts = Hosts::open(&path).unwrap();
    let default_hat = {
        let enrollment = Enrollment {
            public_key: hex::encode(
                ed25519_dalek::SigningKey::from_bytes(&[1; 32])
                    .verifying_key()
                    .as_bytes(),
            ),
            name: "test".into(),
            host_version: "0".into(),
            platform: "test".into(),
        };
        hosts.register("host-1", &enrollment, NOW).unwrap();
        hosts.host("host-1").unwrap().unwrap().default_hat_id
    };
    let HatChange::Done(work) = hosts.create_hat("Work", None, NOW).unwrap() else {
        panic!("a hat");
    };
    hosts.remember("host-1", &default_hat, "/p/a", NOW).unwrap();
    hosts.remember("host-1", &work.id, "/p/b", NOW).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let count = |conn: &rusqlite::Connection| -> i64 {
        conn.query_row("SELECT count(*) FROM project_recents", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count(&conn), 2);
    conn.execute("DELETE FROM hats WHERE id = ?1", [&work.id]).unwrap();
    assert_eq!(count(&conn), 1);
    conn.execute("DELETE FROM hosts WHERE id = 'host-1'", []).unwrap();
    assert_eq!(count(&conn), 0);
}

/// Another owner's recents, written straight into the file with that
/// owner's host and hat, are neither listed nor pruned.
#[test]
fn another_owners_recents_are_invisible_and_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    let hosts = Hosts::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "
        PRAGMA foreign_keys = ON;
        INSERT INTO owners(id, created_at) VALUES ('owner-other', {later});
        INSERT INTO hats(id, owner_id, name, colour, created_at)
            VALUES ('hat-theirs', 'owner-other', 'Theirs', '#000000', {NOW});
        INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, default_hat_id, created_at)
            VALUES ('host-theirs', 'owner-other', 'n', 'k', 'p', 'v', 'hat-theirs', {NOW});
        ",
        later = NOW + 1_000_000
    ))
    .unwrap();
    for n in 0..KEPT + 1 {
        conn.execute(
            "INSERT INTO project_recents(owner_id, host_id, hat_id, path, last_used_at)
             VALUES ('owner-other', 'host-theirs', 'hat-theirs', ?1, ?2)",
            rusqlite::params![format!("/theirs/{n}"), NOW],
        )
        .unwrap();
    }
    assert!(hosts.recents("host-theirs", "hat-theirs", 1000).unwrap().is_empty());
    // A write naming their host and hat fails on the owner's foreign keys,
    // and changes nothing of theirs.
    assert!(hosts.remember("host-theirs", "hat-theirs", "/mine", NOW + 5).is_err());
    let theirs: i64 = conn
        .query_row(
            "SELECT count(*) FROM project_recents WHERE owner_id = 'owner-other'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(theirs, KEPT as i64 + 1);
}
