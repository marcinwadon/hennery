//! The gateway's store (gateway spec §2, §4.6, §6): connections, their
//! mounts and their static credential, in `hennery.db` beside the kernel's
//! tables, under the kernel's owner.

use ed25519_dalek::SigningKey;
use hennery_gateway::key::MasterKey;
use hennery_gateway::model::{Change, ConnectionPatch, CredKind, CredentialChange, MAX_CONNECTIONS, NewConnection};
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hats::HatChange;
use hennery_kernel::hosts::{Enrollment, Hosts};
use std::path::PathBuf;

const NOW: i64 = 1_800_000_000;
const TOKEN: &str = "ghp_s3cr3tT0kenThatMustNeverLeak";

struct World {
    _dir: tempfile::TempDir,
    db: PathBuf,
    hosts: Hosts,
    store: GatewayStore,
    key: MasterKey,
    hat: String,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        let store = GatewayStore::open(&db).unwrap();
        let hat = hosts.default_hat_for_new_hosts().unwrap();
        Self {
            _dir: dir,
            db,
            hosts,
            store,
            key: MasterKey::from_bytes([7; 32]),
            hat,
        }
    }

    fn hat(&self, name: &str) -> String {
        let HatChange::Done(hat) = self.hosts.create_hat(name, None, NOW).unwrap() else {
            panic!("no hat");
        };
        hat.id
    }

    fn host(&self, id: &str, seed: u8) {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let enrollment = Enrollment {
            public_key: hex::encode(key.verifying_key().as_bytes()),
            name: format!("host {seed}"),
            host_version: "0.0.0".into(),
            platform: "macos-aarch64".into(),
        };
        self.hosts.register(id, &enrollment, NOW).unwrap();
    }

    fn new_connection(&self, slug: &str) -> NewConnection {
        NewConnection {
            slug: slug.into(),
            label: "Linear".into(),
            url: "https://mcp.linear.example/sse".into(),
            hat_id: self.hat.clone(),
            cred_kind: CredKind::Static,
            static_header: None,
            static_prefix: None,
            tool_allowlist: None,
            internal_network: false,
        }
    }

    fn create(&self, slug: &str) -> String {
        match self.store.create(&self.new_connection(slug), NOW).unwrap() {
            Change::Done(record) => record.id,
            other => panic!("not created: {other:?}"),
        }
    }

    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.db).unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        self.sql()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}

fn invalid(change: Change) -> String {
    match change {
        Change::Invalid(why) => why,
        other => panic!("not invalid: {other:?}"),
    }
}

fn done(change: Change) -> hennery_gateway::model::ConnectionRecord {
    match change {
        Change::Done(record) => *record,
        other => panic!("not done: {other:?}"),
    }
}

#[test]
fn a_connection_is_created_with_its_defaults_and_listed() {
    let w = World::new();
    let record = done(w.store.create(&w.new_connection("linear"), NOW).unwrap());
    assert!(record.id.starts_with("conn-") && record.id.len() == 21, "{}", record.id);
    assert_eq!(record.slug, "linear");
    assert_eq!(record.url, "https://mcp.linear.example/sse");
    assert_eq!(record.hat_id, w.hat);
    assert_eq!(record.cred_kind, CredKind::Static);
    assert_eq!(record.static_header, "Authorization");
    assert_eq!(record.static_prefix, "Bearer ");
    assert_eq!(record.tool_allowlist, None);
    assert!(!record.internal_network);
    assert_eq!(record.status, "not_connected");
    assert_eq!(
        (record.status_at, record.created_at, record.updated_at),
        (NOW, NOW, NOW)
    );
    assert!(!record.has_credential);
    assert!(record.mounts.is_empty());
    assert_eq!(w.store.list().unwrap(), vec![record.clone()]);
    assert_eq!(w.store.connection(&record.id).unwrap(), Some(record));
    assert_eq!(w.store.connection("conn-0000000000000000").unwrap(), None);
    // A URL is stored as parsed: an empty path becomes `/`.
    let mut bare = w.new_connection("bare");
    bare.url = "HTTPS://Example.COM".into();
    assert_eq!(done(w.store.create(&bare, NOW).unwrap()).url, "https://example.com/");
}

/// One change to a connection that makes it invalid.
type Edit = Box<dyn Fn(&mut NewConnection)>;

