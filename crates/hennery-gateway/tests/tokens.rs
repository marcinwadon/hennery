//! Session tokens and scope (gateway spec §3.1, umbrella §10.2; lane L1):
//! the primitives plan 8e calls inside the sessions store's transactions,
//! and what `ProxyStore` resolves from them. Driven on `hennery.db`
//! directly, as the sessions store would.

mod support;

use hennery_gateway::model::CredKind;
use hennery_gateway::scope::{ClientIdentity, LAST_USED_EVERY, MountPolicy, Principal, PrincipalKind};
use hennery_gateway::tokens::{self, SESSION_TOKEN_PREFIX, is_session_token};
use hennery_kernel::secret::unix_now;
use rusqlite::OptionalExtension;
use support::World;

fn principal(session: &str, host: &str, hat: &str) -> Principal {
    Principal {
        hat_id: hat.into(),
        kind: PrincipalKind::Session {
            session_id: session.into(),
            host_id: host.into(),
        },
    }
}

#[test]
fn a_minted_token_resolves_to_its_session_host_and_hat() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let token = h.mint("s1", "host-a", &hat);
    assert!(token.starts_with(SESSION_TOKEN_PREFIX), "{token}");
    assert!(is_session_token(&token));
    assert_eq!(
        h.proxy_store.resolve(&token, unix_now()).unwrap(),
        Some(principal("s1", "host-a", &hat))
    );
    // Only its hash is stored.
    let stored: String = h
        .raw()
        .query_row(
            "SELECT token_hash FROM gw_session_tokens WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, hennery_kernel::secret::sha256_hex(token.as_bytes()));
    let db = std::fs::read(&h.db).unwrap();
    let wal = std::fs::read(h.db.with_extension("db-wal")).unwrap_or_default();
    for bytes in [db, wal] {
        assert!(
            !bytes.windows(token.len()).any(|w| w == token.as_bytes()),
            "the token is stored in clear"
        );
    }
    // Two mints never give the same token.
    let other = h.mint("s2", "host-a", &hat);
    assert_ne!(other, token);
}

#[test]
fn the_next_mint_supersedes_the_session_s_token() {
    let h = World::new();
    h.host("host-a", 1);
    h.host("host-b", 2);
    let hat = h.hat();
    let work = h.other_hat();
    let first = h.mint("s1", "host-a", &hat);
    // A resume, in another hat on another host (a re-assignment).
    let second = h.mint("s1", "host-b", &work);
    assert_eq!(h.proxy_store.resolve(&first, unix_now()).unwrap(), None, "superseded");
    assert_eq!(
        h.proxy_store.resolve(&second, unix_now()).unwrap(),
        Some(principal("s1", "host-b", &work))
    );
    // A mint after a revoke gives a live token again, and only the new one.
    assert!(h.revoke("s1"));
    let third = h.mint("s1", "host-a", &hat);
    assert_eq!(h.proxy_store.resolve(&second, unix_now()).unwrap(), None);
    assert_eq!(
        h.proxy_store.resolve(&third, unix_now()).unwrap(),
        Some(principal("s1", "host-a", &hat))
    );
}

#[test]
fn a_revoked_token_resolves_to_nothing_and_revoking_twice_is_fine() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let token = h.mint("s1", "host-a", &hat);
    let kept = h.mint("s2", "host-a", &hat);
    assert!(h.revoke("s1"));
    assert!(!h.revoke("s1"), "already revoked");
    assert!(!h.revoke("never-minted"));
    assert_eq!(h.proxy_store.resolve(&token, unix_now()).unwrap(), None);
    assert!(h.proxy_store.resolve(&kept, unix_now()).unwrap().is_some());
}

#[test]
fn a_host_revoke_revokes_that_host_s_tokens_only() {
    let h = World::new();
    h.host("host-a", 1);
    h.host("host-b", 2);
    let hat = h.hat();
    let a1 = h.mint("s1", "host-a", &hat);
    let a2 = h.mint("s2", "host-a", &hat);
    let b = h.mint("s3", "host-b", &hat);
    assert_eq!(h.revoke_host_tokens("host-a"), 2);
    assert_eq!(h.revoke_host_tokens("host-a"), 0);
    for token in [&a1, &a2] {
        assert_eq!(h.proxy_store.resolve(token, unix_now()).unwrap(), None);
    }
    assert!(h.proxy_store.resolve(&b, unix_now()).unwrap().is_some());
}

/// Even without the gateway's revoke, a revoked host's tokens resolve to
/// nothing: the resolve reads the host (a missed revoke site in plan 8e
/// stays closed).
#[test]
fn a_revoked_host_s_tokens_resolve_to_nothing() {
    let h = World::new();
    h.host("host-a", 1);
    let token = h.mint("s1", "host-a", &h.hat());
    h.hosts.revoke("host-a", unix_now()).unwrap();
    assert_eq!(h.proxy_store.resolve(&token, unix_now()).unwrap(), None);
}

