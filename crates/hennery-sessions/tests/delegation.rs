use hennery_gateway::api::GatewayState;
use hennery_gateway::identity::SessionIdentity;
use hennery_gateway::key::MasterKey;
use hennery_gateway::scope::ProxyStore;
use hennery_gateway::session::GatewayMcp;
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hats::HatChange;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use hennery_proto::frames::{McpIsolation, McpServer, SessionBody};
use hennery_sessions::hub::Hub;
use hennery_sessions::store::{McpContext, Store};
use std::sync::Arc;
struct World {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    store: Store,
    gateway: Arc<GatewayStore>,
    hat: String,
    other: String,
}
impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        hosts
            .register(
                "host",
                &Enrollment {
                    public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                    name: "test".into(),
                    host_version: "0".into(),
                    platform: "linux".into(),
                },
                1,
            )
            .unwrap();
        hosts
            .record_hello(
                "host",
                "0",
                &hennery_proto::frames::Capabilities(vec![hennery_proto::frames::Capability::McpServers]),
                unix_now(),
            )
            .unwrap();
        hosts
            .record_mcp_isolation(
                "host",
                &hennery_proto::frames::AgentIsolation(
                    [("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect(),
                ),
            )
            .unwrap();
        let hat = hosts.host("host").unwrap().unwrap().default_hat_id;
        let HatChange::Done(other) = hosts.create_hat("Other", None, 1).unwrap() else {
            panic!()
        };
        let operator = Arc::new(Operator::open(&db).unwrap());
        let setup = operator.issue_setup_token(unix_now()).unwrap().unwrap();
        assert!(matches!(
            operator
                .set_up(&setup, "correct horse battery", "https://hennery.example", unix_now())
                .unwrap(),
            SetupOutcome::Done { .. }
        ));
        let gateway = Arc::new(GatewayStore::open(&db).unwrap());
        let runtime = Arc::new(hennery_gateway::runtime::Runtime::new(
            gateway.clone(),
            Arc::new(ProxyStore::open(&db).unwrap()),
            Arc::new(MasterKey::from_bytes([7; 32])),
            hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap(),
            Arc::new(hennery_gateway::notify::Silent),
        ));
        let state = GatewayState { runtime, operator };
        let store = Store::open(&db).unwrap();
        store.set_session_mcp(Arc::new(GatewayMcp::for_collector(&state)));
        gateway.set_session_tools(&hat, true).unwrap();
        gateway.set_session_tools(&other.id, true).unwrap();
        Self {
            _dir: dir,
            db,
            store,
            gateway,
            hat,
            other: other.id,
        }
    }
    fn start(&self, id: &str, hat: &str, isolation: McpIsolation) -> Option<SessionIdentity> {
        let given = self
            .store
            .create_session_with_mcp(
                id,
                "host",
                "claude",
                "/project",
                hat,
                None,
                McpContext {
                    source_policy: false,
                    native_sources: false,
                    capable: true,
                    isolation,
                    rules_name_other_hats: false,
                },
            )
            .unwrap()
            .unwrap();
        given.servers.iter().find_map(|s| match s {
            McpServer::Http { name, headers, .. } if name == "hennery" => self
                .gateway
                .authenticate_session(headers[0].value.strip_prefix("Bearer ").unwrap())
                .unwrap(),
            _ => None,
        })
    }
    fn active(&self, id: &str) {
        self.store
            .ingest(id, 1, &SessionBody::session_started("request", format!("agent-{id}")))
            .unwrap();
    }
}
#[test]
fn explicit_combined_keeps_builtin_scope_on_a_nondefault_hat_and_rechecks_live_support() {
    use hennery_proto::{
        frames::{Capabilities, Capability},
        mcp_source::McpSourcePolicy,
    };
    let w = World::new();
    let hosts = Hosts::open(&w.db).unwrap();
    hosts
        .update_hat_with_policy(&w.other, None, None, None, Some(McpSourcePolicy::ManagedPlusNative))
        .unwrap();
    let caps = Capabilities(vec![
        Capability::McpServers,
        Capability::McpSourcePolicy,
        Capability::McpNativeSources,
    ]);
    hosts.record_hello("host", "0", &caps, unix_now()).unwrap();
    let given = w
        .store
        .create_session_with_mcp(
            "combined",
            "host",
            "claude",
            "/project",
            &w.other,
            None,
            McpContext {
                capable: true,
                source_policy: true,
                native_sources: true,
                isolation: McpIsolation::ClaudeStrict,
                rules_name_other_hats: true,
            },
        )
        .unwrap()
        .unwrap();
    let token = given
        .servers
        .iter()
        .find_map(|server| match server {
            McpServer::Http { name, headers, .. } if name == "hennery" => headers[0].value.strip_prefix("Bearer "),
            _ => None,
        })
        .unwrap();
    let identity = w.gateway.authenticate_session(token).unwrap().unwrap();
    assert_eq!(identity.principal.hat_id, w.other);
    assert!(w.store.session_tool_context(&identity, 1, false).unwrap().is_some());
    assert!(w.store.session_tool_context(&identity, 1, true).unwrap().is_none());
    let source = given.source_policy.unwrap();
    let mut ack = SessionBody::session_started(&source.request_id, "agent-combined");
    if let SessionBody::SessionStarted { indexed, .. } = &mut ack {
        indexed.mcp_source_policy = Some(source);
    }
    w.store.ingest("combined", 1, &ack).unwrap();
    assert!(w.store.session_tool_context(&identity, 1, true).unwrap().is_some());
    let mut forged = identity.clone();
    forged.principal.hat_id = w.hat.clone();
    assert!(w.store.session_tool_context(&forged, 1, true).unwrap().is_none());
    hosts
        .record_hello("host", "old", &Capabilities(vec![Capability::McpServers]), unix_now())
        .unwrap();
    assert!(w.store.session_tool_context(&identity, 1, false).unwrap().is_none());
}

