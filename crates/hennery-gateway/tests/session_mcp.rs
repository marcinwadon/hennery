//! What a session gets of the gateway (ACP core §1, gateway spec §3.2;
//! plan 8e): its hat's connections mounted on its host, `hennery-<slug>`
//! at `<public_url>/mcp/<slug>` with a token minted in the caller's
//! transaction, then its stdio servers; and what each revoke cuts.

mod support;

use hennery_gateway::model::CredKind;
use hennery_gateway::revocation::Cut;
use hennery_gateway::session::{GatewayMcp, SessionMcp, SessionRef};
use hennery_gateway::stdio::StdioInput;
use hennery_gateway::tokens::is_session_token;
use hennery_kernel::operator::SetupOutcome;
use hennery_kernel::secret::unix_now;
use hennery_proto::frames::McpServer;
use hennery_proto::rest::McpSessionDeliveryMode;
use support::World;

const ORIGIN: &str = "https://hennery.example";

/// A world set up at `ORIGIN`, so the gateway has a `public_url`.
fn world() -> World {
    let w = World::new();
    let operator = hennery_kernel::operator::Operator::open(&w.db).unwrap();
    let now = unix_now();
    let setup = operator.issue_setup_token(now).unwrap().unwrap();
    let SetupOutcome::Done { .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap() else {
        panic!("setup failed");
    };
    w
}

fn session<'a>(id: &'a str, host: &'a str, hat: &'a str) -> SessionRef<'a> {
    SessionRef {
        session_id: id,
        host_id: host,
        hat_id: hat,
    }
}

/// `servers_in` in a transaction of its own, committed, then cut.
fn deliver(w: &World, mcp: &GatewayMcp, s: SessionRef<'_>, mode: McpSessionDeliveryMode) -> (Vec<McpServer>, usize) {
    let mut conn = w.raw();
    let tx = conn.transaction().unwrap();
    let delivered = mcp.servers_in(&tx, s, mode).unwrap();
    tx.commit().unwrap();
    let cut = delivered.cut.len();
    mcp.cut(delivered.cut);
    (delivered.servers, cut)
}

fn token_of(servers: &[McpServer]) -> String {
    let McpServer::Http { headers, .. } = &servers[0] else {
        panic!("{servers:?}");
    };
    headers[0].value.strip_prefix("Bearer ").unwrap().to_string()
}

fn http(name: &str, url: &str, token: &str) -> McpServer {
    McpServer::Http {
        name: name.into(),
        url: url.into(),
        headers: vec![hennery_proto::frames::NameValue::new(
            "Authorization",
            format!("Bearer {token}"),
        )],
    }
}