#[test]
fn what_a_connection_is_made_of_is_checked() {
    let w = World::new();
    let base = w.new_connection("ok");
    let cases: Vec<(&str, Edit)> = vec![
        ("slug", Box::new(|c| c.slug = "".into())),
        ("slug", Box::new(|c| c.slug = "-lead".into())),
        ("slug", Box::new(|c| c.slug = "Upper".into())),
        ("slug", Box::new(|c| c.slug = "under_score".into())),
        ("slug", Box::new(|c| c.slug = "a".repeat(49))),
        ("label", Box::new(|c| c.label = " ".into())),
        ("label", Box::new(|c| c.label = "x".repeat(65))),
        ("label", Box::new(|c| c.label = "bidi\u{202e}".into())),
        ("url", Box::new(|c| c.url = "not a url".into())),
        ("url", Box::new(|c| c.url = "ftp://files.example/".into())),
        ("url", Box::new(|c| c.url = "https://user:pass@mcp.example/".into())),
        ("url", Box::new(|c| c.url = "https://user@mcp.example/".into())),
        ("url", Box::new(|c| c.url = "https://mcp.example/#frag".into())),
        (
            "url",
            Box::new(|c| c.url = format!("https://mcp.example/{}", "a".repeat(2048))),
        ),
        ("http", Box::new(|c| c.url = "http://mcp.example/".into())),
        ("header", Box::new(|c| c.static_header = Some("".into()))),
        ("header", Box::new(|c| c.static_header = Some("Bad Header".into()))),
        ("header", Box::new(|c| c.static_header = Some("Host".into()))),
        ("header", Box::new(|c| c.static_header = Some("content-length".into()))),
        ("header", Box::new(|c| c.static_header = Some("Mcp-Session-Id".into()))),
        ("header", Box::new(|c| c.static_header = Some("Cookie".into()))),
        ("header", Box::new(|c| c.static_header = Some("Forwarded".into()))),
        ("header", Box::new(|c| c.static_header = Some("Proxy-Foo".into()))),
        ("prefix", Box::new(|c| c.static_prefix = Some("Bearer\n".into()))),
        ("prefix", Box::new(|c| c.static_prefix = Some("x".repeat(33)))),
        ("tool", Box::new(|c| c.tool_allowlist = Some(vec!["".into()]))),
        ("tool", Box::new(|c| c.tool_allowlist = Some(vec!["two words".into()]))),
        ("tool", Box::new(|c| c.tool_allowlist = Some(vec!["x".repeat(129)]))),
        ("tool", Box::new(|c| c.tool_allowlist = Some(vec!["t".into(); 1025]))),
    ];
    for (about, change) in cases {
        let mut bad = base.clone();
        change(&mut bad);
        let why = invalid(w.store.create(&bad, NOW).unwrap());
        assert!(why.contains(about), "{about}: {why}");
    }
    assert_eq!(w.count("gw_connections"), 0);
    // `http` only to an upstream the operator marked internal (plan 8a
    // decision 9); other headers and prefixes, and the allowlist, as given.
    let mut internal = base.clone();
    internal.url = "http://10.0.0.5:8080/mcp".into();
    internal.internal_network = true;
    internal.static_header = Some("X-API-Key".into());
    internal.static_prefix = Some("".into());
    internal.tool_allowlist = Some(vec!["search".into(), "fetch".into(), "search".into()]);
    let record = done(w.store.create(&internal, NOW).unwrap());
    assert_eq!(record.static_header, "X-API-Key");
    assert_eq!(record.static_prefix, "");
    assert_eq!(record.tool_allowlist, Some(vec!["search".into(), "fetch".into()]));
    assert!(record.internal_network);
}

#[test]
fn a_slug_is_unique_per_owner_and_a_hat_must_be_the_owners() {
    let w = World::new();
    w.create("linear");
    assert!(matches!(
        w.store.create(&w.new_connection("linear"), NOW).unwrap(),
        Change::SlugTaken
    ));
    let mut elsewhere = w.new_connection("other");
    elsewhere.hat_id = "hat-0000000000000000".into();
    assert!(invalid(w.store.create(&elsewhere, NOW).unwrap()).contains("hat"));
    // Another hat of the owner's is fine.
    let mut work = w.new_connection("work");
    work.hat_id = w.hat("Work");
    assert_eq!(done(w.store.create(&work, NOW).unwrap()).hat_id, work.hat_id);
}

