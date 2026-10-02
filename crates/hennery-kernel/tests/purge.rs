//! A hat's purge, the kernel's part (kernel spec §5.5; plan 9c decisions
//! 10 and 12, A2, A7, A12): the freeze, which refuses the default hats and
//! takes the hat's rules, the refusals of a frozen hat, and the last step,
//! which deletes the hat's row and keeps its `purged_hats` row for good.

use ed25519_dalek::SigningKey;
use hennery_kernel::hats::{HatChange, HostChange, NewRule, PurgeStart, RulesChange};
use hennery_kernel::hosts::{Enrollment, Hosts, Registered, Revoke};
use hennery_kernel::push::PushPolicy;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Later than any real clock reaches, as in `tests/hats.rs`.
const NOW: i64 = 4_000_000_000;

fn enrollment(seed: u8) -> Enrollment {
    Enrollment {
        public_key: hex::encode(SigningKey::from_bytes(&[seed; 32]).verifying_key().as_bytes()),
        name: "laptop".into(),
        host_version: "0.0.0".into(),
        platform: "macos-aarch64".into(),
    }
}

fn host(hosts: &Hosts, host_id: &str, seed: u8) {
    assert_eq!(
        hosts.register(host_id, &enrollment(seed), NOW).unwrap(),
        Registered::Created
    );
}

fn new_hat(hosts: &Hosts, name: &str) -> String {
    match hosts.create_hat(name, None, NOW).unwrap() {
        HatChange::Done(hat) => hat.id,
        other => panic!("expected a hat, got {other:?}"),
    }
}

fn rule(prefix: &str, hat_id: &str) -> NewRule {
    NewRule {
        prefix: prefix.into(),
        hat_id: hat_id.into(),
        verified: true,
    }
}

/// A registry over `hennery.db` in `dir`, and that path.
fn file_hosts(dir: &Path) -> (Hosts, PathBuf) {
    let db = dir.join("hennery.db");
    (Hosts::open(&db).unwrap(), db)
}

fn count(conn: &Connection, sql: &str, hat: &str) -> i64 {
    conn.query_row(sql, [hat], |r| r.get(0)).unwrap()
}

fn rules_naming(conn: &Connection, hat: &str) -> i64 {
    count(conn, "SELECT count(*) FROM hat_path_rules WHERE hat_id = ?1", hat)
}

/// Decision 10a, A2: neither the hat new hosts get nor any host's default
/// hat, a revoked host's included, can be purged; an unknown hat is not
/// found. Nothing is frozen by a refusal.
#[test]
fn the_default_hats_and_an_unknown_hat_are_not_purged() {
    let hosts = Hosts::open_in_memory().unwrap();
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    let work = new_hat(&hosts, "Work");
    let old = new_hat(&hosts, "Old");
    host(&hosts, "host-1", 1);
    host(&hosts, "host-2", 2);
    assert_eq!(
        hosts.update_host("host-1", None, Some(&work)).unwrap(),
        HostChange::Done
    );
    assert_eq!(hosts.update_host("host-2", None, Some(&old)).unwrap(), HostChange::Done);
    assert_eq!(hosts.revoke("host-2", NOW).unwrap(), Revoke::Revoked);

    for hat in [&personal, &work, &old] {
        assert!(hosts.is_default_hat(hat).unwrap(), "{hat}");
        assert_eq!(hosts.begin_purge(hat, NOW).unwrap(), PurgeStart::IsDefault, "{hat}");
        assert!(!hosts.is_frozen(hat).unwrap(), "{hat}");
    }
    assert_eq!(hosts.begin_purge("hat-nope", NOW).unwrap(), PurgeStart::NotFound);
    assert!(hosts.purged_hats().unwrap().is_empty());
    // Control: a hat that is no default is purged.
    let spare = new_hat(&hosts, "Spare");
    assert!(!hosts.is_default_hat(&spare).unwrap());
    assert_eq!(hosts.begin_purge(&spare, NOW).unwrap(), PurgeStart::Frozen { rules: 0 });
}

