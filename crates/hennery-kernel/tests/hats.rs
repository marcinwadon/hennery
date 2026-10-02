//! Hats and path rules (kernel spec §5, §11; umbrella §8): the default hat
//! from the first start, hats the owner adds, a host's default hat and its
//! rules, and resolving a path to a hat. Another owner's hats and rules,
//! written straight into the database, are invisible and unusable.

use ed25519_dalek::SigningKey;
use hennery_kernel::hats::{DEFAULT_COLOUR, HatChange, HostChange, MAX_RULES, NewRule, Resolution, RulesChange};
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, Hosts, Registered};
use hennery_kernel::operator::{Operator, SetupOutcome};

/// Later than any real clock reaches (2096): the migration stamps the
/// default hat with `unixepoch()`, and it must stay the oldest hat.
const NOW: i64 = 4_000_000_000;
const PASSWORD: &str = "correct horse battery";

fn enrollment(seed: u8) -> Enrollment {
    Enrollment {
        public_key: hex::encode(SigningKey::from_bytes(&[seed; 32]).verifying_key().as_bytes()),
        name: "laptop".into(),
        host_version: "0.0.0".into(),
        platform: "macos-aarch64".into(),
    }
}

fn hat_id(change: HatChange) -> String {
    match change {
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

fn stored(change: RulesChange) -> Vec<(String, String)> {
    match change {
        RulesChange::Done(rules) => rules.into_iter().map(|r| (r.prefix, r.hat_id)).collect(),
        other => panic!("expected the rules, got {other:?}"),
    }
}

/// A host of `hosts`' owner, under a fixed id.
fn host(hosts: &Hosts, host_id: &str, seed: u8) {
    assert_eq!(
        hosts.register(host_id, &enrollment(seed), NOW).unwrap(),
        Registered::Created
    );
}

/// Plan 5a decision 1 (umbrella §8.2): no session is ever hat-less, so the
/// default hat exists from the first start, before setup, as the owner
/// does. `up` pairs its host before setup; that host has it.
#[test]
fn the_default_hat_exists_from_the_first_start_and_new_hosts_get_it() {
    let hosts = Hosts::open_in_memory().unwrap();
    let hats = hosts.hats().unwrap();
    assert_eq!(hats.len(), 1, "{hats:?}");
    let default = &hats[0];
    assert_eq!(
        (
            default.name.as_str(),
            default.colour.as_str(),
            default.default_for_new_hosts
        ),
        ("Personal", DEFAULT_COLOUR, true)
    );
    assert!(
        default.id.starts_with("hat-") && default.id.len() == 20,
        "{}",
        default.id
    );
    assert_eq!(hosts.default_hat_for_new_hosts().unwrap(), default.id);

    let code = hosts.mint_pairing_code(NOW).unwrap();
    let EnrollOutcome::Enrolled { host_id } = hosts.enroll(&code.code, &enrollment(1), NOW).unwrap() else {
        panic!("not enrolled");
    };
    assert_eq!(hosts.host(&host_id).unwrap().unwrap().default_hat_id, default.id);
}

/// Kernel spec §3.1: the setup form names the default hat. The host paired
/// before setup keeps the hat, now under that name.
#[test]
fn setup_names_the_default_hat_the_host_paired_before_it_already_has() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    host(&hosts, "host-up", 1);
    let before = hosts.host("host-up").unwrap().unwrap().default_hat_id;

    let op = Operator::open(&db).unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    assert!(matches!(
        op.set_up_naming_hat(&token, PASSWORD, "https://hennery.example", Some("  Acme  "), NOW)
            .unwrap(),
        SetupOutcome::Done { .. }
    ));
    let hats = hosts.hats().unwrap();
    assert_eq!(hats.len(), 1, "{hats:?}");
    assert_eq!((hats[0].id.as_str(), hats[0].name.as_str()), (before.as_str(), "Acme"));
    assert_eq!(hosts.host("host-up").unwrap().unwrap().default_hat_id, before);
}

#[test]
fn a_bad_hat_name_fails_setup_and_leaves_the_token_live() {
    let op = Operator::open_in_memory().unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    for bad in ["", "   ", "a\u{202E}b", &"x".repeat(65)] {
        assert!(
            matches!(
                op.set_up_naming_hat(&token, PASSWORD, "https://hennery.example", Some(bad), NOW)
                    .unwrap(),
                SetupOutcome::Invalid(_)
            ),
            "{bad:?}"
        );
    }
    assert!(!op.is_set_up().unwrap());
    assert!(matches!(
        op.set_up_naming_hat(&token, PASSWORD, "https://hennery.example", None, NOW)
            .unwrap(),
        SetupOutcome::Done { .. }
    ));
}

#[test]
fn hats_are_added_renamed_and_recoloured_with_unique_names() {
    let hosts = Hosts::open_in_memory().unwrap();
    let acme = hat_id(hosts.create_hat(" Acme ", Some("#A1B2C3"), NOW).unwrap());
    let hat = hosts.hat(&acme).unwrap().unwrap();
    assert_eq!(
        (
            hat.name.as_str(),
            hat.colour.as_str(),
            hat.created_at,
            hat.default_for_new_hosts
        ),
        ("Acme", "#a1b2c3", NOW, false)
    );
    // Names are unique in any case, among the owner's hats.
    assert_eq!(hosts.create_hat("ACME", None, NOW).unwrap(), HatChange::NameTaken);
    assert_eq!(hosts.create_hat("personal", None, NOW).unwrap(), HatChange::NameTaken);
    let side = hat_id(hosts.create_hat("Side project", None, NOW + 1).unwrap());
    assert_eq!(hosts.hat(&side).unwrap().unwrap().colour, DEFAULT_COLOUR);
    assert_eq!(
        hosts.update_hat(&side, Some("acme"), None, None).unwrap(),
        HatChange::NameTaken
    );
    // Renaming a hat to its own name in another case is fine.
    let HatChange::Done(renamed) = hosts.update_hat(&acme, Some("ACME"), Some("#000000"), None).unwrap() else {
        panic!("not renamed");
    };
    assert_eq!((renamed.name.as_str(), renamed.colour.as_str()), ("ACME", "#000000"));

    for bad in ["red", "#12345", "#1234567", "#12345g", "url(x)"] {
        assert!(
            matches!(
                hosts.create_hat("Other", Some(bad), NOW).unwrap(),
                HatChange::Invalid(_)
            ),
            "{bad}"
        );
        assert!(
            matches!(
                hosts.update_hat(&acme, None, Some(bad), None).unwrap(),
                HatChange::Invalid(_)
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        hosts.create_hat("a\u{200B}b", None, NOW).unwrap(),
        HatChange::Invalid(_)
    ));
    assert_eq!(
        hosts.update_hat("hat-nope", Some("x"), None, None).unwrap(),
        HatChange::NotFound
    );
    // Oldest first.
    let names: Vec<String> = hosts.hats().unwrap().into_iter().map(|h| h.name).collect();
    assert_eq!(names, ["Personal", "ACME", "Side project"]);
}

/// Kernel spec §5.1: the default hat is the default for new hosts "until
/// changed". A host paired before the change keeps its own default.
#[test]
fn another_hat_can_become_the_default_for_new_hosts() {
    let hosts = Hosts::open_in_memory().unwrap();
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    host(&hosts, "host-old", 1);
    let acme = hat_id(hosts.create_hat("Acme", None, NOW).unwrap());
    assert!(matches!(
        hosts.update_hat(&acme, None, None, Some(false)).unwrap(),
        HatChange::Invalid(_)
    ));
    let HatChange::Done(hat) = hosts.update_hat(&acme, None, None, Some(true)).unwrap() else {
        panic!("not changed");
    };
    assert!(hat.default_for_new_hosts);
    assert_eq!(hosts.default_hat_for_new_hosts().unwrap(), acme);
    let flags: Vec<bool> = hosts.hats().unwrap().iter().map(|h| h.default_for_new_hosts).collect();
    assert_eq!(flags, [false, true]);

    host(&hosts, "host-new", 2);
    assert_eq!(hosts.host("host-new").unwrap().unwrap().default_hat_id, acme);
    assert_eq!(hosts.host("host-old").unwrap().unwrap().default_hat_id, personal);
}

#[test]
fn a_host_is_renamed_and_given_another_default_hat() {
    let hosts = Hosts::open_in_memory().unwrap();
    host(&hosts, "host-1", 1);
    let acme = hat_id(hosts.create_hat("Acme", None, NOW).unwrap());
    assert_eq!(
        hosts.update_host("host-1", Some(" work laptop "), Some(&acme)).unwrap(),
        HostChange::Done
    );
    let record = hosts.host("host-1").unwrap().unwrap();
    assert_eq!(
        (record.name.as_str(), record.default_hat_id.as_str()),
        ("work laptop", acme.as_str())
    );
    assert!(matches!(
        hosts.update_host("host-1", None, Some("hat-nope")).unwrap(),
        HostChange::Invalid(_)
    ));
    assert!(matches!(
        hosts.update_host("host-1", Some("a\u{202E}b"), None).unwrap(),
        HostChange::Invalid(_)
    ));
    // Nothing of a refused change is kept.
    assert_eq!(hosts.host("host-1").unwrap().unwrap(), record);
    assert_eq!(
        hosts.update_host("host-nope", Some("x"), None).unwrap(),
        HostChange::NotFound
    );
}

/// Kernel spec §5.2: rules match by whole segments, the longest prefix
/// wins, and no rule means the host's default hat. A host's rules are its
/// own: another host's default and rules play no part.
#[test]
fn a_path_resolves_through_its_hosts_rules_to_a_hat() {
    let hosts = Hosts::open_in_memory().unwrap();
    host(&hosts, "host-1", 1);
    host(&hosts, "host-2", 2);
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    let acme = hat_id(hosts.create_hat("Acme", None, NOW).unwrap());
    let secret = hat_id(hosts.create_hat("Secret", None, NOW).unwrap());
    assert_eq!(
        stored(
            hosts
                .replace_path_rules("host-1", &[rule("/p/acme", &acme), rule("/p/acme/secret", &secret)])
                .unwrap()
        ),
        [
            ("/p/acme/secret".to_string(), secret.clone()),
            ("/p/acme".to_string(), acme.clone())
        ]
    );
    let rules = hosts.path_rules("host-1").unwrap().unwrap();
    let id_of = |prefix: &str| rules.iter().find(|r| r.prefix == prefix).unwrap().id.clone();
    let resolved = |host: &str, path: &str| hosts.resolve_hat(host, path).unwrap().unwrap();
    assert_eq!(
        resolved("host-1", "/p/acme/x"),
        Resolution {
            hat_id: acme.clone(),
            rule_id: Some(id_of("/p/acme"))
        }
    );
    assert_eq!(resolved("host-1", "/p/acme/secret/x").hat_id, secret);
    assert_eq!(
        resolved("host-1", "/p/acme-infra"),
        Resolution {
            hat_id: personal.clone(),
            rule_id: None
        }
    );
    // Another host: its own default hat, none of host-1's rules.
    hosts.update_host("host-2", None, Some(&secret)).unwrap();
    assert_eq!(
        resolved("host-2", "/p/acme/x"),
        Resolution {
            hat_id: secret.clone(),
            rule_id: None
        }
    );
    assert_eq!(hosts.resolve_hat("host-nope", "/p").unwrap(), None);
    // A path not in canonical form is refused, never matched.
    for bad in ["/p/acme/", "/p/acme/../x", "p/acme", "/p//acme"] {
        assert!(hosts.resolve_hat("host-1", bad).is_err(), "{bad}");
    }
}

/// Kernel spec §8: `PUT …/path-rules` replaces the full set, or nothing.
#[test]
fn rules_are_replaced_as_a_set_and_a_bad_set_changes_nothing() {
    let hosts = Hosts::open_in_memory().unwrap();
    host(&hosts, "host-1", 1);
    let acme = hat_id(hosts.create_hat("Acme", None, NOW).unwrap());
    stored(
        hosts
            .replace_path_rules("host-1", &[rule("/p/acme", &acme), rule("/p/b", &acme)])
            .unwrap(),
    );
    let before = hosts.path_rules("host-1").unwrap().unwrap();
    let kept_id = before.iter().find(|r| r.prefix == "/p/acme").unwrap().id.clone();

    let bad_sets: Vec<Vec<NewRule>> = vec![
        vec![rule("/p/acme", &acme), rule("/p/acme", &acme)],
        vec![rule("/p/acme/", &acme)],
        vec![rule("p/acme", &acme)],
        vec![rule("/p/./acme", &acme)],
        vec![rule("/p/../acme", &acme)],
        vec![rule("/", &acme)],
        vec![rule("/p/a\u{202E}b", &acme)],
        vec![rule("/p/a\u{200B}b", &acme)],
        vec![rule("/p/acme", "hat-nope")],
        (0..=MAX_RULES).map(|i| rule(&format!("/p/{i}"), &acme)).collect(),
    ];
    for set in bad_sets {
        assert!(
            matches!(
                hosts.replace_path_rules("host-1", &set).unwrap(),
                RulesChange::Invalid(_)
            ),
            "{:?}",
            set.first()
        );
        assert_eq!(hosts.path_rules("host-1").unwrap().unwrap(), before);
    }
    assert_eq!(
        hosts.replace_path_rules("host-nope", &[]).unwrap(),
        RulesChange::HostNotFound
    );
    assert_eq!(hosts.path_rules("host-nope").unwrap(), None);

    // A prefix kept keeps its id; the others are gone.
    let RulesChange::Done(after) = hosts
        .replace_path_rules("host-1", &[rule("/p/acme", &acme), rule("/p/c", &acme)])
        .unwrap()
    else {
        panic!("not replaced");
    };
    assert_eq!(after.iter().find(|r| r.prefix == "/p/acme").unwrap().id, kept_id);
    assert!(after.iter().all(|r| r.prefix != "/p/b"));
    // Every rule of every hat is in range of the cap exactly.
    let full: Vec<NewRule> = (0..MAX_RULES).map(|i| rule(&format!("/p/{i}"), &acme)).collect();
    assert_eq!(
        stored(hosts.replace_path_rules("host-1", &full).unwrap()).len(),
        MAX_RULES
    );
    assert_eq!(stored(hosts.replace_path_rules("host-1", &[]).unwrap()), []);
}

/// Umbrella §8.5: half of whether a host is mixed, for the gateway's
/// fallback: a rule naming a hat other than its default (the review's A1;
/// the other half is a live session of another hat).
#[test]
fn a_hosts_rules_name_another_hat_once_one_names_a_hat_but_its_default() {
    let hosts = Hosts::open_in_memory().unwrap();
    host(&hosts, "host-1", 1);
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    let acme = hat_id(hosts.create_hat("Acme", None, NOW).unwrap());
    assert!(!hosts.rules_name_other_hats("host-1").unwrap());
    stored(hosts.replace_path_rules("host-1", &[rule("/p/me", &personal)]).unwrap());
    assert!(!hosts.rules_name_other_hats("host-1").unwrap());
    stored(hosts.replace_path_rules("host-1", &[rule("/p/acme", &acme)]).unwrap());
    assert!(hosts.rules_name_other_hats("host-1").unwrap());
    // Its default hat becoming the rule's makes it single-hat again.
    hosts.update_host("host-1", None, Some(&acme)).unwrap();
    assert!(!hosts.rules_name_other_hats("host-1").unwrap());
    assert!(!hosts.rules_name_other_hats("host-nope").unwrap());
}

/// A second owner, written straight into the database, with a hat of its
/// own and a host whose default it is: nothing in v1 makes one. Kept
/// later than any real clock, so it is never the database's owner.
const OTHER: &str = "owner-00000000000000b2";
const OTHER_HAT: &str = "hat-00000000000000b2";

fn write_other_owner(db: &std::path::Path) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch(&format!(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES ('{OTHER}', {max}, {max});
         INSERT INTO hats(id, owner_id, name, colour, created_at) VALUES ('{OTHER_HAT}', '{OTHER}', 'Acme', '#000000', 1);
         INSERT INTO settings(owner_id, key, value) VALUES ('{OTHER}', 'default_hat_id', '{OTHER_HAT}');
         INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, default_hat_id, created_at)
             VALUES ('host-other', '{OTHER}', 'theirs', '{key}', 'linux', '0.0.0', '{OTHER_HAT}', 1);
         INSERT INTO hat_path_rules(id, owner_id, host_id, prefix, hat_id, verified)
             VALUES ('rule-other', '{OTHER}', 'host-other', '/p', '{OTHER_HAT}', 1);",
        max = i64::MAX,
        key = enrollment(9).public_key,
    ))
    .unwrap();
}