/// Plan 8f: the OAuth kinds are taken, on create and by a change, and an
/// OAuth connection shows its (empty) client in the list.
#[test]
fn oauth_kinds_are_taken() {
    let w = World::new();
    for kind in [CredKind::OauthDcr, CredKind::OauthClient] {
        let slug = format!("o-{}", kind.as_str().replace('_', "-"));
        let mut oauth = w.new_connection(&slug);
        oauth.cred_kind = kind;
        let created = done(w.store.create(&oauth, NOW).unwrap());
        assert_eq!(created.cred_kind, kind);
        assert_eq!(created.oauth_client, Some(Default::default()));
        let id = w.create(&format!("s-{}", kind.as_str().replace('_', "-")));
        assert_eq!(w.store.connection(&id).unwrap().unwrap().oauth_client, None);
        let patch = ConnectionPatch {
            cred_kind: Some(kind),
            ..ConnectionPatch::default()
        };
        assert_eq!(done(w.store.update(&id, &patch, NOW).unwrap()).cred_kind, kind);
    }
    assert_eq!(w.count("gw_connections"), 4);
}

#[test]
fn there_are_at_most_so_many_connections() {
    let w = World::new();
    for i in 0..MAX_CONNECTIONS {
        w.create(&format!("c{i}"));
    }
    assert!(matches!(
        w.store.create(&w.new_connection("one-more"), NOW).unwrap(),
        Change::TooMany
    ));
}

#[test]
fn an_update_changes_what_it_names_and_keeps_the_rest() {
    let w = World::new();
    let mut new = w.new_connection("linear");
    new.tool_allowlist = Some(vec!["search".into()]);
    let before = done(w.store.create(&new, NOW).unwrap());
    let id = before.id.clone();
    // Nothing named: nothing changes but the stamp.
    let same = done(w.store.update(&id, &ConnectionPatch::default(), NOW + 1).unwrap());
    assert_eq!(
        hennery_gateway::model::ConnectionRecord {
            updated_at: NOW,
            ..same.clone()
        },
        before
    );
    assert_eq!(same.updated_at, NOW + 1);
    let renamed = done(
        w.store
            .update(
                &id,
                &ConnectionPatch {
                    label: Some("Linear (work)".into()),
                    static_prefix: Some("".into()),
                    ..ConnectionPatch::default()
                },
                NOW + 2,
            )
            .unwrap(),
    );
    assert_eq!(renamed.label, "Linear (work)");
    assert_eq!(renamed.static_prefix, "");
    assert_eq!(renamed.tool_allowlist, Some(vec!["search".into()]));
    // An explicit `null` clears the allowlist (all tools); `[]` allows none.
    let cleared = ConnectionPatch {
        tool_allowlist: Some(None),
        ..ConnectionPatch::default()
    };
    assert_eq!(done(w.store.update(&id, &cleared, NOW).unwrap()).tool_allowlist, None);
    let none = ConnectionPatch {
        tool_allowlist: Some(Some(vec![])),
        ..ConnectionPatch::default()
    };
    assert_eq!(
        done(w.store.update(&id, &none, NOW).unwrap()).tool_allowlist,
        Some(vec![])
    );
    // Checked as on create, and nothing changed when refused.
    let bad = ConnectionPatch {
        label: Some("".into()),
        ..ConnectionPatch::default()
    };
    assert!(invalid(w.store.update(&id, &bad, NOW).unwrap()).contains("label"));
    assert!(matches!(
        w.store
            .update("conn-0000000000000000", &ConnectionPatch::default(), NOW)
            .unwrap(),
        Change::NotFound
    ));
}

#[test]
fn http_stays_only_while_the_connection_is_internal() {
    let w = World::new();
    let mut new = w.new_connection("lan");
    new.url = "http://10.0.0.5/mcp".into();
    new.internal_network = true;
    let id = done(w.store.create(&new, NOW).unwrap()).id;
    let public = ConnectionPatch {
        internal_network: Some(false),
        ..ConnectionPatch::default()
    };
    assert!(invalid(w.store.update(&id, &public, NOW).unwrap()).contains("http"));
    let both = ConnectionPatch {
        internal_network: Some(false),
        url: Some("https://mcp.example/".into()),
        ..ConnectionPatch::default()
    };
    assert!(!done(w.store.update(&id, &both, NOW).unwrap()).internal_network);
}