/// Decision 10c: the freeze records the hat in `purged_hats` and takes its
/// rules, every host's, and only its own; the hat stays, listed as
/// `purging` (A12). A repeated freeze is a freeze again, and takes a rule
/// that got in since (a re-POST resumes).
#[test]
fn the_freeze_takes_the_hats_rules_and_marks_it_purging() {
    let dir = tempfile::tempdir().unwrap();
    let (hosts, db) = file_hosts(dir.path());
    let acme = new_hat(&hosts, "Acme");
    let other = new_hat(&hosts, "Other");
    host(&hosts, "host-1", 1);
    host(&hosts, "host-2", 2);
    let set = |host_id: &str, rules: &[NewRule]| {
        assert!(matches!(
            hosts.replace_path_rules(host_id, rules).unwrap(),
            RulesChange::Done(_)
        ));
    };
    set("host-1", &[rule("/p/acme", &acme), rule("/p/other", &other)]);
    set("host-2", &[rule("/srv/acme", &acme)]);
    assert_eq!(hosts.purge_counts(&acme).unwrap(), (2, 0));
    assert!(hosts.remember("host-1", &acme, "/p/acme/x", NOW).unwrap());
    assert_eq!(hosts.purge_counts(&acme).unwrap(), (2, 1));

    assert_eq!(hosts.begin_purge(&acme, NOW).unwrap(), PurgeStart::Frozen { rules: 2 });
    let conn = Connection::open(&db).unwrap();
    assert_eq!(rules_naming(&conn, &acme), 0);
    assert_eq!(rules_naming(&conn, &other), 1);
    assert!(hosts.is_frozen(&acme).unwrap());
    assert!(!hosts.is_frozen(&other).unwrap());
    assert_eq!(hosts.purged_hats().unwrap(), [acme.as_str()]);
    let listed = hosts.hat(&acme).unwrap().unwrap();
    assert!(listed.purging);
    assert!(!hosts.hat(&other).unwrap().unwrap().purging);
    let purged_at: i64 = conn
        .query_row("SELECT purged_at FROM purged_hats WHERE hat_id = ?1", [&acme], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(purged_at, NOW);

    // A rule naming it written behind the registry's back: the next freeze
    // takes it too, and keeps the first freeze's time.
    conn.execute(
        "INSERT INTO hat_path_rules(id, owner_id, host_id, prefix, hat_id, verified)
         SELECT 'rule-late', owner_id, 'host-2', '/late', ?1, 1 FROM hats WHERE id = ?1",
        [&acme],
    )
    .unwrap();
    assert_eq!(
        hosts.begin_purge(&acme, NOW + 5).unwrap(),
        PurgeStart::Frozen { rules: 1 }
    );
    assert_eq!(rules_naming(&conn, &acme), 0);
    assert_eq!(
        hosts.begin_purge(&acme, NOW + 9).unwrap(),
        PurgeStart::Frozen { rules: 0 }
    );
    let purged_at: i64 = conn
        .query_row("SELECT purged_at FROM purged_hats WHERE hat_id = ?1", [&acme], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(purged_at, NOW);
}

/// A2: a frozen hat is no hat for the registry's checks. It cannot become
/// the default for new hosts or a host's default hat, and no rule can name
/// it; nothing changes. It can still be renamed and recoloured.
#[test]
fn a_frozen_hat_cannot_become_a_default_or_be_named_by_a_rule() {
    let hosts = Hosts::open_in_memory().unwrap();
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    let acme = new_hat(&hosts, "Acme");
    host(&hosts, "host-1", 1);
    assert!(matches!(
        hosts.begin_purge(&acme, NOW).unwrap(),
        PurgeStart::Frozen { .. }
    ));

    assert_eq!(
        hosts.update_hat(&acme, Some("Acme 2"), None, Some(true)).unwrap(),
        HatChange::Purging
    );
    assert_eq!(hosts.default_hat_for_new_hosts().unwrap(), personal);
    assert_eq!(
        hosts.hat(&acme).unwrap().unwrap().name,
        "Acme",
        "the rename rolled back"
    );
    assert!(matches!(
        hosts.update_host("host-1", None, Some(&acme)).unwrap(),
        HostChange::Invalid(_)
    ));
    assert_eq!(hosts.host("host-1").unwrap().unwrap().default_hat_id, personal);
    assert!(matches!(
        hosts.replace_path_rules("host-1", &[rule("/p/acme", &acme)]).unwrap(),
        RulesChange::Invalid(_)
    ));
    assert_eq!(hosts.path_rules("host-1").unwrap().unwrap(), []);
    // A host paired now gets the hat new hosts get, which is never frozen.
    host(&hosts, "host-2", 2);
    assert_eq!(hosts.host("host-2").unwrap().unwrap().default_hat_id, personal);

    match hosts.update_hat(&acme, Some("Acme 2"), Some("#123456"), None).unwrap() {
        HatChange::Done(hat) => assert_eq!(
            (hat.name.as_str(), hat.colour.as_str(), hat.purging),
            ("Acme 2", "#123456", true)
        ),
        other => panic!("not renamed: {other:?}"),
    }
}

/// Decision 10e, A7: the last step deletes the hat's row, and its recents
/// with it, only once the hat is frozen; its `purged_hats` row stays for
/// good. Then the hat is unknown, and purged.
#[test]
fn the_last_step_deletes_a_frozen_hat_and_keeps_its_purged_row() {
    let dir = tempfile::tempdir().unwrap();
    let (hosts, db) = file_hosts(dir.path());
    let acme = new_hat(&hosts, "Acme");
    host(&hosts, "host-1", 1);
    assert!(hosts.remember("host-1", &acme, "/p/acme", NOW).unwrap());
    let conn = Connection::open(&db).unwrap();
    let recents = |hat: &str| count(&conn, "SELECT count(*) FROM project_recents WHERE hat_id = ?1", hat);

    assert!(!hosts.finish_purge(&acme).unwrap(), "a hat not frozen is not deleted");
    assert!(hosts.hat(&acme).unwrap().is_some());
    assert_eq!(recents(&acme), 1);

    assert!(matches!(
        hosts.begin_purge(&acme, NOW).unwrap(),
        PurgeStart::Frozen { .. }
    ));
    assert!(hosts.finish_purge(&acme).unwrap());
    assert_eq!(hosts.hat(&acme).unwrap(), None);
    assert_eq!(recents(&acme), 0);
    assert_eq!(hosts.purged_hats().unwrap(), [acme.as_str()]);
    assert!(hosts.is_frozen(&acme).unwrap());
    // Done: again, it finds nothing.
    assert!(!hosts.finish_purge(&acme).unwrap());
    assert_eq!(hosts.begin_purge(&acme, NOW).unwrap(), PurgeStart::NotFound);
    assert_eq!(hosts.purge_counts(&acme).unwrap(), (0, 0));
}

/// Decision 10e: the hat's push policy (plan 10a decision 6) goes with
/// its row. Not the default, so a row left behind could not pass for none.
#[test]
fn the_last_step_takes_the_hats_push_policy_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let (hosts, db) = file_hosts(dir.path());
    let acme = new_hat(&hosts, "Acme");
    let muted = PushPolicy {
        muted: true,
        ..PushPolicy::default()
    };
    assert!(hosts.set_push_policy(&acme, muted).unwrap());
    let conn = Connection::open(&db).unwrap();
    let policies = |hat: &str| count(&conn, "SELECT count(*) FROM hat_push_policies WHERE hat_id = ?1", hat);
    assert_eq!(policies(&acme), 1);

    assert!(matches!(
        hosts.begin_purge(&acme, NOW).unwrap(),
        PurgeStart::Frozen { .. }
    ));
    assert_eq!(policies(&acme), 1, "the freeze keeps it");
    assert!(hosts.finish_purge(&acme).unwrap());
    assert_eq!(policies(&acme), 0);
}

/// Another owner's `purged_hats` row, written straight into the database,
/// freezes nothing of this owner's, and is not this owner's to forget.
#[test]
fn another_owners_purged_hat_is_not_this_owners() {
    let dir = tempfile::tempdir().unwrap();
    let (hosts, db) = file_hosts(dir.path());
    let acme = new_hat(&hosts, "Acme");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(&format!(
        "INSERT INTO owners(id, created_at) VALUES ('owner-other', 1);
         INSERT INTO purged_hats(hat_id, owner_id, purged_at) VALUES ('{acme}', 'owner-other', 1);"
    ))
    .unwrap();
    assert!(!hosts.is_frozen(&acme).unwrap());
    assert!(!hosts.hat(&acme).unwrap().unwrap().purging);
    assert!(hosts.purged_hats().unwrap().is_empty());
    assert!(!hosts.finish_purge(&acme).unwrap());
    assert!(hosts.hat(&acme).unwrap().is_some());
    // Its freeze would be no freeze of this owner's: refused, and its rules
    // stay.
    host(&hosts, "host-1", 1);
    assert!(matches!(
        hosts.replace_path_rules("host-1", &[rule("/p/acme", &acme)]).unwrap(),
        RulesChange::Done(_)
    ));
    assert!(hosts.begin_purge(&acme, NOW).is_err());
    assert_eq!(rules_naming(&conn, &acme), 1);
}
