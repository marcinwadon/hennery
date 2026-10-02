//! Local stdio servers (gateway spec §3.4; plan 8e decisions E1–E5): one
//! set per (host, hat), replaced whole; values write-only, sealed per row
//! and bound to its host and hat; names disjoint from connection slugs both
//! ways; a hat's purge takes them.

mod support;

use hennery_gateway::model::{Change, CredKind};
use hennery_gateway::stdio::{MAX_OWNER, StdioChange, StdioInput, StdioServer};
use hennery_kernel::secret::unix_now;
use support::World;

const SECRET: &str = "files-s3cr3t-0123";

fn input(name: &str, env: &[(&str, Option<&str>)]) -> StdioInput {
    StdioInput {
        name: name.into(),
        command: "/usr/local/bin/files-mcp".into(),
        args: vec!["--root".into(), "/srv".into()],
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
            .collect(),
    }
}

fn done(change: StdioChange) -> Vec<StdioServer> {
    match change {
        StdioChange::Done(set) => set,
        other => panic!("not done: {other:?}"),
    }
}

/// The values a session would get, through the delivery read.
fn delivered(w: &World, host: &str, hat: &str) -> Vec<hennery_proto::frames::McpServer> {
    let mcp = hennery_gateway::session::GatewayMcp::new(&w.gateway());
    let mut conn = w.raw();
    let tx = conn.transaction().unwrap();
    use hennery_gateway::session::{SessionMcp, SessionRef};
    let delivered = mcp
        .servers_in(
            &tx,
            SessionRef {
                session_id: "s-probe",
                host_id: host,
                hat_id: hat,
            },
            hennery_proto::rest::McpSessionDeliveryMode::Isolated,
        )
        .unwrap();
    mcp.cut(delivered.cut);
    delivered.servers
}

fn env_of(servers: &[hennery_proto::frames::McpServer], name: &str) -> Vec<(String, String)> {
    servers
        .iter()
        .find_map(|s| match s {
            hennery_proto::frames::McpServer::Stdio { name: n, env, .. } if n == name => Some(
                env.iter()
                    .map(|pair| (pair.name.clone(), pair.value.clone()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn a_set_is_stored_listed_and_replaced_whole() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    let set = done(
        w.store
            .replace_stdio_set(
                "host-a",
                &hat,
                &[input("files", &[("KEY", Some(SECRET))]), input("notes", &[])],
                &w.key,
                100,
            )
            .unwrap(),
    );
    assert_eq!(
        set.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["files", "notes"]
    );
    assert_eq!(set[0].env, ["KEY"]);
    assert_eq!(set[0].args, ["--root", "/srv"]);
    assert_eq!((set[0].created_at, set[0].updated_at), (100, 100));
    assert_eq!(done(w.store.stdio_set("host-a", &hat).unwrap()), set);
    // Another (host, hat) has its own set: none.
    assert!(done(w.store.stdio_set("host-a", &w.other_hat()).unwrap()).is_empty());
    // Replaced whole: `notes` goes, `files` keeps its row and its created_at.
    let set = done(
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", &[("KEY", None)])], &w.key, 200)
            .unwrap(),
    );
    assert_eq!(set.len(), 1);
    assert_eq!((set[0].created_at, set[0].updated_at), (100, 100), "nothing changed");
    assert_eq!(
        env_of(&delivered(&w, "host-a", &hat), "hennery-files"),
        [("KEY".to_string(), SECRET.to_string())]
    );
    // `[]` deletes them all.
    assert!(
        done(w.store.replace_stdio_set("host-a", &hat, &[], &w.key, 300).unwrap()).is_empty()
    );
}

/// Decision E3: absent keeps the stored value, a string sets it (`""` too),
/// a name left out is deleted.
#[test]
fn values_are_kept_set_and_deleted_by_name() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    let put = |env: &[(&str, Option<&str>)]| {
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", env)], &w.key, unix_now())
            .unwrap()
    };
    done(put(&[("A", Some("one")), ("B", Some("two"))]));
    done(put(&[("A", None), ("B", Some("")), ("C", Some("three"))]));
    assert_eq!(
        env_of(&delivered(&w, "host-a", &hat), "hennery-files"),
        [
            ("A".to_string(), "one".to_string()),
            ("B".to_string(), String::new()),
            ("C".to_string(), "three".to_string())
        ]
    );
    done(put(&[("C", None)]));
    assert_eq!(
        env_of(&delivered(&w, "host-a", &hat), "hennery-files"),
        [("C".to_string(), "three".to_string())]
    );
    // A deleted name cannot be kept.
    assert!(matches!(put(&[("A", None)]), StdioChange::EnvValueMissing(_)));
}

#[test]
fn a_value_never_stored_cannot_be_kept_and_nothing_changes() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    done(
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some("one"))])], &w.key, 1)
            .unwrap(),
    );
    let refused = w
        .store
        .replace_stdio_set(
            "host-a",
            &hat,
            &[input("files", &[("A", None), ("NEW", None)]), input("other", &[])],
            &w.key,
            2,
        )
        .unwrap();
    let StdioChange::EnvValueMissing(why) = refused else {
        panic!("{refused:?}");
    };
    assert!(why.contains("files") && why.contains("NEW"), "{why}");
    let set = done(w.store.stdio_set("host-a", &hat).unwrap());
    assert_eq!(set.len(), 1, "a refused set changes nothing");
}