#[test]
fn verified_context_checks_generation_session_owner_hat_cwd_and_live_isolation() {
    let w = World::new();
    let identity = w.start("parent", &w.hat, McpIsolation::None).unwrap();
    let ctx = w.store.session_tool_context(&identity, 1, false).unwrap().unwrap();
    assert_eq!(ctx.session_id(), "parent");
    assert_eq!(ctx.canonical_cwd(), "/project");
    assert!(w.store.session_tool_context(&identity, 1, true).unwrap().is_none());
    w.active("parent");
    assert!(w.store.session_tool_context(&identity, 1, true).unwrap().is_some());
    let mut forged = identity.clone();
    forged.owner_id = "foreign".into();
    assert!(w.store.session_tool_context(&forged, 1, false).unwrap().is_none());
    forged = identity.clone();
    forged.principal.hat_id = w.other.clone();
    assert!(w.store.session_tool_context(&forged, 1, false).unwrap().is_none());
    let raw = hennery_kernel::db::open(&w.db).unwrap();
    raw.execute("UPDATE hosts SET default_hat_id = ?1 WHERE id = 'host'", [&w.other])
        .unwrap();
    assert!(w.store.session_tool_context(&identity, 1, false).unwrap().is_none());
    assert!(w.start("fallback", &w.hat, McpIsolation::None).is_none());
    let isolated = w.start("isolated", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    assert!(w.store.session_tool_context(&isolated, 1, false).unwrap().is_some());
    raw.execute(
        "UPDATE sessions SET cwd = '/project/../escape' WHERE id = 'isolated'",
        [],
    )
    .unwrap();
    assert!(w.store.session_tool_context(&isolated, 1, false).unwrap().is_none());
}
#[test]
fn family_storage_is_immutable_direct_scoped_and_terminally_detached() {
    let w = World::new();
    let identity = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    w.start("child", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.start("sibling", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.start("foreign-hat", &w.other, McpIsolation::ClaudeStrict).unwrap();
    let mut conn = hennery_kernel::db::open(&w.db).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(w.store.bind_family_in(&tx, &identity, 1, "child", 0).unwrap());
    assert!(!w.store.bind_family_in(&tx, &identity, 1, "child", 0).unwrap());
    assert!(!w.store.bind_family_in(&tx, &identity, 1, "parent", 0).unwrap());
    assert!(!w.store.bind_family_in(&tx, &identity, 1, "foreign-hat", 0).unwrap());
    tx.commit().unwrap();
    assert!(w.store.direct_child(&identity, 1, "child").unwrap().is_some());
    assert!(
        conn.execute("UPDATE session_families SET parent_id = 'sibling'", [])
            .is_err()
    );
    for id in ["sibling", "parent", "foreign-hat", "absent"] {
        assert!(w.store.direct_child(&identity, 1, id).unwrap().is_none());
    }
    conn.execute("UPDATE sessions SET close_requested = 1 WHERE id = 'parent'", [])
        .unwrap();
    assert!(w.store.direct_child(&identity, 1, "child").unwrap().is_none());
    conn.execute(
        "UPDATE sessions SET close_requested = 0, lifecycle = 'active' WHERE id = 'parent'",
        [],
    )
    .unwrap();
    assert!(
        w.store.direct_child(&identity, 1, "child").unwrap().is_none(),
        "resume cannot revive a detached edge"
    );
    conn.execute("UPDATE sessions SET lifecycle = 'deleted' WHERE id = 'parent'", [])
        .unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM session_families", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let state: String = conn
        .query_row("SELECT lifecycle FROM sessions WHERE id = 'child'", [], |r| r.get(0))
        .unwrap();
    assert_ne!(state, "deleted");
}

#[test]
fn delegated_new_sessions_resolve_source_overrides_for_the_child_agent() {
    use hennery_proto::mcp_source::McpSourcePolicy::{Legacy, ManagedOnly};
    let w = World::new();
    let parent = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    let hosts = Hosts::open(&w.db).unwrap();
    let patch = [("claude".into(), Some(ManagedOnly)), ("codex".into(), Some(Legacy))]
        .into_iter()
        .collect();
    hosts
        .update_hat_with_sources(&w.hat, None, None, None, None, Some(&patch))
        .unwrap();
    let context = McpContext {
        capable: true,
        source_policy: true,
        native_sources: true,
        isolation: McpIsolation::ClaudeStrict,
        rules_name_other_hats: false,
    };
    assert!(
        w.store
            .create_delegated_session(&parent, 1, "claude-child", "claude", "/child", context, 0)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        w.store
            .mcp_source_state("claude-child")
            .unwrap()
            .unwrap()
            .requested
            .unwrap()
            .policy,
        ManagedOnly
    );
    let codex = McpContext {
        isolation: McpIsolation::None,
        ..context
    };
    assert!(
        w.store
            .create_delegated_session(&parent, 1, "codex-child", "codex", "/child", codex, 0)
            .unwrap()
            .is_some()
    );
    let state = w.store.mcp_source_state("codex-child").unwrap().unwrap();
    assert_eq!(state.policy, Legacy);
    assert!(state.requested.is_none());
}

#[test]
fn delegated_allocation_commits_session_family_token_together_and_rolls_back_all_three() {
    let w = World::new();
    let parent = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    let mcp = McpContext {
        source_policy: false,
        native_sources: false,
        capable: true,
        isolation: McpIsolation::ClaudeStrict,
        rules_name_other_hats: false,
    };
    assert!(
        w.store
            .create_delegated_session(&parent, 1, "child", "claude", "/child", mcp, 0)
            .unwrap()
            .is_some()
    );
    assert!(w.store.direct_child(&parent, 1, "child").unwrap().is_some());
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    conn.execute_batch("CREATE TRIGGER refuse_child_mint BEFORE INSERT ON gw_session_tokens WHEN NEW.session_id = 'fail' BEGIN SELECT RAISE(ABORT, 'test mint failure'); END;").unwrap();
    assert!(
        w.store
            .create_delegated_session(&parent, 1, "fail", "claude", "/child", mcp, 0)
            .is_err()
    );
    for table in ["sessions", "session_families", "gw_session_tokens"] {
        let key = if table == "sessions" {
            "id"
        } else if table == "session_families" {
            "child_id"
        } else {
            "session_id"
        };
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table} WHERE {key} = 'fail'"), [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
    w.gateway.set_session_tools(&w.hat, false).unwrap();
    assert!(
        w.store
            .create_delegated_session(&parent, 1, "stale", "claude", "/child", mcp, 0)
            .unwrap()
            .is_none()
    );
}

#[test]
fn family_rollback_immutability_reassignment_delete_and_purge_cannot_restore_authority() {
    for transition in ["reassign", "delete-child", "purge"] {
        let w = World::new();
        let parent = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
        w.active("parent");
        w.start("child", &w.hat, McpIsolation::ClaudeStrict).unwrap();
        let mut conn = hennery_kernel::db::open(&w.db).unwrap();
        let tx = conn.transaction().unwrap();
        assert!(w.store.bind_family_in(&tx, &parent, 1, "child", 0).unwrap());
        drop(tx);
        assert!(w.store.direct_child(&parent, 1, "child").unwrap().is_none());
        let tx = conn.transaction().unwrap();
        assert!(w.store.bind_family_in(&tx, &parent, 1, "child", 0).unwrap());
        tx.commit().unwrap();
        assert!(
            conn.execute("UPDATE session_families SET parent_id = 'child'", [])
                .is_err()
        );
        // Reverse the endpoints' lifecycles so only the recursive cycle
        // guard rejects this otherwise valid insertion. Roll back the fixture
        // change before exercising the original family's terminal transitions.
        let tx = conn.transaction().unwrap();
        tx.execute(
            "UPDATE sessions SET lifecycle = CASE id WHEN 'parent' THEN 'starting' ELSE 'active' END
             WHERE id IN ('parent', 'child')",
            [],
        )
        .unwrap();
        assert!(tx.execute("INSERT INTO session_families(owner_id,child_id,parent_id,root_id,hat_id,created_event_id) VALUES (?1,'parent','child','child',?2,0)",rusqlite::params![w.store.owner_id(),w.hat]).is_err());
        tx.rollback().unwrap();
        match transition {
            "reassign" => {
                conn.execute("UPDATE sessions SET hat_id = ?1 WHERE id = 'child'", [&w.other])
                    .unwrap();
                conn.execute("UPDATE sessions SET hat_id = ?1 WHERE id = 'child'", [&w.hat])
                    .unwrap();
            }
            "delete-child" => {
                conn.execute("UPDATE sessions SET lifecycle = 'deleted' WHERE id = 'child'", [])
                    .unwrap();
            }
            "purge" => {
                conn.execute(
                    "INSERT INTO purged_hats(hat_id,owner_id,purged_at) VALUES (?1,?2,0)",
                    rusqlite::params![w.hat, w.store.owner_id()],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            w.store.direct_child(&parent, 1, "child").unwrap().is_none(),
            "{transition}"
        );
    }
}

#[test]
fn each_live_caller_row_fence_is_rechecked_inside_the_transaction() {
    let w = World::new();
    let identity = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let _ = w.start("sibling", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let mut forged = identity.clone();
    forged.principal.kind = hennery_gateway::scope::PrincipalKind::Session {
        session_id: "sibling".into(),
        host_id: "host".into(),
    };
    assert!(w.store.session_tool_context(&forged, 1, false).unwrap().is_none());
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    for (change, restore) in [
        ("lifecycle = 'failed'", "lifecycle = 'starting'"),
        ("close_requested = 1", "close_requested = 0"),
        ("presumed_parked = 1", "presumed_parked = 0"),
        ("mcp_delivery_mode = 'fallback'", "mcp_delivery_mode = 'isolated'"),
        ("cwd = '/p/../escape'", "cwd = '/project'"),
    ] {
        conn.execute(&format!("UPDATE sessions SET {change} WHERE id = 'parent'"), [])
            .unwrap();
        assert!(
            w.store.session_tool_context(&identity, 1, false).unwrap().is_none(),
            "{change}"
        );
        conn.execute(&format!("UPDATE sessions SET {restore} WHERE id = 'parent'"), [])
            .unwrap();
        assert!(w.store.session_tool_context(&identity, 1, false).unwrap().is_some());
    }
    conn.execute("UPDATE sessions SET hat_id = ?1 WHERE id = 'parent'", [&w.other])
        .unwrap();
    assert!(w.store.session_tool_context(&identity, 1, false).unwrap().is_none());
    conn.execute("UPDATE sessions SET hat_id = ?1 WHERE id = 'parent'", [&w.hat])
        .unwrap();
    conn.execute(
        "UPDATE gw_session_tokens SET delivered_delegation_revision = NULL WHERE session_id = 'parent'",
        [],
    )
    .unwrap();
    assert!(w.store.session_tool_context(&identity, 1, false).unwrap().is_none());
}

#[test]
fn current_persisted_hello_capability_and_isolation_are_transactional_fences() {
    let w = World::new();
    let identity = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    assert!(w.store.session_tool_context(&identity, 1, false).unwrap().is_some());
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    for capabilities in ["[]", "invalid-json"] {
        conn.execute("UPDATE hosts SET capabilities = ?1 WHERE id = 'host'", [capabilities])
            .unwrap();
        assert!(
            w.store.session_tool_context(&identity, 1, false).unwrap().is_none(),
            "{capabilities}"
        );
    }
    conn.execute(
        "UPDATE hosts SET capabilities = '[\"mcp_servers\"]' WHERE id = 'host'",
        [],
    )
    .unwrap();
    for isolation in ["{\"claude\":\"none\"}", "invalid-json", "{}"] {
        conn.execute("UPDATE hosts SET mcp_isolation = ?1 WHERE id = 'host'", [isolation])
            .unwrap();
        assert!(
            w.store.session_tool_context(&identity, 1, false).unwrap().is_none(),
            "{isolation}"
        );
    }
    conn.execute(
        "UPDATE hosts SET mcp_isolation = '{\"claude\":\"claude_strict\"}' WHERE id = 'host'",
        [],
    )
    .unwrap();
    assert!(w.store.session_tool_context(&identity, 1, false).unwrap().is_some());
}

#[test]
fn allocation_rejects_noncanonical_child_input_without_storing_a_row() {
    let w = World::new();
    let parent = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    let mode = McpContext {
        source_policy: false,
        native_sources: false,
        capable: true,
        isolation: McpIsolation::ClaudeStrict,
        rules_name_other_hats: false,
    };
    assert!(
        w.store
            .create_delegated_session(&parent, 1, "child", "claude", "/p/../escape", mode, 0)
            .unwrap()
            .is_none()
    );
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM sessions WHERE id = 'child'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn purging_an_old_hat_scrubs_detached_edges_when_both_endpoints_moved() {
    let w = World::new();
    let parent = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    w.start("child", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let mut conn = hennery_kernel::db::open(&w.db).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(w.store.bind_family_in(&tx, &parent, 1, "child", 0).unwrap());
    tx.commit().unwrap();
    conn.execute(
        "UPDATE sessions SET hat_id = ?1 WHERE id IN ('parent','child')",
        [&w.other],
    )
    .unwrap();
    conn.execute("UPDATE hosts SET default_hat_id = ?1 WHERE id = 'host'", [&w.other])
        .unwrap();
    conn.execute(
        "DELETE FROM gw_session_tokens WHERE owner_id = ?1 AND hat_id = ?2",
        rusqlite::params![w.store.owner_id(), w.hat],
    )
    .unwrap();
    conn.execute(
        "UPDATE settings SET value = ?1 WHERE key = 'default_hat_id' AND owner_id = ?2",
        rusqlite::params![w.other, w.store.owner_id()],
    )
    .unwrap();
    let hosts = Hosts::open(&w.db).unwrap();
    hosts.begin_purge(&w.hat, unix_now()).unwrap();
    assert!(hosts.finish_purge(&w.hat).unwrap());
    let count: i64 = conn
        .query_row("SELECT count(*) FROM session_families", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM sessions WHERE hat_id = ?1", [&w.other], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn d2_followups_are_durable_idempotent_scoped_and_close_fenced() {
    let w = World::new();
    let parent = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    w.start("child", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.start("unrelated", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let mut conn = hennery_kernel::db::open(&w.db).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(w.store.bind_family_in(&tx, &parent, 1, "child", 0).unwrap());
    tx.commit().unwrap();
    w.active("child");
    let first = w
        .store
        .queue_followup(&parent, 1, "key", "digest", "child", "hello")
        .unwrap();
    let again = w
        .store
        .queue_followup(&parent, 1, "key", "digest", "child", "hello")
        .unwrap();
    assert_eq!(first, again);
    assert!(
        w.store
            .queue_followup(&parent, 1, "key", "other", "child", "changed")
            .unwrap_err()
            .to_string()
            .contains("operation_conflict")
    );
    assert!(
        w.store
            .queue_followup(&parent, 1, "bad", "digest", "unrelated", "hidden")
            .is_err()
    );
    assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (1, 5));
    w.store.record_close_request("parent").unwrap();
    assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (0, 0));
    assert!(
        w.store
            .queue_followup(&parent, 1, "late", "digest", "child", "late")
            .is_err()
    );
}

#[test]
fn d2_queue_caps_and_policy_changes_cancel_without_reviving() {
    let w = World::new();
    let parent = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    w.start("child", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let mut conn = hennery_kernel::db::open(&w.db).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(w.store.bind_family_in(&tx, &parent, 1, "child", 0).unwrap());
    tx.commit().unwrap();
    for i in 0..32 {
        w.store
            .queue_followup(
                &parent,
                1,
                &format!("key-{i}"),
                &format!("digest-{i}"),
                "child",
                "hello",
            )
            .unwrap();
    }
    assert!(
        w.store
            .queue_followup(&parent, 1, "overflow", "digest", "child", "hello")
            .unwrap_err()
            .to_string()
            .contains("queue_full")
    );
    assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (32, 160));
    w.gateway.set_session_tools(&w.hat, false).unwrap();
    w.gateway.set_session_tools(&w.hat, true).unwrap();
    assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (0, 0));
}

fn child_world() -> (World, SessionIdentity) {
    let w = World::new();
    let parent = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    w.start("child", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let mut conn = hennery_kernel::db::open(&w.db).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(w.store.bind_family_in(&tx, &parent, 1, "child", 0).unwrap());
    tx.commit().unwrap();
    w.active("child");
    (w, parent)
}
#[test]
fn d2_answers_reuse_the_operator_slot_but_permissions_require_operator() {
    use hennery_proto::{
        frames::{ElicitationAction, Indexed, PendingExtract, PendingKind},
        rest::AnswerRequest,
    };
    let (w, parent) = child_world();
    for (seq, id, kind) in [
        (2, "ordinary", PendingKind::Elicitation),
        (3, "permission", PendingKind::Permission),
    ] {
        w.store
            .ingest(
                "child",
                seq,
                &SessionBody::PendingOpened {
                    pending_id: id.into(),
                    indexed: Indexed {
                        pending: Some(Box::new(PendingExtract {
                            id: id.into(),
                            kind,
                            option_ids: Some(vec!["allow".into()]),
                            title: Some("question".into()),
                        })),
                        ..Default::default()
                    },
                    payload: serde_json::json!({"message":"question","requestedSchema":{"type":"object"}}),
                },
            )
            .unwrap();
    }
    let answer = AnswerRequest::Elicitation {
        action: ElicitationAction::Accept,
        content: Some(serde_json::json!({"yes":true})),
    };
    let receipt = w
        .store
        .queue_ordinary_answer(
            &parent,
            1,
            "digest",
            &hennery_sessions::delegation_control::DelegatedAnswer {
                key: "answer",
                child: "child",
                pending: "ordinary",
                answer: &answer,
            },
            &Hub::new(),
        )
        .unwrap();
    assert_eq!(
        receipt,
        w.store
            .queue_ordinary_answer(
                &parent,
                1,
                "digest",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "answer",
                    child: "child",
                    pending: "ordinary",
                    answer: &answer
                },
                &Hub::new(),
            )
            .unwrap()
    );
    assert!(matches!(
        w.store.submit_answer("child", "ordinary", &answer).unwrap(),
        hennery_sessions::store::AnswerSubmission::AlreadyAnswered
    ));
    assert!(
        w.store
            .queue_ordinary_answer(
                &parent,
                1,
                "other",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "permission",
                    child: "child",
                    pending: "permission",
                    answer: &answer
                },
                &Hub::new(),
            )
            .unwrap_err()
            .to_string()
            .contains("operator_required")
    );
}
#[test]
fn d2_spawns_are_atomic_independent_and_one_generation() {
    let (w, parent) = child_world();
    let config = hennery_proto::frames::SessionConfig {
        model: Some("independent".into()),
        ..Default::default()
    };
    let receipt = w
        .store
        .queue_spawn(
            &parent,
            1,
            "digest",
            &hennery_sessions::delegation_queue::DelegatedLaunch {
                key: "spawn",
                agent: "codex",
                cwd: "/project",
                brief: "brief",
                name: Some("worker"),
                config: &config,
                mcp: McpContext::NONE,
            },
        )
        .unwrap();
    let child = receipt.child_id.as_ref().unwrap();
    assert_eq!(w.store.find_session(child).unwrap().unwrap().agent, "codex");
    assert_eq!(
        w.store
            .queue_spawn(
                &parent,
                1,
                "digest",
                &hennery_sessions::delegation_queue::DelegatedLaunch {
                    key: "spawn",
                    agent: "codex",
                    cwd: "/project",
                    brief: "brief",
                    name: Some("worker"),
                    config: &config,
                    mcp: McpContext::NONE
                }
            )
            .unwrap(),
        receipt
    );
    assert_eq!(w.store.delegation_queue_depth(child).unwrap(), (1, 5));
    let child_identity = w.start("other-child", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let mut conn = hennery_kernel::db::open(&w.db).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(w.store.bind_family_in(&tx, &parent, 1, "other-child", 0).unwrap());
    tx.commit().unwrap();
    w.active("other-child");
    assert!(
        w.store
            .queue_spawn(
                &child_identity,
                1,
                "digest",
                &hennery_sessions::delegation_queue::DelegatedLaunch {
                    key: "nested",
                    agent: "claude",
                    cwd: "/project",
                    brief: "brief",
                    name: None,
                    config: &Default::default(),
                    mcp: McpContext::NONE
                }
            )
            .unwrap_err()
            .to_string()
            .contains("generation_limit")
    );
}
#[test]
fn d2_resume_and_dispatch_require_fresh_complete_generation_evidence() {
    use hennery_proto::delegation_host::{
        CatalogFreshness, ConfigCompleteness, ConfigEvidence, HostCatalog, SecurityClassification,
    };
    use hennery_proto::frames::{AgentIsolation, Capabilities, Capability, ConfigValue, Indexed};
    let (w, parent) = child_world();
    let hub = hennery_sessions::hub::Hub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let caps = Capabilities(vec![Capability::McpServers, Capability::DelegationConfig]);
    let reg = hub
        .register(
            "host",
            tx,
            caps,
            AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect()),
        )
        .unwrap();
    hub.mark_ready("host", reg.conn_id);
    let catalog = HostCatalog {
        saved_at: None,
        agent: "claude".into(),
        adapter_revision: "v1".into(),
        config_options: vec![],
        defaults: Default::default(),
        security: SecurityClassification {
            known: true,
            axes: Default::default(),
        },
        freshness: CatalogFreshness::Fresh,
    };
    w.store
        .queue_followup(&parent, 1, "send", "digest", "child", "hello")
        .unwrap();
    assert!(!w.store.dispatch_delegated("child", &catalog, &hub).unwrap());
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::SessionParked {
                reason: hennery_proto::frames::ParkReason::Operator,
            },
        )
        .unwrap();
    assert!(
        w.store
            .attach_delegated("child", McpContext::NONE, &catalog, &hub)
            .unwrap()
    );
    let frame = rx.try_recv().unwrap();
    let generation = match frame {
        hennery_proto::frames::CollectorFrame::DelegatedAttach { attach_generation, .. } => attach_generation,
        other => panic!("{other:?}"),
    };
    let mut seq = 3;
    w.store
        .ingest("child", seq, &SessionBody::session_started("resume", "agent-child"))
        .unwrap();
    seq += 1;
    for (attach_gen, complete, should_send) in [
        (generation - 1, ConfigCompleteness::Complete, false),
        (generation, ConfigCompleteness::Partial, false),
        (generation, ConfigCompleteness::Complete, true),
    ] {
        w.store
            .ingest(
                "child",
                seq,
                &SessionBody::AcpUpdate {
                    indexed: Indexed {
                        config_evidence: Some(Box::new(ConfigEvidence {
                            attach_generation: attach_gen,
                            config_revision: seq,
                            completeness: complete,
                            actual: Default::default(),
                        })),
                        ..Default::default()
                    },
                    payload: serde_json::json!({}),
                },
            )
            .unwrap();
        seq += 1;
        assert_eq!(
            w.store.dispatch_delegated("child", &catalog, &hub).unwrap(),
            should_send
        );
    }
    let prompt = rx.try_recv().unwrap();
    assert!(matches!(
        prompt,
        hennery_proto::frames::CollectorFrame::DelegatedPrompt { .. }
    ));
    assert!(!w.store.dispatch_delegated("child", &catalog, &hub).unwrap());
    let _ = ConfigValue::Bool(false);
}