/// Kernel spec §1: every query names the owner. Another owner's hat is
/// not listed, not changed, not a name clash, and no host or rule of the
/// owner's can take it; their host and its rules are unknown here.
#[test]
fn another_owners_hats_and_rules_are_invisible_and_unusable() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    write_other_owner(&db);
    host(&hosts, "host-1", 1);

    let names: Vec<String> = hosts.hats().unwrap().into_iter().map(|h| h.name).collect();
    assert_eq!(names, ["Personal"]);
    assert_eq!(hosts.hat(OTHER_HAT).unwrap(), None);
    assert_ne!(hosts.default_hat_for_new_hosts().unwrap(), OTHER_HAT);
    assert!(matches!(
        hosts.create_hat("Acme", None, NOW).unwrap(),
        HatChange::Done(_)
    ));
    assert_eq!(
        hosts.update_hat(OTHER_HAT, Some("Mine"), None, Some(true)).unwrap(),
        HatChange::NotFound
    );
    assert!(matches!(
        hosts.update_host("host-1", None, Some(OTHER_HAT)).unwrap(),
        HostChange::Invalid(_)
    ));
    assert!(matches!(
        hosts.replace_path_rules("host-1", &[rule("/p", OTHER_HAT)]).unwrap(),
        RulesChange::Invalid(_)
    ));
    assert_eq!(hosts.path_rules("host-other").unwrap(), None);
    assert_eq!(hosts.resolve_hat("host-other", "/p/x").unwrap(), None);
    assert!(!hosts.rules_name_other_hats("host-other").unwrap());
    assert_eq!(
        hosts.update_host("host-other", Some("mine"), None).unwrap(),
        HostChange::NotFound
    );
    assert_eq!(
        hosts.replace_path_rules("host-other", &[]).unwrap(),
        RulesChange::HostNotFound
    );

    let conn = rusqlite::Connection::open(&db).unwrap();
    let theirs: (String, String, String) = conn
        .query_row(
            "SELECT h.name, s.value, (SELECT count(*) FROM hat_path_rules WHERE owner_id = ?1)
             FROM hats h JOIN settings s ON s.owner_id = h.owner_id AND s.key = 'default_hat_id'
             WHERE h.owner_id = ?1",
            [OTHER],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?.to_string())),
        )
        .unwrap();
    assert_eq!(theirs, ("Acme".into(), OTHER_HAT.into(), "1".into()));
}