/// Lane L1: a mint is part of the caller's transaction, so a transition
/// that rolls back leaves no token behind.
#[test]
fn a_rolled_back_mint_leaves_no_token() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    let token = tokens::mint_in(&tx, h.store.owner_id(), "s1", "host-a", &hat, unix_now()).unwrap();
    tx.rollback().unwrap();
    assert_eq!(h.proxy_store.resolve(token.expose(), unix_now()).unwrap(), None);
    let row: Option<String> = h
        .raw()
        .query_row("SELECT session_id FROM gw_session_tokens", [], |r| r.get(0))
        .optional()
        .unwrap();
    assert_eq!(row, None);
    // A revoke rolled back leaves the token live.
    let live = h.mint("s2", "host-a", &hat);
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    assert!(tokens::revoke_in(&tx, h.store.owner_id(), "s2", unix_now()).unwrap());
    tx.rollback().unwrap();
    assert!(h.proxy_store.resolve(&live, unix_now()).unwrap().is_some());
}

/// A host or hat that is not the owner's fails the mint (their foreign
/// keys), and with it the caller's transition.
#[test]
fn a_mint_for_an_unknown_host_or_hat_fails() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    assert!(tokens::mint_in(&tx, h.store.owner_id(), "s1", "nowhere", &hat, unix_now()).is_err());
    assert!(tokens::mint_in(&tx, h.store.owner_id(), "s1", "host-a", "hat-none", unix_now()).is_err());
}

#[test]
fn malformed_and_unknown_tokens_resolve_to_nothing() {
    let h = World::new();
    h.host("host-a", 1);
    let token = h.mint("s1", "host-a", &h.hat());
    let unknown = format!("{SESSION_TOKEN_PREFIX}{}", "0".repeat(64));
    for bad in [
        "",
        "Bearer",
        unknown.as_str(),
        &token[..token.len() - 1],
        &token.to_uppercase(),
        &format!("{token}0"),
    ] {
        assert_eq!(h.proxy_store.resolve(bad, unix_now()).unwrap(), None, "{bad}");
    }
}

