//! Opening the gateway on the collector's database (plan 8a decision 8): a
//! key that is missing while credentials are stored, or that does not open
//! them, is `KeyUnavailable`, told apart from a store that does not open,
//! so the collector could serve with the gateway off instead of refusing.

use hennery_gateway::key::{KEY_FILE, KeySource};
use hennery_gateway::model::{Change, CredKind, NewConnection};
use hennery_gateway::{KeyUnavailable, open};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use std::path::Path;
use std::sync::Arc;

fn keys(dir: &Path) -> KeySource {
    KeySource::from_vars(dir, None, None).unwrap()
}

#[test]
fn a_key_that_cannot_be_had_is_told_apart_from_a_store_that_does_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let operator = Arc::new(Operator::open(&db).unwrap());
    // The first start makes the key.
    let gateway = open(&db, &keys(dir.path()), operator.clone()).unwrap();
    assert!(dir.path().join(KEY_FILE).exists());
    let hat = Hosts::open(&db).unwrap().default_hat_for_new_hosts().unwrap();
    let new = NewConnection {
        slug: "linear".into(),
        label: "Linear".into(),
        url: "https://mcp.linear.example/mcp".into(),
        hat_id: hat,
        cred_kind: CredKind::Static,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: false,
    };
    let Change::Done(record) = gateway.store.create(&new, 1).unwrap() else {
        panic!("not created");
    };
    gateway
        .store
        .set_static_credential(&record.id, "tok", &gateway.key, 1)
        .unwrap();
    drop(gateway);
    // Opened again: the same key opens the credential.
    open(&db, &keys(dir.path()), operator.clone()).unwrap();

    // Another key, then none.
    std::fs::write(dir.path().join(KEY_FILE), [9u8; 32]).unwrap();
    let err = open(&db, &keys(dir.path()), operator.clone()).err().unwrap();
    assert!(err.is::<KeyUnavailable>(), "{err:#}");
    std::fs::remove_file(dir.path().join(KEY_FILE)).unwrap();
    let err = open(&db, &keys(dir.path()), operator.clone()).err().unwrap();
    assert!(err.is::<KeyUnavailable>(), "{err:#}");
    assert!(format!("{err:#}").contains("is missing"), "{err:#}");
    assert!(!dir.path().join(KEY_FILE).exists());

    // A database that does not open is not about the key.
    let err = open(
        &dir.path().join("no-such-dir").join("hennery.db"),
        &keys(dir.path()),
        operator,
    )
    .err()
    .unwrap();
    assert!(!err.is::<KeyUnavailable>(), "{err:#}");
}
