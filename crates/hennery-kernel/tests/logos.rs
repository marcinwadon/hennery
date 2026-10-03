//! Hat logos, the kernel's part (kernel spec §5.1; plan 4d-B2): a logo is
//! set, replaced and cleared on one of the owner's hats; a hat frozen for
//! its purge takes no new one but can lose its own (the review's A6); the
//! purge takes it with the hat's row; another owner's, written straight
//! into the database, is never served, set or cleared.

use hennery_kernel::hats::{HatChange, PurgeStart};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::logo::{Logo, MAX_STORED, MIME_PNG, reencode};
use rusqlite::Connection;

/// Later than any real clock reaches, as in `tests/hats.rs`.
const NOW: i64 = 4_000_000_000;

/// A 1 × 1 grey logo of shade `shade`, re-encoded as an upload is.
fn logo(shade: u8) -> Logo {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, 1, 1);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[shade]).unwrap();
    writer.finish().unwrap();
    reencode(&png, MAX_STORED).unwrap()
}

fn new_hat(hosts: &Hosts, name: &str) -> String {
    match hosts.create_hat(name, None, NOW).unwrap() {
        HatChange::Done(hat) => hat.id,
        other => panic!("expected a hat, got {other:?}"),
    }
}

/// The hat's logo as the registry lists it: its `ETag`, or `None`.
fn listed(hosts: &Hosts, hat: &str) -> Option<String> {
    hosts.hat(hat).unwrap().unwrap().logo
}

#[test]
fn a_logo_is_set_replaced_and_cleared() {
    let hosts = Hosts::open_in_memory().unwrap();
    let acme = new_hat(&hosts, "Acme");
    assert_eq!(listed(&hosts, &acme), None);
    assert_eq!(hosts.hat_logo(&acme).unwrap(), None);

    let first = logo(1);
    let HatChange::Done(hat) = hosts.set_hat_logo(&acme, &first).unwrap() else {
        panic!("expected the hat");
    };
    assert_eq!(hat.logo.as_deref(), Some(first.etag.as_str()));
    assert_eq!(listed(&hosts, &acme), Some(first.etag.clone()));
    let stored = hosts.hat_logo(&acme).unwrap().unwrap();
    assert_eq!(
        (stored.mime.as_str(), &stored.bytes, &stored.etag),
        (MIME_PNG, &first.bytes, &first.etag)
    );
    // Only that hat has one.
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    assert_eq!(listed(&hosts, &personal), None);

    let second = logo(2);
    assert!(matches!(
        hosts.set_hat_logo(&acme, &second).unwrap(),
        HatChange::Done(_)
    ));
    assert_eq!(hosts.hat_logo(&acme).unwrap().unwrap().bytes, second.bytes);
    assert_eq!(listed(&hosts, &acme), Some(second.etag));

    let HatChange::Done(hat) = hosts.clear_hat_logo(&acme).unwrap() else {
        panic!("expected the hat");
    };
    assert_eq!(hat.logo, None);
    assert_eq!(hosts.hat_logo(&acme).unwrap(), None);
    // Clearing again is no error: there is still the hat.
    assert!(matches!(hosts.clear_hat_logo(&acme).unwrap(), HatChange::Done(_)));
}

#[test]
fn an_unknown_hat_has_no_logo_to_set_or_clear() {
    let hosts = Hosts::open_in_memory().unwrap();
    assert_eq!(hosts.hat_logo("hat-nope").unwrap(), None);
    assert_eq!(hosts.set_hat_logo("hat-nope", &logo(1)).unwrap(), HatChange::NotFound);
    assert_eq!(hosts.clear_hat_logo("hat-nope").unwrap(), HatChange::NotFound);
}

/// Plan 9c decision 10c, the review's A6: a frozen hat takes no new logo,
/// in the same statement that would set it, and keeps the one it had; its
/// logo can still go, the purge's own direction. It is served meanwhile.
#[test]
fn a_frozen_hat_takes_no_new_logo_but_can_lose_its_own() {
    let hosts = Hosts::open_in_memory().unwrap();
    let acme = new_hat(&hosts, "Acme");
    let kept = logo(1);
    assert!(matches!(hosts.set_hat_logo(&acme, &kept).unwrap(), HatChange::Done(_)));
    assert_eq!(hosts.begin_purge(&acme, NOW).unwrap(), PurgeStart::Frozen { rules: 0 });

    assert_eq!(hosts.set_hat_logo(&acme, &logo(2)).unwrap(), HatChange::Purging);
    assert_eq!(hosts.hat_logo(&acme).unwrap().unwrap().bytes, kept.bytes);
    assert_eq!(listed(&hosts, &acme), Some(kept.etag));

    let HatChange::Done(hat) = hosts.clear_hat_logo(&acme).unwrap() else {
        panic!("expected the hat");
    };
    assert!(hat.purging);
    assert_eq!(hat.logo, None);
    assert_eq!(hosts.hat_logo(&acme).unwrap(), None);
}

/// Kernel spec §5.5: the purge's last step deletes the hat's row, and its
/// logo with it.
#[test]
fn the_purge_takes_the_logo_with_the_hat() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let acme = new_hat(&hosts, "Acme");
    assert!(matches!(
        hosts.set_hat_logo(&acme, &logo(1)).unwrap(),
        HatChange::Done(_)
    ));
    assert_eq!(hosts.begin_purge(&acme, NOW).unwrap(), PurgeStart::Frozen { rules: 0 });
    assert!(hosts.finish_purge(&acme).unwrap());
    assert_eq!(hosts.hat_logo(&acme).unwrap(), None);
    let conn = Connection::open(&db).unwrap();
    let logos: i64 = conn
        .query_row("SELECT count(*) FROM hats WHERE logo_bytes IS NOT NULL", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(logos, 0);
}

/// A second owner, written straight into the database, with a hat and a
/// logo of its own.
const OTHER: &str = "owner-00000000000000b2";
const OTHER_HAT: &str = "hat-00000000000000b2";

/// Kernel spec §1: every query names the owner. Another owner's logo is
/// not served, replaced or cleared, and its hat stays as it was.
#[test]
fn another_owners_logo_is_never_served_set_or_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let theirs = logo(9);
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        &format!(
            "INSERT INTO owners(id, created_at, set_up_at) VALUES ('{OTHER}', {max}, {max})",
            max = i64::MAX
        ),
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO hats(id, owner_id, name, colour, created_at, logo_mime, logo_bytes, logo_etag)
         VALUES (?1, ?2, 'Acme', '#000000', 1, 'image/png', ?3, ?4)",
        rusqlite::params![OTHER_HAT, OTHER, theirs.bytes, theirs.etag],
    )
    .unwrap();

    assert_eq!(hosts.hat_logo(OTHER_HAT).unwrap(), None);
    assert_eq!(hosts.set_hat_logo(OTHER_HAT, &logo(1)).unwrap(), HatChange::NotFound);
    assert_eq!(hosts.clear_hat_logo(OTHER_HAT).unwrap(), HatChange::NotFound);
    let stored: (Vec<u8>, String) = conn
        .query_row(
            "SELECT logo_bytes, logo_etag FROM hats WHERE id = ?1",
            [OTHER_HAT],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored, (theirs.bytes, theirs.etag));
}