/// Plan 8d decision 12: `last_used_at` is written on use, at most once a
/// minute.
#[test]
fn use_is_recorded_at_most_once_a_minute() {
    let h = World::new();
    h.host("host-a", 1);
    let token = h.mint("s1", "host-a", &h.hat());
    let used = |h: &World| -> Option<i64> {
        h.raw()
            .query_row(
                "SELECT last_used_at FROM gw_session_tokens WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(used(&h), None);
    let t0 = 1_000_000;
    h.proxy_store.resolve(&token, t0).unwrap().unwrap();
    assert_eq!(used(&h), Some(t0));
    h.proxy_store
        .resolve(&token, t0 + LAST_USED_EVERY - 1)
        .unwrap()
        .unwrap();
    assert_eq!(used(&h), Some(t0));
    h.proxy_store.resolve(&token, t0 + LAST_USED_EVERY).unwrap().unwrap();
    assert_eq!(used(&h), Some(t0 + LAST_USED_EVERY));
    // A new token starts unused, and its first use is recorded.
    let next = h.mint("s1", "host-a", &h.hat());
    assert_eq!(used(&h), None);
    h.proxy_store.resolve(&next, t0 + LAST_USED_EVERY + 1).unwrap().unwrap();
    assert_eq!(used(&h), Some(t0 + LAST_USED_EVERY + 1));
}

/// Decision 2: a mint never takes over another owner's row under the same
/// session id; it fails, and the row stays as it was. The host and hat
/// foreign keys would refuse the takeover too; the upsert's own guard
/// refuses it first, by name.
#[test]
fn a_mint_never_takes_over_another_owner_s_token() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let conn = h.raw();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO owners(id, created_at) VALUES ('other', 0);
         INSERT INTO gw_session_tokens(session_id, owner_id, host_id, hat_id, token_hash, created_at)
         VALUES ('s1', 'other', 'x', 'y', 'deadbeef', 0);",
    )
    .unwrap();
    drop(conn);
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    let err = tokens::mint_in(&tx, h.store.owner_id(), "s1", "host-a", &hat, unix_now()).unwrap_err();
    assert!(err.to_string().contains("another owner's"), "{err:#}");
    drop(tx);
    let row: (String, String) = h
        .raw()
        .query_row(
            "SELECT owner_id, token_hash FROM gw_session_tokens WHERE session_id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row, ("other".to_string(), "deadbeef".to_string()));
}

#[test]
fn scope_is_the_hat_s_connections_mounted_on_the_host() {
    let h = World::new();
    h.host("host-a", 1);
    h.host("host-b", 2);
    let hat = h.hat();
    let work = h.other_hat();
    let mine = h.connection("mine", "http://127.0.0.1:9/mcp", CredKind::None);
    let theirs = h.connection_in("theirs", "http://127.0.0.1:9/mcp", CredKind::None, &work, None);
    let elsewhere = h.connection("elsewhere", "http://127.0.0.1:9/mcp", CredKind::None);
    h.mount(&mine, &["host-a"]);
    h.mount(&theirs, &["host-a"]);
    h.mount(&elsewhere, &["host-b"]);
    let me = principal("s1", "host-a", &hat);
    let found = h.proxy_store.connection(&me, "mine").unwrap().unwrap();
    assert_eq!(found.id, mine);
    assert_eq!(found.slug, "mine");
    assert_eq!(found.cred_kind, CredKind::None);
    assert!(found.internal_network);
    // Another hat's, mounted here: out of scope. Mounted elsewhere: out.
    // No such slug: out.
    for slug in ["theirs", "elsewhere", "nothing"] {
        assert_eq!(h.proxy_store.connection(&me, slug).unwrap(), None, "{slug}");
    }
    // The other hat's principal on this host reaches its own.
    let them = principal("s2", "host-a", &work);
    assert_eq!(h.proxy_store.connection(&them, "theirs").unwrap().unwrap().id, theirs);
    assert_eq!(h.proxy_store.connection(&them, "mine").unwrap(), None);
    // Unmounted now: out at once.
    h.mount(&mine, &[]);
    assert_eq!(h.proxy_store.connection(&me, "mine").unwrap(), None);
    // A revoked host's mounts are out too.
    h.mount(&mine, &["host-a"]);
    h.hosts.revoke("host-a", unix_now()).unwrap();
    assert_eq!(h.proxy_store.connection(&me, "mine").unwrap(), None);
}

/// Lane L6 and gateway spec §2: a hat's tokens go with its purge, revoked
/// ones too, so the hat row can be deleted once the gateway's purge has
/// run; without it, the tokens' foreign key keeps the hat.
#[test]
fn a_hat_purge_takes_its_tokens_and_then_the_hat_can_go() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let work = h.other_hat();
    let live = h.mint("s1", "host-a", &work);
    h.mint("s2", "host-a", &work);
    h.revoke("s2");
    let kept = h.mint("s3", "host-a", &hat);
    let delete_hat = |h: &World| {
        h.raw().execute(
            "DELETE FROM hats WHERE id = ?1 AND owner_id = ?2",
            [&work, h.store.owner_id()],
        )
    };
    h.store.purge_hat(&work).unwrap();
    assert!(delete_hat(&h).is_err(), "the hat went with tokens left");
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    assert_eq!(tokens::purge_hat_in(&tx, h.store.owner_id(), &work).unwrap(), 2);
    assert_eq!(tokens::purge_hat_in(&tx, h.store.owner_id(), &work).unwrap(), 0);
    tx.commit().unwrap();
    assert_eq!(h.proxy_store.resolve(&live, unix_now()).unwrap(), None);
    assert!(h.proxy_store.resolve(&kept, unix_now()).unwrap().is_some());
    assert_eq!(delete_hat(&h).unwrap(), 1);
}

/// Gateway spec §7: live traffic sets `ok`, and a 401 `needs_auth`, only for
/// the connection as it was when the request went: not after its URL moved,
/// nor for a static connection whose token was deleted meanwhile.
#[test]
fn live_status_is_only_for_the_connection_as_it_was() {
    let h = World::new();
    let url = "http://127.0.0.1:9/mcp";
    let id = h.connection("linear", url, CredKind::Static);
    assert!(!h.proxy_store.mark_ok(&id, url, 1).unwrap(), "no token: not ok");
    h.set_token(&id, "tok");
    assert!(!h.proxy_store.mark_ok(&id, "http://127.0.0.1:9/other", 1).unwrap());
    assert!(
        !h.proxy_store
            .mark_needs_auth(&id, "http://127.0.0.1:9/other", 1)
            .unwrap()
    );
    assert_eq!(h.status(&id), "not_connected");
    assert!(h.proxy_store.mark_needs_auth(&id, url, 2).unwrap());
    assert!(!h.proxy_store.mark_needs_auth(&id, url, 3).unwrap(), "already");
    assert_eq!(h.status(&id), "needs_auth");
    assert!(h.proxy_store.mark_ok(&id, url, 4).unwrap());
    assert!(!h.proxy_store.mark_ok(&id, url, 5).unwrap(), "already");
    assert_eq!(h.status(&id), "ok");
    let none = h.connection("public", url, CredKind::None);
    assert!(h.proxy_store.mark_ok(&none, url, 1).unwrap());
}
