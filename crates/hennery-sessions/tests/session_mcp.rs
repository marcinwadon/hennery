//! Plan 8e: sessions get their MCP servers through `SessionMcp`, inside
//! their own transitions (lane L1). The delivery decision in each of its
//! outcomes (lane L2); a failed mint rolls the transition back; every
//! revoke site of lane L4 revokes in its transaction and cuts the token's
//! open streams once it commits (the fleet parent's ruling), and a
//! presumed park or a stale fact does not; a token the agent printed is
//! stored and published redacted (decision 11).

use hennery_gateway::api::GatewayState;
use hennery_gateway::key::MasterKey;
use hennery_gateway::model::{Change, CredKind, NewConnection};
use hennery_gateway::revocation::{Revocations, Watch};
use hennery_gateway::scope::{ClientIdentity, ProxyStore};
use hennery_gateway::session::GatewayMcp;
use hennery_gateway::stdio::StdioInput;
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hats::HatChange;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::{sha256_hex, unix_now};
use hennery_proto::frames::{McpIsolation, McpServer, ParkReason, SessionBody};
use hennery_proto::rest::McpSessionDeliveryMode;
use hennery_sessions::store::{Deletion, McpContext, McpGiven, Reassign, ResumeRequest, Store, Unattached};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

const ORIGIN: &str = "https://hennery.example";
const HOST: &str = "h1";

const CLAUDE: McpContext = McpContext {
    capable: true,
    isolation: McpIsolation::ClaudeStrict,
    rules_name_other_hats: false,
};
const CODEX: McpContext = McpContext {
    capable: true,
    isolation: McpIsolation::None,
    rules_name_other_hats: false,
};

struct World {
    _dir: tempfile::TempDir,
    db: PathBuf,
    store: Store,
    gateway: GatewayStore,
    /// The gateway as the store was given it, for another store.
    state: GatewayState,
    proxy: ProxyStore,
    revocations: Revocations,
    key: MasterKey,
    /// The host's default hat, with `linear` mounted on it.
    hat: String,
    /// Another hat, with `acme` mounted on the host.
    work: String,
}

fn connection(gateway: &GatewayStore, slug: &str, hat: &str) {
    let new = NewConnection {
        slug: slug.into(),
        label: slug.into(),
        url: "http://127.0.0.1:9/mcp".into(),
        hat_id: hat.into(),
        cred_kind: CredKind::None,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: true,
    };
    let Change::Done(record) = gateway.create(&new, unix_now()).unwrap() else {
        panic!("no connection");
    };
    assert!(matches!(
        gateway.replace_mounts(&record.id, &[HOST.to_string()]).unwrap(),
        Change::Done(_)
    ));
}

impl World {
    fn new() -> Self {
        Self::with_setup(true)
    }

    /// `set_up`: the owner is set up at `ORIGIN`, so the gateway has a
    /// `public_url`.
    fn with_setup(set_up: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        let enrollment = Enrollment {
            public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
            name: "laptop".into(),
            host_version: "0".into(),
            platform: "linux".into(),
        };
        hosts.register(HOST, &enrollment, 1).unwrap();
        let hat = hosts.host(HOST).unwrap().unwrap().default_hat_id;
        let HatChange::Done(work) = hosts.create_hat("Work", None, 1).unwrap() else {
            panic!("no hat");
        };
        let operator = Operator::open(&db).unwrap();
        if set_up {
            let now = unix_now();
            let token = operator.issue_setup_token(now).unwrap().unwrap();
            let SetupOutcome::Done { .. } = operator.set_up(&token, "correct horse battery", ORIGIN, now).unwrap()
            else {
                panic!("setup failed");
            };
        }
        let gateway_store = Arc::new(GatewayStore::open(&db).unwrap());
        connection(&gateway_store, "linear", &hat);
        connection(&gateway_store, "acme", &work.id);
        let revocations = Revocations::new();
        let key = MasterKey::from_bytes([7; 32]);
        let gateway = GatewayState {
            store: gateway_store,
            key: Arc::new(MasterKey::from_bytes([7; 32])),
            operator: Arc::new(operator),
            revocations: revocations.clone(),
        };
        let store = Store::open(&db).unwrap();
        store.set_session_mcp(Arc::new(GatewayMcp::new(&gateway)));
        Self {
            proxy: ProxyStore::open(&db).unwrap(),
            gateway: GatewayStore::open(&db).unwrap(),
            state: gateway,
            _dir: dir,
            db,
            store,
            revocations,
            key,
            hat,
            work: work.id,
        }
    }