/// S1: a kept value needs its server's command unchanged.
#[test]
fn a_changed_command_must_resend_its_values() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    done(
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some(SECRET))])], &w.key, 1)
            .unwrap(),
    );
    let mut moved = input("files", &[("A", None)]);
    moved.command = "/tmp/evil".into();
    assert!(matches!(
        w.store
            .replace_stdio_set("host-a", &hat, &[moved.clone()], &w.key, 2)
            .unwrap(),
        StdioChange::EnvValueMissing(_)
    ));
    moved.env = vec![("A".into(), Some("new".into()))];
    done(w.store.replace_stdio_set("host-a", &hat, &[moved], &w.key, 3).unwrap());
}

/// R5: a kept value is read only from the same set: the same name in
/// another hat or on another host keeps nothing.
#[test]
fn a_kept_value_never_comes_from_another_hat_or_host() {
    let w = World::new();
    w.host("host-a", 1);
    w.host("host-b", 2);
    let hat = w.hat();
    let work = w.other_hat();
    done(
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some(SECRET))])], &w.key, 1)
            .unwrap(),
    );
    for (host, hat) in [("host-a", work.as_str()), ("host-b", hat.as_str())] {
        assert!(
            matches!(
                w.store
                    .replace_stdio_set(host, hat, &[input("files", &[("A", None)])], &w.key, 2)
                    .unwrap(),
                StdioChange::EnvValueMissing(_)
            ),
            "{host} {hat}"
        );
    }
}

/// R5: a blob copied to another host's or hat's row does not open there.
#[test]
fn a_blob_moved_to_another_row_does_not_open() {
    let w = World::new();
    w.host("host-a", 1);
    w.host("host-b", 2);
    let hat = w.hat();
    for host in ["host-a", "host-b"] {
        let value = if host == "host-a" { SECRET } else { "other" };
        done(
            w.store
                .replace_stdio_set(host, &hat, &[input("files", &[("A", Some(value))])], &w.key, 1)
                .unwrap(),
        );
    }
    w.raw()
        .execute(
            "UPDATE gw_stdio_servers SET env_ciphertext =
                 (SELECT env_ciphertext FROM gw_stdio_servers WHERE host_id = 'host-a')
             WHERE host_id = 'host-b'",
            [],
        )
        .unwrap();
    let mcp = hennery_gateway::session::GatewayMcp::new(&w.gateway());
    let mut conn = w.raw();
    let tx = conn.transaction().unwrap();
    use hennery_gateway::session::{SessionMcp, SessionRef};
    let err = mcp
        .servers_in(
            &tx,
            SessionRef {
                session_id: "s1",
                host_id: "host-b",
                hat_id: &hat,
            },
            hennery_proto::rest::McpSessionDeliveryMode::Isolated,
        )
        .unwrap_err();
    assert!(!format!("{err:#}").contains(SECRET), "{err:#}");
}

/// Decision E2: a stdio name and a connection slug never collide, in
/// either direction, whatever their hats.
#[test]
fn names_and_slugs_are_one_namespace_both_ways() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    let work = w.other_hat();
    w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &work, None);
    let refused = w
        .store
        .replace_stdio_set("host-a", &hat, &[input("linear", &[])], &w.key, 1)
        .unwrap();
    assert!(matches!(refused, StdioChange::SlugTaken(_)), "{refused:?}");
    done(
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", &[])], &w.key, 1)
            .unwrap(),
    );
    let mut new = support::new_connection("files", &work);
    new.internal_network = true;
    assert!(matches!(w.store.create(&new, 1).unwrap(), Change::SlugTaken));
}

/// Q1's default: the same name may repeat across (host, hat) sets.
#[test]
fn a_name_may_repeat_in_another_set() {
    let w = World::new();
    w.host("host-a", 1);
    w.host("host-b", 2);
    let hat = w.hat();
    for host in ["host-a", "host-b"] {
        done(
            w.store
                .replace_stdio_set(host, &hat, &[input("files", &[])], &w.key, 1)
                .unwrap(),
        );
    }
}