/// Gateway spec §4.6 (G-14): another origin or another kind deletes the
/// credential before the change is saved, and the status starts over.
#[test]
fn another_origin_or_kind_deletes_the_credential() {
    let w = World::new();
    let id = w.create("linear");
    let set = || {
        assert_eq!(
            w.store.set_static_credential(&id, TOKEN, &w.key, NOW).unwrap(),
            CredentialChange::Done
        );
        assert!(w.store.connection(&id).unwrap().unwrap().has_credential);
    };
    let has = || w.store.connection(&id).unwrap().unwrap().has_credential;
    set();
    w.sql()
        .execute(
            "UPDATE gw_connections SET status = 'ok', status_note = 'fine', account_label = 'me' WHERE id = ?1",
            [&id],
        )
        .unwrap();
    // The same origin, another path; another header and prefix: kept.
    for patch in [
        ConnectionPatch {
            url: Some("https://mcp.linear.example/v2/mcp?team=1".into()),
            ..ConnectionPatch::default()
        },
        ConnectionPatch {
            static_header: Some("X-API-Key".into()),
            static_prefix: Some("".into()),
            ..ConnectionPatch::default()
        },
        ConnectionPatch {
            cred_kind: Some(CredKind::Static),
            ..ConnectionPatch::default()
        },
    ] {
        let record = done(w.store.update(&id, &patch, NOW + 1).unwrap());
        assert!(record.has_credential, "{patch:?}");
        assert_eq!(record.status, "ok");
    }
    // Another host, scheme or port: deleted, and not connected any more.
    for url in [
        "https://mcp.other.example/v2/mcp",
        "https://mcp.other.example:8443/v2/mcp",
        "http://mcp.other.example:8443/v2/mcp",
    ] {
        set();
        w.sql()
            .execute(
                "UPDATE gw_connections SET status = 'ok', account_label = 'me' WHERE id = ?1",
                [&id],
            )
            .unwrap();
        let patch = ConnectionPatch {
            url: Some(url.into()),
            internal_network: Some(true),
            ..ConnectionPatch::default()
        };
        let record = done(w.store.update(&id, &patch, NOW + 2).unwrap());
        assert!(!record.has_credential, "{url}");
        assert_eq!(
            (
                record.status.as_str(),
                record.status_note,
                record.account_label,
                record.status_at
            ),
            ("not_connected", None, None, NOW + 2)
        );
        assert_eq!(w.count("gw_credentials"), 0);
    }
    // Another kind.
    set();
    let none = ConnectionPatch {
        cred_kind: Some(CredKind::None),
        ..ConnectionPatch::default()
    };
    let record = done(w.store.update(&id, &none, NOW).unwrap());
    assert_eq!(record.cred_kind, CredKind::None);
    assert!(!has());
    assert_eq!(w.count("gw_credentials"), 0);
    // A refused change deletes nothing.
    let back = ConnectionPatch {
        cred_kind: Some(CredKind::Static),
        ..ConnectionPatch::default()
    };
    done(w.store.update(&id, &back, NOW).unwrap());
    set();
    let refused = ConnectionPatch {
        url: Some("http://mcp.elsewhere.example/".into()),
        internal_network: Some(false),
        ..ConnectionPatch::default()
    };
    invalid(w.store.update(&id, &refused, NOW).unwrap());
    assert!(has());
}