    fn start(&self, id: &str, hat: &str, mcp: McpContext) -> McpGiven {
        self.store
            .create_session_with_mcp(id, HOST, "agent", "/p", hat, None, mcp)
            .unwrap()
            .expect("created")
    }

    /// Started and `active`, with its token.
    fn active(&self, id: &str) -> String {
        let given = self.start(id, &self.hat.clone(), CLAUDE);
        self.store
            .ingest(id, 1, &SessionBody::session_started("r0", format!("agent-{id}")))
            .unwrap();
        token(&given)
    }

    fn live(&self, token: &str) -> bool {
        self.proxy.resolve(token, unix_now()).unwrap().is_some()
    }

    fn revoked_row(&self, token: &str) -> bool {
        rusqlite::Connection::open(&self.db)
            .unwrap()
            .query_row(
                "SELECT revoked_at IS NOT NULL FROM gw_session_tokens WHERE token_hash = ?1",
                [sha256_hex(token.as_bytes())],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn token_rows(&self, session: &str) -> i64 {
        rusqlite::Connection::open(&self.db)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM gw_session_tokens WHERE session_id = ?1",
                [session],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn watch(&self, token: &str) -> Watch {
        self.revocations.watch(&sha256_hex(token.as_bytes()))
    }

    /// The token is revoked in its row and its watch cut: what every revoke
    /// site must do.
    fn assert_revoked(&self, token: &str, watch: &Watch, site: &str) {
        assert!(self.revoked_row(token), "{site}: the token's row is still live");
        assert!(!self.live(token), "{site}: the token still resolves");
        assert!(watch.token().is_cancelled(), "{site}: its open streams were not cut");
    }

    fn assert_live(&self, token: &str, watch: &Watch, site: &str) {
        assert!(self.live(token), "{site}: the token was revoked");
        assert!(!watch.token().is_cancelled(), "{site}: its streams were cut");
    }

    fn lifecycle(&self, id: &str) -> String {
        self.store.find_session(id).unwrap().unwrap().lifecycle
    }
}

fn token(given: &McpGiven) -> String {
    let Some(McpServer::Http { headers, .. }) = given.servers.first() else {
        panic!("no http server: {given:?}");
    };
    headers[0].value.strip_prefix("Bearer ").unwrap().to_string()
}

fn names(given: &McpGiven) -> Vec<&str> {
    given
        .servers
        .iter()
        .map(|s| match s {
            McpServer::Http { name, .. } | McpServer::Stdio { name, .. } => name.as_str(),
        })
        .collect()
}

// The delivery decision (umbrella §8.5, lane L2), one outcome each.

#[test]
fn claude_gets_its_hats_servers_isolated() {
    let w = World::new();
    let given = w.start("s1", &w.hat.clone(), CLAUDE);
    assert_eq!(given.mode, McpSessionDeliveryMode::Isolated);
    assert_eq!(names(&given), ["hennery-linear"]);
    let McpServer::Http { url, .. } = &given.servers[0] else {
        panic!()
    };
    assert_eq!(url, &format!("{ORIGIN}/mcp/linear"));
    let principal = w.proxy.resolve(&token(&given), unix_now()).unwrap().unwrap();
    assert_eq!(principal.hat_id, w.hat);
    assert!(!given.clone().frame().isolation_waived);
    let recorded = w.store.mcp_delivery("s1").unwrap().unwrap();
    assert_eq!((recorded.mode, recorded.servers), (McpSessionDeliveryMode::Isolated, 1));
}

#[test]
fn claude_in_another_hat_gets_that_hats_servers_only() {
    let w = World::new();
    let given = w.start("s1", &w.work.clone(), CLAUDE);
    assert_eq!(given.mode, McpSessionDeliveryMode::Isolated);
    assert_eq!(names(&given), ["hennery-acme"]);
    assert_eq!(
        w.proxy.resolve(&token(&given), unix_now()).unwrap().unwrap().hat_id,
        w.work
    );
}

/// The default hat of a host that cannot isolate the agent: its servers,
/// isolation waived in the frame (plan 8c's hand-off).
#[test]
fn codex_in_the_default_hat_is_unisolated() {
    let w = World::new();
    let given = w.start("s1", &w.hat.clone(), CODEX);
    assert_eq!(given.mode, McpSessionDeliveryMode::Unisolated);
    assert_eq!(names(&given), ["hennery-linear"]);
    assert!(given.clone().frame().isolation_waived);
}

/// "A mixed host gives Codex nothing" (the brief): a session outside the
/// default hat counts itself, so its host is mixed, and it falls back.
#[test]
fn codex_in_another_hat_falls_back_to_nothing() {
    let w = World::new();
    let given = w.start("s1", &w.work.clone(), CODEX);
    assert_eq!(given.mode, McpSessionDeliveryMode::Fallback);
    assert!(given.servers.is_empty());
    assert_eq!(w.token_rows("s1"), 0, "no token is minted for a fallback");
    let recorded = w.store.mcp_delivery("s1").unwrap().unwrap();
    assert_eq!((recorded.mode, recorded.servers), (McpSessionDeliveryMode::Fallback, 0));
    assert!(!given.frame().isolation_waived);
}

/// Mixedness never moves the default hat: a Codex session there still gets
/// its servers while another hat's session is live, or a rule names one.
#[test]
fn a_mixed_host_keeps_the_default_hats_servers() {
    let w = World::new();
    w.start("other", &w.work.clone(), CLAUDE);
    let given = w.start("s1", &w.hat.clone(), CODEX);
    assert_eq!(given.mode, McpSessionDeliveryMode::Unisolated);
    let ruled = McpContext {
        rules_name_other_hats: true,
        ..CODEX
    };
    assert_eq!(
        w.start("s2", &w.hat.clone(), ruled).mode,
        McpSessionDeliveryMode::Unisolated
    );
}

/// Lane L3: a host without `mcp_servers` gets no server and no token.
#[test]
fn a_host_without_the_capability_gets_nothing() {
    let w = World::new();
    let given = w.start("s1", &w.hat.clone(), McpContext::NONE);
    assert_eq!(given.mode, McpSessionDeliveryMode::Unsupported);
    assert!(given.servers.is_empty());
    assert_eq!(w.token_rows("s1"), 0);
    let recorded = w.store.mcp_delivery("s1").unwrap().unwrap();
    assert_eq!(recorded.mode, McpSessionDeliveryMode::Unsupported);
}

#[test]
fn a_session_not_started_since_plan_8e_has_no_record() {
    let w = World::new();
    rusqlite::Connection::open(&w.db)
        .unwrap()
        .execute(
            "INSERT INTO sessions(id, host_id, agent, cwd, hat_id, lifecycle, created_at, last_event_at, owner_id)
             SELECT 'old', ?1, 'agent', '/p', ?2, 'parked', 't', 't', owner_id FROM hosts WHERE id = ?1",
            [HOST, &w.hat],
        )
        .unwrap();
    assert_eq!(w.store.mcp_delivery("old").unwrap(), None);
}

// Lane L1: a failed mint rolls the transition back.

/// A stdio server whose values no longer open (another key's, or moved):
/// the delivery read fails.
fn break_stdio(w: &World) {
    let files = StdioInput {
        name: "files".into(),
        command: "files-mcp".into(),
        args: vec![],
        env: vec![("KEY".into(), Some("v".into()))],
    };
    let _ = w
        .gateway
        .replace_stdio_set(HOST, &w.hat, &[files], &w.key, unix_now())
        .unwrap();
    rusqlite::Connection::open(&w.db)
        .unwrap()
        .execute("UPDATE gw_stdio_servers SET env_ciphertext = zeroblob(64)", [])
        .unwrap();
}

#[test]
fn a_start_whose_mint_fails_stores_no_session() {
    let w = World::with_setup(false);
    let failed = w
        .store
        .create_session_with_mcp("s1", HOST, "agent", "/p", &w.hat.clone(), None, CLAUDE);
    assert!(failed.is_err());
    assert_eq!(w.store.find_session("s1").unwrap(), None);
    assert_eq!(w.token_rows("s1"), 0);
    let w = World::new();
    break_stdio(&w);
    assert!(
        w.store
            .create_session_with_mcp("s2", HOST, "agent", "/p", &w.hat.clone(), None, CLAUDE)
            .is_err()
    );
    assert_eq!(w.store.find_session("s2").unwrap(), None);
}

#[test]
fn a_resume_whose_mint_fails_leaves_the_session_parked() {
    let w = World::new();
    let first = w.active("s1");
    w.store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    break_stdio(&w);
    assert!(w.store.request_resume_with_mcp("s1", &w.hat.clone(), CLAUDE).is_err());
    assert_eq!(w.lifecycle("s1"), "parked");
    assert!(w.revoked_row(&first), "the old token stays revoked");
}

/// ACP core §4.3: every resume mints a fresh token that supersedes the
/// one before; the old one's streams are cut once the resume commits.
#[test]
fn a_resume_supersedes_the_token_and_cuts_the_old_ones_streams() {
    let w = World::new();
    let first = w.active("s1");
    w.store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    let watch = w.watch(&first);
    let ResumeRequest::Starting { mcp, .. } = w.store.request_resume_with_mcp("s1", &w.hat.clone(), CLAUDE).unwrap()
    else {
        panic!("not resumed");
    };
    let second = token(&mcp);
    assert_ne!(first, second);
    assert!(watch.token().is_cancelled());
    assert!(!w.live(&first));
    assert!(w.live(&second));
}

// Lane L4: every revoke site, each on a session whose token is live there.

#[test]
fn a_host_reported_park_revokes() {
    let w = World::new();
    let token = w.active("s1");
    let watch = w.watch(&token);
    w.store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    w.assert_revoked(&token, &watch, "session_parked");
}

#[test]
fn a_host_reported_close_revokes() {
    let w = World::new();
    let token = w.active("s1");
    let watch = w.watch(&token);
    w.store.ingest("s1", 2, &SessionBody::SessionClosed).unwrap();
    w.assert_revoked(&token, &watch, "session_closed");
}

#[test]
fn an_adapter_exit_revokes_ahead_of_its_park() {
    let w = World::new();
    let token = w.active("s1");
    let watch = w.watch(&token);
    let exited = SessionBody::AdapterExited {
        code: Some(1),
        signal: None,
        stderr_tail: String::new(),
    };
    w.store.ingest("s1", 2, &exited).unwrap();
    assert_eq!(w.lifecycle("s1"), "active", "the park comes after");
    w.assert_revoked(&token, &watch, "adapter_exited");
}

#[test]
fn a_failed_start_revokes() {
    let w = World::new();
    let token_of = token(&w.start("s1", &w.hat.clone(), CLAUDE));
    let watch = w.watch(&token_of);
    let failed = SessionBody::StartFailed {
        request_id: "r0".into(),
        code: "start_failed".into(),
        message: "no".into(),
    };
    w.store.ingest("s1", 1, &failed).unwrap();
    w.assert_revoked(&token_of, &watch, "start_failed");
}

#[test]
fn a_start_the_route_fails_revokes() {
    let w = World::new();
    let token_of = token(&w.start("s1", &w.hat.clone(), CLAUDE));
    let watch = w.watch(&token_of);
    w.store.mark_failed("s1", "mcp_isolation_unavailable").unwrap();
    w.assert_revoked(&token_of, &watch, "mark_failed");
}

#[test]
fn a_resume_the_route_fails_revokes() {
    let w = World::new();
    let token_of = token(&w.start("s1", &w.hat.clone(), CLAUDE));
    let watch = w.watch(&token_of);
    w.store
        .mark_failed_if_starting("s1", "mcp_isolation_unavailable")
        .unwrap();
    w.assert_revoked(&token_of, &watch, "mark_failed_if_starting");
}

/// `close_now`: an active session on a host that is away.
#[test]
fn closing_an_unattached_session_revokes() {
    let w = World::new();
    let token = w.active("s1");
    let watch = w.watch(&token);
    w.store.close_now("s1").unwrap();
    w.assert_revoked(&token, &watch, "close_now");
}

#[test]
fn a_rejected_reconcile_close_revokes() {
    let w = World::new();
    let token = w.active("s1");
    w.store.record_close_request("s1").unwrap();
    let watch = w.watch(&token);
    w.store.close_after_rejected_reconcile_close("s1").unwrap();
    assert_eq!(w.lifecycle("s1"), "closed");
    w.assert_revoked(&token, &watch, "close_after_rejected_reconcile_close");
}

#[test]
fn reconcile_revokes_a_start_the_host_never_got() {
    let w = World::new();
    let token_of = token(&w.start("s1", &w.hat.clone(), CLAUDE));
    let watch = w.watch(&token_of);
    w.store.reconcile_host(HOST, &[]).unwrap();
    assert_eq!(w.lifecycle("s1"), "failed");
    w.assert_revoked(&token_of, &watch, "reconcile start_not_delivered");
}

#[test]
fn reconcile_revokes_a_session_a_restarted_host_lost() {
    let w = World::new();
    let token = w.active("s1");
    let watch = w.watch(&token);
    w.store.reconcile_host(HOST, &[]).unwrap();
    assert_eq!(w.lifecycle("s1"), "parked");
    w.assert_revoked(&token, &watch, "reconcile host_restarted");
}

/// The row itself, not only the proxy's view: the proxy refuses a revoked
/// host's tokens by its join anyway, which would hide a missed revoke.
#[test]
fn a_host_revoke_revokes_every_token_of_the_host() {
    let w = World::new();
    let one = w.active("s1");
    let two = token(&w.start("s2", &w.hat.clone(), CLAUDE));
    let (watch_one, watch_two) = (w.watch(&one), w.watch(&two));
    w.store.revoke_host(HOST).unwrap();
    w.assert_revoked(&one, &watch_one, "revoke_host (active)");
    w.assert_revoked(&two, &watch_two, "revoke_host (starting)");
}

/// A re-assigned session has no running adapter, so no live token should
/// be left; one a missed revoke (or a database from before 8e) left would
/// reach the old hat's connections (plan 8d's O8).
#[test]
fn a_reassignment_revokes_a_token_left_live() {
    let w = World::new();
    let token = w.active("s1");
    w.store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    // Left live by hand.
    rusqlite::Connection::open(&w.db)
        .unwrap()
        .execute("UPDATE gw_session_tokens SET revoked_at = NULL", [])
        .unwrap();
    assert!(w.live(&token));
    let watch = w.watch(&token);
    assert!(matches!(
        w.store.reassign_hat("s1", &w.work.clone()).unwrap(),
        Reassign::Done(_)
    ));
    w.assert_revoked(&token, &watch, "reassign_hat");
}

/// The purge lane's marker: a delete revokes in its transaction.
#[test]
fn a_delete_revokes() {
    let w = World::new();
    let token = w.active("s1");
    let watch = w.watch(&token);
    let unattached = Unattached {
        lifecycle: "active".into(),
        presumed_parked: false,
    };
    assert!(matches!(
        w.store.delete_session("s1", Some(&unattached)).unwrap(),
        Deletion::Done { .. }
    ));
    w.assert_revoked(&token, &watch, "delete_session");
    // The tombstone keeps nothing of what it was given (plan 9a's scrub).
    let kept: (Option<String>, Option<i64>, Option<String>) = rusqlite::Connection::open(&w.db)
        .unwrap()
        .query_row(
            "SELECT mcp_delivery_mode, mcp_delivery_servers, mcp_delivery_at FROM sessions WHERE id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(kept, (None, None, None));
}

/// The delete's own revoke (the purge lane's marker), beyond `close_in`'s:
/// a closed session whose token a missed revoke left live.
#[test]
fn a_delete_revokes_a_token_left_live_on_a_closed_session() {
    let w = World::new();
    let token = w.active("s1");
    w.store.ingest("s1", 2, &SessionBody::SessionClosed).unwrap();
    rusqlite::Connection::open(&w.db)
        .unwrap()
        .execute("UPDATE gw_session_tokens SET revoked_at = NULL", [])
        .unwrap();
    let watch = w.watch(&token);
    assert!(matches!(
        w.store.delete_session("s1", None).unwrap(),
        Deletion::Done { .. }
    ));
    w.assert_revoked(&token, &watch, "delete_session of a closed session");
}

/// The kernel's purge hook (`LifecycleHooks::on_hat_purged`, lane L6):
/// the sessions' part, then the gateway's, so the hat's row can go.
#[test]
fn the_purge_hook_runs_the_gateways_part_too() {
    use hennery_kernel::lifecycle::LifecycleHooks;
    let w = World::new();
    w.active("s1");
    let given = w.start("s2", &w.work.clone(), CLAUDE);
    w.store
        .ingest("s2", 1, &SessionBody::session_started("r0", "agent-s2"))
        .unwrap();
    w.store
        .ingest(
            "s2",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    let hosts = Hosts::open(&w.db).unwrap();
    assert!(matches!(
        hosts.begin_purge(&w.work, unix_now()).unwrap(),
        hennery_kernel::hats::PurgeStart::Frozen { .. }
    ));
    let state = hennery_sessions::AppState::new(Store::open(&w.db).unwrap(), hosts, Operator::open(&w.db).unwrap());
    state.store.set_session_mcp(Arc::new(GatewayMcp::new(&w.state)));
    state.on_hat_purged(&w.work).unwrap();
    assert_eq!(w.token_rows("s2"), 0, "the hat's tokens went");
    assert!(!w.live(&token(&given)));
    let left: i64 = rusqlite::Connection::open(&w.db)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM gw_connections WHERE hat_id = ?1",
            [&w.work],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "the hat's connections went");
    assert_eq!(w.token_rows("s1"), 1, "another hat's token stays");
}

/// Lane L6: the gateway's part of a hat's purge deletes the hat's tokens
/// and cuts them; another hat's are left.
#[test]
fn a_hat_purge_takes_and_cuts_its_tokens() {
    let w = World::new();
    let theirs = token(&w.start("s1", &w.work.clone(), CLAUDE));
    let mine = token(&w.start("s2", &w.hat.clone(), CLAUDE));
    let (watch_theirs, watch_mine) = (w.watch(&theirs), w.watch(&mine));
    w.store.purge_gateway_hat(&w.work).unwrap();
    assert!(!w.live(&theirs));
    assert_eq!(w.token_rows("s1"), 0);
    assert!(watch_theirs.token().is_cancelled());
    w.assert_live(&mine, &watch_mine, "another hat's purge");
    // Idempotent.
    w.store.purge_gateway_hat(&w.work).unwrap();
}

// What does not revoke.

/// Lane L4's negative: a presumed park is a guess that the host may still
/// run the session; its token stays.
#[test]
fn a_presumed_park_does_not_revoke() {
    let w = World::new();
    let token = w.active("s1");
    let watch = w.watch(&token);
    w.store.presume_parked(HOST).unwrap();
    assert_eq!(w.lifecycle("s1"), "parked");
    w.assert_live(&token, &watch, "presume_parked");
}

/// The advisor's race: facts that do not apply to the session as it is now
/// (a resume in flight) must not end its fresh token.
#[test]
fn a_stale_fact_does_not_revoke_a_resumes_fresh_token() {
    let w = World::new();
    w.active("s1");
    w.store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    let ResumeRequest::Starting { mcp, .. } = w.store.request_resume_with_mcp("s1", &w.hat.clone(), CLAUDE).unwrap()
    else {
        panic!("not resumed");
    };
    let fresh = token(&mcp);
    let watch = w.watch(&fresh);
    for (seq, stale) in [
        (
            3,
            SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        ),
        (4, SessionBody::SessionClosed),
        (
            5,
            SessionBody::AdapterExited {
                code: Some(1),
                signal: None,
                stderr_tail: String::new(),
            },
        ),
    ] {
        w.store.ingest("s1", seq, &stale).unwrap();
        assert_eq!(w.lifecycle("s1"), "starting", "{stale:?}");
        w.assert_live(&fresh, &watch, &format!("{stale:?}"));
    }
}

#[test]
fn a_start_failed_that_does_not_apply_does_not_revoke() {
    let w = World::new();
    let token = w.active("s1");
    let watch = w.watch(&token);
    let failed = SessionBody::StartFailed {
        request_id: "r0".into(),
        code: "start_failed".into(),
        message: "late".into(),
    };
    w.store.ingest("s1", 2, &failed).unwrap();
    assert_eq!(w.lifecycle("s1"), "active");
    w.assert_live(&token, &watch, "a late start_failed");
}

// Decision 11 (the maintainer's open Q2, default chosen): a token the agent
// printed is stored and published redacted.

#[test]
fn a_token_in_an_acp_payload_is_stored_and_published_redacted() {
    let w = World::new();
    let token = w.active("s1");
    let printed = SessionBody::AcpUpdate {
        indexed: Default::default(),
        payload: json!({"sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": format!("my token is {token}")}}),
    };
    let published = w.store.ingest("s1", 2, &printed).unwrap();
    let stored = w.store.events("s1", 0, 100).unwrap();
    for (what, text) in [
        ("published", serde_json::to_string(&published).unwrap()),
        ("stored", serde_json::to_string(&stored).unwrap()),
    ] {
        assert!(!text.contains(&token), "{what}: {text}");
        assert!(text.contains("my token is hnry_session_<redacted>"), "{what}: {text}");
    }
    // The host re-sending it is the same fact, not a conflict.
    assert!(w.store.ingest("s1", 2, &printed).unwrap().is_empty());
    let raw = std::fs::read(&w.db).unwrap();
    let wal = std::fs::read(w.db.with_extension("db-wal")).unwrap_or_default();
    for bytes in [raw, wal] {
        assert!(
            !bytes.windows(token.len()).any(|window| window == token.as_bytes()),
            "the token is in the database"
        );
    }
}

/// Every place a fact's text is extracted from reads the redacted body: a
/// question's title, the session's title.
#[test]
fn a_token_in_a_question_or_a_title_is_redacted_too() {
    let w = World::new();
    let token = w.active("s1");
    let question: SessionBody = serde_json::from_value(json!({
        "kind": "pending_opened", "pending_id": "p1",
        "indexed": {"pending": {"id": "p1", "kind": "permission", "option_ids": ["allow"],
                                "title": format!("run {token}?")}},
        "payload": {"toolCall": {"title": format!("run {token}?")}}
    }))
    .unwrap();
    w.store.ingest("s1", 2, &question).unwrap();
    let pending = serde_json::to_string(&w.store.open_pending("s1").unwrap()).unwrap();
    assert!(!pending.contains(&token), "{pending}");
    let titled = SessionBody::AcpUpdate {
        indexed: serde_json::from_value(json!({"title": format!("about {token}")})).unwrap(),
        payload: json!({"sessionUpdate": "session_info_update", "title": format!("about {token}")}),
    };
    w.store.ingest("s1", 3, &titled).unwrap();
    let item = serde_json::to_string(&w.store.find_session_item("s1").unwrap()).unwrap();
    assert!(!item.contains(&token), "{item}");
}

/// The advisor's 7: redaction reads a body back from its JSON, so that
/// round trip must be the identity for every kind of body.
#[test]
fn every_body_reads_back_as_it_was_written() {
    let bodies = [
        json!({"kind": "session_started", "request_id": "r", "agent_session_id": "a",
               "indexed": {"title": "t", "current_model": "m"}}),
        json!({"kind": "start_failed", "request_id": "r", "code": "c", "message": "m"}),
        json!({"kind": "turn_started", "request_id": "r", "turn_id": "t"}),
        json!({"kind": "acp_update", "indexed": {"turn_id": "t", "early": true}, "payload": {"a": [1, "b"]}}),
        json!({"kind": "turn_ended", "turn_id": "t", "outcome": "failed", "stop_reason": "s", "error": "e"}),
        json!({"kind": "session_parked", "reason": "adapter_exited"}),
        json!({"kind": "session_closed"}),
        json!({"kind": "adapter_exited", "code": 1, "signal": 9, "stderr_tail": "x"}),
        json!({"kind": "host_note", "note": "n", "text": "t"}),
        json!({"kind": "config_applied", "request_id": "r", "indexed": {"current_mode": "m"}}),
        json!({"kind": "pending_opened", "pending_id": "p",
               "indexed": {"pending": {"id": "p", "kind": "elicitation", "title": "t"}}, "payload": {}}),
        json!({"kind": "pending_resolved", "pending_id": "p", "resolution": "cancelled", "reason": "session_closed"}),
        json!({"kind": "answer_result", "pending_id": "p", "request_id": "r", "delivered": true}),
        json!({"kind": "git_state", "branch": "b", "dirty": true, "worktree": false, "head": "h", "base_commit": "c"}),
    ];
    let mut kinds = std::collections::BTreeSet::new();
    for json in bodies {
        let body: SessionBody = serde_json::from_value(json.clone()).unwrap_or_else(|e| panic!("{json}: {e}"));
        kinds.insert(json["kind"].as_str().unwrap().to_string());
        let again: SessionBody = serde_json::from_value(serde_json::to_value(&body).unwrap()).unwrap();
        assert_eq!(again, body, "{json}");
    }
    assert_eq!(kinds.len(), 14, "one of each kind");
}