#[test]
fn an_unknown_host_or_hat_is_not_found_and_a_revoked_host_is_read_only() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    assert_eq!(w.store.stdio_set("host-x", &hat).unwrap(), StdioChange::NotFound);
    assert_eq!(w.store.stdio_set("host-a", "hat-x").unwrap(), StdioChange::NotFound);
    assert_eq!(
        w.store.replace_stdio_set("host-x", &hat, &[], &w.key, 1).unwrap(),
        StdioChange::NotFound
    );
    assert_eq!(
        w.store.replace_stdio_set("host-a", "hat-x", &[], &w.key, 1).unwrap(),
        StdioChange::NotFound
    );
    done(
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", &[])], &w.key, 1)
            .unwrap(),
    );
    w.hosts.revoke("host-a", 2).unwrap();
    assert_eq!(done(w.store.stdio_set("host-a", &hat).unwrap()).len(), 1);
    assert_eq!(
        w.store.replace_stdio_set("host-a", &hat, &[], &w.key, 3).unwrap(),
        StdioChange::HostRevoked
    );
}

/// A hat frozen for its purge takes no new set: its sets go with the purge
/// (kernel spec §5.5), and one written after would keep the hat's row.
#[test]
fn a_hat_being_purged_takes_no_set() {
    let w = World::new();
    w.host("host-a", 1);
    let work = w.other_hat();
    w.raw()
        .execute(
            "INSERT INTO purged_hats(hat_id, owner_id, purged_at) VALUES (?1, ?2, 1)",
            [&work, w.store.owner_id()],
        )
        .unwrap();
    assert_eq!(
        w.store
            .replace_stdio_set("host-a", &work, &[input("files", &[])], &w.key, 1)
            .unwrap(),
        StdioChange::NotFound
    );
}

#[test]
fn the_owner_has_at_most_1024() {
    let w = World::new();
    let hat = w.hat();
    let sets = MAX_OWNER / 32;
    for i in 0..sets {
        let host = format!("host-{i}");
        w.host(&host, (i + 1) as u8);
        let servers: Vec<StdioInput> = (0..32).map(|j| input(&format!("s{j}"), &[])).collect();
        done(w.store.replace_stdio_set(&host, &hat, &servers, &w.key, 1).unwrap());
    }
    w.host("host-last", 200);
    assert!(matches!(
        w.store
            .replace_stdio_set("host-last", &hat, &[input("one", &[])], &w.key, 1)
            .unwrap(),
        StdioChange::TooMany(_)
    ));
    // Replacing a full set with as many is not more.
    let servers: Vec<StdioInput> = (0..32).map(|j| input(&format!("t{j}"), &[])).collect();
    done(w.store.replace_stdio_set("host-0", &hat, &servers, &w.key, 2).unwrap());
}

/// Gateway spec §2, kernel §5.5: a hat's purge takes its sets, so the hat
/// can go; another hat's stay.
#[test]
fn a_hat_purge_takes_its_sets() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    let work = w.other_hat();
    for hat in [&hat, &work] {
        done(
            w.store
                .replace_stdio_set("host-a", hat, &[input("files", &[("A", Some("v"))])], &w.key, 1)
                .unwrap(),
        );
    }
    let _ = w.store.purge_hat(&work).unwrap();
    assert_eq!(
        w.raw()
            .execute(
                "DELETE FROM hats WHERE id = ?1 AND owner_id = ?2",
                [&work, w.store.owner_id()]
            )
            .unwrap(),
        1
    );
    assert_eq!(done(w.store.stdio_set("host-a", &hat).unwrap()).len(), 1);
}

/// Plan 8a's hand-off: a stored stdio value counts as ciphertext, so a
/// missing master key is an error, and the newest one must open with the
/// key the gateway starts with.
#[test]
fn stdio_values_count_as_ciphertext_and_are_checked_against_the_key() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    assert!(!w.store.has_ciphertext().unwrap());
    done(
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", &[])], &w.key, 1)
            .unwrap(),
    );
    assert!(!w.store.has_ciphertext().unwrap(), "no values, nothing sealed");
    done(
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some("v"))])], &w.key, 2)
            .unwrap(),
    );
    assert!(w.store.has_ciphertext().unwrap());
    w.store.check_key(&w.key).unwrap();
    let other = hennery_gateway::key::MasterKey::from_bytes([9; 32]);
    let err = w.store.check_key(&other).unwrap_err();
    assert!(format!("{err:#}").contains("stdio server"), "{err:#}");
}

#[test]
fn values_are_sealed_at_rest() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    done(
        w.store
            .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some(SECRET))])], &w.key, 1)
            .unwrap(),
    );
    let raw = std::fs::read(&w.db).unwrap();
    let wal = std::fs::read(w.db.with_extension("db-wal")).unwrap_or_default();
    for bytes in [raw, wal] {
        assert!(
            !bytes.windows(SECRET.len()).any(|window| window == SECRET.as_bytes()),
            "a value is in the database in clear"
        );
    }
}
