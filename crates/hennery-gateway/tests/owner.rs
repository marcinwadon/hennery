//! `owner_id` in the gateway's store (kernel spec §1, lane L6): every row
//! carries it, and another owner's connections, credentials and mounts are
//! invisible to, and unchanged by, every method of the store.

use hennery_gateway::key::MasterKey;
use hennery_gateway::model::{Change, ConnectionPatch, CredKind, CredentialChange, NewConnection};
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hosts::Hosts;
use rusqlite::types::Value;

/// A second owner, written straight into the database: nothing in v1
/// makes one. Its `created_at` is later than any real clock reaches, so it
/// is never the database's owner (3b-iii review, A5).
const OTHER: &str = "owner-00000000000000b2";
const OTHER_CREATED_AT: i64 = i64::MAX;
const THEIR_HAT: &str = "hat-00000000000000b2";
const THEIR_HOST: &str = "host-00000000000000b2";
const THEIRS: &str = "conn-00000000000000b2";

const TABLES: &[&str] = &["gw_connections", "gw_credentials", "gw_mounts"];

fn rows(conn: &rusqlite::Connection) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for table in TABLES {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
        let width = stmt.column_count();
        let found = stmt
            .query_map([], |r| {
                (0..width).map(|i| r.get::<_, Value>(i)).collect::<Result<Vec<_>, _>>()
            })
            .unwrap();
        out.extend(found.map(Result::unwrap));
    }
    out
}

/// The other owner, with a hat, a host, and a static connection named
/// `linear` in that hat, credentialled and mounted on that host.
fn write_other_owner(conn: &rusqlite::Connection, key: &MasterKey) {
    conn.execute_batch(&format!(
        "
        INSERT INTO owners(id, created_at, set_up_at) VALUES ('{OTHER}', {OTHER_CREATED_AT}, {OTHER_CREATED_AT});
        INSERT INTO hats(id, owner_id, name, colour, created_at) VALUES ('{THEIR_HAT}', '{OTHER}', 'Theirs', '#000000', 0);
        INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, default_hat_id, created_at)
            VALUES ('{THEIR_HOST}', '{OTHER}', 'theirs', 'b2', 'linux-x86_64', '0.0.0', '{THEIR_HAT}', 0);
        INSERT INTO gw_connections(id, owner_id, slug, label, url, hat_id, cred_kind, internal_network,
                                   status_at, created_at, updated_at)
            VALUES ('{THEIRS}', '{OTHER}', 'linear', 'Theirs', 'https://theirs.example/', '{THEIR_HAT}', 'static',
                    0, 0, 0, 0);
        INSERT INTO gw_mounts(connection_id, host_id, owner_id) VALUES ('{THEIRS}', '{THEIR_HOST}', '{OTHER}');
        "
    ))
    .unwrap();
    let blob = hennery_gateway::crypto::seal(key, THEIRS, hennery_gateway::crypto::STATIC_TOKEN, b"their-token");
    conn.execute(
        "INSERT INTO gw_credentials(connection_id, owner_id, key_version, ciphertext, updated_at)
         VALUES (?1, ?2, 1, ?3, 0)",
        rusqlite::params![THEIRS, OTHER, blob],
    )
    .unwrap();
}

#[test]
fn another_owners_connections_are_invisible_to_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let store = GatewayStore::open(&db).unwrap();
    let key = MasterKey::from_bytes([7; 32]);
    let conn = rusqlite::Connection::open(&db).unwrap();
    write_other_owner(&conn, &key);
    assert!(!store.has_ciphertext().unwrap());
    let before = rows(&conn);

    // Theirs is not found, not changed, not deleted.
    assert!(store.list().unwrap().is_empty());
    assert_eq!(store.connection(THEIRS).unwrap(), None);
    let patch = ConnectionPatch {
        label: Some("Mine now".into()),
        url: Some("https://elsewhere.example/".into()),
        ..ConnectionPatch::default()
    };
    assert!(matches!(store.update(THEIRS, &patch, 1).unwrap(), Change::NotFound));
    assert!(matches!(store.replace_mounts(THEIRS, &[]).unwrap(), Change::NotFound));
    assert_eq!(
        store.set_static_credential(THEIRS, "mine", &key, 1).unwrap(),
        CredentialChange::NotFound
    );
    assert_eq!(store.static_credential(THEIRS, &key).unwrap(), None);
    assert!(!store.delete(THEIRS).unwrap());
    let _ = store.purge_hat(THEIR_HAT).unwrap();
    store.check_key(&MasterKey::from_bytes([9; 32])).unwrap();
    assert_eq!(rows(&conn), before);

    // Their hat and host are not the owner's either.
    let new = |slug: &str, hat: &str| NewConnection {
        slug: slug.into(),
        label: "Mine".into(),
        url: "https://mine.example/".into(),
        hat_id: hat.into(),
        cred_kind: CredKind::Static,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: false,
    };
    assert!(matches!(
        store.create(&new("mine", THEIR_HAT), 1).unwrap(),
        Change::Invalid(_)
    ));
    assert_eq!(rows(&conn), before);
    // Their slug is the owner's to take (unique per owner, decision 1).
    let hat = hosts.default_hat_for_new_hosts().unwrap();
    let Change::Done(mine) = store.create(&new("linear", &hat), 1).unwrap() else {
        panic!("not created");
    };
    assert!(matches!(
        store.replace_mounts(&mine.id, &[THEIR_HOST.into()]).unwrap(),
        Change::Invalid(_)
    ));
    // The owner's own purge of their own hat leaves theirs alone.
    let _ = store.purge_hat(&hat).unwrap();
    assert_eq!(rows(&conn), before);
}