#[test]
fn a_static_credential_is_stored_sealed_and_opens_again() {
    let w = World::new();
    let id = w.create("linear");
    assert_eq!(
        w.store.set_static_credential(&id, TOKEN, &w.key, NOW).unwrap(),
        CredentialChange::Done
    );
    // The token with where it goes, from one read (the review's R2).
    let stored = w.store.static_credential(&id, &w.key).unwrap().unwrap();
    assert_eq!(stored.token.as_str(), TOKEN);
    assert_eq!(
        (
            stored.url.as_str(),
            stored.static_header.as_str(),
            stored.static_prefix.as_str()
        ),
        ("https://mcp.linear.example/sse", "Authorization", "Bearer ")
    );
    assert!(!stored.internal_network);
    assert!(!format!("{stored:?}").contains(TOKEN), "{stored:?}");
    // Replaced, not added.
    w.store
        .set_static_credential(&id, "second-token", &w.key, NOW + 1)
        .unwrap();
    assert_eq!(w.count("gw_credentials"), 1);
    assert_eq!(
        w.store.static_credential(&id, &w.key).unwrap().unwrap().token.as_str(),
        "second-token"
    );
    let (version, blob): (i64, Vec<u8>) = w
        .sql()
        .query_row(
            "SELECT key_version, ciphertext FROM gw_credentials WHERE connection_id = ?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(version, 1);
    assert!(!blob.windows(12).any(|w| w == b"second-token"));
    // Another key opens nothing, and says so.
    let other = MasterKey::from_bytes([8; 32]);
    assert!(w.store.static_credential(&id, &other).is_err());
    assert!(w.store.check_key(&w.key).is_ok());
    assert!(w.store.check_key(&other).is_err());
    // A blob moved to another connection does not open there (AAD).
    let moved = w.create("moved");
    w.sql()
        .execute(
            "INSERT INTO gw_credentials(connection_id, owner_id, key_version, ciphertext, updated_at)
             SELECT ?2, owner_id, key_version, ciphertext, updated_at FROM gw_credentials WHERE connection_id = ?1",
            [&id, &moved],
        )
        .unwrap();
    assert!(w.store.static_credential(&moved, &w.key).is_err());
}

#[test]
fn a_credential_goes_only_to_a_static_connection_and_is_checked() {
    let w = World::new();
    let mut none = w.new_connection("public");
    none.cred_kind = CredKind::None;
    let none = done(w.store.create(&none, NOW).unwrap()).id;
    assert_eq!(
        w.store.set_static_credential(&none, TOKEN, &w.key, NOW).unwrap(),
        CredentialChange::WrongKind(CredKind::None)
    );
    let id = w.create("linear");
    for bad in [
        "",
        "two words",
        "line\nbreak",
        "tab\there",
        "ünïcode",
        &"x".repeat(8193),
    ] {
        assert!(
            matches!(
                w.store.set_static_credential(&id, bad, &w.key, NOW).unwrap(),
                CredentialChange::Invalid(_)
            ),
            "{bad:?}"
        );
    }
    assert_eq!(
        w.store
            .set_static_credential("conn-0000000000000000", TOKEN, &w.key, NOW)
            .unwrap(),
        CredentialChange::NotFound
    );
    assert_eq!(w.count("gw_credentials"), 0);
    assert!(!w.store.has_ciphertext().unwrap());
    w.store.set_static_credential(&id, TOKEN, &w.key, NOW).unwrap();
    assert!(w.store.has_ciphertext().unwrap());
}

#[test]
fn mounts_are_replaced_as_a_set_of_the_owners_live_hosts() {
    let w = World::new();
    w.host("host-a", 1);
    w.host("host-b", 2);
    w.host("host-c", 3);
    let id = w.create("linear");
    let mounts = |hosts: &[&str]| {
        let hosts: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
        w.store.replace_mounts(&id, &hosts).unwrap()
    };
    assert_eq!(
        done(mounts(&["host-b", "host-a", "host-b"])).mounts,
        ["host-a", "host-b"]
    );
    assert_eq!(done(mounts(&["host-c"])).mounts, ["host-c"]);
    // An unknown host refuses the whole set.
    assert!(invalid(mounts(&["host-a", "host-x"])).contains("host-x"));
    assert_eq!(w.store.connection(&id).unwrap().unwrap().mounts, ["host-c"]);
    // A revoked one too; and its mounts are no longer listed (plan 8a
    // decision 12), while the row stays until the set is next replaced.
    done(mounts(&["host-a", "host-c"]));
    w.hosts.revoke("host-a", NOW).unwrap();
    assert!(invalid(mounts(&["host-a"])).contains("host-a"));
    assert_eq!(w.store.connection(&id).unwrap().unwrap().mounts, ["host-c"]);
    assert_eq!(w.store.list().unwrap()[0].mounts, ["host-c"]);
    assert_eq!(w.count("gw_mounts"), 2);
    assert!(done(mounts(&[])).mounts.is_empty());
    assert_eq!(w.count("gw_mounts"), 0);
    // Bounded before anything is read; a long id is not echoed back.
    let many: Vec<String> = (0..=hennery_gateway::model::MAX_MOUNTS)
        .map(|i| format!("host-{i}"))
        .collect();
    assert!(invalid(w.store.replace_mounts(&id, &many).unwrap()).contains("at most"));
    let long = "h".repeat(hennery_gateway::model::MAX_HOST_ID + 1);
    let why = invalid(w.store.replace_mounts(&id, std::slice::from_ref(&long)).unwrap());
    assert!(why.contains("at most") && !why.contains(&long), "{why}");
    assert!(matches!(
        w.store.replace_mounts("conn-0000000000000000", &[]).unwrap(),
        Change::NotFound
    ));
}

#[test]
fn a_deleted_connection_takes_its_mounts_and_credential_with_it() {
    let w = World::new();
    w.host("host-a", 1);
    let id = w.create("linear");
    let kept = w.create("kept");
    for connection in [&id, &kept] {
        w.store.replace_mounts(connection, &["host-a".into()]).unwrap();
        w.store.set_static_credential(connection, TOKEN, &w.key, NOW).unwrap();
    }
    assert!(w.store.delete(&id).unwrap());
    assert!(!w.store.delete(&id).unwrap());
    assert_eq!(w.store.connection(&id).unwrap(), None);
    assert_eq!((w.count("gw_mounts"), w.count("gw_credentials")), (1, 1));
    assert!(w.store.connection(&kept).unwrap().unwrap().has_credential);
    // The slug is free again.
    w.create("linear");
}

/// Gateway spec §2, kernel spec §5.5 (lane L6): a hat's purge deletes its
/// connections with their credentials and mounts, nothing of another hat's,
/// and repeats harmlessly; the hat's own row can go after it.
#[test]
fn purging_a_hat_deletes_its_connections_and_repeats_harmlessly() {
    let w = World::new();
    w.host("host-a", 1);
    let work = w.hat("Work");
    let mut in_work = w.new_connection("work-linear");
    in_work.hat_id = work.clone();
    let gone = done(w.store.create(&in_work, NOW).unwrap()).id;
    let kept = w.create("personal-linear");
    for connection in [&gone, &kept] {
        w.store.replace_mounts(connection, &["host-a".into()]).unwrap();
        w.store.set_static_credential(connection, TOKEN, &w.key, NOW).unwrap();
    }
    // The hat cannot go while it has connections (no cascade, L6).
    assert!(w.sql().execute("DELETE FROM hats WHERE id = ?1", [&work]).is_err());
    w.store.purge_hat(&work).unwrap();
    assert_eq!(w.store.connection(&gone).unwrap(), None);
    assert!(w.store.connection(&kept).unwrap().unwrap().has_credential);
    assert_eq!(
        (
            w.count("gw_connections"),
            w.count("gw_mounts"),
            w.count("gw_credentials")
        ),
        (1, 1, 1)
    );
    w.store.purge_hat(&work).unwrap();
    w.sql().execute("DELETE FROM hats WHERE id = ?1", [&work]).unwrap();
    w.store.purge_hat(&work).unwrap();
    w.store.purge_hat("hat-0000000000000000").unwrap();
    assert_eq!(w.count("gw_connections"), 1);
}

#[test]
fn the_store_and_the_kernel_agree_on_the_owner_whichever_opens_first() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("gateway-first.db");
    let store = GatewayStore::open(&first).unwrap();
    assert_eq!(Hosts::open(&first).unwrap().owner_id(), store.owner_id());
    let last = dir.path().join("gateway-last.db");
    let hosts = Hosts::open(&last).unwrap();
    assert_eq!(GatewayStore::open(&last).unwrap().owner_id(), hosts.owner_id());
    // Opened again, nothing is migrated twice.
    GatewayStore::open(&last).unwrap();
    let version: i64 = rusqlite::Connection::open(&last)
        .unwrap()
        .query_row(
            "SELECT version FROM schema_versions WHERE component = 'gateway'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Plan 8a's tables, then plan 8d's session tokens, then plan 8f's OAuth
    // clients.
    assert_eq!(version, 3);
}

/// Plan 8a decision 19 (lane L11): a connection's `Debug` shows only its
/// URL's origin, never the path or query some vendors put a secret in.
#[test]
fn a_connections_debug_shows_only_its_urls_origin() {
    let w = World::new();
    let canary = "c4n4ry-path-secret";
    let mut new = w.new_connection("linear");
    new.url = format!("https://mcp.vendor.example:8443/s/{canary}/mcp?k={canary}");
    let record = done(w.store.create(&new, NOW).unwrap());
    let patch = ConnectionPatch {
        url: Some(new.url.clone()),
        ..ConnectionPatch::default()
    };
    for shown in [format!("{record:?}"), format!("{new:?}"), format!("{patch:?}")] {
        assert!(!shown.contains(canary), "{shown}");
        assert!(shown.contains("https://mcp.vendor.example:8443"), "{shown}");
    }
    assert_eq!(
        hennery_gateway::model::url_for_logs("https://user:pw@h.example/x"),
        "https://h.example"
    );
    assert_eq!(hennery_gateway::model::url_for_logs("not a url"), "<not a url>");
}