#[test]
fn a_session_gets_its_hats_mounted_connections_then_its_stdio_servers() {
    let w = world();
    w.host("host-a", 1);
    w.host("host-b", 2);
    let hat = w.hat();
    let work = w.other_hat();
    let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    let notes = w.connection_in("notes", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    let elsewhere = w.connection_in("elsewhere", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    let theirs = w.connection_in("theirs", "http://127.0.0.1:9/mcp", CredKind::None, &work, None);
    w.mount(&linear, &["host-a"]);
    w.mount(&notes, &["host-a", "host-b"]);
    w.mount(&elsewhere, &["host-b"]);
    w.mount(&theirs, &["host-a"]);
    let files = StdioInput {
        name: "files".into(),
        command: "files-mcp".into(),
        args: vec!["--root".into(), "/srv".into()],
        env: vec![("KEY".into(), Some("v".into()))],
    };
    let _ = w.store.replace_stdio_set("host-a", &hat, &[files], &w.key, 1).unwrap();
    let mcp = GatewayMcp::new(&w.gateway());
    let (servers, cut) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    assert_eq!(cut, 0, "a first start supersedes nothing");
    let token = token_of(&servers);
    assert!(is_session_token(&token));
    assert_eq!(
        servers,
        vec![
            http("hennery-linear", &format!("{ORIGIN}/mcp/linear"), &token),
            http("hennery-notes", &format!("{ORIGIN}/mcp/notes"), &token),
            McpServer::Stdio {
                name: "hennery-files".into(),
                command: "files-mcp".into(),
                args: vec!["--root".into(), "/srv".into()],
                env: vec![hennery_proto::frames::NameValue::new("KEY", "v")],
            },
        ]
    );
    let principal = w.proxy_store_resolve(&token).unwrap();
    assert_eq!(principal.hat_id, hat);
}

#[test]
fn unisolated_delivers_as_isolated_does() {
    let w = world();
    w.host("host-a", 1);
    let hat = w.hat();
    let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    w.mount(&linear, &["host-a"]);
    let mcp = GatewayMcp::new(&w.gateway());
    let (servers, _) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Unisolated,
    );
    assert_eq!(servers.len(), 1);
    assert!(w.proxy_store_resolve(&token_of(&servers)).is_some());
}

/// Fallback and unsupported get nothing, and mint nothing: no token row.
#[test]
fn a_mode_that_does_not_deliver_mints_nothing() {
    for mode in [McpSessionDeliveryMode::Fallback, McpSessionDeliveryMode::Unsupported] {
        let w = world();
        w.host("host-a", 1);
        let hat = w.hat();
        let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
        w.mount(&linear, &["host-a"]);
        let _ = w.store.replace_stdio_set(
            "host-a",
            &hat,
            &[StdioInput {
                name: "files".into(),
                command: "c".into(),
                args: vec![],
                env: vec![],
            }],
            &w.key,
            1,
        );
        let mcp = GatewayMcp::new(&w.gateway());
        let (servers, _) = deliver(&w, &mcp, session("s1", "host-a", &hat), mode);
        assert!(servers.is_empty(), "{mode:?}: {servers:?}");
        assert_eq!(w.token_rows(), 0, "{mode:?}");
    }
}

/// No connection mounted for the hat: no token is minted at all, only the
/// stdio servers go.
#[test]
fn no_connection_no_token() {
    let w = world();
    w.host("host-a", 1);
    let hat = w.hat();
    let mcp = GatewayMcp::new(&w.gateway());
    let (servers, _) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    assert!(servers.is_empty());
    assert_eq!(w.token_rows(), 0);
}

/// A revoked host's mounts are not delivered (plan 8a decision 12).
#[test]
fn a_revoked_hosts_mounts_are_not_delivered() {
    let w = world();
    w.host("host-a", 1);
    let hat = w.hat();
    let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    w.mount(&linear, &["host-a"]);
    w.hosts.revoke("host-a", unix_now()).unwrap();
    let mcp = GatewayMcp::new(&w.gateway());
    let (servers, _) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    assert!(servers.is_empty(), "{servers:?}");
}

/// A session from before hats has no hat to deliver.
#[test]
fn a_session_without_a_hat_gets_nothing() {
    let w = world();
    w.host("host-a", 1);
    let mcp = GatewayMcp::new(&w.gateway());
    let (servers, _) = deliver(&w, &mcp, session("s1", "host-a", ""), McpSessionDeliveryMode::Isolated);
    assert!(servers.is_empty());
}

/// A resume mints a fresh token: the old one no longer resolves, and is
/// what the resume's transaction names to cut.
#[test]
fn a_resume_supersedes_and_cuts_the_old_token() {
    let w = world();
    w.host("host-a", 1);
    let hat = w.hat();
    let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    w.mount(&linear, &["host-a"]);
    let mcp = GatewayMcp::new(&w.gateway());
    let (first, _) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    let old = token_of(&first);
    let watch = w.revocations.watch(&hennery_kernel::secret::sha256_hex(old.as_bytes()));
    let (second, cut) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    let new = token_of(&second);
    assert_ne!(old, new);
    assert_eq!(cut, 1);
    assert!(watch.token().is_cancelled(), "the old token's watch is cut");
    assert!(w.proxy_store_resolve(&old).is_none());
    assert!(w.proxy_store_resolve(&new).is_some());
    // A resume that delivers nothing revokes the token, and cuts it.
    let watch = w.revocations.watch(&hennery_kernel::secret::sha256_hex(new.as_bytes()));
    let (none, cut) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Fallback,
    );
    assert!(none.is_empty());
    assert_eq!(cut, 1);
    assert!(watch.token().is_cancelled());
    assert!(w.proxy_store_resolve(&new).is_none());
}

/// Nothing is cut before the caller commits: the cut is the caller's, and
/// a transaction rolled back leaves the token working, its streams too.
#[test]
fn a_rolled_back_mint_leaves_no_token_and_cuts_nothing() {
    let w = world();
    w.host("host-a", 1);
    let hat = w.hat();
    let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    w.mount(&linear, &["host-a"]);
    let mcp = GatewayMcp::new(&w.gateway());
    let (first, _) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    let old = token_of(&first);
    let watch = w.revocations.watch(&hennery_kernel::secret::sha256_hex(old.as_bytes()));
    {
        let mut conn = w.raw();
        let tx = conn.transaction().unwrap();
        let delivered = mcp
            .servers_in(&tx, session("s1", "host-a", &hat), McpSessionDeliveryMode::Isolated)
            .unwrap();
        // Rolled back: dropped uncommitted, and its cut is never made.
        drop(tx);
        drop(delivered);
    }
    assert!(!watch.token().is_cancelled());
    assert!(w.proxy_store_resolve(&old).is_some());
}

#[test]
fn revoke_in_cuts_a_live_token_and_only_that() {
    let w = world();
    w.host("host-a", 1);
    let hat = w.hat();
    let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    w.mount(&linear, &["host-a"]);
    let mcp = GatewayMcp::new(&w.gateway());
    let (one, _) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    let (two, _) = deliver(
        &w,
        &mcp,
        session("s2", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    let revoke = |id: &str| -> Cut {
        let mut conn = w.raw();
        let tx = conn.transaction().unwrap();
        let cut = mcp.revoke_in(&tx, id).unwrap();
        tx.commit().unwrap();
        cut
    };
    let cut = revoke("s1");
    assert_eq!(cut.len(), 1);
    mcp.cut(cut);
    assert!(w.proxy_store_resolve(&token_of(&one)).is_none());
    assert!(w.proxy_store_resolve(&token_of(&two)).is_some());
    assert!(revoke("s1").is_empty(), "twice: nothing live to cut");
    assert!(revoke("never").is_empty());
}

#[test]
fn revoke_host_in_cuts_every_live_token_of_the_host() {
    let w = world();
    w.host("host-a", 1);
    w.host("host-b", 2);
    let hat = w.hat();
    let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    w.mount(&linear, &["host-a", "host-b"]);
    let mcp = GatewayMcp::new(&w.gateway());
    let (a1, _) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    let (a2, _) = deliver(
        &w,
        &mcp,
        session("s2", "host-a", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    let (b, _) = deliver(
        &w,
        &mcp,
        session("s3", "host-b", &hat),
        McpSessionDeliveryMode::Isolated,
    );
    let mut conn = w.raw();
    let tx = conn.transaction().unwrap();
    let cut = mcp.revoke_host_in(&tx, "host-a").unwrap();
    tx.commit().unwrap();
    assert_eq!(cut.len(), 2);
    mcp.cut(cut);
    for token in [&a1, &a2] {
        assert!(w.token_revoked(&token_of(token)));
    }
    assert!(!w.token_revoked(&token_of(&b)));
}

#[test]
fn a_hat_purge_cuts_its_tokens() {
    let w = world();
    w.host("host-a", 1);
    let hat = w.hat();
    let work = w.other_hat();
    let theirs = w.connection_in("theirs", "http://127.0.0.1:9/mcp", CredKind::None, &work, None);
    w.mount(&theirs, &["host-a"]);
    let mcp = GatewayMcp::new(&w.gateway());
    let (servers, _) = deliver(
        &w,
        &mcp,
        session("s1", "host-a", &work),
        McpSessionDeliveryMode::Isolated,
    );
    let token = token_of(&servers);
    let watch = w
        .revocations
        .watch(&hennery_kernel::secret::sha256_hex(token.as_bytes()));
    mcp.purge_hat(&work).unwrap();
    assert!(watch.token().is_cancelled());
    assert!(w.proxy_store_resolve(&token).is_none());
    // The default hat is not touched.
    let _ = hat;
}

/// No `public_url`: a session with a connection to reach cannot be given
/// one, and its transition should roll back rather than start without.
#[test]
fn without_a_public_url_a_mint_fails() {
    let w = World::new();
    w.host("host-a", 1);
    let hat = w.hat();
    let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
    w.mount(&linear, &["host-a"]);
    let mcp = GatewayMcp::new(&w.gateway());
    let mut conn = w.raw();
    let tx = conn.transaction().unwrap();
    let err = mcp
        .servers_in(&tx, session("s1", "host-a", &hat), McpSessionDeliveryMode::Isolated)
        .unwrap_err();
    assert!(format!("{err:#}").contains("public_url"), "{err:#}");
}
