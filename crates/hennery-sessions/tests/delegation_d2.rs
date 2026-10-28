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
use hennery_sessions::store::{McpContext, Store};
use std::sync::Arc;
struct World {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    store: Store,
    gateway: Arc<GatewayStore>,
    revocations: hennery_gateway::revocation::Revocations,
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
        let revocations = runtime.revocations.clone();
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
            revocations,
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

use hennery_proto::{
    delegation::{OperationReceipt, OperationState},
    delegation_host::{CatalogFreshness, ConfigCompleteness, ConfigEvidence, HostCatalog, SecurityClassification},
    frames::{
        AgentIsolation, Capabilities, Capability, CollectorFrame, ConfigValue, ElicitationAction, Indexed, ParkReason,
        PendingExtract, PendingKind, PendingReason, PendingResolution, SessionConfig, TurnOutcome,
    },
    plugin::InputOrigin,
    rest::AnswerRequest,
};
use hennery_sessions::{hub::Hub, store::AnswerSubmission};
use rusqlite::params;
use serde_json::json;
use std::{collections::BTreeMap, sync::Barrier};

fn raw(w: &World) -> hennery_kernel::db::OwnedConnection {
    hennery_kernel::db::open(&w.db).unwrap()
}
fn child(w: &World, parent: &SessionIdentity, id: &str, hat: &str) -> SessionIdentity {
    let identity = w.start(id, hat, McpIsolation::ClaudeStrict).unwrap();
    let mut conn = raw(w);
    let tx = conn.transaction().unwrap();
    assert!(w.store.bind_family_in(&tx, parent, 1, id, 0).unwrap());
    tx.commit().unwrap();
    w.active(id);
    identity
}
fn pair() -> (World, SessionIdentity) {
    let w = World::new();
    let parent = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    child(&w, &parent, "child", &w.hat);
    (w, parent)
}

fn start_child_report(w: &World) {
    assert!(
        w.store
            .open_turn("child", "report", &[json!({"type":"text","text":"work"})])
            .unwrap()
    );
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::TurnStarted {
                request_id: "work".into(),
                turn_id: "report".into(),
            },
        )
        .unwrap();
}

#[test]
fn closed_attention_child_closed_by_parent_or_operator_suppresses_its_final_report() {
    for parent_closes in [true, false] {
        let (w, p) = pair();
        start_child_report(&w);
        let (hub, _rx) = connected();
        if parent_closes {
            w.store
                .queue_child_close(&p, 1, "close", "close", "child", &hub)
                .unwrap();
        } else {
            w.store.record_close_request("child").unwrap();
        }
        ended(&w, "child", 3, "report");
        assert!(
            attention(&w).is_empty(),
            "close intent must suppress the final report: parent_closes={parent_closes}"
        );
        w.store.ingest("child", 4, &SessionBody::SessionClosed).unwrap();
        assert!(attention(&w).is_empty());
        assert_eq!(w.store.family_summary("child").unwrap().unwrap().operator_attention, 0);
    }
}

#[test]
fn closed_attention_parent_closed_with_open_or_closed_children_suppresses_status_reports() {
    for report_first in [true, false] {
        for close_child in [true, false] {
            let (w, _) = pair();
            start_child_report(&w);
            if report_first {
                ended(&w, "child", 3, "report");
            }
            w.store.close_now("parent").unwrap();
            if !report_first {
                ended(&w, "child", 3, "report");
            }
            if close_child {
                w.store.close_now("child").unwrap();
            }
            assert!(
                attention(&w).is_empty(),
                "report_first={report_first}, close_child={close_child}"
            );
            assert_eq!(w.store.family_summary("child").unwrap().unwrap().operator_attention, 0);
        }
    }
}

#[test]
fn closed_attention_parent_closed_keeps_only_a_live_child_needing_a_human() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store.close_now("parent").unwrap();
    let rows = attention(&w);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason, "parent_closed_child_needs_human");
    assert_eq!(rows[0].pending_id.as_deref(), Some("question"));
    w.store.close_now("child").unwrap();
    assert!(attention(&w).is_empty());
}

#[test]
fn closed_attention_deleted_child_resolves_its_attention() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Permission);
    assert!(!attention(&w).is_empty());
    w.store.close_now("child").unwrap();
    w.store.delete_session("child", None).unwrap();
    assert!(attention(&w).is_empty());
}

#[test]
fn closed_attention_migration_repairs_existing_closed_and_deleted_rows() {
    for deleted in [false, true] {
        let (w, _) = pair();
        w.store.close_now("child").unwrap();
        if deleted {
            w.store.delete_session("child", None).unwrap();
        }
        let conn = raw(&w);
        conn.execute_batch(include_str!("fixtures/undo_delegation_limits.sql"))
            .unwrap();
        conn.execute_batch("DROP TRIGGER attention_terminal_session; DROP TRIGGER attention_terminal_insert; DROP TRIGGER attention_terminal_update;
            PRAGMA user_version=24;").unwrap();
        conn.execute(
            "INSERT INTO delegation_attention(owner_id,attention_id,child_id,hat_id,reason,state,incident)
            VALUES (?1,'stale','child',?2,'endpoint_unavailable','open','stale')",
            params![w.store.owner_id(), w.hat],
        )
        .unwrap();
        drop(conn);
        let migrated = Store::open(&w.db).unwrap();
        assert!(migrated.operator_attention(None, "").unwrap().is_empty());
        assert_eq!(
            raw(&w)
                .query_row(
                    "SELECT state FROM delegation_attention WHERE attention_id='stale'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "resolved"
        );
    }
}

#[test]
fn closed_attention_first_summary_read_repairs_stale_closed_parent_reports() {
    let (w, _) = pair();
    w.store.close_now("parent").unwrap();
    raw(&w)
        .execute(
            "INSERT INTO delegation_attention(owner_id,attention_id,child_id,hat_id,reason,state,incident)
        VALUES (?1,'stale','child',?2,'endpoint_unavailable','open','stale')",
            params![w.store.owner_id(), w.hat],
        )
        .unwrap();
    assert_eq!(w.store.family_summary("child").unwrap().unwrap().operator_attention, 0);
    assert!(attention(&w).is_empty());
}

#[test]
fn attention_push_keeps_provisional_prompt_titles_private() {
    let (w, _) = pair();
    start_child_report(&w);
    pending(&w, "child", 3, "permission", PendingKind::Permission);
    let (push, mut notices) = hennery_kernel::push::Push::new();
    w.store.enqueue_delegation_attention(&push, "prompt", "", 1000).unwrap();
    assert_eq!(notices.try_recv().unwrap().title, "project");
    w.store
        .rename_session("child", Some("Operator title"))
        .unwrap()
        .unwrap();
    w.store
        .enqueue_delegation_attention(&push, "operator", "", 1000)
        .unwrap();
    assert_eq!(notices.try_recv().unwrap().title, "Operator title");
}

fn idle_connected(w: &World) -> (Arc<Hub>, tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>) {
    let hub = Arc::new(Hub::new());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let registration = hub
        .register(
            "host",
            tx,
            Capabilities(vec![
                Capability::IdleHold,
                Capability::DelegationConfig,
                Capability::McpServers,
            ]),
            AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect()),
        )
        .unwrap();
    hub.mark_ready("host", registration.conn_id);
    w.store.set_idle_hub(&hub);
    w.store.resend_idle_holds("host", &hub).unwrap();
    // Clear dirtiness inherited from fixture creation before the Hub existed,
    // so each later transition must provide its own invalidation.
    w.store.flush_idle_holds(&hub).unwrap();
    (hub, rx)
}

fn received_hold(rx: &mut tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>, id: &str) -> Option<bool> {
    let mut held = None;
    while let Ok(frame) = rx.try_recv() {
        if let CollectorFrame::SetIdleHold {
            session_id,
            held: value,
        } = frame
            && session_id == id
        {
            held = Some(value);
        }
    }
    held
}

#[test]
fn park_hold_counts_idle_attached_children_and_detached_work_until_terminal() {
    for terminal in ["parked", "closed", "failed", "deleted"] {
        let (w, _) = pair();
        let (hub, mut rx) = idle_connected(&w);
        assert_eq!(received_hold(&mut rx, "parent"), Some(true));
        assert!(w.store.idle_hold("parent").unwrap().unwrap().family_active);
        w.store.take_over_child("child", &hub).unwrap();
        assert!(
            w.store.idle_hold("parent").unwrap().unwrap().family_active,
            "takeover does not stop its adapter"
        );
        match terminal {
            "parked" => {
                w.store
                    .ingest(
                        "child",
                        2,
                        &SessionBody::SessionParked {
                            reason: ParkReason::Idle,
                        },
                    )
                    .unwrap();
            }
            "closed" => {
                w.store.ingest("child", 2, &SessionBody::SessionClosed).unwrap();
            }
            "failed" => {
                w.store.mark_failed("child", "fixture").unwrap();
            }
            "deleted" => {
                assert!(matches!(
                    w.store
                        .delete_session(
                            "child",
                            Some(&hennery_sessions::store::Unattached {
                                lifecycle: "active".into(),
                                presumed_parked: false
                            })
                        )
                        .unwrap(),
                    hennery_sessions::store::Deletion::Done { .. }
                ));
            }
            _ => unreachable!(),
        }
        let state = w.store.idle_hold("parent").unwrap().unwrap();
        assert!(!state.family_active, "{terminal}");
        assert!(!state.held, "detached notices cannot retain a hold: {terminal}");
        assert_eq!(received_hold(&mut rx, "parent"), Some(false), "{terminal}");
    }
}

#[test]
fn park_hold_pin_is_persisted_and_independent_of_family_release() {
    let (w, _) = pair();
    let (_hub, mut rx) = idle_connected(&w);
    received_hold(&mut rx, "parent");
    assert!(w.store.set_idle_pin("parent", true).unwrap().unwrap().pinned);
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    let state = w.store.idle_hold("parent").unwrap().unwrap();
    assert!(state.pinned && state.held && !state.family_active && !state.notices_pending);
    assert_eq!(received_hold(&mut rx, "parent"), Some(true));
    let reopened = Store::open(&w.db).unwrap();
    assert!(reopened.idle_hold("parent").unwrap().unwrap().pinned);
    assert!(!w.store.set_idle_pin("parent", false).unwrap().unwrap().held);
    assert_eq!(received_hold(&mut rx, "parent"), Some(false));
    assert!(w.store.set_idle_pin("missing", true).unwrap().is_none());
}

#[test]
fn park_hold_pending_report_outlives_child_park_until_factual_delivery() {
    let (w, _) = pair();
    ready_parent(&w);
    let (hub, mut rx) = idle_connected(&w);
    received_hold(&mut rx, "parent");
    w.store
        .open_turn("child", "work", &[json!({"type":"text","text":"work"})])
        .unwrap();
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::TurnStarted {
                request_id: "work".into(),
                turn_id: "work".into(),
            },
        )
        .unwrap();
    ended(&w, "child", 3, "work");
    w.store
        .ingest(
            "child",
            4,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    let state = w.store.idle_hold("parent").unwrap().unwrap();
    assert!(state.held && state.notices_pending && !state.family_active);
    received_hold(&mut rx, "parent");
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    let (request_id, turn_id) = loop {
        match rx.try_recv().unwrap() {
            CollectorFrame::DelegatedPrompt {
                request_id, turn_id, ..
            } => break (request_id, turn_id),
            CollectorFrame::SetIdleHold { held: true, .. } => {}
            other => panic!("unexpected report handoff frame: {other:?}"),
        }
    };
    assert!(
        w.store.idle_hold("parent").unwrap().unwrap().notices_pending,
        "socket handoff is not delivery"
    );
    w.store
        .ingest("parent", 20, &SessionBody::TurnStarted { request_id, turn_id })
        .unwrap();
    assert!(!w.store.idle_hold("parent").unwrap().unwrap().held);
    assert_eq!(received_hold(&mut rx, "parent"), Some(false));
}

#[test]
fn park_hold_reconnect_sends_current_true_and_false_and_gates_old_hosts() {
    let (w, _) = pair();
    let (hub, mut rx) = idle_connected(&w);
    assert_eq!(received_hold(&mut rx, "parent"), Some(true));
    w.store.resend_idle_holds("host", &hub).unwrap();
    assert_eq!(received_hold(&mut rx, "parent"), Some(true));
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert_eq!(received_hold(&mut rx, "parent"), Some(false));
    w.store.resend_idle_holds("host", &hub).unwrap();
    assert_eq!(received_hold(&mut rx, "parent"), Some(false));
    hub.unregister("host", hub.routable_conn("host").unwrap());
    let (tx, mut old) = tokio::sync::mpsc::unbounded_channel();
    let registration = hub
        .register("host", tx, Capabilities::default(), AgentIsolation::default())
        .unwrap();
    hub.mark_ready("host", registration.conn_id);
    w.store.set_idle_pin("parent", true).unwrap();
    w.store.resend_idle_holds("host", &hub).unwrap();
    assert!(old.try_recv().is_err(), "old host must not receive the unknown frame");
}

#[test]
fn park_hold_allocation_publishes_before_receipt_and_resume_before_attach() {
    let (w, p) = pair();
    let (hub, mut rx) = idle_connected(&w);
    received_hold(&mut rx, "parent");
    ready(&w);
    w.store
        .ingest(
            "child",
            10,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert_eq!(received_hold(&mut rx, "parent"), Some(false));
    queue(&w, &p, "resume-work");
    assert!(
        w.store
            .attach_delegated("child", McpContext::NONE, &catalog(), &hub)
            .unwrap()
    );
    let mut parent_held = false;
    loop {
        match rx.try_recv().unwrap() {
            CollectorFrame::SetIdleHold { session_id, held } if session_id == "parent" => parent_held = held,
            CollectorFrame::DelegatedAttach { .. } => {
                assert!(parent_held, "parent hold must be handed off before child attach");
                break;
            }
            _ => {}
        }
    }
    w.store.mark_failed("child", "fixture").unwrap();
    // Detach/fallback the failed child's report so a fresh spawn is the only hold.
    w.store.take_over_child("child", &hub).unwrap();
    received_hold(&mut rx, "parent");
    let receipt = spawn(&w, &p, "new-work").unwrap();
    assert!(receipt.child_id.is_some());
    assert_eq!(
        received_hold(&mut rx, "parent"),
        Some(true),
        "allocation sends the hold before returning its receipt"
    );
}

#[test]
fn park_hold_notice_insert_update_delete_and_family_delete_invalidate_the_parent() {
    let (w, _) = pair();
    let (hub, mut rx) = idle_connected(&w);
    w.store
        .open_turn("child", "work", &[json!({"type":"text","text":"work"})])
        .unwrap();
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::TurnStarted {
                request_id: "work".into(),
                turn_id: "work".into(),
            },
        )
        .unwrap();
    ended(&w, "child", 3, "work");
    w.store
        .ingest(
            "child",
            4,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    received_hold(&mut rx, "parent");
    let conn = raw(&w);
    // Preserve the real producer's complete notice, including its provenance,
    // to exercise each SQL invalidation independently of lifecycle triggers.
    conn.execute_batch("CREATE TEMP TABLE original_notice AS SELECT * FROM delegation_notices")
        .unwrap();
    for state in ["waiting_config", "awaiting_reconciliation", "parent_queued"] {
        conn.execute(
            "UPDATE delegation_notices SET state=?2 WHERE owner_id=?1",
            params![w.store.owner_id(), state],
        )
        .unwrap();
        w.store.flush_idle_holds(&hub).unwrap();
        assert_eq!(received_hold(&mut rx, "parent"), Some(true), "queued state {state}");
    }
    conn.execute(
        "UPDATE delegation_notices SET state='parent_delivered' WHERE owner_id=?1",
        [w.store.owner_id()],
    )
    .unwrap();
    w.store.flush_idle_holds(&hub).unwrap();
    assert_eq!(received_hold(&mut rx, "parent"), Some(false));
    conn.execute("DELETE FROM delegation_notices WHERE owner_id=?1", [w.store.owner_id()])
        .unwrap();
    w.store.flush_idle_holds(&hub).unwrap();
    received_hold(&mut rx, "parent");
    conn.execute(
        "INSERT INTO delegation_notices SELECT * FROM original_notice WHERE owner_id=?1",
        [w.store.owner_id()],
    )
    .unwrap();
    w.store.flush_idle_holds(&hub).unwrap();
    assert_eq!(received_hold(&mut rx, "parent"), Some(true));
    conn.execute("DELETE FROM delegation_notices WHERE owner_id=?1", [w.store.owner_id()])
        .unwrap();
    w.store.flush_idle_holds(&hub).unwrap();
    assert_eq!(received_hold(&mut rx, "parent"), Some(false));
    drop(conn);
    // A separate live family, so removing its edge is the only invalidation.
    let (w, _) = pair();
    let (hub, mut rx) = idle_connected(&w);
    received_hold(&mut rx, "parent");
    raw(&w)
        .execute(
            "DELETE FROM session_families WHERE owner_id=?1 AND child_id='child'",
            [w.store.owner_id()],
        )
        .unwrap();
    w.store.flush_idle_holds(&hub).unwrap();
    assert_eq!(received_hold(&mut rx, "parent"), Some(false));
}

#[test]
fn park_hold_deleted_and_frozen_sessions_are_not_pin_targets() {
    let (w, _) = pair();
    w.store.ingest("child", 2, &SessionBody::SessionClosed).unwrap();
    w.store.delete_session("child", None).unwrap();
    assert!(w.store.idle_hold("child").unwrap().is_none());
    assert!(w.store.set_idle_pin("child", true).unwrap().is_none());
    raw(&w)
        .execute(
            "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES (?1,?2,0)",
            params![w.store.owner_id(), w.hat],
        )
        .unwrap();
    assert!(w.store.idle_hold("parent").unwrap().is_none());
    assert!(w.store.set_idle_pin("parent", true).unwrap().is_none());
}

#[test]
fn park_hold_failed_flush_compensates_child_resume_before_any_handoff() {
    let (w, p) = pair();
    let (hub, mut rx) = idle_connected(&w);
    ready(&w);
    w.store
        .ingest(
            "child",
            10,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    queue(&w, &p, "retry-resume");
    received_hold(&mut rx, "parent");
    raw(&w).execute_batch("CREATE TRIGGER fail_idle_clear BEFORE UPDATE OF idle_hold_dirty ON sessions WHEN NEW.idle_hold_dirty=0 BEGIN SELECT RAISE(ABORT,'fixture hold clear failed'); END;").unwrap();
    assert!(
        !w.store
            .attach_delegated("child", McpContext::NONE, &catalog(), &hub)
            .unwrap()
    );
    assert_eq!(w.store.find_session("child").unwrap().unwrap().lifecycle, "parked");
    while let Ok(frame) = rx.try_recv() {
        assert!(
            !matches!(frame, CollectorFrame::DelegatedAttach { .. }),
            "no child handoff after a failed parent-hold flush"
        );
    }
    raw(&w).execute_batch("DROP TRIGGER fail_idle_clear").unwrap();
    w.store.flush_idle_holds(&hub).unwrap();
    received_hold(&mut rx, "parent");
    assert!(
        w.store
            .attach_delegated("child", McpContext::NONE, &catalog(), &hub)
            .unwrap(),
        "compensated launch remains retryable"
    );
    let mut held = false;
    loop {
        match rx.try_recv().unwrap() {
            CollectorFrame::SetIdleHold {
                session_id,
                held: value,
            } if session_id == "parent" => held = value,
            CollectorFrame::DelegatedAttach { .. } => {
                assert!(held);
                break;
            }
            _ => {}
        }
    }
}

#[test]
fn d4_counts_parent_handled_questions_and_takeover_are_factual_and_atomic() {
    let (w, p) = pair();
    pending(&w, "child", 2, "ordinary", PendingKind::Elicitation);
    let s = w.store.family_summary("child").unwrap().unwrap();
    assert!(s.question_waits);
    assert_eq!((s.operator_attention, s.parent_handled_questions), (0, 1));
    assert_eq!(w.store.operator_waiting_count(None).unwrap(), 0);
    pending(&w, "child", 3, "permission", PendingKind::Permission);
    let s = w.store.family_summary("child").unwrap().unwrap();
    assert_eq!((s.operator_attention, s.parent_handled_questions), (1, 1));
    assert_eq!(w.store.operator_waiting_count(Some(&w.hat)).unwrap(), 1);
    assert_eq!(w.store.operator_waiting_count(Some(&w.other)).unwrap(), 0);
    let hub = Hub::new();
    assert!(w.store.take_over_child("child", &hub).unwrap());
    assert!(w.store.take_over_child("child", &hub).unwrap());
    assert!(w.store.direct_child(&p, 1, "child").unwrap().is_none());
    let s = w.store.family_summary("child").unwrap().unwrap();
    assert_eq!((s.operator_attention, s.parent_handled_questions), (2, 0));
    assert_eq!(
        w.store.family_page("parent", "", 10).unwrap().unwrap().children[0].routing,
        "detached"
    );
    assert!(!w.store.take_over_child("parent", &hub).unwrap());
    assert!(!w.store.take_over_child("unknown", &hub).unwrap());
}

#[test]
fn review_takeover_reopening_question_advances_its_stream_version() {
    let (w, p) = pair();
    let hub = Hub::new();
    let mut heard = hub.subscribe();
    pending(&w, "child", 2, "ordinary", PendingKind::Elicitation);
    w.store
        .queue_ordinary_answer(
            &p,
            1,
            "digest",
            &hennery_sessions::delegation_control::DelegatedAnswer {
                key: "review",
                child: "child",
                pending: "ordinary",
                answer: &answer(),
            },
            &hub,
        )
        .unwrap();
    let before = w.store.question_snapshot("child", "ordinary").unwrap().unwrap();
    let hennery_view::Body::Question(q) = &before.body else {
        panic!()
    };
    assert!(q.answered && !q.answerable);
    let cursor = w.store.max_event_id().unwrap();
    assert!(w.store.take_over_child("child", &hub).unwrap());
    let after = w.store.question_snapshot("child", "ordinary").unwrap().unwrap();
    let hennery_view::Body::Question(q) = &after.body else {
        panic!()
    };
    assert!(!q.answered && q.answerable, "takeover reopens canonical answerability");
    assert!(
        after.version > before.version,
        "a reopened question must have a newer stream version: before={}, after={}",
        before.version,
        after.version
    );
    assert_eq!(
        w.store.pending_touched_after("child", cursor).unwrap(),
        vec!["ordinary".to_string()]
    );
    let mut published = Vec::new();
    while let Ok(event) = heard.try_recv() {
        published.push(event);
    }
    assert!(
        published
            .iter()
            .any(|event| event.kind == "answer_cancelled" && event.body["pending_id"] == "ordinary")
    );
    let cursor = w.store.max_event_id().unwrap();
    assert!(w.store.take_over_child("child", &hub).unwrap());
    assert_eq!(
        w.store.max_event_id().unwrap(),
        cursor,
        "repeated takeover creates no second reopening"
    );
}

#[test]
fn review_notice_routing_state_reason_and_grant_changes_invalidate_both_endpoints() {
    let (w, _) = pair();
    pending(&w, "child", 2, "ordinary", PendingKind::Elicitation);
    let hub = Hub::new();
    w.store.flush_family_changes(&hub).unwrap();
    for (state, reason) in [
        ("waiting_config", Some("config_wait")),
        ("awaiting_reconciliation", None),
        ("parent_delivered", None),
        ("parent_delivered", Some("readback_changed")),
        ("parent_delivered", None),
    ] {
        let cursor = w.store.max_event_id().unwrap();
        raw(&w).execute("UPDATE delegation_notices SET state=?2,reason=?3 WHERE owner_id=?1 AND child_id='child' AND pending_id='ordinary'",params![w.store.owner_id(),state,reason]).unwrap();
        w.store.flush_family_changes(&hub).unwrap();
        let page = w.store.family_page("child", "", 20).unwrap().unwrap();
        assert_eq!(page.questions[0].handling, state);
        assert!(
            page.summary.family.unwrap().revision > cursor,
            "state={state} reason={reason:?}"
        );
        let changed = w.store.family_changed_after(cursor).unwrap();
        assert!(changed.iter().any(|id| id == "parent") && changed.iter().any(|id| id == "child"));
    }
    let cursor = w.store.max_event_id().unwrap();
    raw(&w).execute("UPDATE delegation_notices SET accepted_revision=accepted_revision+1 WHERE owner_id=?1 AND child_id='child' AND pending_id='ordinary'",[w.store.owner_id()]).unwrap();
    w.store.flush_family_changes(&hub).unwrap();
    let page = w.store.family_page("child", "", 20).unwrap().unwrap();
    assert_eq!(page.questions[0].handling, "operator_required");
    assert!(page.summary.family.unwrap().revision > cursor);
}

#[test]
fn review_scope_invalidations_are_owner_bound_and_survive_reopen() {
    let (w, _) = pair();
    let before = w.store.view_scope_revision().unwrap();
    raw(&w)
        .execute(
            "INSERT INTO view_scope_invalidations(owner_id,revision) VALUES ('other-owner',99)",
            [],
        )
        .unwrap();
    assert_eq!(w.store.view_scope_revision().unwrap(), before);
    raw(&w)
        .execute(
            "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES (?1,?2,1)",
            params![w.store.owner_id(), w.hat],
        )
        .unwrap();
    assert!(w.store.view_scope_revision().unwrap() > before);
    assert!(w.store.session_scope_frozen("child").unwrap());
    assert!(!w.store.session_scope_frozen("foreign").unwrap());
    let reopened = Store::open(&w.db).unwrap();
    assert_eq!(
        reopened.view_scope_revision().unwrap(),
        w.store.view_scope_revision().unwrap()
    );
    let changed = reopened.family_changed_after(i64::MAX).unwrap();
    assert!(
        changed.iter().any(|id| id == "child"),
        "a resumed event cursor cannot skip durable scope removal"
    );
}

#[test]
fn review_notice_dispatch_invalidates_family_routing() {
    let (w, _) = pair();
    ready_parent(&w);
    pending(&w, "child", 2, "ordinary", PendingKind::Elicitation);
    let (hub, _rx) = connected();
    w.store.flush_family_changes(&hub).unwrap();
    let before = w.store.family_page("child", "", 20).unwrap().unwrap();
    assert_eq!(before.questions[0].handling, "parent_queued");
    let cursor = w.store.max_event_id().unwrap();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    w.store.flush_family_changes(&hub).unwrap();
    let after = w.store.family_page("child", "", 20).unwrap().unwrap();
    assert_eq!(after.questions[0].handling, "awaiting_reconciliation");
    assert!(
        after.summary.family.unwrap().revision > before.summary.family.unwrap().revision,
        "routing changed without a family revision"
    );
    assert!(
        w.store
            .family_changed_after(cursor)
            .unwrap()
            .contains(&"child".to_string())
    );
}

#[test]
fn review_freeze_invalidates_existing_family_and_attention_subscribers() {
    let w = World::new();
    let p = w.start("parent", &w.other, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    child(&w, &p, "child", &w.other);
    for id in ["parent", "child"] {
        w.store
            .ingest(
                id,
                2,
                &SessionBody::SessionParked {
                    reason: ParkReason::Idle,
                },
            )
            .unwrap();
    }
    raw(&w)
        .execute(
            "INSERT INTO delegation_attention(owner_id,attention_id,child_id,hat_id,reason,state,incident)
        VALUES (?1,'review-status','child',?2,'child_failed','open','review-status')",
            params![w.store.owner_id(), w.other],
        )
        .unwrap();
    let hub = Hub::new();
    w.store.flush_family_changes(&hub).unwrap();
    assert_eq!(w.store.operator_waiting_count(Some(&w.other)).unwrap(), 1);
    let cursor = w.store.max_event_id().unwrap();
    assert!(matches!(
        Hosts::open(&w.db).unwrap().begin_purge(&w.other, 1).unwrap(),
        hennery_kernel::hats::PurgeStart::Frozen { .. }
    ));
    w.store.refresh_delegation_attention(&hub, i64::MAX / 2).unwrap();
    assert!(w.store.family_page("parent", "", 20).unwrap().is_none());
    assert_eq!(w.store.operator_waiting_count(Some(&w.other)).unwrap(), 0);
    assert!(
        w.store
            .family_changed_after(cursor)
            .unwrap()
            .contains(&"child".to_string()),
        "family disappeared and badge changed, but subscribed clients have no invalidation"
    );
}

#[test]
fn d4_parent_child_counts_include_parent_handled_waiting_and_status_attention() {
    let (w, _) = pair();
    pending(&w, "child", 2, "ordinary", PendingKind::Elicitation);
    let family = w.store.family_summary("parent").unwrap().unwrap().family.unwrap();
    assert_eq!(family.child_counts.waiting, 1);
    assert_eq!(family.attention, 0);
    // A status incident without a question still contributes operator work.
    raw(&w)
        .execute(
            "INSERT INTO delegation_attention(owner_id,attention_id,child_id,hat_id,reason,state,incident)
        VALUES (?1,'status','child',?2,'start_failed','open','status')",
            params![w.store.owner_id(), w.hat],
        )
        .unwrap();
    assert_eq!(
        w.store
            .family_summary("parent")
            .unwrap()
            .unwrap()
            .family
            .unwrap()
            .attention,
        1
    );
}

#[test]
fn d4_dirty_projection_cannot_write_into_a_frozen_hat() {
    let (w, _) = pair();
    pending(&w, "child", 2, "ordinary", PendingKind::Elicitation);
    raw(&w)
        .execute(
            "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES (?1,?2,1)",
            params![w.store.owner_id(), w.hat],
        )
        .unwrap();
    let before = w.store.max_event_id().unwrap();
    w.store.flush_family_changes(&Hub::new()).unwrap();
    assert_eq!(
        w.store.max_event_id().unwrap(),
        before,
        "purge freezes even projection writes"
    );
    assert!(w.store.family_page("parent", "", 20).unwrap().is_none());
    assert_eq!(w.store.operator_waiting_count(None).unwrap(), 0);
}

#[test]
fn d4_deadlines_and_receipts_invalidate_the_child_and_parent_summary() {
    let (w, p) = pair();
    w.store.flush_family_changes(&Hub::new()).unwrap();
    let revision = w
        .store
        .family_summary("parent")
        .unwrap()
        .unwrap()
        .family
        .unwrap()
        .revision;
    pending(&w, "child", 2, "ordinary", PendingKind::Elicitation);
    let before = w.store.max_event_id().unwrap();
    let hub = Hub::new();
    let mut rings = hub.subscribe();
    w.store.refresh_delegation_attention(&hub, i64::MAX / 2).unwrap();
    assert!(w.store.max_event_id().unwrap() > before);
    let mut published = Vec::new();
    while let Ok(event) = rings.try_recv() {
        published.push(event);
    }
    assert!(
        published
            .iter()
            .any(|event| event.session_id == "child" && event.kind == "family_changed" && event.event_id > before),
        "committed invalidations wake existing subscribers"
    );
    let flushed = w.store.max_event_id().unwrap();
    w.store.flush_family_changes(&Hub::new()).unwrap();
    assert_eq!(
        w.store.max_event_id().unwrap(),
        flushed,
        "no duplicate invalidation without a new fact"
    );
    let changed = w.store.family_changed_after(before).unwrap();
    assert!(changed.contains(&"child".to_string()) && changed.contains(&"parent".to_string()));
    assert!(
        w.store
            .family_summary("parent")
            .unwrap()
            .unwrap()
            .family
            .unwrap()
            .revision
            > revision
    );
    let s = w.store.family_summary("child").unwrap().unwrap();
    assert_eq!((s.operator_attention, s.parent_handled_questions), (1, 0));
    let hub = Hub::new();
    w.store
        .queue_ordinary_answer(
            &p,
            1,
            "digest",
            &hennery_sessions::delegation_control::DelegatedAnswer {
                key: "answer",
                child: "child",
                pending: "ordinary",
                answer: &answer(),
            },
            &hub,
        )
        .unwrap();
    let s = w.store.family_summary("child").unwrap().unwrap();
    assert!(s.question_waits);
    assert_eq!((s.operator_attention, s.parent_handled_questions), (0, 0));
    assert_eq!(w.store.operator_waiting_count(None).unwrap(), 0);
    assert!(w.store.take_over_child("child", &hub).unwrap());
    assert_eq!(w.store.family_summary("child").unwrap().unwrap().operator_attention, 1);
    assert!(matches!(
        w.store.submit_answer("child", "ordinary", &answer()).unwrap(),
        AnswerSubmission::Queued(_)
    ));
}

#[test]
fn d4_each_parent_routing_state_stays_distinct_until_operator_takeover() {
    for state in [
        "parent_queued",
        "parent_delivered",
        "waiting_config",
        "awaiting_reconciliation",
    ] {
        let (w, _) = pair();
        pending(&w, "child", 2, "ordinary", PendingKind::Elicitation);
        raw(&w)
            .execute("UPDATE delegation_notices SET state=?1 WHERE child_id='child'", [state])
            .unwrap();
        let page = w.store.family_page("child", "", 20).unwrap().unwrap();
        assert_eq!(page.questions[0].handling, state);
        assert_eq!(
            (page.summary.operator_attention, page.summary.parent_handled_questions),
            (0, 1)
        );
        assert!(w.store.take_over_child("child", &Hub::new()).unwrap());
        let page = w.store.family_page("child", "", 20).unwrap().unwrap();
        assert_eq!(page.questions[0].handling, "operator_required");
        assert_eq!(page.questions[0].reason.as_deref(), Some("operator_takeover"));
    }
}

#[test]
fn d4_takeover_races_parent_answer_without_losing_operator_control() {
    let (w, p) = pair();
    pending(&w, "child", 2, "ordinary", PendingKind::Elicitation);
    let second = Store::open(&w.db).unwrap();
    let barrier = Barrier::new(2);
    let hub = Hub::new();
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            w.store.take_over_child("child", &hub).unwrap()
        });
        let b = scope.spawn(|| {
            barrier.wait();
            second.queue_ordinary_answer(
                &p,
                1,
                "digest",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "race",
                    child: "child",
                    pending: "ordinary",
                    answer: &answer(),
                },
                &hub,
            )
        });
        assert!(a.join().unwrap());
        let _ = b.join().unwrap();
    });
    assert!(w.store.direct_child(&p, 1, "child").unwrap().is_none());
    assert_eq!(w.store.family_summary("child").unwrap().unwrap().operator_attention, 1);
    assert!(matches!(
        w.store.submit_answer("child", "ordinary", &answer()).unwrap(),
        AnswerSubmission::Queued(_)
    ));
}
fn connected() -> (Arc<Hub>, tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>) {
    let h = Arc::new(Hub::new());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let reg = h
        .register(
            "host",
            tx,
            Capabilities(vec![
                Capability::McpServers,
                Capability::DelegationConfig,
                Capability::CatalogProbe,
            ]),
            AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect()),
        )
        .unwrap();
    h.mark_ready("host", reg.conn_id);
    (h, rx)
}
fn catalog() -> HostCatalog {
    HostCatalog {
        saved_at: None,
        agent: "claude".into(),
        adapter_revision: "pin".into(),
        config_options: vec![
            json!({"id":"model","options":[{"value":"wanted"},{"value":"wrong"},{"value":"private-model"}]}),
            json!({"id":"mode","options":[{"value":"default"},{"value":"plan"},{"value":"agent-full-access"}]}),
            json!({"id":"effort","options":[{"value":"high"},{"value":"low"}]}),
        ],
        defaults: SessionConfig::default(),
        security: SecurityClassification {
            known: true,
            axes: Default::default(),
        },
        freshness: CatalogFreshness::Fresh,
    }
}

#[test]
fn changed_model_does_not_inherit_another_models_nonsecurity_axes() {
    use hennery_sessions::delegation_runtime::launch_configuration;
    let mut c = catalog();
    c.defaults = SessionConfig {
        model: Some("opus".into()),
        mode: Some("default".into()),
        axes: [
            ("fast".into(), ConfigValue::Bool(false)),
            ("effort".into(), ConfigValue::Id("medium".into())),
            ("sandbox".into(), ConfigValue::Id("safe".into())),
            ("custom_security".into(), ConfigValue::Bool(false)),
        ]
        .into(),
    };
    c.security
        .axes
        .insert("sandbox".into(), vec![ConfigValue::Id("safe".into())]);
    c.security
        .axes
        .insert("custom_security".into(), vec![ConfigValue::Bool(false)]);
    let wanted = SessionConfig {
        model: Some("sonnet".into()),
        axes: [("effort".into(), ConfigValue::Id("low".into()))].into(),
        ..Default::default()
    };
    let actual = launch_configuration(&c, &wanted);
    assert!(
        !actual.axes.contains_key("fast"),
        "Opus fast-mode default must not become a Sonnet requirement"
    );
    assert_eq!(actual.axes["effort"], ConfigValue::Id("low".into()));
    assert_eq!(actual.axes["sandbox"], ConfigValue::Id("safe".into()));
    assert_eq!(actual.axes["custom_security"], ConfigValue::Bool(false));
    let mut explicit = wanted.clone();
    explicit.axes.insert("fast".into(), ConfigValue::Bool(false));
    assert_eq!(
        launch_configuration(&c, &explicit).axes["fast"],
        ConfigValue::Bool(false),
        "never silently discard an explicit request"
    );
    assert_eq!(
        launch_configuration(&c, &SessionConfig::default()),
        c.defaults,
        "unchanged model retains catalog defaults"
    );
}
fn config(model: &str) -> SessionConfig {
    SessionConfig {
        model: Some(model.into()),
        ..Default::default()
    }
}
fn seed(w: &World, id: &str, required: &SessionConfig, generation: u64) {
    raw(w).execute("INSERT INTO session_attach_evidence(owner_id,session_id,attach_generation,requested_config) VALUES (?1,?2,?3,?4)",params![w.store.owner_id(),id,i64::try_from(generation).unwrap(),serde_json::to_string(required).unwrap()]).unwrap();
}
fn proof(
    w: &World,
    id: &str,
    seq: u64,
    generation: u64,
    revision: u64,
    completeness: ConfigCompleteness,
    actual: SessionConfig,
) {
    w.store
        .ingest(
            id,
            seq,
            &SessionBody::AcpUpdate {
                indexed: Indexed {
                    config_evidence: Some(Box::new(ConfigEvidence {
                        attach_generation: generation,
                        config_revision: revision,
                        completeness,
                        actual,
                    })),
                    ..Default::default()
                },
                payload: json!({}),
            },
        )
        .unwrap();
}
fn ready(w: &World) {
    seed(w, "child", &config("wanted"), 5);
    proof(w, "child", 2, 5, 10, ConfigCompleteness::Complete, config("wanted"));
}
fn queue(w: &World, p: &SessionIdentity, key: &str) -> OperationReceipt {
    w.store.queue_followup(p, 1, key, key, "child", key).unwrap()
}

#[test]
fn explicit_operator_park_holds_delegated_followups_in_the_shared_fifo() {
    for reason in [ParkReason::Operator, ParkReason::Idle] {
        let (w, parent) = pair();
        ready(&w);
        queue(&w, &parent, "held-followup");
        if reason == ParkReason::Operator {
            w.store.record_park_request("child").unwrap();
        }
        w.store
            .ingest("child", 10, &SessionBody::SessionParked { reason })
            .unwrap();
        let (hub, mut frames) = connected();
        let attached = w
            .store
            .attach_delegated("child", McpContext::NONE, &catalog(), &hub)
            .unwrap();
        assert_eq!(attached, reason == ParkReason::Idle);
        if reason == ParkReason::Operator {
            assert!(frames.try_recv().is_err());
            assert_eq!(receipt(&w, &parent, "held-followup").state, OperationState::Queued);
            assert_eq!(w.store.find_session("child").unwrap().unwrap().lifecycle, "parked");
        } else {
            assert!(matches!(
                frames.try_recv().unwrap(),
                CollectorFrame::DelegatedAttach { .. }
            ));
        }
    }
}

#[test]
fn retained_operator_failures_do_not_consume_the_shared_delegated_capacity() {
    let (w, parent) = pair();
    ready(&w);
    let prompt =
        hennery_sessions::content::check(vec![json!({"type":"text","text":"retained operator input"})]).unwrap();
    assert!(w.store.open_prompt("child", "failed-work", &prompt).unwrap());
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::TurnStarted {
                request_id: "initial".into(),
                turn_id: "failed-work".into(),
            },
        )
        .unwrap();
    for _ in 0..32 {
        w.store.enqueue_operator_prompt("child", &prompt).unwrap();
    }
    w.store
        .ingest(
            "child",
            4,
            &SessionBody::TurnEnded {
                turn_id: "failed-work".into(),
                outcome: TurnOutcome::Failed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap();
    assert_eq!(w.store.operator_prompt_queue("child").unwrap().len(), 32);
    assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (0, 0));
    for index in 0..32 {
        queue(&w, &parent, &format!("new-parent-{index}"));
    }
    assert_eq!(w.store.delegation_queue_depth("child").unwrap().0, 32);
    assert_eq!(
        w.store
            .queue_followup(&parent, 1, "overflow", "overflow", "child", "overflow")
            .unwrap_err()
            .to_string(),
        "queue_full"
    );
}
fn receipt(w: &World, p: &SessionIdentity, key: &str) -> OperationReceipt {
    w.store.delegation_receipt(p, 1, key).unwrap().unwrap()
}
fn take_prompt(rx: &mut tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>) -> (String, String) {
    match rx.try_recv().unwrap() {
        CollectorFrame::DelegatedPrompt {
            request_id, turn_id, ..
        } => (request_id, turn_id),
        other => panic!("{other:?}"),
    }
}
fn ended(w: &World, id: &str, seq: u64, turn: &str) {
    w.store
        .ingest(
            id,
            seq,
            &SessionBody::TurnEnded {
                turn_id: turn.into(),
                outcome: TurnOutcome::Completed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap();
}
fn pending(w: &World, id: &str, seq: u64, pending: &str, kind: PendingKind) -> hennery_sessions::store::Ingested {
    w.store.ingest_fact(id,seq,&SessionBody::PendingOpened{pending_id:pending.into(),indexed:Indexed{pending:Some(Box::new(PendingExtract{id:pending.into(),kind,option_ids:Some(vec!["allow".into()]),title:Some("question".into())})),..Default::default()},payload:json!({"message":"question","requestedSchema":{"type":"object","properties":{"ok":{"type":"boolean"}}}})}).unwrap()
}
fn answer() -> AnswerRequest {
    AnswerRequest::Elicitation {
        action: ElicitationAction::Accept,
        content: Some(json!({"ok":true})),
    }
}

fn resource_form(case: &str) -> serde_json::Value {
    match case {
        "expanded" | "legacy_expanded" => {
            let mut defs = serde_json::Map::new();
            defs.insert("s0".into(), json!({"type":"object"}));
            for i in 1..=40 {
                let previous = format!("#/$defs/s{}", i - 1);
                defs.insert(format!("s{i}"), json!({"allOf":[{"$ref":previous},{"$ref":previous}]}));
            }
            json!({"$defs":defs,"$ref":"#/$defs/s40"})
        }
        "deep" => {
            let mut schema = json!({});
            for _ in 0..64 {
                schema = json!({"properties":{"next":schema}});
            }
            schema
        }
        "regex" => json!({"type":"object","properties":{"ok":{"type":"string","pattern":"^(a+)+$"}}}),
        _ => {
            json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false})
        }
    }
}

#[test]
fn hostile_forms_have_a_bounded_subprocess() {
    if let Ok(case) = std::env::var("HENNERY_TEST_FORM_RESOURCE_CASE") {
        let (w, p) = pair();
        let schema = resource_form(&case);
        if case == "legacy_expanded" {
            pending(&w, "child", 2, "question", PendingKind::Elicitation);
            raw(&w)
                .execute(
                    "UPDATE pending SET payload=?1 WHERE pending_id='question'",
                    [json!({"requestedSchema":schema}).to_string()],
                )
                .unwrap();
            assert!(
                w.store
                    .queue_ordinary_answer(
                        &p,
                        1,
                        "digest",
                        &hennery_sessions::delegation_control::DelegatedAnswer {
                            key: "answer",
                            child: "child",
                            pending: "question",
                            answer: &answer()
                        },
                        &Hub::new()
                    )
                    .is_err()
            );
        } else {
            let ingested = w
                .store
                .ingest_fact(
                    "child",
                    2,
                    &SessionBody::PendingOpened {
                        pending_id: "question".into(),
                        indexed: Indexed {
                            pending: Some(Box::new(PendingExtract {
                                id: "question".into(),
                                kind: PendingKind::Elicitation,
                                option_ids: None,
                                title: Some("Resource question".into()),
                            })),
                            ..Default::default()
                        },
                        payload: json!({"mode":"form","message":"Resource question","requestedSchema":schema}),
                    },
                )
                .unwrap();
            if case == "normal" {
                w.store
                    .queue_ordinary_answer(
                        &p,
                        1,
                        "digest",
                        &hennery_sessions::delegation_control::DelegatedAnswer {
                            key: "answer",
                            child: "child",
                            pending: "question",
                            answer: &answer(),
                        },
                        &Hub::new(),
                    )
                    .unwrap();
            } else {
                assert!(
                    w.store.open_pending("child").unwrap().is_empty(),
                    "refused form must not remain answerable"
                );
                assert!(
                    ingested.events.iter().any(|e| e.kind == "host_note"
                        && e.body["note"] == "elicitation_schema_refused"
                        && e.body["text"]
                            .as_str()
                            .is_some_and(|text| text.contains("Resource question"))),
                    "operator must retain the refusal and question"
                );
                assert!(
                    !serde_json::to_string(&ingested.events)
                        .unwrap()
                        .contains("requestedSchema"),
                    "do not store the hostile schema"
                );
            }
        }
        return;
    }
    for case in ["legacy_expanded", "expanded", "deep", "regex", "normal"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "hostile_forms_have_a_bounded_subprocess", "--nocapture"])
            .env("HENNERY_TEST_FORM_RESOURCE_CASE", case)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                let output = child.wait_with_output().unwrap();
                assert!(
                    status.success(),
                    "case {case}: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                break;
            }
            if std::time::Instant::now() > until {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("case {case} exceeded the subprocess resource bound");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[test]
#[cfg(feature = "test-hooks")]
fn validation_releases_the_store_lock_and_rechecks_scope_state_and_schema() {
    if let Ok(case) = std::env::var("HENNERY_TEST_FORM_RECHECK_CASE") {
        let (w, p) = pair();
        pending(&w, "child", 2, "question", PendingKind::Elicitation);
        let result = w.store.queue_ordinary_answer_during_validation(
            &p,
            1,
            "digest",
            &hennery_sessions::delegation_control::DelegatedAnswer {
                key: "answer",
                child: "child",
                pending: "question",
                answer: &answer(),
            },
            &Hub::new(),
            || match case.as_str() {
                "lock" => {
                    assert!(w.store.find_session("parent").unwrap().is_some());
                }
                "scope" => {
                    w.store.record_close_request("parent").unwrap();
                }
                "state" => {
                    w.store.submit_answer("child", "question", &answer()).unwrap();
                }
                "schema" => {
                    raw(&w)
                        .execute(
                            "UPDATE pending SET payload=?1 WHERE pending_id='question'",
                            [
                                json!({"requestedSchema":{"type":"object","properties":{"ok":{"const":false}}}})
                                    .to_string(),
                            ],
                        )
                        .unwrap();
                }
                _ => panic!("unknown case"),
            },
        );
        match case.as_str() {
            "lock" => {
                result.unwrap();
            }
            "scope" => assert_eq!(result.unwrap_err().to_string(), "not_found"),
            "state" => assert_eq!(result.unwrap_err().to_string(), "already_answered"),
            "schema" => assert_eq!(result.unwrap_err().to_string(), "schema_changed"),
            _ => unreachable!(),
        }
        return;
    }
    for case in ["lock", "scope", "state", "schema"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "validation_releases_the_store_lock_and_rechecks_scope_state_and_schema",
                "--nocapture",
            ])
            .env("HENNERY_TEST_FORM_RECHECK_CASE", case)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                let output = child.wait_with_output().unwrap();
                assert!(
                    status.success(),
                    "case {case}: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                break;
            }
            if std::time::Instant::now() > until {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("case {case} exceeded subprocess bound: validation retained the shared store lock");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[cfg(feature = "test-hooks")]
async fn validation_worker_keeps_tokio_and_other_session_reads_responsive() {
    let (w, p) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let store = Arc::new(Store::open(&w.db).unwrap());
    let worker = store.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let task = tokio::spawn(async move {
        worker
            .queue_ordinary_answer_async_during_validation(
                &p,
                1,
                "digest",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "answer",
                    child: "child",
                    pending: "question",
                    answer: &answer(),
                },
                &Arc::new(Hub::new()),
                move || {
                    entered.send(()).unwrap();
                    held.recv_timeout(std::time::Duration::from_secs(2))
                        .expect("Tokio must run while validation is held");
                },
            )
            .await
    });
    ready.await.unwrap();
    assert!(
        store.find_session("parent").unwrap().is_some(),
        "another session read must not wait for evaluation"
    );
    release.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[test]
fn delegated_answers_validate_the_original_schema_without_consuming_a_failed_key() {
    for content in [json!({"ok":"yes"}), json!({}), json!({"ok":true,"extra":1})] {
        let (w, p) = pair();
        pending(&w, "child", 2, "question", PendingKind::Elicitation);
        raw(&w).execute("UPDATE pending SET payload=?1 WHERE pending_id='question'", [json!({"requestedSchema":{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}}).to_string()]).unwrap();
        let invalid = AnswerRequest::Elicitation {
            action: ElicitationAction::Accept,
            content: Some(content),
        };
        let error = w
            .store
            .queue_ordinary_answer(
                &p,
                1,
                "digest",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "answer",
                    child: "child",
                    pending: "question",
                    answer: &invalid,
                },
                &Hub::new(),
            )
            .unwrap_err();
        assert_eq!(hennery_sessions::session_tools::error_code(&error), "invalid_arguments");
        assert!(
            error.to_string().contains("content must be a JSON object matching:"),
            "{error}"
        );
        assert!(error.to_string().contains("additionalProperties"));
        assert!(w.store.answers_to_send("host").unwrap().is_empty());
        assert!(w.store.delegation_retry(&p, 1, "answer", "digest").unwrap().is_none());
        w.store
            .queue_ordinary_answer(
                &p,
                1,
                "digest",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "answer",
                    child: "child",
                    pending: "question",
                    answer: &answer(),
                },
                &Hub::new(),
            )
            .unwrap();
    }
}

#[test]
fn delegated_mcp_formats_and_enum_labels_admit_forms_and_validate_answers() {
    for (field, good, bad) in [
        (json!({"type":"string","format":"email"}), "person@example.org", "bad"),
        (json!({"type":"string","format":"uri"}), "urn:example:form", "relative"),
        (json!({"type":"string","format":"date"}), "2024-02-29", "2025-02-29"),
        (
            json!({"type":"string","format":"date-time"}),
            "2026-10-16T12:34:56Z",
            "2026-10-16T25:00:00Z",
        ),
        (
            json!({"type":"string","enum":["r","g"],"enumNames":["Red","Green"]}),
            "r",
            "Red",
        ),
    ] {
        let (w, p) = pair();
        let schema = json!({"type":"object","properties":{"value":field},"required":["value"]});
        w.store
            .ingest_fact(
                "child",
                2,
                &SessionBody::PendingOpened {
                    pending_id: "question".into(),
                    indexed: Indexed {
                        pending: Some(Box::new(PendingExtract {
                            id: "question".into(),
                            kind: PendingKind::Elicitation,
                            option_ids: None,
                            title: None,
                        })),
                        ..Default::default()
                    },
                    payload: json!({"mode":"form","requestedSchema":schema}),
                },
            )
            .unwrap();
        assert_eq!(w.store.open_pending("child").unwrap().len(), 1, "{field}");
        for (value, valid) in [(bad, false), (good, true)] {
            let answer = AnswerRequest::Elicitation {
                action: ElicitationAction::Accept,
                content: Some(json!({"value":value})),
            };
            let result = w.store.queue_ordinary_answer(
                &p,
                1,
                "digest",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "answer",
                    child: "child",
                    pending: "question",
                    answer: &answer,
                },
                &Hub::new(),
            );
            if valid {
                result.unwrap();
            } else {
                let error = result.unwrap_err();
                assert_eq!(hennery_sessions::session_tools::error_code(&error), "invalid_arguments");
                assert!(w.store.answers_to_send("host").unwrap().is_empty());
                assert!(w.store.delegation_retry(&p, 1, "answer", "digest").unwrap().is_none());
            }
        }
    }
}

#[test]
fn child_question_delivery_contains_the_exact_form_contract() {
    let (w, p) = pair();
    ready_parent(&w);
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let (hub, mut rx) = connected();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    let CollectorFrame::DelegatedPrompt { content, .. } = rx.try_recv().unwrap() else {
        panic!("prompt")
    };
    let text = serde_json::to_string(&content).unwrap();
    assert!(
        !text.contains("operator-only") && !text.contains("operator only"),
        "{text}"
    );
    assert!(text.contains("Permission requests raised by a child go to the operator; the parent cannot answer them."));
    assert!(text.contains("requestedSchema"), "{text}");
    assert!(text.contains("JSON object"), "{text}");
    assert!(text.contains("boolean"), "{text}");
    let schema = json!({"type":"object","properties":{"ok":{"type":"boolean"}}});
    let blocks = serde_json::to_value(content).unwrap();
    let metadata: serde_json::Value =
        serde_json::from_str(blocks[0]["text"].as_str().unwrap().split_once('\n').unwrap().1).unwrap();
    assert_eq!(metadata["requestedSchema"], schema);
    assert_eq!(
        w.store.child_status(&p, 1, "child", None).unwrap().questions[0]
            .question
            .as_ref()
            .unwrap()["requestedSchema"],
        schema
    );
}

fn notices(w: &World) -> Vec<(String, String, String)> {
    raw(w)
        .prepare("SELECT kind,state,source_events FROM delegation_notices WHERE owner_id = ?1 ORDER BY rowid")
        .unwrap()
        .query_map([w.store.owner_id()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn d3_question_is_transactional_and_replayed_or_unapplied_facts_add_nothing() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    assert_eq!(notices(&w).len(), 1);
    assert_eq!(notices(&w)[0].0, "question");
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    ended(&w, "child", 3, "unknown");
    assert_eq!(notices(&w).len(), 1);
    let conn = raw(&w);
    conn.execute_batch("CREATE TRIGGER fail_notice BEFORE INSERT ON delegation_notices BEGIN SELECT RAISE(ABORT,'notice failed'); END;").unwrap();
    assert!(
        w.store
            .ingest(
                "child",
                4,
                &SessionBody::PendingOpened {
                    pending_id: "rollback".into(),
                    indexed: Indexed {
                        pending: Some(Box::new(PendingExtract {
                            id: "rollback".into(),
                            kind: PendingKind::Elicitation,
                            option_ids: None,
                            title: None
                        })),
                        ..Default::default()
                    },
                    payload: json!({})
                }
            )
            .is_err()
    );
    assert!(!w.store.still_open("child", "rollback").unwrap());
}

#[test]
fn d3_idle_adapter_loss_and_following_park_share_one_incident() {
    let (w, _) = pair();
    ready_parent(&w);
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::AdapterExited {
                code: Some(1),
                signal: None,
                stderr_tail: String::new(),
            },
        )
        .unwrap();
    assert_eq!(notices(&w).len(), 1);
    assert_eq!(notices(&w)[0].0, "session_unavailable");
    let (h, mut rx) = connected();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    w.store
        .ingest(
            "parent",
            3,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn,
            },
        )
        .unwrap();
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        )
        .unwrap();
    assert_eq!(notices(&w).len(), 1);
    assert_eq!(
        notices(&w)[0].1,
        "parent_delivered",
        "coalescing must not wake the parent twice"
    );
    let refs: Vec<i64> = serde_json::from_str(&notices(&w)[0].2).unwrap();
    assert_eq!(refs.len(), 2);
}

#[test]
fn d3_loss_obsoletes_out_of_turn_question_before_cancellation_frame() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::AdapterExited {
                code: Some(1),
                signal: None,
                stderr_tail: String::new(),
            },
        )
        .unwrap();
    assert_eq!(notices(&w)[0].1, "obsolete");
    assert!(
        w.store.still_open("child", "question").unwrap(),
        "canonical cancellation comes later"
    );
    w.store
        .ingest(
            "child",
            4,
            &SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        )
        .unwrap();
    assert_eq!(notices(&w).len(), 2);
    assert_eq!(notices(&w)[1].0, "session_unavailable");
}

#[test]
fn d3_withdrawal_is_distinct_and_idle_park_does_not_wake_parent() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::PendingResolved {
                pending_id: "question".into(),
                resolution: PendingResolution::Cancelled,
                reason: Some(PendingReason::AgentWithdrew),
            },
        )
        .unwrap();
    assert_eq!(notices(&w)[0].1, "obsolete");
    assert_eq!(notices(&w)[1].0, "question_cancelled");
    w.store
        .ingest(
            "child",
            4,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert_eq!(notices(&w).len(), 2);
}

#[test]
fn d3_collector_reconciliation_and_host_revoke_record_loss() {
    for revoke in [false, true] {
        let (w, _) = pair();
        pending(&w, "child", 2, "question", PendingKind::Elicitation);
        if revoke {
            w.store.revoke_host("host").unwrap();
        } else {
            w.store.reconcile_host("host", &[]).unwrap();
        }
        assert_eq!(notices(&w)[0].1, "obsolete");
        assert_eq!(notices(&w).iter().filter(|n| n.0 == "session_unavailable").count(), 1);
    }
}

#[test]
fn d3_permission_and_closed_parent_record_operator_disposition() {
    let (w, _) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    assert_eq!(notices(&w)[0].1, "operator_required");
    w.store.record_close_request("parent").unwrap();
    pending(&w, "child", 3, "question", PendingKind::Elicitation);
    assert!(notices(&w).iter().all(|n| n.1 == "operator_required"));
}

#[test]
fn d3_delete_scrubs_notices_and_reassignment_disables_routing() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    raw(&w)
        .execute(
            "UPDATE sessions SET hat_id = ?1 WHERE owner_id = ?2 AND id = 'child'",
            params![w.other, w.store.owner_id()],
        )
        .unwrap();
    assert_eq!(notices(&w)[0].1, "operator_required");
    w.store.close_now("child").unwrap();
    w.store.delete_session("child", None).unwrap();
    assert!(notices(&w).is_empty());
}

fn ready_parent(w: &World) {
    seed(w, "parent", &config("wanted"), 7);
    proof(w, "parent", 2, 7, 10, ConfigCompleteness::Complete, config("wanted"));
}

#[test]
fn d3_ordinary_parent_start_is_the_only_generation_zero_seed() {
    let w = World::new();
    let p = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let mut started = SessionBody::session_started("start", "agent-parent");
    if let SessionBody::SessionStarted { indexed, .. } = &mut started {
        indexed.config_evidence = Some(Box::new(ConfigEvidence {
            attach_generation: 0,
            config_revision: 1,
            completeness: ConfigCompleteness::Complete,
            actual: config("wanted"),
        }));
    }
    w.store.ingest("parent", 1, &started).unwrap();
    child(&w, &p, "child", &w.hat);
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let (h, _rx) = connected();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    // An unapplied duplicate start cannot replace current attach proof.
    proof(&w, "parent", 2, 0, 5, ConfigCompleteness::Complete, config("wanted"));
    w.store.ingest("parent", 3, &started).unwrap();
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT config_revision FROM session_attach_evidence WHERE owner_id=?1 AND session_id='parent'",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        5
    );
}

#[test]
fn d3_collector_start_failure_has_one_durable_factual_source() {
    for starting in [false, true] {
        let (w, _) = pair();
        if starting {
            raw(&w)
                .execute(
                    "UPDATE sessions SET lifecycle='starting' WHERE owner_id=?1 AND id='child'",
                    [w.store.owner_id()],
                )
                .unwrap();
            w.store.mark_failed_if_starting("child", "start_failed").unwrap();
            w.store.mark_failed_if_starting("child", "start_failed").unwrap();
        } else {
            w.store.mark_failed("child", "start_failed").unwrap();
            w.store.mark_failed("child", "start_failed").unwrap();
        }
        assert_eq!(notices(&w).len(), 1);
        assert_eq!(notices(&w)[0].0, "session_failed");
    }
}

#[test]
fn d3_late_factual_start_failure_updates_the_existing_incident() {
    let (w, _) = pair();
    w.store.mark_failed("child", "start_not_delivered").unwrap();
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::StartFailed {
                request_id: "late".into(),
                code: "adapter_failed".into(),
                message: "failure".into(),
            },
        )
        .unwrap();
    assert_eq!(notices(&w).len(), 1);
    let refs: Vec<i64> = serde_json::from_str(&notices(&w)[0].2).unwrap();
    assert_eq!(refs.len(), 2);
}

#[test]
fn d3_operator_answer_obsoletes_queued_question_in_its_acceptance_transaction() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store.submit_answer("child", "question", &answer()).unwrap();
    assert_eq!(notices(&w)[0].1, "obsolete");
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::PendingResolved {
                pending_id: "question".into(),
                resolution: PendingResolution::Delivered,
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(notices(&w).len(), 1, "a delivered answer is not a cancellation notice");
}

#[test]
fn d3_report_uses_exact_source_turn_and_keeps_utf8_tail_under_eight_kib() {
    let (w, p) = pair();
    ready(&w);
    ready_parent(&w);
    let (h, mut rx) = connected();
    let mut seq = 3;
    for (key, text) in [
        ("first", format!("{}END-第一", "ą".repeat(6000))),
        ("second", "SECOND-ONLY".to_string()),
    ] {
        queue(&w, &p, key);
        assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
        let (request, turn) = take_prompt(&mut rx);
        w.store
            .ingest(
                "child",
                seq,
                &SessionBody::TurnStarted {
                    request_id: request,
                    turn_id: turn.clone(),
                },
            )
            .unwrap();
        seq += 1;
        w.store.ingest("child",seq,&SessionBody::AcpUpdate {indexed:Indexed {turn_id:Some(turn.clone()),..Default::default()},payload:json!({"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}})}).unwrap();
        seq += 1;
        ended(&w, "child", seq, &turn);
        seq += 1;
    }
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    let frame = rx.try_recv().unwrap();
    let CollectorFrame::DelegatedPrompt { content, .. } = frame else {
        panic!()
    };
    let text = content[0]["text"].as_str().unwrap();
    assert!(text.len() <= 8192);
    assert!(text.contains("END-第一"));
    assert!(!text.contains("SECOND-ONLY"));
    assert!(text.contains("\"truncated\":true"));
}

#[test]
fn d3_reverse_delivery_reserves_once_and_only_facts_acknowledge_it() {
    let (w, _) = pair();
    ready_parent(&w);
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let (h, mut rx) = connected();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    assert_eq!(
        w.store.find_session("parent").unwrap().unwrap().open_turn_id.as_deref(),
        Some(turn.as_str())
    );
    assert_eq!(notices(&w)[0].1, "awaiting_reconciliation");
    assert!(!w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    w.store
        .ingest(
            "parent",
            3,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn.clone(),
            },
        )
        .unwrap();
    assert_eq!(notices(&w)[0].1, "parent_delivered");
    let user = w
        .store
        .events("parent", 0, 100)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "user_turn")
        .unwrap();
    let origin: InputOrigin = serde_json::from_value(user.body["origin"].clone()).unwrap();
    assert!(matches!(origin,InputOrigin::ChildReport {child_id,..} if child_id=="child"));
    assert!(
        user.body["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not operator input or approval")
    );
    ended(&w, "parent", 4, &turn);
    assert_eq!(
        notices(&w)[0].1,
        "parent_delivered",
        "a delivered question remains unresolved"
    );
}

#[test]
fn d3_reverse_delivery_checks_pending_config_close_grant_and_family() {
    for cut in [
        "pending",
        "config",
        "partial",
        "stale",
        "close_parent",
        "close_child",
        "grant",
        "family",
    ] {
        let (w, _) = pair();
        ready_parent(&w);
        pending(&w, "child", 2, "question", PendingKind::Elicitation);
        match cut {
            "pending" => {
                pending(&w, "parent", 3, "parent-question", PendingKind::Elicitation);
            }
            "config" => {
                raw(&w)
                    .execute(
                        "UPDATE session_attach_evidence SET evidence=NULL WHERE owner_id=?1 AND session_id='parent'",
                        [w.store.owner_id()],
                    )
                    .unwrap();
            }
            "partial" => {
                proof(&w, "parent", 3, 7, 11, ConfigCompleteness::Partial, config("wanted"));
            }
            "stale" => {
                let stale = ConfigEvidence {
                    attach_generation: 8,
                    config_revision: 11,
                    completeness: ConfigCompleteness::Complete,
                    actual: config("wanted"),
                };
                raw(&w)
                    .execute(
                        "UPDATE session_attach_evidence SET evidence=?2 WHERE owner_id=?1 AND session_id='parent'",
                        params![w.store.owner_id(), serde_json::to_string(&stale).unwrap()],
                    )
                    .unwrap();
            }
            "close_parent" => {
                w.store.record_close_request("parent").unwrap();
            }
            "close_child" => {
                w.store.record_close_request("child").unwrap();
            }
            "grant" => {
                w.gateway.set_session_tools(&w.hat, false).unwrap();
            }
            _ => {
                raw(&w)
                    .execute(
                        "UPDATE session_families SET detached=1 WHERE owner_id=?1 AND child_id='child'",
                        [w.store.owner_id()],
                    )
                    .unwrap();
            }
        }
        let (h, mut rx) = connected();
        if cut == "family" {
            assert_eq!(notices(&w)[0].1, "operator_required");
        }
        assert!(
            !w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap(),
            "{cut}"
        );
        assert!(rx.try_recv().is_err(), "{cut}");
    }
}

#[test]
fn d3_two_reverse_workers_and_an_operator_compete_for_one_slot() {
    let (w, _) = pair();
    ready_parent(&w);
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let (h, mut rx) = connected();
    let second = Store::open(&w.db).unwrap();
    let third = Store::open(&w.db).unwrap();
    let gate = Barrier::new(3);
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            gate.wait();
            w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap()
        });
        let b = scope.spawn(|| {
            gate.wait();
            second.dispatch_child_notice("parent", &catalog(), &h).unwrap()
        });
        let c = scope.spawn(|| {
            gate.wait();
            third
                .open_turn("parent", "operator-race", &[json!({"type":"text","text":"operator"})])
                .unwrap()
        });
        (a.join().unwrap(), b.join().unwrap(), c.join().unwrap())
    });
    assert_eq!([results.0, results.1, results.2].into_iter().filter(|v| *v).count(), 1);
    if results.0 || results.1 {
        take_prompt(&mut rx);
    }
    assert!(rx.try_recv().is_err());
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM turns WHERE owner_id=?1 AND session_id='parent'",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn d3_restart_keeps_uncertain_association_and_late_start_can_win() {
    for rejection in [true, false] {
        let (w, _) = pair();
        ready_parent(&w);
        pending(&w, "child", 2, "question", PendingKind::Elicitation);
        let (h, mut rx) = connected();
        assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
        let (request, turn) = take_prompt(&mut rx);
        let reopened = Store::open(&w.db).unwrap();
        assert!(!reopened.dispatch_child_notice("parent", &catalog(), &h).unwrap());
        if rejection {
            assert!(reopened.reject_delegation("host", &request).unwrap());
        } else {
            reopened
                .reconcile_host(
                    "host",
                    &[
                        hennery_proto::frames::AttachedSession {
                            ending: None,
                            session_id: "parent".into(),
                            last_seq: 2,
                            open_turn_id: None,
                        },
                        hennery_proto::frames::AttachedSession {
                            ending: None,
                            session_id: "child".into(),
                            last_seq: 2,
                            open_turn_id: None,
                        },
                    ],
                )
                .unwrap();
        }
        assert_eq!(notices(&w)[0].1, "operator_required");
        reopened
            .ingest(
                "parent",
                3,
                &SessionBody::TurnStarted {
                    request_id: request,
                    turn_id: turn.clone(),
                },
            )
            .unwrap();
        assert_eq!(notices(&w)[0].1, "parent_delivered");
        assert!(!reopened.dispatch_child_notice("parent", &catalog(), &h).unwrap());
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn d3_wake_budget_survives_reopen_and_child_questions_do_not_reset_it() {
    let (w, _) = pair();
    ready_parent(&w);
    let (h, mut rx) = connected();
    for n in 0..21 {
        pending(&w, "child", n + 2, &format!("question-{n}"), PendingKind::Elicitation);
        let reopened = Store::open(&w.db).unwrap();
        let sent = reopened.dispatch_child_notice("parent", &catalog(), &h).unwrap();
        assert_eq!(sent, n < 20);
        if sent {
            let (request, turn) = take_prompt(&mut rx);
            w.store
                .ingest(
                    "parent",
                    n * 2 + 3,
                    &SessionBody::TurnStarted {
                        request_id: request,
                        turn_id: turn.clone(),
                    },
                )
                .unwrap();
            ended(&w, "parent", n * 2 + 4, &turn);
        }
    }
    assert_eq!(notices(&w).last().unwrap().1, "operator_required");
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT SUM(wakes) FROM delegation_budgets WHERE owner_id=?1",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        20
    );
    user_turn_with_origin(&w, "parent", 43, "operator-root", InputOrigin::Operator {});
    pending(&w, "child", 23, "new-epoch-question", PendingKind::Elicitation);
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    assert_eq!(
        notices(&w)[20].1,
        "operator_required",
        "a new operator epoch does not replay an escalated notice"
    );
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM delegation_budgets WHERE owner_id=?1",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn d3_unsent_handoff_spends_no_budget_and_does_not_leave_a_turn() {
    let (w, _) = pair();
    ready_parent(&w);
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let (h, rx) = connected();
    drop(rx);
    assert!(!w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    assert_eq!(notices(&w)[0].1, "parent_queued");
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COALESCE(SUM(wakes),0) FROM delegation_budgets WHERE owner_id=?1",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert!(w.store.find_session("parent").unwrap().unwrap().open_turn_id.is_none());
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM turns WHERE owner_id=?1 AND session_id='parent'",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert!(
        raw(&w)
            .query_row(
                "SELECT target_turn FROM delegation_notices WHERE owner_id=?1",
                [w.store.owner_id()],
                |r| r.get::<_, Option<String>>(0)
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn d3_queue_overflow_has_a_durable_disposition_without_evicting_questions() {
    let (w, _) = pair();
    for n in 0..33 {
        pending(&w, "child", n + 2, &format!("q-{n}"), PendingKind::Elicitation);
    }
    let rows = notices(&w);
    assert_eq!(rows.len(), 33);
    assert_eq!(rows.iter().filter(|r| r.1 == "parent_queued").count(), 32);
    assert_eq!(rows.last().unwrap().1, "operator_required");
    assert_eq!(w.store.open_pending("child").unwrap().len(), 33);
}

#[test]
fn d3_automatic_parent_resume_requires_genuine_park_and_fresh_proof() {
    let (w, _) = pair();
    ready_parent(&w);
    // Notices capture the session's current config, independently of attach
    // evidence. Seed both, as a real host's configuration update does.
    raw(&w)
        .execute("UPDATE sessions SET model='wanted' WHERE id='parent'", [])
        .unwrap();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store
        .ingest(
            "parent",
            3,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    let (h, mut rx) = connected();
    let mcp = McpContext {
        source_policy: false,
        native_sources: false,
        capable: true,
        isolation: McpIsolation::ClaudeStrict,
        rules_name_other_hats: false,
    };
    let mut unavailable = catalog();
    unavailable.config_options.clear();
    assert!(!w.store.attach_notice_parent("parent", mcp, &unavailable, &h).unwrap());
    assert!(rx.try_recv().is_err());
    unavailable = catalog();
    unavailable.freshness = CatalogFreshness::Stale;
    assert!(!w.store.attach_notice_parent("parent", mcp, &unavailable, &h).unwrap());
    assert!(rx.try_recv().is_err());
    assert!(w.store.attach_notice_parent("parent", mcp, &catalog(), &h).unwrap());
    let generation = match rx.try_recv().unwrap() {
        CollectorFrame::DelegatedAttach { attach_generation, .. } => attach_generation,
        other => panic!("{other:?}"),
    };
    assert_eq!(generation, 8);
    assert!(!w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    w.store
        .ingest("parent", 4, &SessionBody::session_started("resume", "agent-parent"))
        .unwrap();
    proof(
        &w,
        "parent",
        5,
        generation,
        1,
        ConfigCompleteness::Complete,
        config("wanted"),
    );
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
}

#[test]
fn d3_collector_close_obsoletes_its_question_before_endpoint_routing_cuts() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store.close_now("child").unwrap();
    assert_eq!(notices(&w)[0].1, "obsolete");
    assert!(notices(&w).iter().any(|n| n.0 == "question_cancelled"));
}

#[test]
fn delegated_unattached_close_records_parent_intent_and_publishes_all_committed_events() {
    let (w, p) = pair();
    pending(&w, "child", 2, "closing-question", PendingKind::Elicitation);
    let hub = Hub::new();
    let mut live = hub.subscribe();
    let before = w.store.max_event_id().unwrap();
    let accepted = w
        .store
        .queue_child_close(&p, 1, "close-events", "digest", "child", &hub)
        .unwrap();
    assert_eq!(w.store.find_session("child").unwrap().unwrap().lifecycle, "closed");
    let events = w.store.events("child", before, 100).unwrap();
    let close = events
        .iter()
        .find(|event| event.kind == "operator_closed")
        .expect("durable close intent");
    assert_eq!(close.host_seq, None);
    assert_eq!(
        close.body["origin"],
        json!(InputOrigin::Parent {
            session_id: "parent".into(),
            operation_id: accepted.operation_id.clone()
        })
    );
    assert!(events.iter().any(|event| event.kind == "pending_cancelled"));
    let published: Vec<_> = std::iter::from_fn(|| live.try_recv().ok()).collect();
    assert_eq!(
        published, events,
        "every committed close change must wake existing subscribers"
    );
    assert_eq!(
        w.store
            .queue_child_close(&p, 1, "close-events", "digest", "child", &hub)
            .unwrap()
            .operation_id,
        accepted.operation_id
    );
    assert!(live.try_recv().is_err(), "idempotent retry publishes nothing");
}

#[test]
fn close_intent_at_either_endpoint_cancels_before_prompt_handoff() {
    for endpoint in ["parent", "child"] {
        let (w, p) = pair();
        ready(&w);
        let q = queue(&w, &p, "followup");
        let (h, mut rx) = connected();
        w.store.record_close_request(endpoint).unwrap();
        assert!(!w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
        assert!(rx.try_recv().is_err());
        assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (0, 0));
        let(state,payload):(String,String)=raw(&w).query_row("SELECT o.state,q.payload FROM session_operations o JOIN session_dispatch_queue q USING(owner_id,operation_id) WHERE o.operation_id=?1",[q.operation_id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(state, "cancelled");
        assert!(payload.is_empty());
    }
}
#[test]
fn automatic_resume_accepts_only_genuinely_parked_and_ready_host() {
    for (lifecycle, presumed, close, host_ready, expected) in [
        ("parked", false, false, true, true),
        ("parked", true, false, true, false),
        ("closed", false, false, true, false),
        ("failed", false, false, true, false),
        ("active", false, false, true, false),
        ("parked", false, true, true, false),
        ("parked", false, false, false, false),
    ] {
        let (w, p) = pair();
        ready(&w);
        queue(&w, &p, "followup");
        let (h, mut rx) = connected();
        raw(&w)
            .execute(
                "UPDATE sessions SET lifecycle=?1,presumed_parked=?2,close_requested=?3 WHERE id='child'",
                params![lifecycle, presumed, close],
            )
            .unwrap();
        if !host_ready {
            h.unregister("host", h.routable_conn("host").unwrap());
        }
        let old_hash: String = raw(&w)
            .query_row(
                "SELECT token_hash FROM gw_session_tokens WHERE session_id='child'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let old_watch = w.revocations.watch(&old_hash);
        assert!(!old_watch.token().is_cancelled());
        if expected {
            let mut unavailable = catalog();
            unavailable.config_options.clear();
            assert!(
                !w.store
                    .attach_delegated("child", McpContext::NONE, &unavailable, &h)
                    .unwrap()
            );
            assert!(!old_watch.token().is_cancelled());
            assert!(rx.try_recv().is_err());
        }
        assert_eq!(
            w.store
                .attach_delegated("child", McpContext::NONE, &catalog(), &h)
                .unwrap(),
            expected,
            "{lifecycle} {presumed} {close} {host_ready}"
        );
        assert_eq!(old_watch.token().is_cancelled(), expected);
        if expected {
            assert_eq!(w.store.find_session("child").unwrap().unwrap().lifecycle, "starting");
            assert!(matches!(rx.try_recv().unwrap(), CollectorFrame::DelegatedAttach { .. }));
        } else {
            assert!(rx.try_recv().is_err());
            assert_eq!(w.store.find_session("child").unwrap().unwrap().close_requested, close);
        }
    }
}
#[test]
fn automatic_resume_invalidates_stored_proof_until_new_complete_generation() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "followup");
    let (h, mut rx) = connected();
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert!(
        w.store
            .attach_delegated("child", McpContext::NONE, &catalog(), &h)
            .unwrap()
    );
    let generation = match rx.try_recv().unwrap() {
        CollectorFrame::DelegatedAttach { attach_generation, .. } => attach_generation,
        other => panic!("{other:?}"),
    };
    assert_eq!(generation, 6);
    let old: Option<String> = raw(&w)
        .query_row(
            "SELECT evidence FROM session_attach_evidence WHERE session_id='child'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(old.is_none());
    w.store
        .ingest("child", 4, &SessionBody::session_started("resume", "agent-child"))
        .unwrap();
    assert!(!w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    for (seq, generation, complete) in [
        (5, 5, ConfigCompleteness::Complete),
        (6, 6, ConfigCompleteness::Partial),
        (7, 6, ConfigCompleteness::Unknown),
    ] {
        proof(&w, "child", seq, generation, seq, complete, config("wanted"));
        assert!(!w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
        assert!(rx.try_recv().is_err());
    }
    proof(&w, "child", 8, 6, 8, ConfigCompleteness::Complete, config("wanted"));
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    take_prompt(&mut rx);
}
#[test]
fn required_model_mode_and_axes_hold_mismatch_then_current_repair_releases() {
    for field in ["model", "mode", "effort"] {
        let (w, p) = pair();
        let required = SessionConfig {
            model: Some("wanted".into()),
            mode: Some("plan".into()),
            axes: BTreeMap::from([("effort".into(), ConfigValue::Id("high".into()))]),
        };
        seed(&w, "child", &required, 5);
        let mut wrong = required.clone();
        match field {
            "model" => wrong.model = Some("other".into()),
            "mode" => wrong.mode = Some("default".into()),
            _ => {
                wrong.axes.insert("effort".into(), ConfigValue::Id("low".into()));
            }
        }
        proof(&w, "child", 2, 5, 10, ConfigCompleteness::Complete, wrong.clone());
        queue(&w, &p, "followup");
        let (h, mut rx) = connected();
        assert!(!w.store.dispatch_delegated("child", &catalog(), &h).unwrap(), "{field}");
        assert_eq!(receipt(&w, &p, "followup").state, OperationState::WaitingConfig);
        let status = w.store.child_status(&p, 1, "child", None).unwrap();
        assert_eq!(status.requested, required);
        assert_eq!(status.actual, Some(wrong));
        proof(&w, "child", 3, 5, 11, ConfigCompleteness::Complete, required);
        assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
        take_prompt(&mut rx);
    }
}

#[tokio::test(start_paused = true)]
async fn configuration_mismatch_expires_visibly_after_a_bounded_wait() {
    let (w, p) = pair();
    let required = SessionConfig {
        model: Some("wanted".into()),
        mode: Some("plan".into()),
        axes: BTreeMap::new(),
    };
    seed(&w, "child", &required, 5);
    let mut wrong = required.clone();
    wrong.model = Some("other".into());
    proof(&w, "child", 2, 5, 10, ConfigCompleteness::Complete, wrong);
    queue(&w, &p, "brief");
    let (hub, mut rx) = connected();
    assert!(!w.store.dispatch_delegated("child", &catalog(), &hub).unwrap());
    // The deadline starts at the persisted evidence, not at a later wall-clock
    // read: a loaded runner can cross a whole second while preparing dispatch.
    let now: i64 = raw(&w)
        .query_row(
            "SELECT config_updated_at FROM session_attach_evidence WHERE owner_id=?1 AND session_id='child'",
            [w.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap();
    let started = tokio::time::Instant::now();
    tokio::time::advance(std::time::Duration::from_secs(29)).await;
    w.store
        .refresh_delegation_attention(&hub, now + started.elapsed().as_secs() as i64)
        .unwrap();
    assert_eq!(receipt(&w, &p, "brief").state, OperationState::WaitingConfig);
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    w.store
        .refresh_delegation_attention(&hub, now + started.elapsed().as_secs() as i64)
        .unwrap();
    assert_eq!(
        receipt(&w, &p, "brief").state,
        OperationState::Failed {
            code: "configuration_mismatch".into()
        }
    );
    assert!(rx.try_recv().is_err(), "a mismatched brief must never reach the child");
    assert!(
        w.store
            .notice_parents_after("")
            .unwrap()
            .contains(&"parent".to_string())
    );
    seed(&w, "parent", &SessionConfig::default(), 1);
    proof(
        &w,
        "parent",
        2,
        1,
        1,
        ConfigCompleteness::Complete,
        SessionConfig::default(),
    );
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    let CollectorFrame::DelegatedPrompt { content, .. } = rx.try_recv().unwrap() else {
        panic!("expected child failure report");
    };
    let report = serde_json::to_string(&content).unwrap();
    assert!(report.contains("wanted") && report.contains("other") && report.contains("plan"));
    assert_eq!(
        w.store
            .find_session("child")
            .unwrap()
            .unwrap()
            .failure_reason
            .as_deref(),
        Some("configuration_mismatch")
    );
    assert!(w.store.events("child", 0, 100).unwrap().iter().any(|event| {
        event.kind == "start_failed"
            && event.body["message"]
                .as_str()
                .is_some_and(|s| s.contains("wanted") && s.contains("other") && s.contains("plan"))
    }));
    assert!(
        w.store
            .operator_attention(None, "")
            .unwrap()
            .iter()
            .any(|a| a.child_id == "child")
    );
}

#[test]
fn live_switch_readback_does_not_replace_a_delegated_childs_required_configuration() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "followup");
    let wrong = config("other");
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::ConfigApplied {
                request_id: "switch".into(),
                indexed: Indexed {
                    config_options: Some(vec![json!({"id":"model","currentValue":"other"})]),
                    current_model: wrong.model.clone(),
                    current_axes: Some(wrong.axes.clone()),
                    config_evidence: Some(Box::new(ConfigEvidence {
                        attach_generation: 5,
                        config_revision: 11,
                        completeness: ConfigCompleteness::Complete,
                        actual: wrong,
                    })),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    let (hub, mut rx) = connected();
    assert!(!w.store.dispatch_delegated("child", &catalog(), &hub).unwrap());
    w.store.refresh_delegation_attention(&hub, unix_now() + 31).unwrap();
    assert_eq!(
        w.store.child_status(&p, 1, "child", None).unwrap().requested,
        config("wanted")
    );
    assert_eq!(
        receipt(&w, &p, "followup").state,
        OperationState::Failed {
            code: "configuration_mismatch".into()
        }
    );
    assert!(rx.try_recv().is_err());
}
#[test]
fn stale_config_evidence_cannot_overwrite_current_proof_and_config_changes_clear_it() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "followup");
    let (h, mut rx) = connected();
    proof(&w, "child", 3, 5, 9, ConfigCompleteness::Complete, config("wrong"));
    proof(&w, "child", 4, 4, 11, ConfigCompleteness::Complete, config("wrong"));
    assert_eq!(
        w.store.child_status(&p, 1, "child", None).unwrap().actual,
        Some(config("wanted"))
    );
    raw(&w)
        .execute("UPDATE sessions SET model='changed' WHERE id='child'", [])
        .unwrap();
    assert!(!w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    assert!(rx.try_recv().is_err());
    proof(&w, "child", 5, 5, 11, ConfigCompleteness::Complete, config("wanted"));
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    take_prompt(&mut rx);
}

#[tokio::test(start_paused = true)]
async fn configuration_wait_renews_on_new_proof_and_unknown_values_do_not_fail() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "brief");
    let (hub, _rx) = connected();
    raw(&w)
        .execute(
            "UPDATE session_attach_evidence SET config_updated_at=?1 WHERE session_id='child'",
            [unix_now() - 29],
        )
        .unwrap();
    proof(&w, "child", 3, 5, 11, ConfigCompleteness::Complete, config("wrong"));
    let at = unix_now();
    let started = tokio::time::Instant::now();
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    w.store
        .refresh_delegation_attention(&hub, at + started.elapsed().as_secs() as i64)
        .unwrap();
    assert!(!matches!(receipt(&w, &p, "brief").state, OperationState::Failed { .. }));
    proof(&w, "child", 4, 5, 12, ConfigCompleteness::Unknown, config("wrong"));
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    w.store
        .refresh_delegation_attention(&hub, at + started.elapsed().as_secs() as i64)
        .unwrap();
    assert!(!matches!(receipt(&w, &p, "brief").state, OperationState::Failed { .. }));
}
#[test]
fn fresh_catalog_is_rechecked_and_attach_config_revision_is_bound_to_dispatch() {
    let (w, p) = pair();
    let mut required = config("wanted");
    required.mode = Some("agent-full-access".into());
    seed(&w, "child", &required, 5);
    proof(&w, "child", 2, 5, 10, ConfigCompleteness::Complete, required);
    let q = queue(&w, &p, "followup");
    let (h, mut rx) = connected();
    let mut c = catalog();
    c.freshness = CatalogFreshness::Stale;
    assert!(!w.store.dispatch_delegated("child", &c, &h).unwrap());
    assert!(rx.try_recv().is_err());
    c.freshness = CatalogFreshness::Fresh;
    c.security.known = false;
    assert!(w.store.dispatch_delegated("child", &c, &h).unwrap());
    take_prompt(&mut rx);
    let validated: (i64, i64) = raw(&w)
        .query_row(
            "SELECT validated_attach,validated_config FROM session_operations WHERE operation_id=?1",
            [q.operation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(validated, (5, 10));
}

#[test]
fn collector_restart_preserves_receipts_and_operation_key_namespace_across_tools() {
    let (w, p) = pair();
    let first = w
        .store
        .queue_spawn(
            &p,
            1,
            "spawn-digest",
            &hennery_sessions::delegation_queue::DelegatedLaunch {
                key: "shared",
                agent: "claude",
                cwd: "/project",
                brief: "brief",
                name: None,
                config: &config("wanted"),
                mcp: McpContext::NONE,
            },
        )
        .unwrap();
    let child = first.child_id.clone().unwrap();
    let reopened = Store::open(&w.db).unwrap();
    assert_eq!(
        reopened.delegation_retry(&p, 1, "shared", "spawn-digest").unwrap(),
        Some(first.clone())
    );
    assert_eq!(reopened.delegation_receipt(&p, 1, "shared").unwrap(), Some(first));
    assert!(
        reopened
            .queue_followup(&p, 1, "shared", "send-digest", &child, "other")
            .unwrap_err()
            .to_string()
            .contains("operation_conflict")
    );
    assert!(
        reopened
            .queue_ordinary_answer(
                &p,
                1,
                "answer-digest",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "shared",
                    child: &child,
                    pending: "missing",
                    answer: &answer()
                },
                &Hub::new(),
            )
            .unwrap_err()
            .to_string()
            .contains("operation_conflict")
    );
    let (h, mut rx) = connected();
    assert!(
        reopened
            .queue_child_close(&p, 1, "shared", "close-digest", &child, &h)
            .unwrap_err()
            .to_string()
            .contains("operation_conflict")
    );
    assert!(rx.try_recv().is_err());
    assert!(!reopened.find_session(&child).unwrap().unwrap().close_requested);
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM session_operations WHERE caller_id='parent'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
#[test]
fn four_workers_reserve_one_turn_and_drain_fifo_after_factual_completion() {
    let (w, p) = pair();
    ready(&w);
    for i in 0..4 {
        queue(&w, &p, &format!("fifo-{i}"));
    }
    let (h, mut rx) = connected();
    let barrier = Arc::new(Barrier::new(4));
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|_| {
                let db = w.db.clone();
                let h = h.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let s = Store::open(&db).unwrap();
                    barrier.wait();
                    s.dispatch_delegated("child", &catalog(), &h).unwrap()
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|sent| **sent).count(), 1);
    let (request, turn) = match rx.try_recv().unwrap() {
        CollectorFrame::DelegatedPrompt {
            request_id,
            turn_id,
            content,
            ..
        } => {
            assert!(content[0]["text"].as_str().unwrap().ends_with("fifo-0"));
            (request_id, turn_id)
        }
        other => panic!("{other:?}"),
    };
    assert!(rx.try_recv().is_err());
    assert_eq!(receipt(&w, &p, "fifo-0").state, OperationState::AwaitingReconciliation);
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn.clone(),
            },
        )
        .unwrap();
    ended(&w, "child", 4, &turn);
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    match rx.try_recv().unwrap() {
        CollectorFrame::DelegatedPrompt { content, .. } => {
            assert!(content[0]["text"].as_str().unwrap().ends_with("fifo-1"))
        }
        other => panic!("{other:?}"),
    }
}
#[test]
fn queue_count_cap_is_atomic_for_four_workers() {
    let (w, p) = pair();
    let barrier = Arc::new(Barrier::new(4));
    let accepted = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|worker| {
                let db = w.db.clone();
                let p = p.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let s = Store::open(&db).unwrap();
                    barrier.wait();
                    let mut accepted = 0;
                    for n in 0..20 {
                        let key = format!("worker-{worker}-{n}");
                        match s.queue_followup(&p, 1, &key, &key, "child", "é") {
                            Ok(_) => accepted += 1,
                            Err(e) => assert_eq!(e.to_string(), "queue_full"),
                        }
                    }
                    accepted
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).sum::<usize>()
    });
    assert_eq!(accepted, 32);
    assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (32, 64));
}
#[test]
fn queue_byte_limit_counts_utf8_bytes_and_oversized_message_is_rejected() {
    let (w, p) = pair();
    let text = "é".repeat(32768);
    for i in 0..16 {
        let key = format!("bytes-{i}");
        w.store.queue_followup(&p, 1, &key, &key, "child", &text).unwrap();
    }
    assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (16, 1024 * 1024));
    assert_eq!(
        w.store
            .queue_followup(&p, 1, "overflow", "overflow", "child", "x")
            .unwrap_err()
            .to_string(),
        "queue_full"
    );
    assert_eq!(
        w.store
            .queue_followup(&p, 1, "oversized", "oversized", "child", &"x".repeat(65537))
            .unwrap_err()
            .to_string(),
        "invalid_arguments"
    );
}
#[test]
fn concurrent_identical_keys_create_one_durable_operation() {
    let (w, p) = pair();
    let barrier = Arc::new(Barrier::new(4));
    let receipts = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|_| {
                let db = w.db.clone();
                let p = p.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let s = Store::open(&db).unwrap();
                    barrier.wait();
                    s.queue_followup(&p, 1, "same", "digest", "child", "same").unwrap()
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect::<Vec<_>>()
    });
    assert!(receipts.iter().all(|r| r == &receipts[0]));
    assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (1, 4));
}
fn spawn(w: &World, p: &SessionIdentity, key: &str) -> anyhow::Result<OperationReceipt> {
    w.store.queue_spawn(
        p,
        1,
        key,
        &hennery_sessions::delegation_queue::DelegatedLaunch {
            key,
            agent: "claude",
            cwd: "/project",
            brief: "brief",
            name: None,
            config: &SessionConfig::default(),
            mcp: McpContext::NONE,
        },
    )
}

#[test]
fn rejected_child_configuration_is_terminal_and_visible_to_parent_and_operator() {
    let (w, p) = pair();
    let receipt = spawn(&w, &p, "unsupported-config").unwrap();
    let child = receipt.child_id.unwrap();
    w.store
        .ingest(
            &child,
            1,
            &SessionBody::StartFailed {
                request_id: "attach".into(),
                code: "configuration_unavailable".into(),
                message: "fast: the selected model offers no such option".into(),
            },
        )
        .unwrap();
    assert!(
        matches!(w.store.delegation_receipt(&p,1,"unsupported-config").unwrap().unwrap().state,OperationState::Failed {code} if code=="configuration_unavailable")
    );
    assert_eq!(w.store.delegation_queue_depth(&child).unwrap(), (0, 0));
    let page = w.store.family_page("parent", "", 100).unwrap().unwrap();
    let row = page
        .children
        .iter()
        .find(|row| row.summary.session.session_id == child)
        .unwrap();
    assert_eq!(row.summary.session.lifecycle, "failed");
    assert_eq!(
        row.summary.session.failure_reason.as_deref(),
        Some("configuration_unavailable")
    );
    assert!(
        row.operations
            .iter()
            .any(|op| op.state == "failed" && op.code.as_deref() == Some("configuration_unavailable"))
    );
    assert!(notices(&w).iter().any(|notice| notice.0 == "session_failed"));
}
fn user_turn_with_origin(w: &World, id: &str, seq: u64, turn: &str, origin: InputOrigin) {
    assert!(
        w.store
            .open_turn(id, turn, &[json!({"type":"text","text":"message"})])
            .unwrap()
    );
    raw(w)
        .execute(
            "UPDATE turns SET input_origin=?1 WHERE turn_id=?2",
            params![serde_json::to_string(&origin).unwrap(), turn],
        )
        .unwrap();
    w.store
        .ingest(
            id,
            seq,
            &SessionBody::TurnStarted {
                request_id: format!("request-{turn}"),
                turn_id: turn.into(),
            },
        )
        .unwrap();
    ended(w, id, seq + 1, turn);
}
#[test]
fn live_child_cap_and_one_generation_are_independent_of_runtime_choice() {
    let (w, p) = pair();
    for i in 0..7 {
        spawn(&w, &p, &format!("spawn-{i}")).unwrap();
    }
    assert_eq!(spawn(&w, &p, "ninth").unwrap_err().to_string(), "child_limit");
    let child_identity = w.start("nested-parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let mut conn = raw(&w);
    let tx = conn.transaction().unwrap();
    assert!(w.store.bind_family_in(&tx, &p, 1, "nested-parent", 0).unwrap());
    tx.commit().unwrap();
    w.active("nested-parent");
    assert_eq!(
        spawn(&w, &child_identity, "grandchild").unwrap_err().to_string(),
        "generation_limit"
    );
}

fn limits_auth(w: &World) -> hennery_kernel::operator::Authenticated {
    let operator = Operator::open(&w.db).unwrap();
    let phc: String = raw(w)
        .query_row(
            "SELECT phc FROM password_credentials WHERE owner_id=?1",
            [w.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap();
    let token = operator.open_session("limits test", &phc, unix_now()).unwrap().unwrap();
    operator.authenticate(&token, unix_now()).unwrap().unwrap()
}
fn set_limits(w: &World, patch: serde_json::Value) {
    assert!(matches!(
        w.store
            .patch_delegation_limits(&serde_json::from_value(patch).unwrap(), &limits_auth(w))
            .unwrap(),
        hennery_sessions::delegation_settings::LimitsChange::Done(_)
    ));
}

#[test]
fn operator_limits_defaults_bounds_partial_patch_and_reopen() {
    use hennery_proto::delegation_status::OperatorDelegationLimits;
    use hennery_sessions::delegation_settings::LimitsChange;
    let w = World::new();
    assert_eq!(
        w.store.delegation_limits().unwrap(),
        OperatorDelegationLimits::default()
    );
    set_limits(&w, json!({"children":2}));
    let expected = OperatorDelegationLimits {
        children: 2,
        ..Default::default()
    };
    assert_eq!(Store::open(&w.db).unwrap().delegation_limits().unwrap(), expected);
    for field in [
        "children",
        "starts_per_operator_epoch",
        "automatic_wakes_per_operator_epoch",
    ] {
        for value in [0, 65, u16::MAX] {
            let patch = serde_json::from_value(json!({field:value})).unwrap();
            assert_eq!(
                w.store.patch_delegation_limits(&patch, &limits_auth(&w)).unwrap(),
                LimitsChange::Invalid
            );
            assert_eq!(w.store.delegation_limits().unwrap(), expected);
        }
    }
    for value in [1, 64] {
        set_limits(
            &w,
            json!({"children":value,"starts_per_operator_epoch":value,"automatic_wakes_per_operator_epoch":value}),
        );
        assert_eq!(w.store.delegation_limits().unwrap().children, value);
    }
}

#[test]
fn operator_limits_recheck_auth_inside_write_transaction() {
    use hennery_sessions::delegation_settings::LimitsChange;
    let w = World::new();
    for sql in [
        "UPDATE auth_sessions SET last_step_up_at=0 WHERE owner_id=?1 AND id_hash=?2",
        "UPDATE auth_sessions SET expires_at=0 WHERE owner_id=?1 AND id_hash=?2",
        "DELETE FROM auth_sessions WHERE owner_id=?1 AND id_hash=?2",
    ] {
        let auth = limits_auth(&w);
        raw(&w)
            .execute(sql, params![w.store.owner_id(), auth.session_id])
            .unwrap();
        assert!(auth.stepped_up(unix_now()));
        assert_eq!(
            w.store
                .patch_delegation_limits(&serde_json::from_value(json!({"children":1})).unwrap(), &auth)
                .unwrap(),
            LimitsChange::StepUpRequired
        );
        assert_eq!(w.store.delegation_limits().unwrap().children, 8);
    }
}

#[test]
fn operator_limits_are_owner_scoped_and_database_bounds_are_enforced() {
    let w = World::new();
    raw(&w).execute_batch("INSERT INTO owners(id,created_at,set_up_at) VALUES ('other-limits-owner',9223372036854775807,9223372036854775807);
        INSERT INTO delegation_limits(owner_id,children,starts_per_operator_epoch,automatic_wakes_per_operator_epoch) VALUES ('other-limits-owner',1,2,3);").unwrap();
    assert_eq!(w.store.delegation_limits().unwrap().children, 8);
    set_limits(&w, json!({"children":4}));
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT children FROM delegation_limits WHERE owner_id='other-limits-owner'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    for field in [
        "children",
        "starts_per_operator_epoch",
        "automatic_wakes_per_operator_epoch",
    ] {
        for value in [0, 65] {
            assert!(
                raw(&w)
                    .execute(
                        &format!("UPDATE delegation_limits SET {field}=?1 WHERE owner_id=?2"),
                        params![value, w.store.owner_id()]
                    )
                    .is_err()
            );
        }
    }
    let mut auth = limits_auth(&w);
    auth.owner_id = "other-limits-owner".into();
    assert_eq!(
        w.store
            .patch_delegation_limits(&serde_json::from_value(json!({"children":1})).unwrap(), &auth)
            .unwrap(),
        hennery_sessions::delegation_settings::LimitsChange::StepUpRequired
    );
}

#[test]
fn operator_limits_concurrent_spawns_cannot_overrun_the_configured_cap() {
    for starts in [false, true] {
        let (w, p) = pair();
        set_limits(
            &w,
            if starts {
                json!({"starts_per_operator_epoch":1})
            } else {
                json!({"children":2})
            },
        );
        let barrier = Arc::new(Barrier::new(4));
        let accepted = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..4)
                .map(|i| {
                    let barrier = barrier.clone();
                    let db = w.db.clone();
                    let p = p.clone();
                    scope.spawn(move || {
                        let store = Store::open(&db).unwrap();
                        barrier.wait();
                        match store.queue_spawn(
                            &p,
                            1,
                            "digest",
                            &hennery_sessions::delegation_queue::DelegatedLaunch {
                                key: &format!("race-{i}"),
                                agent: "claude",
                                cwd: "/project",
                                brief: "brief",
                                name: None,
                                config: &SessionConfig::default(),
                                mcp: McpContext::NONE,
                            },
                        ) {
                            Ok(_) => 1,
                            Err(error) => {
                                assert_eq!(error.to_string(), if starts { "start_budget" } else { "child_limit" });
                                0
                            }
                        }
                    })
                })
                .collect();
            jobs.into_iter().map(|job| job.join().unwrap()).sum::<i32>()
        });
        assert_eq!(accepted, 1);
    }
}

#[test]
fn operator_limits_next_spawn_checks_children_and_starts_without_resetting_counters() {
    let (w, p) = pair(); // One live child, no charged starts.
    set_limits(&w, json!({"children":2,"starts_per_operator_epoch":3}));
    spawn(&w, &p, "second").unwrap();
    assert_eq!(
        w.store.precheck_spawn(&p, 1, None).unwrap_err().to_string(),
        "child_limit"
    );
    assert_eq!(spawn(&w, &p, "third").unwrap_err().to_string(), "child_limit");
    set_limits(&w, json!({"children":4}));
    spawn(&w, &p, "third").unwrap();
    set_limits(&w, json!({"starts_per_operator_epoch":2}));
    assert_eq!(
        w.store.precheck_spawn(&p, 1, None).unwrap_err().to_string(),
        "start_budget"
    );
    assert_eq!(spawn(&w, &p, "fourth").unwrap_err().to_string(), "start_budget");
    set_limits(&w, json!({"starts_per_operator_epoch":3}));
    spawn(&w, &p, "fourth").unwrap();
    raw(&w)
        .execute(
            "UPDATE sessions SET lifecycle='closed' WHERE owner_id=?1 AND id<>'parent'",
            [w.store.owner_id()],
        )
        .unwrap();
    assert_eq!(spawn(&w, &p, "fifth").unwrap_err().to_string(), "start_budget");
    user_turn_with_origin(&w, "parent", 2, "new-operator-epoch", InputOrigin::Operator {});
    assert!(spawn(&w, &p, "new-epoch").is_ok());
}

#[test]
fn operator_limits_next_wake_uses_saved_budget() {
    let (w, _) = pair();
    set_limits(&w, json!({"automatic_wakes_per_operator_epoch":1}));
    ready_parent(&w);
    let (h, mut rx) = connected();
    pending(&w, "child", 2, "question-1", PendingKind::Elicitation);
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    w.store
        .ingest(
            "parent",
            3,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn.clone(),
            },
        )
        .unwrap();
    ended(&w, "parent", 4, &turn);
    pending(&w, "child", 3, "question-2", PendingKind::Elicitation);
    assert!(!w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    assert_eq!(notices(&w).last().unwrap().1, "operator_required");
    set_limits(&w, json!({"automatic_wakes_per_operator_epoch":2}));
    pending(&w, "child", 4, "question-3", PendingKind::Elicitation);
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
}

#[test]
fn operator_limits_wakes_cross_twenty_and_stop_at_twenty_one_or_sixty_four() {
    for limit in [21, 64] {
        let (w, _) = pair();
        set_limits(&w, json!({"automatic_wakes_per_operator_epoch":limit}));
        ready_parent(&w);
        raw(&w)
            .execute(
                "INSERT INTO delegation_budgets(owner_id,root_id,epoch,wakes) VALUES (?1,'parent',0,20)",
                [w.store.owner_id()],
            )
            .unwrap();
        let (h, mut rx) = connected();
        for spent in 20..=limit {
            let index = (spent - 20) as u64;
            pending(
                &w,
                "child",
                index + 2,
                &format!("boundary-{spent}"),
                PendingKind::Elicitation,
            );
            assert_eq!(
                w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap(),
                spent < limit
            );
            if spent < limit {
                let (request, turn) = take_prompt(&mut rx);
                w.store
                    .ingest(
                        "parent",
                        index * 2 + 3,
                        &SessionBody::TurnStarted {
                            request_id: request,
                            turn_id: turn.clone(),
                        },
                    )
                    .unwrap();
                ended(&w, "parent", index * 2 + 4, &turn);
            } else {
                let state:(String,String)=raw(&w).query_row("SELECT state,reason FROM delegation_notices WHERE owner_id=?1 ORDER BY source_event_id DESC LIMIT 1",[w.store.owner_id()],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
                assert_eq!(state, ("operator_required".into(), "wake_budget".into()));
                assert!(rx.try_recv().is_err());
            }
        }
        assert_eq!(
            raw(&w)
                .query_row(
                    "SELECT wakes FROM delegation_budgets WHERE owner_id=?1 AND root_id='parent' AND epoch=0",
                    [w.store.owner_id()],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            limit
        );
    }
}

#[test]
fn operator_limits_children_and_starts_allow_sixty_four_and_refuse_sixty_five() {
    for starts in [false, true] {
        let (w, p) = pair();
        set_limits(&w, json!({"children":64,"starts_per_operator_epoch":64}));
        if starts {
            w.store.close_now("child").unwrap();
        }
        for i in 0..if starts { 64 } else { 63 } {
            let receipt = spawn(&w, &p, &format!("maximum-{i}")).unwrap();
            if starts {
                w.store.close_now(receipt.child_id.as_deref().unwrap()).unwrap();
            }
        }
        let code = if starts { "start_budget" } else { "child_limit" };
        assert_eq!(w.store.precheck_spawn(&p, 1, None).unwrap_err().to_string(), code);
        assert_eq!(spawn(&w, &p, "over-maximum").unwrap_err().to_string(), code);
    }
}
#[test]
fn replacement_children_cannot_reset_starts_budget_or_parent_report_cannot_reset_epoch() {
    let (w, p) = pair();
    for i in 0..32 {
        let q = spawn(&w, &p, &format!("spawn-{i}")).unwrap();
        raw(&w)
            .execute(
                "UPDATE sessions SET lifecycle='closed' WHERE id=?1",
                [q.child_id.unwrap()],
            )
            .unwrap();
    }
    assert_eq!(spawn(&w, &p, "33rd").unwrap_err().to_string(), "start_budget");
    user_turn_with_origin(
        &w,
        "parent",
        2,
        "parent-data",
        InputOrigin::Parent {
            session_id: "foreign-parent".into(),
            operation_id: "report".into(),
        },
    );
    assert_eq!(spawn(&w, &p, "after-parent").unwrap_err().to_string(), "start_budget");
    user_turn_with_origin(
        &w,
        "parent",
        4,
        "child-data",
        InputOrigin::ChildReport {
            child_id: "child".into(),
            source_event_id: "source".into(),
            delivery_id: "delivery".into(),
        },
    );
    assert_eq!(spawn(&w, &p, "after-report").unwrap_err().to_string(), "start_budget");
    user_turn_with_origin(&w, "parent", 6, "operator", InputOrigin::Operator {});
    assert!(spawn(&w, &p, "new-operator-epoch").is_ok());
    let epochs: Vec<i64> = {
        let c = raw(&w);
        let mut s=c.prepare("SELECT DISTINCT epoch FROM session_operations WHERE caller_id='parent' AND tool='session_spawn' ORDER BY epoch").unwrap();
        s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(epochs.len(), 2);
    assert_eq!(epochs[0], 0);
    assert!(epochs[1] > 0);
}
#[test]
fn owned_child_checks_reject_owner_hat_sibling_ancestor_and_scope_changed_cursor() {
    let (w, p) = pair();
    let child_identity = w.start("sibling", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    let mut c = raw(&w);
    let tx = c.transaction().unwrap();
    assert!(w.store.bind_family_in(&tx, &p, 1, "sibling", 0).unwrap());
    tx.commit().unwrap();
    w.active("sibling");
    let unrelated = w.start("unrelated", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("unrelated");
    let foreign = w.start("foreign-hat", &w.other, McpIsolation::ClaudeStrict).unwrap();
    w.active("foreign-hat");
    queue(&w, &p, "owned-key");
    assert!(
        w.store
            .delegation_receipt(&unrelated, 1, "owned-key")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        w.store
            .child_status(&p, 1, "child", Some("owned-key"))
            .unwrap()
            .operations
            .len(),
        1
    );
    assert!(
        w.store
            .child_status(&p, 1, "sibling", Some("owned-key"))
            .unwrap()
            .operations
            .is_empty()
    );
    for target in ["parent", "unrelated", "foreign-hat", "missing"] {
        assert!(w.store.child_status(&p, 1, target, None).is_err(), "{target}");
        assert!(
            w.store
                .queue_followup(&p, 1, &format!("deny-{target}"), target, target, "hidden")
                .is_err()
        );
    }
    for target in ["parent", "child"] {
        assert!(w.store.child_status(&child_identity, 1, target, None).is_err());
    }
    assert!(w.store.child_status(&unrelated, 1, "child", None).is_err());
    assert!(w.store.child_status(&foreign, 1, "child", None).is_err());
    let mut owner = p.clone();
    owner.owner_id = "other-owner".into();
    assert!(w.store.child_status(&owner, 1, "child", None).is_err());
    let first = w.store.child_list(&p, 1, None, 1).unwrap();
    assert_eq!(first.children.len(), 1);
    let cursor = first.next_cursor.unwrap();
    let second = w.store.child_list(&p, 1, Some(&cursor), 1).unwrap();
    assert_eq!(second.children.len(), 1);
    assert_ne!(first.children[0].child_id, second.children[0].child_id);
    assert!(second.next_cursor.is_none());
    assert!(
        w.store
            .child_list(&unrelated, 1, Some(&cursor), 1)
            .unwrap_err()
            .to_string()
            .contains("invalid_cursor")
    );
    let (scope, last) = cursor.split_once('.').unwrap();
    for cursor in [
        format!("foreign.{last}"),
        format!("{scope}.{}", "x".repeat(129)),
        "malformed".into(),
    ] {
        assert_eq!(
            w.store.child_list(&p, 1, Some(&cursor), 1).unwrap_err().to_string(),
            "invalid_cursor"
        );
    }
    assert!(w.store.child_list(&p, 1, None, 0).is_err());
    assert!(w.store.child_list(&p, 1, None, 101).is_err());
}
#[test]
fn ordinary_answers_share_one_slot_under_four_worker_race() {
    let (w, p) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let barrier = Arc::new(Barrier::new(4));
    let accepted = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|i| {
                let db = w.db.clone();
                let p = p.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let s = Store::open(&db).unwrap();
                    barrier.wait();
                    if i % 2 == 0 {
                        match s.queue_ordinary_answer(
                            &p,
                            1,
                            "digest",
                            &hennery_sessions::delegation_control::DelegatedAnswer {
                                key: &format!("parent-{i}"),
                                child: "child",
                                pending: "question",
                                answer: &answer(),
                            },
                            &Hub::new(),
                        ) {
                            Ok(_) => 1,
                            Err(e) => {
                                assert_eq!(e.to_string(), "already_answered");
                                0
                            }
                        }
                    } else {
                        match s.submit_answer("child", "question", &answer()).unwrap() {
                            AnswerSubmission::Queued(_) => 1,
                            AnswerSubmission::AlreadyAnswered => 0,
                            other => panic!("{other:?}"),
                        }
                    }
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).sum::<u32>()
    });
    assert_eq!(accepted, 1);
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM answer_queue WHERE pending_id='question'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
#[test]
fn parent_close_frees_unhanded_answer_without_stealing_operator_slot() {
    let (w, p) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store
        .queue_ordinary_answer(
            &p,
            1,
            "digest",
            &hennery_sessions::delegation_control::DelegatedAnswer {
                key: "answer",
                child: "child",
                pending: "question",
                answer: &answer(),
            },
            &Hub::new(),
        )
        .unwrap();
    assert!(w.store.answers_to_send("host").unwrap().is_empty());
    assert!(matches!(
        w.store.submit_answer("child", "question", &answer()).unwrap(),
        AnswerSubmission::AlreadyAnswered
    ));
    w.store.record_close_request("parent").unwrap();
    assert!(matches!(
        w.store.submit_answer("child", "question", &answer()).unwrap(),
        AnswerSubmission::Queued(_)
    ));
    assert_eq!(w.store.answers_to_send("host").unwrap().len(), 1);
}
#[test]
fn permissions_are_operator_only_and_status_contains_no_answer_capable_schema() {
    let (w, p) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    assert_eq!(
        w.store
            .queue_ordinary_answer(
                &p,
                1,
                "digest",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "permission",
                    child: "child",
                    pending: "permission",
                    answer: &answer()
                },
                &Hub::new(),
            )
            .unwrap_err()
            .to_string(),
        "operator_required"
    );
    let status = w.store.child_status(&p, 1, "child", None).unwrap();
    assert!(status.questions[0].operator_required);
    assert!(status.questions[0].question.is_none());
    assert!(matches!(
        w.store
            .submit_answer(
                "child",
                "permission",
                &AnswerRequest::Permission {
                    option_id: "allow".into()
                }
            )
            .unwrap(),
        AnswerSubmission::Queued(_)
    ));
}
#[test]
fn accepted_answer_is_not_delivered_until_factual_host_verdict_and_survives_source_close_after_handoff() {
    let (w, p) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store
        .queue_ordinary_answer(
            &p,
            1,
            "digest",
            &hennery_sessions::delegation_control::DelegatedAnswer {
                key: "answer",
                child: "child",
                pending: "question",
                answer: &answer(),
            },
            &Hub::new(),
        )
        .unwrap();
    let (h, mut rx) = connected();
    w.store.dispatch_delegation_answers(&h).unwrap();
    let request = match rx.try_recv().unwrap() {
        CollectorFrame::AnswerElicitation { request_id, .. } => request_id,
        other => panic!("{other:?}"),
    };
    assert_eq!(receipt(&w, &p, "answer").state, OperationState::AwaitingReconciliation);
    w.store.record_close_request("parent").unwrap();
    assert!(matches!(
        w.store.submit_answer("child", "question", &answer()).unwrap(),
        AnswerSubmission::AlreadyAnswered
    ));
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::AnswerResult {
                pending_id: "question".into(),
                request_id: request,
                delivered: true,
            },
        )
        .unwrap();
    let state: String = raw(&w)
        .query_row(
            "SELECT state FROM session_operations WHERE caller_id='parent' AND operation_key='answer'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "delivered");
    assert!(matches!(
        w.store.submit_answer("child", "question", &answer()).unwrap(),
        AnswerSubmission::AlreadyAnswered
    ));
}
#[test]
fn delivery_and_completion_follow_facts_not_handoff_and_late_rejection_is_ignored() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "followup");
    let (h, mut rx) = connected();
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    assert_eq!(
        receipt(&w, &p, "followup").state,
        OperationState::AwaitingReconciliation
    );
    assert!(!w.store.reject_delegation("foreign-host", &request).unwrap());
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::TurnStarted {
                request_id: request.clone(),
                turn_id: turn.clone(),
            },
        )
        .unwrap();
    assert_eq!(receipt(&w, &p, "followup").state, OperationState::Delivered);
    ended(&w, "child", 4, &turn);
    assert_eq!(receipt(&w, &p, "followup").state, OperationState::Completed);
    assert!(!w.store.reject_delegation("host", &request).unwrap());
}
#[test]
fn proven_rejection_is_not_delivered_but_late_factual_start_can_reconcile() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "followup");
    let (h, mut rx) = connected();
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    assert!(w.store.reject_delegation("host", &request).unwrap());
    assert_eq!(receipt(&w, &p, "followup").state, OperationState::NotDelivered);
    assert!(w.store.find_session("child").unwrap().unwrap().open_turn_id.is_none());
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn.clone(),
            },
        )
        .unwrap();
    assert_eq!(receipt(&w, &p, "followup").state, OperationState::Delivered);
    ended(&w, "child", 4, &turn);
    assert_eq!(receipt(&w, &p, "followup").state, OperationState::Completed);
}
#[test]
fn typed_parent_provenance_does_not_reset_operator_push_epoch() {
    let (w, p) = pair();
    ready(&w);
    assert!(
        pending(&w, "child", 3, "first", PendingKind::Elicitation)
            .edge
            .is_some()
    );
    w.store
        .ingest(
            "child",
            4,
            &SessionBody::PendingResolved {
                pending_id: "first".into(),
                resolution: PendingResolution::Cancelled,
                reason: Some(PendingReason::AgentWithdrew),
            },
        )
        .unwrap();
    queue(&w, &p, "followup");
    let (h, mut rx) = connected();
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    w.store
        .ingest(
            "child",
            5,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn.clone(),
            },
        )
        .unwrap();
    ended(&w, "child", 6, &turn);
    let events = w.store.events("child", 0, 100).unwrap();
    let event = events.iter().find(|e| e.kind == "user_turn").unwrap();
    assert_eq!(event.body["origin"]["kind"], "parent");
    assert_eq!(event.body["origin"]["session_id"], "parent");
    assert!(event.body["origin"]["operation_id"].is_string());
    assert!(
        pending(&w, "child", 7, "second", PendingKind::Elicitation)
            .edge
            .is_none()
    );
    w.store
        .ingest(
            "child",
            8,
            &SessionBody::PendingResolved {
                pending_id: "second".into(),
                resolution: PendingResolution::Cancelled,
                reason: Some(PendingReason::AgentWithdrew),
            },
        )
        .unwrap();
    queue(&w, &p, "followup-running");
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    w.store
        .ingest(
            "child",
            9,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn,
            },
        )
        .unwrap();
    assert!(
        pending(&w, "child", 10, "third", PendingKind::Elicitation)
            .edge
            .is_none()
    );
    assert_eq!(
        w.store.find_session("child").unwrap().unwrap().activity.as_deref(),
        Some("blocked")
    );
}

fn persisted_state_case(stored: &str, code: Option<&str>, expected: OperationState) {
    let (w, p) = pair();
    let q = queue(&w, &p, "state");
    raw(&w)
        .execute(
            "UPDATE session_operations SET state=?1,code=?2 WHERE operation_id=?3",
            params![stored, code, q.operation_id],
        )
        .unwrap();
    assert_eq!(receipt(&w, &p, "state").state, expected);
}
macro_rules! receipt_case {
    ($name:ident,$stored:literal,$code:expr,$expected:expr) => {
        #[test]
        fn $name() {
            persisted_state_case($stored, $code, $expected);
        }
    };
}
receipt_case!(receipt_accepted, "accepted", None, OperationState::Accepted);
receipt_case!(
    receipt_waiting_config,
    "waiting_config",
    None,
    OperationState::WaitingConfig
);
receipt_case!(receipt_queued, "queued", None, OperationState::Queued);
receipt_case!(
    receipt_awaiting_reconciliation,
    "awaiting_reconciliation",
    None,
    OperationState::AwaitingReconciliation
);
receipt_case!(receipt_delivered, "delivered", None, OperationState::Delivered);
receipt_case!(receipt_completed, "completed", None, OperationState::Completed);
receipt_case!(
    receipt_not_delivered,
    "not_delivered",
    None,
    OperationState::NotDelivered
);
receipt_case!(receipt_cancelled, "cancelled", None, OperationState::Cancelled);
receipt_case!(
    receipt_failed_preserves_code,
    "failed",
    Some("adapter_failed"),
    OperationState::Failed {
        code: "adapter_failed".into()
    }
);
receipt_case!(
    receipt_failed_default_code,
    "failed",
    None,
    OperationState::Failed { code: "failed".into() }
);
receipt_case!(
    receipt_unknown_state_is_failure,
    "future_state",
    Some("future_failure"),
    OperationState::Failed {
        code: "future_failure".into()
    }
);
receipt_case!(
    receipt_failure_detail_is_redacted,
    "failed",
    Some("private provider failure text"),
    OperationState::Failed { code: "failed".into() }
);
#[test]
fn receipt_failure_code_is_bounded() {
    persisted_state_case(
        "failed",
        Some(&"x".repeat(129)),
        OperationState::Failed { code: "failed".into() },
    );
    persisted_state_case(
        "failed",
        Some(&"x".repeat(128)),
        OperationState::Failed { code: "x".repeat(128) },
    );
}
macro_rules! error_case {
    ($name:ident,$code:literal) => {
        #[test]
        fn $name() {
            assert_eq!(
                hennery_sessions::session_tools::error_code(&anyhow::anyhow!($code)),
                $code
            );
            assert_eq!(
                hennery_sessions::session_tools::error_code(&anyhow::anyhow!(concat!($code, " untrusted detail"))),
                "invalid_arguments"
            );
        }
    };
}
#[test]
fn model_facing_error_hints_do_not_imply_a_mode_restriction() {
    use hennery_sessions::session_tools;
    for code in [
        "auth_required",
        "host_offline",
        "runtime_unavailable",
        "agent_has_no_record",
        "adapter_failed",
        "catalog_unavailable",
        "catalog_stale",
        "configuration_unavailable",
        "timeout",
        "busy",
        "cwd_unavailable",
        "schema_changed",
        "invalid_arguments",
    ] {
        let hint = session_tools::error_message(&anyhow::anyhow!(code)).to_ascii_lowercase();
        assert!(
            !hint.contains("operator-only") && !hint.contains("operator only"),
            "{code}: {hint}"
        );
    }
}

error_case!(error_not_found, "not_found");
error_case!(error_operation_conflict, "operation_conflict");
error_case!(error_operator_required, "operator_required");
error_case!(error_queue_full, "queue_full");
error_case!(error_child_limit, "child_limit");
error_case!(error_start_budget, "start_budget");
error_case!(error_generation_limit, "generation_limit");
error_case!(error_session_starting, "session_starting");
error_case!(error_runtime_unavailable, "runtime_unavailable");
error_case!(error_configuration_unavailable, "configuration_unavailable");
error_case!(error_session_tools_unavailable, "session_tools_unavailable");
error_case!(error_catalog_unavailable, "catalog_unavailable");
error_case!(error_catalog_stale, "catalog_stale");
error_case!(error_catalog_busy, "catalog_busy");
error_case!(error_host_offline, "host_offline");
error_case!(error_invalid_cwd, "invalid_cwd");
error_case!(error_not_open, "not_open");
error_case!(error_already_answered, "already_answered");
error_case!(error_invalid_arguments, "invalid_arguments");
error_case!(error_invalid_cursor, "invalid_cursor");
#[test]
fn error_unknown_details_are_opaque_fallback() {
    assert_eq!(
        hennery_sessions::session_tools::error_code(&anyhow::anyhow!("database/internal/arbitrary")),
        "invalid_arguments"
    );
}

#[test]
fn actual_foreign_owner_rows_receipts_and_queue_targets_remain_invisible() {
    let (w, p) = pair();
    let c = raw(&w);
    c.execute_batch("INSERT INTO owners(id,created_at,set_up_at) VALUES ('owner-other',9223372036854775807,9223372036854775807);
    INSERT INTO hats(id,owner_id,name,colour,created_at) VALUES ('hat-other','owner-other','Other','#64748b',1);
    INSERT INTO hosts(id,owner_id,name,public_key,platform,host_version,default_hat_id,created_at) VALUES ('host-other','owner-other','Fixture','other-key','linux','0','hat-other',1);
    INSERT INTO sessions(id,owner_id,host_id,hat_id,agent,cwd,lifecycle,activity,created_at,last_event_at) VALUES ('parent-other','owner-other','host-other','hat-other','claude','/foreign','active','idle','0','0');
    INSERT INTO sessions(id,owner_id,host_id,hat_id,agent,cwd,lifecycle,activity,created_at,last_event_at) VALUES ('child-other','owner-other','host-other','hat-other','claude','/foreign','starting','idle','0','0');
    INSERT INTO session_families(owner_id,parent_id,child_id,root_id,hat_id,created_event_id) VALUES ('owner-other','parent-other','child-other','parent-other','hat-other',0);
    UPDATE sessions SET lifecycle='active' WHERE id='child-other';
    INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,request_id) VALUES ('owner-other','parent-other','guessed','foreign-operation','digest','session_send','child-other','queued','foreign-request');
    INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES ('owner-other','foreign-operation','parent-other','child-other',1,'private fixture','{}');").unwrap();
    assert_eq!(w.store.child_list(&p, 1, None, 100).unwrap().children.len(), 1);
    assert!(w.store.child_status(&p, 1, "child-other", None).is_err());
    assert!(w.store.delegation_receipt(&p, 1, "guessed").unwrap().is_none());
    assert!(w.store.delegation_targets().unwrap().is_empty());
    assert_eq!(w.store.delegation_queue_depth("child-other").unwrap(), (0, 0));
    assert!(!w.store.reject_delegation("host-other", "foreign-request").unwrap());
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM session_dispatch_queue WHERE owner_id='owner-other'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(queue(&w, &p, "own").state, OperationState::Queued);
}
#[test]
fn historical_child_status_does_not_authorize_new_close_after_operator_reopens() {
    let (w, p) = pair();
    raw(&w)
        .execute("UPDATE sessions SET lifecycle='closed' WHERE id='child'", [])
        .unwrap();
    raw(&w)
        .execute(
            "UPDATE sessions SET lifecycle='active',activity='idle' WHERE id='child'",
            [],
        )
        .unwrap();
    assert!(w.store.child_status(&p, 1, "child", None).is_ok());
    let (h, mut rx) = connected();
    assert_eq!(
        w.store
            .queue_child_close(&p, 1, "new-close", "digest", "child", &h)
            .unwrap_err()
            .to_string(),
        "not_found"
    );
    assert!(rx.try_recv().is_err());
    assert!(!w.store.find_session("child").unwrap().unwrap().close_requested);
}
#[test]
fn child_identity_cannot_spawn_grandchildren_after_parent_deletion_removes_edge() {
    let w = World::new();
    let p = w.start("parent", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("parent");
    let identity = child(&w, &p, "child", &w.hat);
    raw(&w)
        .execute("UPDATE sessions SET lifecycle='deleted' WHERE id='parent'", [])
        .unwrap();
    assert_eq!(
        raw(&w)
            .query_row("SELECT COUNT(*) FROM session_families", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        spawn(&w, &identity, "nested-after-delete").unwrap_err().to_string(),
        "generation_limit"
    );
}

#[test]
fn disabling_and_reenabling_grant_cancels_queue_without_reviving_old_token() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "followup");
    pending(&w, "child", 3, "grant-question", PendingKind::Elicitation);
    w.store
        .queue_ordinary_answer(
            &p,
            1,
            "digest",
            &hennery_sessions::delegation_control::DelegatedAnswer {
                key: "grant-answer",
                child: "child",
                pending: "grant-question",
                answer: &answer(),
            },
            &Hub::new(),
        )
        .unwrap();
    let (h, mut rx) = connected();
    w.gateway.set_session_tools(&w.hat, false).unwrap();
    w.gateway.set_session_tools(&w.hat, true).unwrap();
    assert_eq!(w.store.delegation_queue_depth("child").unwrap(), (0, 0));
    assert!(!w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    assert!(rx.try_recv().is_err());
    assert!(w.store.session_tool_context(&p, 1, true).unwrap().is_none());
    let states: Vec<String> = {
        let c = raw(&w);
        let mut stmt = c
            .prepare("SELECT state FROM session_operations WHERE caller_id='parent'")
            .unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert!(states.iter().all(|state| state == "cancelled"));
    let payload: String = raw(&w)
        .query_row(
            "SELECT payload FROM session_dispatch_queue WHERE source_id='parent'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(payload.is_empty());
    assert!(matches!(
        w.store.submit_answer("child", "grant-question", &answer()).unwrap(),
        AnswerSubmission::Queued(_)
    ));
    assert_eq!(
        w.store
            .queue_followup(&p, 1, "later", "later", "child", "later")
            .unwrap_err()
            .to_string(),
        "not_found"
    );
}

#[test]
fn current_path_rule_blocks_cross_hat_allocation_and_queued_dispatch() {
    use hennery_kernel::hats::{NewRule, RulesChange};
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "followup");
    let hosts = Hosts::open(&w.db).unwrap();
    assert!(matches!(
        hosts
            .replace_path_rules(
                "host",
                &[NewRule {
                    prefix: "/other".into(),
                    hat_id: w.other.clone(),
                    verified: true
                }]
            )
            .unwrap(),
        RulesChange::Done(_)
    ));
    assert_eq!(
        w.store
            .queue_spawn(
                &p,
                1,
                "digest",
                &hennery_sessions::delegation_queue::DelegatedLaunch {
                    key: "foreign-cwd",
                    agent: "claude",
                    cwd: "/other",
                    brief: "brief",
                    name: None,
                    config: &SessionConfig::default(),
                    mcp: McpContext::NONE
                }
            )
            .unwrap_err()
            .to_string(),
        "not_found"
    );
    assert!(matches!(
        hosts
            .replace_path_rules(
                "host",
                &[NewRule {
                    prefix: "/project".into(),
                    hat_id: w.other.clone(),
                    verified: true
                }]
            )
            .unwrap(),
        RulesChange::Done(_)
    ));
    let (h, mut rx) = connected();
    assert!(!w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    assert!(rx.try_recv().is_err());
}

#[test]
fn evidence_validator_requires_each_independent_proof_field() {
    use hennery_sessions::delegation_runtime::evidence_satisfies;
    let required = SessionConfig {
        model: Some("wanted".into()),
        mode: Some("default".into()),
        axes: BTreeMap::from([("effort".into(), ConfigValue::Id("high".into()))]),
    };
    let mut c = catalog();
    c.security.axes.insert(
        "mode".into(),
        vec![ConfigValue::Id("default".into()), ConfigValue::Id("plan".into())],
    );
    let good = ConfigEvidence {
        attach_generation: 7,
        config_revision: 2,
        completeness: ConfigCompleteness::Complete,
        actual: required.clone(),
    };
    assert!(evidence_satisfies(&good, 7, &required, &c));
    let mut unoffered = good.clone();
    unoffered
        .actual
        .axes
        .insert("not-offered".into(), ConfigValue::Bool(true));
    assert!(evidence_satisfies(&unoffered, 7, &required, &c));
    let mut security = c.clone();
    security.security.axes.insert("not-offered".into(), vec![]);
    assert!(!evidence_satisfies(&unoffered, 7, &required, &security));
    let mut removed = c.clone();
    removed.config_options.retain(|o| o["id"] != "mode");
    assert!(!evidence_satisfies(&good, 7, &required, &removed));
    let mut stale = c.clone();
    stale.freshness = CatalogFreshness::Stale;
    assert!(!evidence_satisfies(&good, 7, &required, &stale));
    for field in ["generation", "revision", "partial", "unknown", "model", "mode", "axis"] {
        let mut bad = good.clone();
        match field {
            "generation" => bad.attach_generation = 6,
            "revision" => bad.config_revision = 0,
            "partial" => bad.completeness = ConfigCompleteness::Partial,
            "unknown" => bad.completeness = ConfigCompleteness::Unknown,
            "model" => bad.actual.model = Some("other".into()),
            "mode" => bad.actual.mode = Some("plan".into()),
            _ => {
                bad.actual.axes.insert("effort".into(), ConfigValue::Id("low".into()));
            }
        }
        assert!(!evidence_satisfies(&bad, 7, &required, &c), "{field}");
    }
    let mut unknown = c.clone();
    unknown.security.known = false;
    unknown
        .security
        .axes
        .insert("effort".into(), vec![ConfigValue::Id("high".into())]);
    assert!(evidence_satisfies(&good, 7, &required, &unknown));
    let mut forbidden = c.clone();
    forbidden
        .security
        .axes
        .insert("mode".into(), vec![ConfigValue::Id("plan".into())]);
    assert!(evidence_satisfies(&good, 7, &required, &forbidden));
}

#[test]
fn endpoint_and_family_transition_each_cancel_and_scrub_undispatched_transfer() {
    for transition in ["endpoint", "family"] {
        let (w, p) = pair();
        ready(&w);
        queue(&w, &p, "followup");
        if transition == "endpoint" {
            // Isolate endpoint cleanup from the independently tested family hook.
            raw(&w).execute_batch("DROP TRIGGER delegation_family_end;").unwrap();
            w.store.record_close_request("child").unwrap();
            let proof: Option<String> = raw(&w)
                .query_row(
                    "SELECT evidence FROM session_attach_evidence WHERE session_id='child'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(proof.is_none());
        } else {
            raw(&w)
                .execute(
                    "UPDATE session_families SET detached=1,reason='reassigned' WHERE child_id='child'",
                    [],
                )
                .unwrap();
        }
        let (operation,queue,payload):(String,String,String)=raw(&w).query_row("SELECT o.state,q.state,q.payload FROM session_operations o JOIN session_dispatch_queue q USING(owner_id,operation_id) WHERE o.operation_key='followup'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(operation, "cancelled", "{transition}");
        assert_eq!(queue, "cancelled", "{transition}");
        assert!(payload.is_empty(), "{transition}");
    }
}

#[test]
fn parent_close_receipt_waits_for_confirmed_child_close_and_retry_sends_nothing() {
    let (w, p) = pair();
    let (h, mut rx) = connected();
    let accepted = w
        .store
        .queue_child_close(&p, 1, "close", "digest", "child", &h)
        .unwrap();
    assert_eq!(accepted.state, OperationState::AwaitingReconciliation);
    assert!(matches!(rx.try_recv().unwrap(),CollectorFrame::CloseSession{session_id,..} if session_id=="child"));
    assert_eq!(w.store.find_session("child").unwrap().unwrap().lifecycle, "active");
    assert!(w.store.find_session("child").unwrap().unwrap().close_requested);
    w.store.ingest("child", 2, &SessionBody::SessionClosed).unwrap();
    let completed = receipt(&w, &p, "close");
    assert_eq!(completed.state, OperationState::Completed);
    assert_eq!(
        w.store
            .queue_child_close(&p, 1, "close", "digest", "child", &h)
            .unwrap(),
        completed
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(w.store.find_session("parent").unwrap().unwrap().lifecycle, "active");
}

#[test]
fn deleted_endpoint_scrubs_handed_payload_and_required_config_and_removes_evidence() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "delete");
    let (h, mut rx) = connected();
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    take_prompt(&mut rx);
    let before: (String, String) = raw(&w)
        .query_row("SELECT payload,required_config FROM session_dispatch_queue", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert!(!before.0.is_empty());
    assert!(before.1.contains("wanted"));
    // Exercise the deletion hook independently of prior close/settlement hooks.
    raw(&w)
        .execute("UPDATE sessions SET lifecycle='deleted' WHERE id='child'", [])
        .unwrap();
    let remaining: i64 = raw(&w)
        .query_row("SELECT COUNT(*) FROM session_dispatch_queue", [], |r| r.get(0))
        .unwrap();
    assert_eq!(remaining, 0);
    let evidence_count: i64 = raw(&w)
        .query_row(
            "SELECT COUNT(*) FROM session_attach_evidence WHERE session_id='child'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(evidence_count, 0);
}

fn delete_endpoint(w: &World, id: &str) {
    use hennery_sessions::store::{Deletion, Unattached};
    let current = w.store.find_session(id).unwrap().unwrap();
    assert!(matches!(
        w.store
            .delete_session(
                id,
                Some(&Unattached {
                    lifecycle: current.lifecycle,
                    presumed_parked: current.presumed_parked,
                })
            )
            .unwrap(),
        Deletion::Done { .. }
    ));
}

fn stored_operation(w: &World, operation: &str) -> serde_json::Value {
    let row: String = raw(w).query_row(
        "SELECT json_object('owner_id',owner_id,'caller_id',caller_id,'operation_key',operation_key,
        'operation_id',operation_id,'digest',digest,'tool',tool,'child_id',child_id,'state',state,'code',code,
        'turn_id',turn_id,'pending_id',pending_id,'request_id',request_id,'accepted_revision',accepted_revision,
        'epoch',epoch,'validated_attach',validated_attach,'validated_config',validated_config,'validated_policy',validated_policy)
        FROM session_operations WHERE operation_id=?1",
        [operation], |r| r.get(0)
    ).unwrap();
    serde_json::from_str(&row).unwrap()
}

#[test]
fn deleted_target_keeps_only_opaque_gone_dedup_and_spawn_retry_never_allocates() {
    for prior in ["queued", "delivered", "completed"] {
        let (w, p) = pair();
        let accepted = w
            .store
            .queue_spawn(
                &p,
                1,
                "digest-private",
                &hennery_sessions::delegation_queue::DelegatedLaunch {
                    key: "old-spawn",
                    agent: "claude",
                    cwd: "/project/private",
                    brief: "private brief",
                    name: Some("private title"),
                    config: &config("private-model"),
                    mcp: McpContext::NONE,
                },
            )
            .unwrap();
        let target = accepted.child_id.as_deref().unwrap();
        w.active(target);
        proof(
            &w,
            target,
            2,
            1,
            9,
            ConfigCompleteness::Complete,
            config("private-model"),
        );
        raw(&w)
            .execute(
                "UPDATE session_operations SET state=?1,turn_id='private-turn',pending_id='private-question',
            request_id='private-request',accepted_revision=7,epoch=11,validated_attach=13,validated_config=17,
            validated_policy=19 WHERE operation_id=?2",
                params![prior, accepted.operation_id],
            )
            .unwrap();
        let before: (i64, i64) = raw(&w)
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sessions),
            (SELECT COUNT(*) FROM session_operations)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        delete_endpoint(&w, target);
        assert_eq!(
            stored_operation(&w, &accepted.operation_id),
            json!({
                "owner_id":w.store.owner_id(),"caller_id":"parent","operation_key":"old-spawn",
                "operation_id":accepted.operation_id,"digest":"digest-private","tool":"","child_id":null,
                "state":"failed","code":"gone","turn_id":null,"pending_id":null,"request_id":null,
                "accepted_revision":0,"epoch":0,"validated_attach":null,"validated_config":null,"validated_policy":null
            }),
            "{prior}"
        );
        let gone = receipt(&w, &p, "old-spawn");
        assert_eq!(gone.operation_id, accepted.operation_id);
        assert_eq!(gone.child_id, None);
        assert_eq!(gone.turn_id, None);
        assert_eq!(gone.pending_id, None);
        assert_eq!(gone.state, OperationState::Failed { code: "gone".into() });
        let retry = w
            .store
            .queue_spawn(
                &p,
                1,
                "digest-private",
                &hennery_sessions::delegation_queue::DelegatedLaunch {
                    key: "old-spawn",
                    agent: "claude",
                    cwd: "/project/private",
                    brief: "private brief",
                    name: Some("private title"),
                    config: &config("private-model"),
                    mcp: McpContext::NONE,
                },
            )
            .unwrap();
        assert_eq!(retry, gone);
        assert_eq!(
            w.store.delegation_retry(&p, 1, "old-spawn", "digest-private").unwrap(),
            Some(gone)
        );
        assert_eq!(
            w.store
                .delegation_retry(&p, 1, "old-spawn", "different")
                .unwrap_err()
                .to_string(),
            "operation_conflict"
        );
        let after: (i64, i64) = raw(&w)
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sessions),
            (SELECT COUNT(*) FROM session_operations)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(after, before);
        let remaining: (i64, i64, i64) = raw(&w)
            .query_row(
                "SELECT
            (SELECT COUNT(*) FROM session_dispatch_queue WHERE source_id=?1 OR target_id=?1),
            (SELECT COUNT(*) FROM session_attach_evidence WHERE session_id=?1),
            (SELECT COUNT(*) FROM session_families WHERE parent_id=?1 OR child_id=?1)",
                [target],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(remaining, (0, 0, 0));
        assert_eq!(w.store.find_session("parent").unwrap().unwrap().lifecycle, "active");
        delete_endpoint(&w, "parent");
        assert_eq!(
            raw(&w)
                .query_row(
                    "SELECT COUNT(*) FROM session_operations WHERE operation_id=?1",
                    [&accepted.operation_id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
fn deleting_caller_removes_receipts_and_queues_while_child_work_survives() {
    let (w, p) = pair();
    ready(&w);
    let delivered = queue(&w, &p, "delivered");
    let (h, mut rx) = connected();
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    let (_, turn) = take_prompt(&mut rx);
    queue(&w, &p, "queued");
    let unrelated = w.start("unrelated", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("unrelated");
    child(&w, &unrelated, "unrelated-child", &w.hat);
    let kept = w
        .store
        .queue_followup(&unrelated, 1, "kept", "digest", "unrelated-child", "kept private")
        .unwrap();
    let kept_row = stored_operation(&w, &kept.operation_id);
    delete_endpoint(&w, "parent");
    let child = w.store.find_session("child").unwrap().unwrap();
    assert_eq!(child.lifecycle, "active");
    assert_eq!(child.open_turn_id, Some(turn));
    assert!(!child.close_requested);
    let remaining: (i64, i64, i64) = raw(&w)
        .query_row(
            "SELECT
        (SELECT COUNT(*) FROM session_operations WHERE caller_id='parent'),
        (SELECT COUNT(*) FROM session_dispatch_queue WHERE source_id='parent' OR target_id='parent'),
        (SELECT COUNT(*) FROM session_families WHERE parent_id='parent')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(remaining, (0, 0, 0));
    assert_eq!(stored_operation(&w, &kept.operation_id), kept_row);
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT payload FROM session_dispatch_queue WHERE operation_id=?1",
                [&kept.operation_id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "kept private"
    );
    assert!(w.store.delegation_receipt(&p, 1, "delivered").is_err());
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM session_operations WHERE operation_id=?1",
                [delivered.operation_id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn only_gone_disposition_allows_retry_without_owned_child_reference() {
    let (w, p) = pair();
    let accepted = queue(&w, &p, "opaque");
    raw(&w)
        .execute(
            "UPDATE session_operations SET child_id=NULL WHERE operation_id=?1",
            [&accepted.operation_id],
        )
        .unwrap();
    for (state, code) in [("queued", "gone"), ("failed", "other")] {
        raw(&w)
            .execute(
                "UPDATE session_operations SET state=?1,code=?2 WHERE operation_id=?3",
                params![state, code, accepted.operation_id],
            )
            .unwrap();
        assert_eq!(
            w.store
                .delegation_retry(&p, 1, "opaque", "opaque")
                .unwrap_err()
                .to_string(),
            "not_found"
        );
    }
    raw(&w)
        .execute(
            "UPDATE session_operations SET state='failed',code='gone' WHERE operation_id=?1",
            [&accepted.operation_id],
        )
        .unwrap();
    let gone = w.store.delegation_retry(&p, 1, "opaque", "opaque").unwrap().unwrap();
    assert_eq!(gone.child_id, None);
    assert_eq!(gone.state, OperationState::Failed { code: "gone".into() });
}

#[test]
fn deleted_replacement_children_do_not_reset_starts_budget_and_caller_delete_removes_it() {
    let (w, p) = pair();
    for i in 0..32 {
        let accepted = spawn(&w, &p, &format!("deleted-spawn-{i}")).unwrap();
        delete_endpoint(&w, accepted.child_id.as_deref().unwrap());
    }
    assert_eq!(
        spawn(&w, &p, "33rd-after-delete").unwrap_err().to_string(),
        "start_budget"
    );
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT starts FROM delegation_start_budgets WHERE owner_id=?1 AND caller_id='parent' AND epoch=0",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        32
    );
    user_turn_with_origin(&w, "parent", 2, "operator", InputOrigin::Operator {});
    assert!(spawn(&w, &p, "new-operator-epoch").is_ok());
    let epochs: Vec<(i64, i64)> = {
        let c = raw(&w);
        let mut s=c.prepare("SELECT epoch,starts FROM delegation_start_budgets WHERE owner_id=?1 AND caller_id='parent' ORDER BY epoch").unwrap();
        s.query_map([w.store.owner_id()], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(epochs.len(), 2);
    assert_eq!(epochs[0], (0, 32));
    assert!(epochs[1].0 > 0);
    assert_eq!(epochs[1].1, 1);
    w.store.record_close_request("parent").unwrap();
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM delegation_start_budgets WHERE owner_id=?1 AND caller_id='parent'",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM session_operations WHERE owner_id=?1 AND caller_id='parent'",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        33
    );
    delete_endpoint(&w, "parent");
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM delegation_start_budgets WHERE owner_id=?1 AND caller_id='parent'",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn starts_budget_last_slot_is_atomic_for_four_workers_after_target_deletion() {
    let (w, p) = pair();
    for i in 0..31 {
        let accepted = spawn(&w, &p, &format!("prefill-{i}")).unwrap();
        delete_endpoint(&w, accepted.child_id.as_deref().unwrap());
    }
    let barrier = Arc::new(Barrier::new(4));
    let accepted = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|i| {
                let db = w.db.clone();
                let identity = p.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let store = Store::open(&db).unwrap();
                    barrier.wait();
                    match store.queue_spawn(
                        &identity,
                        1,
                        "digest",
                        &hennery_sessions::delegation_queue::DelegatedLaunch {
                            key: &format!("racing-{i}"),
                            agent: "claude",
                            cwd: "/project",
                            brief: "brief",
                            name: None,
                            config: &SessionConfig::default(),
                            mcp: McpContext::NONE,
                        },
                    ) {
                        Ok(_) => 1,
                        Err(error) => {
                            assert_eq!(error.to_string(), "start_budget");
                            0
                        }
                    }
                })
            })
            .collect();
        jobs.into_iter().map(|job| job.join().unwrap()).sum::<i32>()
    });
    assert_eq!(accepted, 1);
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT starts FROM delegation_start_budgets WHERE owner_id=?1 AND caller_id='parent' AND epoch=0",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        32
    );
}

#[test]
fn endpoint_deletion_preserves_unrelated_owner_commands_proof_and_budget() {
    for endpoint in ["parent", "child"] {
        let (w, p) = pair();
        ready(&w);
        queue(&w, &p, "own");
        spawn(&w, &p, "budget").unwrap();
        let c = raw(&w);
        c.execute_batch("INSERT INTO owners(id,created_at,set_up_at) VALUES ('owner-other',9223372036854775807,9223372036854775807);
        INSERT INTO hats(id,owner_id,name,colour,created_at) VALUES ('hat-other','owner-other','Other','#64748b',1);
        INSERT INTO hosts(id,owner_id,name,public_key,platform,host_version,default_hat_id,created_at) VALUES ('host-other','owner-other','Fixture','other-key','linux','0','hat-other',1);
        INSERT INTO sessions(id,owner_id,host_id,hat_id,agent,cwd,lifecycle,activity,created_at,last_event_at) VALUES ('parent-other','owner-other','host-other','hat-other','claude','/foreign','active','idle','0','0');
        INSERT INTO sessions(id,owner_id,host_id,hat_id,agent,cwd,lifecycle,activity,created_at,last_event_at) VALUES ('child-other','owner-other','host-other','hat-other','claude','/foreign','starting','idle','0','0');
        INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,request_id) VALUES ('owner-other','parent-other','foreign-key','foreign-operation','digest','session_send','child-other','queued','foreign-request');
        INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES ('owner-other','foreign-operation','parent-other','child-other',7,'foreign private brief','{\"model\":\"foreign-model\"}');
        INSERT INTO session_attach_evidence(owner_id,session_id,attach_generation,config_revision,evidence,requested_config,request_id) VALUES ('owner-other','child-other',7,9,'{\"private\":\"proof\"}','{\"model\":\"foreign-model\"}','foreign-attach');
        INSERT INTO delegation_start_budgets(owner_id,caller_id,epoch,starts) VALUES ('owner-other','parent-other',0,23);").unwrap();
        let before = stored_operation(&w, "foreign-operation");
        delete_endpoint(&w, endpoint);
        assert_eq!(stored_operation(&w, "foreign-operation"), before);
        let preserved: (String, String, i64, String, String, i64, i64) = c
            .query_row(
                "SELECT q.payload,q.required_config,q.accepted_revision,
            e.evidence,e.requested_config,e.attach_generation,b.starts FROM session_dispatch_queue q
            JOIN session_attach_evidence e ON e.owner_id=q.owner_id AND e.session_id=q.target_id
            JOIN delegation_start_budgets b ON b.owner_id=q.owner_id AND b.caller_id=q.source_id
            WHERE q.owner_id='owner-other'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            preserved,
            (
                "foreign private brief".into(),
                "{\"model\":\"foreign-model\"}".into(),
                7,
                "{\"private\":\"proof\"}".into(),
                "{\"model\":\"foreign-model\"}".into(),
                7,
                23
            )
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM delegation_start_budgets WHERE owner_id=?1 AND caller_id='parent'",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            i64::from(endpoint == "child")
        );
    }
}

#[test]
fn delegated_unattached_close_requires_unchanged_intent_lifecycle_and_presence() {
    use hennery_sessions::store::Unattached;
    for (guard, mutation) in [
        ("intent", "UPDATE sessions SET close_requested=0 WHERE id='child'"),
        ("lifecycle", "UPDATE sessions SET lifecycle='starting' WHERE id='child'"),
        ("presence", "UPDATE sessions SET presumed_parked=1 WHERE id='child'"),
    ] {
        let (w, _) = pair();
        w.store.record_close_request("child").unwrap();
        raw(&w).execute(mutation, []).unwrap();
        let before = w.store.find_session("child").unwrap().unwrap();
        let events_before: i64 = raw(&w)
            .query_row("SELECT COUNT(*) FROM events WHERE session_id='child'", [], |r| r.get(0))
            .unwrap();
        assert!(
            w.store
                .close_delegated_unattached(
                    "child",
                    &Unattached {
                        lifecycle: "active".into(),
                        presumed_parked: false
                    },
                )
                .unwrap()
                .is_empty(),
            "{guard}"
        );
        let after = w.store.find_session("child").unwrap().unwrap();
        assert_eq!(after.lifecycle, before.lifecycle, "{guard}");
        assert_eq!(after.close_requested, before.close_requested, "{guard}");
        assert_eq!(after.presumed_parked, before.presumed_parked, "{guard}");
        let events_after: i64 = raw(&w)
            .query_row("SELECT COUNT(*) FROM events WHERE session_id='child'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events_after, events_before, "{guard}");
    }
    let (w, _) = pair();
    w.store.record_close_request("child").unwrap();
    w.store
        .close_delegated_unattached(
            "child",
            &Unattached {
                lifecycle: "active".into(),
                presumed_parked: false,
            },
        )
        .unwrap();
    assert_eq!(w.store.find_session("child").unwrap().unwrap().lifecycle, "closed");
}

#[test]
fn operator_resume_clears_delegated_close_intent_before_late_unattached_verdict() {
    use hennery_sessions::store::{ResumeRequest, Unattached};
    let (w, _) = pair();
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    w.store.record_close_request("child").unwrap();
    let judged = Unattached {
        lifecycle: "parked".into(),
        presumed_parked: false,
    };
    assert!(matches!(
        w.store.request_resume("child", &w.hat).unwrap(),
        ResumeRequest::Starting { .. }
    ));
    assert!(w.store.close_delegated_unattached("child", &judged).unwrap().is_empty());
    let resumed = w.store.find_session("child").unwrap().unwrap();
    assert_eq!(resumed.lifecycle, "starting");
    assert!(!resumed.close_requested);
}

#[test]
fn target_pages_bound_progress_and_wrap_without_foreign_or_settled_rows() {
    let (w, _) = pair();
    let mut c = raw(&w);
    let tx = c.transaction().unwrap();
    for i in 0..133 {
        let id = if i < 131 {
            format!("target-{i:03}")
        } else {
            format!("hidden-{i}")
        };
        let state = if i == 131 {
            "cancelled"
        } else if i == 132 {
            "settled"
        } else if i % 2 == 0 {
            "queued"
        } else {
            "waiting_config"
        };
        tx.execute("INSERT INTO sessions(id,owner_id,host_id,hat_id,agent,cwd,lifecycle,activity,created_at,last_event_at) VALUES (?1,?2,'host',?3,'claude','/project','active','idle','0','0')",params![id,w.store.owner_id(),w.hat]).unwrap();
        tx.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,request_id) VALUES (?1,'parent',?2,?2,'digest','session_send',?2,'queued',?2)",params![w.store.owner_id(),id]).unwrap();
        tx.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config,state) VALUES (?1,?2,'parent',?2,1,'fixture','{}',?3)",params![w.store.owner_id(),id,state]).unwrap();
    }
    tx.execute_batch("INSERT INTO owners(id,created_at,set_up_at) VALUES ('owner-other',9223372036854775807,9223372036854775807);
    INSERT INTO hats(id,owner_id,name,colour,created_at) VALUES ('hat-other','owner-other','Other','#64748b',1);
    INSERT INTO hosts(id,owner_id,name,public_key,platform,host_version,default_hat_id,created_at) VALUES ('host-other','owner-other','Fixture','other-key','linux','0','hat-other',1);
    INSERT INTO sessions(id,owner_id,host_id,hat_id,agent,cwd,lifecycle,activity,created_at,last_event_at) VALUES ('foreign-first','owner-other','host-other','hat-other','claude','/foreign','active','idle','0','0');
    INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,request_id) VALUES ('owner-other','foreign-first','foreign','foreign-operation','digest','session_send','foreign-first','queued','foreign-request');
    INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES ('owner-other','foreign-operation','foreign-first','foreign-first',1,'fixture','{}');").unwrap();
    tx.commit().unwrap();
    let first = w.store.delegation_targets_after("").unwrap();
    assert_eq!(first, (0..128).map(|i| format!("target-{i:03}")).collect::<Vec<_>>());
    let second = w.store.delegation_targets_after(first.last().unwrap()).unwrap();
    assert_eq!(second, ["target-128", "target-129", "target-130"]);
    assert!(
        w.store
            .delegation_targets_after(second.last().unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(w.store.delegation_targets_after("").unwrap(), first);
}

#[test]
fn reviewer_actual_host_crash_has_one_parent_delivery() {
    let (w, p) = pair();
    ready(&w);
    ready_parent(&w);
    let (h, mut rx) = connected();
    queue(&w, &p, "crash-work");
    assert!(w.store.dispatch_delegated("child", &catalog(), &h).unwrap());
    let (request_id, turn_id) = take_prompt(&mut rx);
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::TurnStarted {
                request_id,
                turn_id: turn_id.clone(),
            },
        )
        .unwrap();
    pending(&w, "child", 4, "crash-question", PendingKind::Elicitation);
    // Exact order emitted by Actor::adapter_exited.
    w.store
        .ingest(
            "child",
            5,
            &SessionBody::TurnEnded {
                turn_id,
                outcome: TurnOutcome::Interrupted,
                stop_reason: None,
                error: Some("the adapter exited".into()),
            },
        )
        .unwrap();
    w.store
        .ingest(
            "child",
            6,
            &SessionBody::PendingResolved {
                pending_id: "crash-question".into(),
                resolution: PendingResolution::Cancelled,
                reason: Some(PendingReason::AdapterLost),
            },
        )
        .unwrap();
    w.store
        .ingest(
            "child",
            7,
            &SessionBody::AdapterExited {
                code: Some(1),
                signal: None,
                stderr_tail: String::new(),
            },
        )
        .unwrap();
    w.store
        .ingest(
            "child",
            8,
            &SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        )
        .unwrap();
    let mut delivered = 0;
    while w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap() {
        let (request_id, turn_id) = take_prompt(&mut rx);
        w.store
            .ingest(
                "parent",
                3 + delivered * 2,
                &SessionBody::TurnStarted {
                    request_id,
                    turn_id: turn_id.clone(),
                },
            )
            .unwrap();
        ended(&w, "parent", 4 + delivered * 2, &turn_id);
        delivered += 1;
        assert!(delivered < 10);
    }
    assert_eq!(
        delivered,
        1,
        "one adapter incident must not cause several parent turns: {:?}",
        notices(&w)
    );
}

#[test]
fn reviewer_out_of_turn_question_keeps_its_projection_after_later_operator_turn() {
    let (w, _) = pair();
    ready_parent(&w);
    // A prompt reserved before an out-of-turn callback can be acknowledged
    // after that callback. The projection must retain the earlier question.
    assert!(
        w.store
            .open_turn("child", "later-operator", &[json!({"type":"text","text":"continue"})])
            .unwrap()
    );
    pending(&w, "child", 2, "early-question", PendingKind::Elicitation);
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::TurnStarted {
                request_id: "operator-request".into(),
                turn_id: "later-operator".into(),
            },
        )
        .unwrap();
    let (h, mut rx) = connected();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &h).unwrap());
    let CollectorFrame::DelegatedPrompt { content, .. } = rx.try_recv().unwrap() else {
        panic!()
    };
    let text = content[0]["text"].as_str().unwrap();
    let metadata: serde_json::Value = serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
    assert!(
        metadata["question"].is_object(),
        "the queued question must not be lost to a later user_turn: {text}"
    );
}

#[test]
fn reviewer_host_crash_delivered_between_frames_never_wakes_twice_and_keeps_error() {
    for cut in 1..=4 {
        let (w, p) = pair();
        ready(&w);
        ready_parent(&w);
        let (hub, mut rx) = connected();
        queue(&w, &p, "crash-work");
        assert!(w.store.dispatch_delegated("child", &catalog(), &hub).unwrap());
        let (request, turn) = take_prompt(&mut rx);
        w.store
            .ingest(
                "child",
                3,
                &SessionBody::TurnStarted {
                    request_id: request,
                    turn_id: turn.clone(),
                },
            )
            .unwrap();
        pending(&w, "child", 4, "crash-question", PendingKind::Elicitation);
        let frames = [
            SessionBody::TurnEnded {
                turn_id: turn,
                outcome: TurnOutcome::Interrupted,
                stop_reason: None,
                error: Some("the adapter exited: diagnostic".into()),
            },
            SessionBody::PendingResolved {
                pending_id: "crash-question".into(),
                resolution: PendingResolution::Cancelled,
                reason: Some(PendingReason::AdapterLost),
            },
            SessionBody::AdapterExited {
                code: Some(1),
                signal: None,
                stderr_tail: "diagnostic stderr".into(),
            },
            SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        ];
        for (i, frame) in frames.iter().enumerate() {
            w.store.ingest("child", 5 + i as u64, frame).unwrap();
            if i + 1 == cut {
                assert!(w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
                let CollectorFrame::DelegatedPrompt {
                    request_id,
                    turn_id,
                    content,
                    ..
                } = rx.try_recv().unwrap()
                else {
                    panic!()
                };
                assert!(
                    content[0]["text"]
                        .as_str()
                        .unwrap()
                        .contains("the adapter exited: diagnostic")
                );
                w.store
                    .ingest(
                        "parent",
                        3,
                        &SessionBody::TurnStarted {
                            request_id,
                            turn_id: turn_id.clone(),
                        },
                    )
                    .unwrap();
                ended(&w, "parent", 4, &turn_id);
            }
        }
        let rows = notices(&w);
        assert_eq!(rows.len(), 2, "one obsolete question and one loss incident: {rows:?}");
        assert_eq!(rows[0].1, "obsolete");
        assert_eq!(rows[1].1, "parent_delivered");
        assert_eq!(serde_json::from_str::<Vec<i64>>(&rows[1].2).unwrap().len(), 4);
        assert!(!w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
        assert!(rx.try_recv().is_err());
        assert_eq!(
            raw(&w)
                .query_row(
                    "SELECT SUM(wakes) FROM delegation_budgets WHERE owner_id=?1",
                    [w.store.owner_id()],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
}

// D3b: operator attention is durable state, independent of push success.
#[test]
fn d3b_delegated_edges_use_durable_routing_and_ordinary_edges_still_notify() {
    let (w, _) = pair();
    let edge = pending(&w, "child", 2, "question", PendingKind::Elicitation)
        .edge
        .unwrap();
    let (push, mut notices) = hennery_kernel::push::Push::new();
    w.store.route_push_edge(&edge, &push).unwrap();
    assert!(
        notices.try_recv().is_none(),
        "parent-handled work gets the grace period"
    );
    let mut ordinary = edge;
    ordinary.session.id = "parent".into();
    w.store.route_push_edge(&ordinary, &push).unwrap();
    assert_eq!(notices.try_recv().unwrap().tag, "parent");
}
#[test]
fn d3b_permission_cannot_be_rearmed_as_parent_work() {
    let (w, _) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    let id = attention(&w)[0].id.clone();
    user_turn_with_origin(&w, "parent", 3, "operator", InputOrigin::Operator {});
    assert!(!w.store.rearm_delegation_attention(&id, "operator", unix_now()).unwrap());
    assert_eq!(attention(&w).len(), 1);
}
#[test]
fn d3b_delete_racing_enqueue_leaves_no_current_push_or_attention() {
    let (w, _) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    let (push, mut notices) = hennery_kernel::push::Push::new();
    let start = Barrier::new(3);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            start.wait();
            w.store.enqueue_delegation_attention(&push, "run", "", 1000).unwrap();
        });
        scope.spawn(|| {
            start.wait();
            w.store.close_now("child").unwrap();
            w.store.delete_session("child", None).unwrap();
        });
        start.wait();
    });
    while let Some(notice) = notices.try_recv() {
        assert!(!w.store.delegation_push_current(&notice).unwrap());
    }
    assert!(attention(&w).is_empty());
}
fn attention(w: &World) -> Vec<hennery_sessions::delegation_attention::OperatorAttention> {
    w.store.operator_attention(None, "").unwrap()
}
fn deadline(w: &World) -> i64 {
    raw(w)
        .query_row(
            "SELECT deadline FROM delegation_notices WHERE owner_id=?1 ORDER BY rowid LIMIT 1",
            [w.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap()
}
#[test]
fn d3b_deadline_is_persisted_and_does_not_expire_the_question() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let (hub, _) = connected();
    let due = deadline(&w);
    w.store.refresh_delegation_attention(&hub, due - 1).unwrap();
    assert!(attention(&w).is_empty());
    Store::open(&w.db)
        .unwrap()
        .refresh_delegation_attention(&hub, due)
        .unwrap();
    assert_eq!(attention(&w)[0].reason, "handling_deadline");
    assert!(w.store.still_open("child", "question").unwrap());
    assert_eq!(notices(&w)[0].1, "operator_required");
}
#[test]
fn d3b_delivered_unanswered_question_escalation_is_sticky() {
    let (w, _) = pair();
    ready_parent(&w);
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let (hub, mut rx) = connected();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    w.store
        .ingest(
            "parent",
            3,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn.clone(),
            },
        )
        .unwrap();
    w.store.refresh_delegation_attention(&hub, deadline(&w)).unwrap();
    assert_eq!(attention(&w).len(), 1);
    ended(&w, "parent", 4, &turn);
    w.store.refresh_delegation_attention(&hub, deadline(&w) + 1).unwrap();
    assert_eq!(attention(&w).len(), 1, "delivery facts cannot erase fallback");
    assert!(!w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    assert!(rx.try_recv().is_err());
    let id = attention(&w)[0].id.clone();
    let now = deadline(&w) + 2;
    user_turn_with_origin(&w, "parent", 5, "new-operator", InputOrigin::Operator {});
    w.store.refresh_delegation_attention(&hub, now).unwrap();
    assert!(w.store.rearm_delegation_attention(&id, "new-operator", now).unwrap());
    assert!(attention(&w).is_empty());
    assert_eq!(notices(&w)[0].1, "parent_delivered");
    assert!(!w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    assert!(rx.try_recv().is_err(), "rearm must not duplicate a delivered question");
}
#[test]
fn d3b_delivered_report_needs_no_answer_or_deadline_fallback() {
    let (w, _) = pair();
    ready_parent(&w);
    user_turn_with_origin(&w, "child", 2, "report-turn", InputOrigin::Operator {});
    ended(&w, "child", 3, "report-turn");
    let (hub, mut rx) = connected();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    w.store
        .ingest(
            "parent",
            3,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn,
            },
        )
        .unwrap();
    w.store.refresh_delegation_attention(&hub, deadline(&w) + 1).unwrap();
    assert!(attention(&w).is_empty());
}
#[test]
fn d3b_permission_and_parent_close_commit_attention_atomically() {
    let (w, _) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    assert_eq!(attention(&w).len(), 1);
    pending(&w, "child", 3, "question", PendingKind::Elicitation);
    assert_eq!(attention(&w).len(), 1);
    w.store.record_close_request("parent").unwrap();
    assert_eq!(attention(&w).len(), 2);
    let (push, mut notices) = hennery_kernel::push::Push::new();
    w.store
        .enqueue_delegation_attention(&push, "run", "", unix_now())
        .unwrap();
    let notice = notices.try_recv().unwrap();
    assert!(notice.url.contains("pending="));
    assert!(notice.tag.starts_with("delegation:"));
    assert_eq!(notice.detail.as_deref(), Some("question"));
}
#[test]
fn d3b_attention_write_failure_rolls_back_permission_source() {
    let (w, _) = pair();
    raw(&w).execute_batch("CREATE TRIGGER reject_attention BEFORE INSERT ON delegation_attention BEGIN SELECT RAISE(ABORT,'attention rejected'); END;").unwrap();
    let result = w.store.ingest(
        "child",
        2,
        &SessionBody::PendingOpened {
            pending_id: "permission".into(),
            indexed: Indexed {
                pending: Some(Box::new(PendingExtract {
                    id: "permission".into(),
                    kind: PendingKind::Permission,
                    option_ids: Some(vec!["allow".into()]),
                    title: None,
                })),
                ..Default::default()
            },
            payload: json!({}),
        },
    );
    assert!(result.is_err());
    assert!(!w.store.still_open("child", "permission").unwrap());
}
#[test]
fn d3b_push_reconciliation_reuses_tags_and_never_resolves_attention() {
    let (w, _) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    let (push, mut notices) = hennery_kernel::push::Push::new();
    w.store.enqueue_delegation_attention(&push, "first", "", 1000).unwrap();
    let first = notices.try_recv().unwrap();
    w.store.enqueue_delegation_attention(&push, "first", "", 1001).unwrap();
    assert!(notices.try_recv().is_none());
    Store::open(&w.db)
        .unwrap()
        .enqueue_delegation_attention(&push, "restart", "", 1001)
        .unwrap();
    assert_eq!(notices.try_recv().unwrap(), first);
    w.store
        .enqueue_delegation_attention(&push, "restart", "", 1301)
        .unwrap();
    assert_eq!(
        notices.try_recv().unwrap(),
        first,
        "bounded retry after in-memory queue loss"
    );
    assert_eq!(attention(&w).len(), 1);
    assert!(w.store.delegation_push_current(&first).unwrap());
    w.store
        .submit_answer(
            "child",
            "permission",
            &AnswerRequest::Permission {
                option_id: "allow".into(),
            },
        )
        .unwrap();
    assert!(attention(&w).is_empty());
    assert!(!w.store.delegation_push_current(&first).unwrap());
}
#[test]
fn d3b_parent_delete_preserves_scrubbed_surviving_child_attention() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store.close_now("parent").unwrap();
    w.store.delete_session("parent", None).unwrap();
    assert!(notices(&w).is_empty());
    let rows = attention(&w);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].child_id, "child");
    assert_eq!(rows[0].reason, "parent_closed_child_needs_human");
    assert!(w.store.find_session("child").unwrap().is_some());
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT delivery_id FROM delegation_attention WHERE owner_id=?1",
                [w.store.owner_id()],
                |r| r.get::<_, Option<String>>(0)
            )
            .unwrap(),
        None
    );
    assert!(matches!(
        w.store.submit_answer("child", "question", &answer()).unwrap(),
        AnswerSubmission::Queued(_)
    ));
    assert!(
        attention(&w).is_empty(),
        "the detached child remains directly answerable"
    );
    w.store.close_now("child").unwrap();
    w.store.delete_session("child", None).unwrap();
    assert!(attention(&w).is_empty());
}
#[test]
fn d3b_reassignment_invalidates_old_hat_push_and_moves_fallback() {
    let (w, _) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    let (push, mut notices) = hennery_kernel::push::Push::new();
    w.store.enqueue_delegation_attention(&push, "run", "", 1000).unwrap();
    let old = notices.try_recv().unwrap();
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert!(matches!(
        w.store.reassign_hat("child", &w.other).unwrap(),
        hennery_sessions::store::Reassign::Done(_)
    ));
    assert!(!w.store.delegation_push_current(&old).unwrap());
    assert!(w.store.operator_attention(Some(&w.hat), "").unwrap().is_empty());
    // Park cancelled the permission. Its separate loss status moves to the
    // child's new hat, without retaining an answer link to the cancelled callback.
    let rows = attention(&w);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].hat_id, w.other);
    assert_eq!(rows[0].pending_id, None);
    assert_ne!(format!("delegation:{}", rows[0].id), old.tag);
}
#[test]
fn d3b_rearm_requires_explicit_intent_a_new_factual_operator_turn_and_live_edge() {
    let (w, _) = pair();
    ready_parent(&w);
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let (hub, _) = connected();
    let due = deadline(&w);
    w.store.refresh_delegation_attention(&hub, due).unwrap();
    let id = attention(&w)[0].id.clone();
    assert!(!w.store.rearm_delegation_attention(&id, "missing", due + 1).unwrap());
    user_turn_with_origin(
        &w,
        "parent",
        3,
        "copied-report",
        InputOrigin::ChildReport {
            child_id: "child".into(),
            source_event_id: "7".into(),
            delivery_id: "copy".into(),
        },
    );
    assert!(
        !w.store
            .rearm_delegation_attention(&id, "copied-report", due + 1)
            .unwrap()
    );
    user_turn_with_origin(&w, "parent", 5, "operator", InputOrigin::Operator {});
    assert_eq!(attention(&w).len(), 1, "a plain operator prompt does not rearm");
    assert!(w.store.rearm_delegation_attention(&id, "operator", due + 1).unwrap());
    assert!(attention(&w).is_empty());
    assert_eq!(notices(&w)[0].1, "parent_queued");
    assert!(!w.store.rearm_delegation_attention(&id, "operator", due + 2).unwrap());
    w.store.refresh_delegation_attention(&hub, due + 301).unwrap();
    assert_eq!(attention(&w).len(), 1);
    w.store.record_close_request("parent").unwrap();
    assert!(!w.store.rearm_delegation_attention(&id, "operator", due + 302).unwrap());
}
#[test]
fn d3b_unsupported_connected_parent_falls_back_immediately_but_offline_gets_grace() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let hub = Hub::new();
    w.store.refresh_delegation_attention(&hub, deadline(&w) - 299).unwrap();
    assert!(attention(&w).is_empty());
    let (sender, _) = tokio::sync::mpsc::unbounded_channel();
    let reg = hub
        .register("host", sender, Capabilities(vec![]), AgentIsolation::default())
        .unwrap();
    hub.mark_ready("host", reg.conn_id);
    w.store.refresh_delegation_attention(&hub, deadline(&w) - 299).unwrap();
    assert_eq!(attention(&w)[0].reason, "unsupported_delivery");
}
#[test]
fn d3b_final_push_guard_rejects_purge_and_deleted_children() {
    for purge in [false, true] {
        let (w, _) = pair();
        pending(&w, "child", 2, "permission", PendingKind::Permission);
        let (push, mut notices) = hennery_kernel::push::Push::new();
        w.store.enqueue_delegation_attention(&push, "run", "", 1000).unwrap();
        let n = notices.try_recv().unwrap();
        assert!(w.store.delegation_push_current(&n).unwrap());
        if purge {
            raw(&w)
                .execute(
                    "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES (?1,?2,1)",
                    params![w.store.owner_id(), w.hat],
                )
                .unwrap();
        } else {
            w.store.close_now("child").unwrap();
            w.store.delete_session("child", None).unwrap();
        }
        assert!(!w.store.delegation_push_current(&n).unwrap());
    }
}

#[test]
fn d3b_acknowledging_a_report_is_durable_but_cannot_answer_a_question() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store.record_close_request("parent").unwrap();
    let question = attention(&w)[0].id.clone();
    assert!(!w.store.acknowledge_delegation_attention(&question).unwrap());
    assert!(w.store.still_open("child", "question").unwrap());
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::AdapterExited {
                code: Some(1),
                signal: None,
                stderr_tail: String::new(),
            },
        )
        .unwrap();
    let report = attention(&w).into_iter().find(|a| a.pending_id.is_none()).unwrap().id;
    assert!(w.store.acknowledge_delegation_attention(&report).unwrap());
    assert_eq!(
        notices(&w)
            .into_iter()
            .find(|n| n.0 == "session_unavailable")
            .unwrap()
            .1,
        "resolved"
    );
    assert!(!w.store.acknowledge_delegation_attention(&report).unwrap());
    let (hub, _) = connected();
    Store::open(&w.db)
        .unwrap()
        .refresh_delegation_attention(&hub, unix_now() + 600)
        .unwrap();
    assert!(attention(&w).is_empty());
}
#[test]
fn d3b_rearm_rejects_an_operator_prompt_that_predates_the_fallback() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    user_turn_with_origin(&w, "parent", 3, "before-deadline", InputOrigin::Operator {});
    let (hub, _) = connected();
    let due = deadline(&w);
    w.store.refresh_delegation_attention(&hub, due).unwrap();
    let id = attention(&w)[0].id.clone();
    assert!(
        !w.store
            .rearm_delegation_attention(&id, "before-deadline", due + 1)
            .unwrap()
    );
}

#[test]
fn d3b_revoked_parent_host_falls_back_without_waiting_for_deadline() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    Hosts::open(&w.db).unwrap().revoke("host", unix_now()).unwrap();
    w.store.revoke_host("host").unwrap();
    w.store
        .refresh_delegation_attention(&Hub::new(), deadline(&w) - 299)
        .unwrap();
    assert!(attention(&w).iter().any(|a| a.reason == "parent_unavailable"));
}
#[test]
fn d3b_push_admission_checks_the_canonical_answer_queue() {
    let (w, _) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    let (push, mut notices) = hennery_kernel::push::Push::new();
    w.store.enqueue_delegation_attention(&push, "run", "", 1000).unwrap();
    let notice = notices.try_recv().unwrap();
    w.store
        .submit_answer(
            "child",
            "permission",
            &AnswerRequest::Permission {
                option_id: "allow".into(),
            },
        )
        .unwrap();
    // Final admission remains safe even if a derived projection is stale.
    raw(&w)
        .execute(
            "UPDATE delegation_attention SET state='open' WHERE owner_id=?1",
            [w.store.owner_id()],
        )
        .unwrap();
    assert!(!w.store.delegation_push_current(&notice).unwrap());
}
#[test]
fn d3b_reassignment_racing_enqueue_rejects_old_hat_payloads() {
    let (w, _) = pair();
    user_turn_with_origin(&w, "child", 2, "report", InputOrigin::Operator {});
    // A live endpoint whose grant changed still has status work to acknowledge.
    w.gateway.set_session_tools(&w.hat, false).unwrap();
    w.store
        .ingest(
            "child",
            4,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    let (push, mut notices) = hennery_kernel::push::Push::new();
    let start = Barrier::new(3);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            start.wait();
            w.store.enqueue_delegation_attention(&push, "run", "", 1000).unwrap();
        });
        scope.spawn(|| {
            start.wait();
            assert!(matches!(
                w.store.reassign_hat("child", &w.other).unwrap(),
                hennery_sessions::store::Reassign::Done(_)
            ));
        });
        start.wait();
    });
    while let Some(notice) = notices.try_recv() {
        assert_eq!(
            w.store.delegation_push_current(&notice).unwrap(),
            notice.hat_id == w.other
        );
    }
    assert!(w.store.operator_attention(Some(&w.hat), "").unwrap().is_empty());
    assert_eq!(w.store.operator_attention(Some(&w.other), "").unwrap().len(), 1);
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM delegation_attention WHERE owner_id=?1 AND delivery_id IS NOT NULL",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
#[test]
fn d3b_muted_and_dropped_pushes_leave_durable_work_for_reconciliation() {
    let (w, _) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    w.store
        .enqueue_delegation_attention(&hennery_kernel::push::Push::detached(), "lost", "", 1000)
        .unwrap();
    assert_eq!(attention(&w).len(), 1);
    let (push, mut notices) = hennery_kernel::push::Push::new();
    w.store
        .enqueue_delegation_attention(&push, "restart", "", 1001)
        .unwrap();
    let template = notices.try_recv().unwrap();
    assert!(
        hennery_kernel::delivery::payload_for(
            &template,
            hennery_kernel::push::PushPolicy {
                muted: true,
                ..Default::default()
            }
        )
        .is_none()
    );
    for i in 0..hennery_kernel::push::NOTICE_QUEUE {
        let mut notice = template.clone();
        notice.tag = format!("fill-{i}");
        push.notify(notice);
    }
    w.store
        .enqueue_delegation_attention(&push, "overflow", "", 1002)
        .unwrap();
    assert_eq!(notices.dropped(), 1);
    assert_eq!(attention(&w).len(), 1);
    while notices.try_recv().is_some() {}
    w.store
        .enqueue_delegation_attention(&push, "overflow", "", 1302)
        .unwrap();
    assert_eq!(notices.try_recv().unwrap(), template);
    assert_eq!(attention(&w).len(), 1);
}
#[test]
fn d3b_parent_delete_removes_inert_attention_metadata_for_delivered_reports() {
    let (w, _) = pair();
    ready_parent(&w);
    user_turn_with_origin(&w, "child", 2, "report", InputOrigin::Operator {});
    let (hub, mut rx) = connected();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    w.store
        .ingest(
            "parent",
            3,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn.clone(),
            },
        )
        .unwrap();
    ended(&w, "parent", 4, &turn);
    w.store.close_now("parent").unwrap();
    w.store.delete_session("parent", None).unwrap();
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM delegation_attention WHERE owner_id=?1",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
#[test]
fn d3b_attention_pages_and_push_pages_are_bounded_and_advance() {
    let (w, _) = pair();
    for i in 0..129 {
        pending(&w, "child", 2 + i, &format!("permission-{i}"), PendingKind::Permission);
    }
    let first = w.store.operator_attention(Some(&w.hat), "").unwrap();
    assert_eq!(first.len(), 100);
    let second = w
        .store
        .operator_attention(Some(&w.hat), &first.last().unwrap().id)
        .unwrap();
    assert_eq!(second.len(), 29);
    let (push, mut notices) = hennery_kernel::push::Push::new();
    let after = w.store.enqueue_delegation_attention(&push, "run", "", 1000).unwrap();
    assert!(!after.is_empty());
    let mut count = 0;
    while notices.try_recv().is_some() {
        count += 1;
    }
    assert_eq!(count, 128);
    w.store
        .enqueue_delegation_attention(&push, "run", &after, 1000)
        .unwrap();
    assert!(notices.try_recv().is_some());
    assert!(notices.try_recv().is_none());
}

#[tokio::test]
async fn d3b_repeated_drains_share_one_notification_run_but_restart_reconciles() {
    let (w, _) = pair();
    pending(&w, "child", 2, "permission", PendingKind::Permission);
    let (push, mut notices) = hennery_kernel::push::Push::new();
    let mut state = hennery_sessions::AppState::new(
        Store::open(&w.db).unwrap(),
        Hosts::open(&w.db).unwrap(),
        Operator::open(&w.db).unwrap(),
    );
    state.push = push.clone();
    hennery_sessions::delegation_worker::drain(&state).await.unwrap();
    let first = notices.try_recv().unwrap();
    hennery_sessions::delegation_worker::drain(&state.clone())
        .await
        .unwrap();
    assert!(
        notices.try_recv().is_none(),
        "workers in one collector share a run identity"
    );
    let mut restarted = hennery_sessions::AppState::new(
        Store::open(&w.db).unwrap(),
        Hosts::open(&w.db).unwrap(),
        Operator::open(&w.db).unwrap(),
    );
    restarted.push = push;
    hennery_sessions::delegation_worker::drain(&restarted).await.unwrap();
    assert_eq!(notices.try_recv().unwrap(), first);
}

#[test]
fn d3b_automatic_report_completion_does_not_bypass_operator_push_routing() {
    let (w, _) = pair();
    ready_parent(&w);
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    let (hub, mut rx) = connected();
    assert!(w.store.dispatch_child_notice("parent", &catalog(), &hub).unwrap());
    let (request, turn) = take_prompt(&mut rx);
    w.store
        .ingest(
            "parent",
            3,
            &SessionBody::TurnStarted {
                request_id: request,
                turn_id: turn.clone(),
            },
        )
        .unwrap();
    let ended = w
        .store
        .ingest_fact(
            "parent",
            4,
            &SessionBody::TurnEnded {
                turn_id: turn,
                outcome: TurnOutcome::Completed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap();
    let (push, mut notices) = hennery_kernel::push::Push::new();
    w.store.route_push_edge(&ended.edge.unwrap(), &push).unwrap();
    assert!(
        notices.try_recv().is_none(),
        "automatic root reports must not create ordinary completion pushes"
    );
    assert!(
        w.store
            .open_turn("parent", "operator", &[json!({"type":"text","text":"continue"})])
            .unwrap()
    );
    w.store
        .ingest(
            "parent",
            5,
            &SessionBody::TurnStarted {
                request_id: "operator".into(),
                turn_id: "operator".into(),
            },
        )
        .unwrap();
    let ended = w
        .store
        .ingest_fact(
            "parent",
            6,
            &SessionBody::TurnEnded {
                turn_id: "operator".into(),
                outcome: TurnOutcome::Completed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap();
    w.store.route_push_edge(&ended.edge.unwrap(), &push).unwrap();
    assert_eq!(notices.try_recv().unwrap().tag, "parent");
}
#[test]
fn d3b_new_completion_after_parent_delete_is_status_only() {
    let (w, _) = pair();
    assert!(
        w.store
            .open_turn("child", "ongoing", &[json!({"type":"text","text":"work"})])
            .unwrap()
    );
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::TurnStarted {
                request_id: "work".into(),
                turn_id: "ongoing".into(),
            },
        )
        .unwrap();
    w.store.close_now("parent").unwrap();
    w.store.delete_session("parent", None).unwrap();
    let ended = w
        .store
        .ingest_fact(
            "child",
            3,
            &SessionBody::TurnEnded {
                turn_id: "ongoing".into(),
                outcome: TurnOutcome::Completed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap();
    let rows = attention(&w);
    assert!(rows.is_empty());
    assert!(
        w.store
            .events("child", 0, 100)
            .unwrap()
            .iter()
            .any(|e| e.kind == "turn_ended")
    );
    let (push, mut notices) = hennery_kernel::push::Push::new();
    w.store.route_push_edge(&ended.edge.unwrap(), &push).unwrap();
    assert!(notices.try_recv().is_none());
    w.store.enqueue_delegation_attention(&push, "run", "", 1000).unwrap();
    assert!(notices.try_recv().is_none());
}
#[test]
fn d3b_orphan_host_crash_resolves_attention_as_soon_as_question_is_cancelled() {
    let (w, _) = pair();
    assert!(w.store.open_turn("child", "ongoing", &[]).unwrap());
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::TurnStarted {
                request_id: "work".into(),
                turn_id: "ongoing".into(),
            },
        )
        .unwrap();
    pending(&w, "child", 3, "question", PendingKind::Elicitation);
    w.store.close_now("parent").unwrap();
    w.store.delete_session("parent", None).unwrap();
    let frames = [
        SessionBody::TurnEnded {
            turn_id: "ongoing".into(),
            outcome: TurnOutcome::Interrupted,
            stop_reason: None,
            error: Some("lost".into()),
        },
        SessionBody::PendingResolved {
            pending_id: "question".into(),
            resolution: PendingResolution::Cancelled,
            reason: Some(PendingReason::AdapterLost),
        },
        SessionBody::AdapterExited {
            code: Some(1),
            signal: None,
            stderr_tail: String::new(),
        },
        SessionBody::SessionParked {
            reason: ParkReason::AdapterExited,
        },
    ];
    for (i, frame) in frames.iter().enumerate() {
        w.store.ingest("child", 4 + i as u64, frame).unwrap();
        let rows = attention(&w);
        if w.store.still_open("child", "question").unwrap() {
            assert!(!rows.is_empty(), "live orphan question must remain visible");
            assert!(rows.iter().all(|row| row.reason == "parent_closed_child_needs_human"));
        } else {
            assert!(
                rows.is_empty(),
                "no human work remains after cancellation: frame {i}, {rows:?}"
            );
        }
    }
}

#[test]
fn d3b_start_failure_after_parent_delete_is_recorded_without_attention() {
    let (w, parent) = pair();
    w.start("starting", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    {
        let mut conn = raw(&w);
        let tx = conn.transaction().unwrap();
        assert!(w.store.bind_family_in(&tx, &parent, 1, "starting", 0).unwrap());
        tx.commit().unwrap();
    }
    w.store.close_now("parent").unwrap();
    w.store.delete_session("parent", None).unwrap();
    w.store.mark_failed_if_starting("starting", "late failure").unwrap();
    assert_eq!(w.store.find_session("starting").unwrap().unwrap().lifecycle, "failed");
    let rows = attention(&w);
    assert!(rows.is_empty());
    assert!(
        w.store
            .events("starting", 0, 100)
            .unwrap()
            .iter()
            .any(|e| e.kind == "start_failed")
    );
}

#[test]
fn reviewer_cancelled_parent_answer_reopens_operator_attention() {
    let mut remaining = Vec::new();
    for end in ["close", "delete", "tombstone"] {
        let (w, parent) = pair();
        pending(&w, "child", 2, "question", PendingKind::Elicitation);
        w.store
            .queue_ordinary_answer(
                &parent,
                1,
                "digest",
                &hennery_sessions::delegation_control::DelegatedAnswer {
                    key: "answer",
                    child: "child",
                    pending: "question",
                    answer: &answer(),
                },
                &Hub::new(),
            )
            .unwrap();
        assert!(attention(&w).is_empty());
        if end == "tombstone" {
            // Exercise the deletion trigger itself while an answer is still
            // queued, independently of the public close-then-delete path.
            raw(&w)
                .execute(
                    "UPDATE sessions SET lifecycle='deleted' WHERE owner_id=?1 AND id='parent'",
                    [w.store.owner_id()],
                )
                .unwrap();
        } else {
            w.store.close_now("parent").unwrap();
        }
        assert_eq!(
            attention(&w).len(),
            1,
            "cancellation must restore attention in the close transaction"
        );
        if end == "delete" {
            w.store.delete_session("parent", None).unwrap();
        }
        assert_eq!(attention(&w).len(), 1, "deletion must preserve restored attention");
        Store::open(&w.db)
            .unwrap()
            .refresh_delegation_attention(&Hub::new(), unix_now() + 600)
            .unwrap();
        assert!(w.store.still_open("child", "question").unwrap());
        let queued: i64 = raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM answer_queue WHERE owner_id=?1 AND pending_id='question'",
                [w.store.owner_id()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(queued, 0, "the undelivered answer was cancelled");
        remaining.push((end, attention(&w).len()));
    }
    assert_eq!(
        remaining,
        vec![("close", 1), ("delete", 1), ("tombstone", 1)],
        "close and delete must restore unresolved operator work"
    );
}

#[test]
fn reviewer_acknowledged_crash_does_not_reappear_after_parent_delete() {
    let (w, _) = pair();
    assert!(w.store.open_turn("child", "ongoing", &[]).unwrap());
    w.store
        .ingest(
            "child",
            2,
            &SessionBody::TurnStarted {
                request_id: "work".into(),
                turn_id: "ongoing".into(),
            },
        )
        .unwrap();
    w.gateway.set_session_tools(&w.hat, false).unwrap();
    w.store
        .ingest(
            "child",
            3,
            &SessionBody::TurnEnded {
                turn_id: "ongoing".into(),
                outcome: TurnOutcome::Interrupted,
                stop_reason: None,
                error: Some("lost".into()),
            },
        )
        .unwrap();
    let id = attention(&w)[0].id.clone();
    w.store
        .enqueue_delegation_attention(&hennery_kernel::push::Push::detached(), "before-delete", "", unix_now())
        .unwrap();
    assert!(w.store.acknowledge_delegation_attention(&id).unwrap());
    w.store.close_now("parent").unwrap();
    w.store.delete_session("parent", None).unwrap();
    let tombstone: (String, Option<String>, Option<String>, String, i64, String, i64) = raw(&w)
        .query_row(
            "SELECT attention_id,delivery_id,pending_id,reason,operator_epoch,push_run,push_at
         FROM delegation_attention WHERE owner_id=?1 AND child_id='child' AND incident LIKE 'loss:%'",
            [w.store.owner_id()],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .unwrap();
    assert_ne!(tombstone.0, id, "the former delivery identity must be scrubbed");
    assert_eq!(
        (
            tombstone.1,
            tombstone.2,
            tombstone.3,
            tombstone.4,
            tombstone.5,
            tombstone.6
        ),
        (None, None, "incident_resolved".into(), 0, String::new(), 0)
    );
    Store::open(&w.db).unwrap();
    w.store
        .ingest(
            "child",
            4,
            &SessionBody::AdapterExited {
                code: Some(1),
                signal: None,
                stderr_tail: String::new(),
            },
        )
        .unwrap();
    assert!(
        attention(&w).is_empty(),
        "the same acknowledged loss incident became new operator work"
    );
}

#[tokio::test(start_paused = true)]
async fn reviewer_slow_host_probes_do_not_starve_deadlines() {
    let w = World::new();
    for i in 0..32 {
        let parent_id = format!("parent-{i:02}");
        let child_id = format!("child-{i:02}");
        let parent = w.start(&parent_id, &w.hat, McpIsolation::ClaudeStrict).unwrap();
        w.active(&parent_id);
        child(&w, &parent, &child_id, &w.hat);
        pending(&w, &child_id, 2, &format!("question-{i}"), PendingKind::Elicitation);
    }
    let state = hennery_sessions::AppState::new(
        Store::open(&w.db).unwrap(),
        Hosts::open(&w.db).unwrap(),
        Operator::open(&w.db).unwrap(),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let reg = state
        .hub
        .register(
            "host",
            tx,
            Capabilities(vec![Capability::ResolvePath, Capability::DelegationConfig]),
            AgentIsolation::default(),
        )
        .unwrap();
    state.hub.mark_ready("host", reg.conn_id);
    hennery_sessions::delegation_worker::after_startup(&state);
    assert!(matches!(rx.recv().await.unwrap(), CollectorFrame::ResolvePath { .. }));
    let before_deadlines = w.store.max_event_id().unwrap();
    raw(&w)
        .execute(
            "UPDATE delegation_notices SET deadline=?2 WHERE owner_id=?1",
            params![w.store.owner_id(), unix_now() - 1],
        )
        .unwrap();
    for _ in 0..4 {
        tokio::time::advance(std::time::Duration::from_millis(250)).await;
        tokio::task::yield_now().await;
    }
    let open = attention(&w).len();
    let invalidated: i64 = raw(&w)
        .query_row(
            "SELECT count(*) FROM events WHERE owner_id=?1
        AND kind='family_changed' AND event_id>?2",
            params![w.store.owner_id(), before_deadlines],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        invalidated >= 32,
        "deadline routing must invalidate all child summaries while the host probe is pending"
    );
    let notified: i64 = raw(&w)
        .query_row(
            "SELECT COUNT(*) FROM delegation_attention WHERE owner_id=?1 AND state='open' AND push_run<>''",
            [w.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        rx.try_recv().is_err(),
        "the first 15-second host probe must still be pending"
    );
    state.shutdown.cancel();
    assert_eq!(
        open, 32,
        "all deadlines must reconcile within one second while the first host probe waits"
    );
    assert_eq!(
        notified, 32,
        "notification reconciliation must also be independent of host probes"
    );
}

#[test]
fn reviewer_migration20_recovers_preexisting_orphan_questions() {
    let (w, _) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store.close_now("parent").unwrap();
    w.store.delete_session("parent", None).unwrap();
    assert!(w.store.still_open("child", "question").unwrap());
    assert!(notices(&w).is_empty());
    // Reconstruct the D3a v19 schema after deletion: its notices and family
    // edge are scrubbed, while the surviving child's callback is still open.
    raw(&w)
        .execute_batch(concat!(
            include_str!("fixtures/undo_delegation_limits.sql"),
            include_str!("fixtures/undo_family.sql"),
            "DROP TRIGGER attention_notice_update;
        DROP TRIGGER attention_parent_delete;
        DROP TRIGGER attention_child_end;
        DROP TRIGGER attention_pending_end;
        DROP TRIGGER attention_answer_cancelled;
        DROP TABLE delegation_attention;
        DROP INDEX delegation_notice_deadline;
        PRAGMA user_version=19;"
        ))
        .unwrap();
    let upgraded = Store::open(&w.db).unwrap();
    upgraded
        .refresh_delegation_attention(&Hub::new(), unix_now() + 600)
        .unwrap();
    assert!(upgraded.still_open("child", "question").unwrap());
    assert_eq!(
        upgraded.operator_attention(None, "").unwrap().len(),
        1,
        "migration lost a preexisting orphan question"
    );
}

#[test]
fn d3b_cancelled_answer_restoration_rolls_back_with_the_endpoint() {
    let (w, parent) = pair();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    w.store
        .queue_ordinary_answer(
            &parent,
            1,
            "digest",
            &hennery_sessions::delegation_control::DelegatedAnswer {
                key: "answer",
                child: "child",
                pending: "question",
                answer: &answer(),
            },
            &Hub::new(),
        )
        .unwrap();
    let mut conn = raw(&w);
    let tx = conn.transaction().unwrap();
    tx.execute(
        "UPDATE sessions SET close_requested=1 WHERE owner_id=?1 AND id='parent'",
        [w.store.owner_id()],
    )
    .unwrap();
    let restored: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM delegation_attention WHERE owner_id=?1 AND state='open'",
            [w.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(restored, 1);
    tx.rollback().unwrap();
    assert!(attention(&w).is_empty());
    assert_eq!(
        w.store.answers_to_send("host").unwrap().len(),
        0,
        "a parent answer still waits for its guarded handoff"
    );
    assert_eq!(
        raw(&w)
            .query_row(
                "SELECT COUNT(*) FROM answer_queue WHERE owner_id=?1 AND delegation_handoff=0",
                [w.store.owner_id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn d3b_answer_cleanup_never_reopens_terminal_or_handed_off_questions() {
    for case in ["terminal", "canonical_terminal", "handed_off", "delivered", "operator"] {
        let (w, parent) = pair();
        pending(&w, "child", 2, "question", PendingKind::Elicitation);
        if case == "operator" {
            assert!(matches!(
                w.store.submit_answer("child", "question", &answer()).unwrap(),
                hennery_sessions::store::AnswerSubmission::Queued(_)
            ));
        } else {
            w.store
                .queue_ordinary_answer(
                    &parent,
                    1,
                    "digest",
                    &hennery_sessions::delegation_control::DelegatedAnswer {
                        key: "answer",
                        child: "child",
                        pending: "question",
                        answer: &answer(),
                    },
                    &Hub::new(),
                )
                .unwrap();
        }
        match case {
            "canonical_terminal" => {
                // The canonical projection fences cleanup even before the
                // queue's verdict has been updated in the same transaction.
                raw(&w)
                    .execute(
                        "UPDATE pending SET state='cancelled' WHERE owner_id=?1 AND pending_id='question'",
                        [w.store.owner_id()],
                    )
                    .unwrap();
            }
            "terminal" => {
                w.store
                    .ingest(
                        "child",
                        3,
                        &SessionBody::PendingResolved {
                            pending_id: "question".into(),
                            resolution: PendingResolution::Cancelled,
                            reason: None,
                        },
                    )
                    .unwrap();
            }
            "handed_off" => {
                raw(&w)
                    .execute(
                        "UPDATE answer_queue SET delegation_handoff=1 WHERE owner_id=?1",
                        [w.store.owner_id()],
                    )
                    .unwrap();
            }
            "delivered" => {
                raw(&w)
                    .execute(
                        "UPDATE answer_queue SET delivered=1 WHERE owner_id=?1",
                        [w.store.owner_id()],
                    )
                    .unwrap();
            }
            _ => {}
        }
        // Queue cleanup itself must not manufacture a second answer opportunity.
        raw(&w)
            .execute("DELETE FROM answer_queue WHERE owner_id=?1", [w.store.owner_id()])
            .unwrap();
        assert!(attention(&w).is_empty(), "{case}");
    }
}

#[test]
fn d3b_migration20_filters_answers_terminal_and_root_questions() {
    let (w, _) = pair();
    for (seq, id) in [(2, "open"), (3, "accepted"), (4, "terminal")] {
        pending(&w, "child", seq, id, PendingKind::Elicitation);
    }
    assert!(matches!(
        w.store.submit_answer("child", "accepted", &answer()).unwrap(),
        hennery_sessions::store::AnswerSubmission::Queued(_)
    ));
    w.store
        .ingest(
            "child",
            5,
            &SessionBody::PendingResolved {
                pending_id: "terminal".into(),
                resolution: PendingResolution::Cancelled,
                reason: None,
            },
        )
        .unwrap();
    w.start("root", &w.hat, McpIsolation::ClaudeStrict).unwrap();
    w.active("root");
    pending(&w, "root", 2, "root-question", PendingKind::Elicitation);
    w.store.close_now("parent").unwrap();
    w.store.delete_session("parent", None).unwrap();
    raw(&w)
        .execute_batch(concat!(
            include_str!("fixtures/undo_delegation_limits.sql"),
            include_str!("fixtures/undo_family.sql"),
            "DROP TRIGGER attention_notice_update;
        DROP TRIGGER attention_parent_delete;
        DROP TRIGGER attention_child_end;
        DROP TRIGGER attention_pending_end;
        DROP TRIGGER attention_answer_cancelled;
        DROP TABLE delegation_attention;
        DROP INDEX delegation_notice_deadline;
        PRAGMA user_version=19;"
        ))
        .unwrap();
    let upgraded = Store::open(&w.db).unwrap();
    let work = upgraded.operator_attention(None, "").unwrap();
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].pending_id.as_deref(), Some("open"));
    // Reopening the database neither duplicates the backfill nor opens accepted answers.
    assert_eq!(
        Store::open(&w.db).unwrap().operator_attention(None, "").unwrap().len(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn independent_catalog_keys_probe_without_global_serialization() {
    let w = World::new();
    let state = Arc::new(hennery_sessions::AppState::new(
        Store::open(&w.db).unwrap(),
        Hosts::open(&w.db).unwrap(),
        Operator::open(&w.db).unwrap(),
    ));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let reg = state
        .hub
        .register(
            "host",
            tx,
            Capabilities(vec![Capability::CatalogProbe]),
            AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect()),
        )
        .unwrap();
    state.hub.mark_ready("host", reg.conn_id);
    let first = {
        let state = state.clone();
        let hat = w.hat.clone();
        tokio::spawn(async move {
            state
                .catalogs
                .get_for_spawn(&state, "host", &hat, "/project", "claude")
                .await
        })
    };
    let CollectorFrame::ProbeCatalog {
        request_id: first_id, ..
    } = rx.recv().await.unwrap()
    else {
        panic!("probe expected")
    };
    let second = {
        let state = state.clone();
        let hat = w.hat.clone();
        tokio::spawn(async move {
            state
                .catalogs
                .get_for_spawn(&state, "host", &hat, "/other", "claude")
                .await
        })
    };
    let CollectorFrame::ProbeCatalog {
        request_id: second_id, ..
    } = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("another key must not wait for the first probe")
        .unwrap()
    else {
        panic!("probe expected")
    };
    state.hub.probe_reply(
        "host",
        reg.conn_id,
        hennery_proto::frames::HostFrame::Catalog {
            request_id: second_id,
            catalog: catalog(),
        },
    );
    assert!(second.await.unwrap().is_ok());
    assert!(
        !first.is_finished(),
        "second result must be available while first is blocked"
    );
    state.hub.probe_reply(
        "host",
        reg.conn_id,
        hennery_proto::frames::HostFrame::Catalog {
            request_id: first_id,
            catalog: catalog(),
        },
    );
    assert!(first.await.unwrap().is_ok());
}

fn worker_probe_fixture(
    w: &World,
) -> (
    hennery_sessions::AppState,
    tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>,
) {
    let state = hennery_sessions::AppState::new(
        Store::open(&w.db).unwrap(),
        Hosts::open(&w.db).unwrap(),
        Operator::open(&w.db).unwrap(),
    );
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let reg = state
        .hub
        .register(
            "host",
            tx,
            Capabilities(vec![
                Capability::McpServers,
                Capability::DelegationConfig,
                Capability::CatalogProbe,
                Capability::ResolvePath,
            ]),
            AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into()),
        )
        .unwrap();
    state.hub.mark_ready("host", reg.conn_id);
    (state, rx)
}
async fn drain_count_catalogs(
    state: &hennery_sessions::AppState,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>,
    catalog: HostCatalog,
) -> usize {
    drain_catalogs_with_hook(state, rx, catalog, || {}).await
}
async fn drain_catalogs_with_hook(
    state: &hennery_sessions::AppState,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>,
    catalog: HostCatalog,
    mut hook: impl FnMut(),
) -> usize {
    let work = hennery_sessions::delegation_worker::drain(state);
    tokio::pin!(work);
    let mut probes = 0;
    loop {
        tokio::select! {
            result=&mut work=>{result.unwrap();return probes;},
            frame=rx.recv()=>{
                let reply=match frame.unwrap() {
                    CollectorFrame::ResolvePath{request_id,path}=>Some(hennery_proto::frames::HostFrame::ResolvedPath{request_id,canonical:path,exists:true,is_dir:true}),
                    CollectorFrame::ProbeCatalog{request_id,..}=>{probes+=1;hook();Some(hennery_proto::frames::HostFrame::Catalog{request_id,catalog:catalog.clone()})},
                    _=>None,
                };
                if let Some(reply)=reply {state.hub.probe_reply("host",state.hub.routable_conn("host").unwrap(),reply);}
            }
        }
    }
}
#[tokio::test]
async fn worker_launched_starting_child_never_probes_again() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "waiting-start");
    raw(&w).execute_batch("UPDATE sessions SET lifecycle='starting' WHERE id='child'; UPDATE session_attach_evidence SET launch_sent=1 WHERE session_id='child'; UPDATE session_dispatch_queue SET state='waiting_config' WHERE target_id='child';").unwrap();
    let (state, mut rx) = worker_probe_fixture(&w);
    let mut probes = 0;
    for _ in 0..4 {
        probes += drain_count_catalogs(&state, &mut rx, catalog()).await;
    }
    assert_eq!(
        probes, 0,
        "a handed-off start must wait for host evidence without probing"
    );
    assert_eq!(
        state.store.find_session("child").unwrap().unwrap().lifecycle,
        "starting"
    );
}

#[test]
fn absent_profile_never_maps_cross_runtime_modes() {
    use hennery_proto::delegation_host::RuntimeProfile::{Claude, Codex};
    use hennery_sessions::delegation_runtime::cross_runtime_mode;
    for mode in ["bypassPermissions", "agent-full-access"] {
        for profile in [Some(Claude), Some(Codex), None] {
            assert_eq!(cross_runtime_mode(None, profile, Some(mode)), None);
            assert_eq!(cross_runtime_mode(profile, None, Some(mode)), None);
        }
    }
}

#[tokio::test]
async fn legacy_parent_profile_and_unoffered_same_runtime_mode_use_defaults_without_probing_parent() {
    for (agent, offer_bypass, expected) in [
        ("claude", true, "bypassPermissions"),
        ("claude", false, "default"),
        ("worker", true, "default"),
    ] {
        let (w, p) = pair();
        let mut c = catalog();
        c.config_options[1]["options"] =
            json!([{"value":"default"},{"value":"bypassPermissions"},{"value":"agent-full-access"}]);
        c.defaults.mode = Some("default".into());
        w.store
            .ingest(
                "parent",
                2,
                &SessionBody::ConfigApplied {
                    request_id: "set-mode".into(),
                    indexed: Indexed {
                        config_options: Some(c.config_options.clone()),
                        current_mode: Some("bypassPermissions".into()),
                        current_model: Some("wanted".into()),
                        ..Default::default()
                    },
                },
            )
            .unwrap();
        let state = hennery_sessions::AppState::new(
            Store::open(&w.db).unwrap(),
            Hosts::open(&w.db).unwrap(),
            Operator::open(&w.db).unwrap(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = state
            .hub
            .register(
                "host",
                tx,
                Capabilities(vec![
                    Capability::ResolvePath,
                    Capability::CatalogProbe,
                    Capability::DelegationConfig,
                    Capability::McpServers,
                ]),
                AgentIsolation(
                    [
                        ("claude".into(), McpIsolation::ClaudeStrict),
                        ("worker".into(), McpIsolation::ClaudeStrict),
                    ]
                    .into(),
                ),
            )
            .unwrap();
        state.hub.mark_ready("host", reg.conn_id);
        let spawn = hennery_sessions::session_tools::execute(
            &state,
            &p,
            1,
            "session_spawn",
            json!({"operation_key":"legacy-mode","cwd":"/project","agent":agent,"brief":"work","config":{}}),
        );
        tokio::pin!(spawn);
        let mut probes = Vec::new();
        let receipt = loop {
            tokio::select! {
                result=&mut spawn=>break result.unwrap(),
                frame=rx.recv()=>{
                    let reply=match frame.unwrap() {
                        CollectorFrame::ResolvePath { request_id,path } => hennery_proto::frames::HostFrame::ResolvedPath { request_id,canonical:path,exists:true,is_dir:true },
                        CollectorFrame::ProbeCatalog { request_id,agent:requested,.. } => {
                            probes.push(requested.clone());
                            let mut reply=c.clone(); reply.agent=requested.clone();
                            if !offer_bypass { reply.config_options[1]["options"]=json!([{"value":"default"}]); }
                            hennery_proto::frames::HostFrame::Catalog { request_id,catalog:reply }
                        },
                        other=>panic!("unexpected frame: {other:?}"),
                    };
                    state.hub.probe_reply("host",reg.conn_id,reply);
                }
            }
        };
        let child = receipt["child_id"].as_str().unwrap();
        assert_eq!(
            state
                .store
                .child_status(&p, 1, child, None)
                .unwrap()
                .requested
                .mode
                .as_deref(),
            Some(expected)
        );
        assert_eq!(probes, vec![agent], "profile lookup must not issue another probe");
    }
}
#[tokio::test]
async fn worker_dropped_model_followup_fails_after_one_probe_and_stays_parked() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "dropped-model");
    raw(&w)
        .execute("UPDATE sessions SET lifecycle='parked' WHERE id='child'", [])
        .unwrap();
    let (state, mut rx) = worker_probe_fixture(&w);
    let mut removed = catalog();
    removed.config_options.retain(|o| o["id"] != "model");
    assert_eq!(drain_count_catalogs(&state, &mut rx, removed.clone()).await, 1);
    let op = w.store.delegation_receipt(&p, 1, "dropped-model").unwrap().unwrap();
    assert_eq!(
        op.state,
        OperationState::Failed {
            code: "configuration_unavailable".into()
        }
    );
    assert_eq!(w.store.delegation_queue_depth("child").unwrap().0, 0);
    assert_eq!(w.store.find_session("child").unwrap().unwrap().lifecycle, "parked");
    assert_eq!(
        drain_count_catalogs(&state, &mut rx, removed).await,
        0,
        "settled work must leave the probe loop"
    );
}
#[tokio::test]
async fn worker_missing_resume_record_fails_without_a_catalog_probe() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "missing-record");
    raw(&w)
        .execute(
            "UPDATE sessions SET lifecycle='parked',agent_session_id=NULL WHERE id='child'",
            [],
        )
        .unwrap();
    let (state, mut rx) = worker_probe_fixture(&w);
    assert_eq!(drain_count_catalogs(&state, &mut rx, catalog()).await, 0);
    assert_eq!(
        w.store
            .delegation_receipt(&p, 1, "missing-record")
            .unwrap()
            .unwrap()
            .state,
        OperationState::Failed {
            code: "agent_has_no_record".into()
        }
    );
    assert_eq!(w.store.find_session("child").unwrap().unwrap().lifecycle, "parked");
}

#[tokio::test]
async fn worker_unsupported_attach_fails_without_a_catalog_probe() {
    let (w, p) = pair();
    ready(&w);
    queue(&w, &p, "unsupported-attach");
    raw(&w)
        .execute("UPDATE sessions SET lifecycle='parked' WHERE id='child'", [])
        .unwrap();
    let (state, _rx) = worker_probe_fixture(&w);
    state.hub.unregister("host", state.hub.routable_conn("host").unwrap());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let reg = state
        .hub
        .register(
            "host",
            tx,
            Capabilities(vec![
                Capability::McpServers,
                Capability::CatalogProbe,
                Capability::ResolvePath,
            ]),
            AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into()),
        )
        .unwrap();
    state.hub.mark_ready("host", reg.conn_id);
    assert_eq!(drain_count_catalogs(&state, &mut rx, catalog()).await, 0);
    assert_eq!(
        w.store
            .delegation_receipt(&p, 1, "unsupported-attach")
            .unwrap()
            .unwrap()
            .state,
        OperationState::Failed {
            code: "configuration_unavailable".into()
        }
    );
}

#[tokio::test]
async fn worker_notice_configuration_refusal_is_fenced_to_the_probed_head() {
    let (w, _p) = pair();
    ready_parent(&w);
    raw(&w)
        .execute("UPDATE sessions SET model='wanted' WHERE id='parent'", [])
        .unwrap();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    raw(&w)
        .execute("UPDATE sessions SET lifecycle='parked' WHERE id='parent'", [])
        .unwrap();
    let (state, mut rx) = worker_probe_fixture(&w);
    let replacement = uuid::Uuid::now_v7().to_string();
    let mut offered = catalog();
    offered.config_options[0]["options"] = json!([{"value":"wrong"}]);
    assert_eq!(
        drain_catalogs_with_hook(&state, &mut rx, offered, || {
            raw(&w)
                .execute(
                    "UPDATE delegation_notices SET delivery_id=?1,required_config=?2 WHERE parent_id='parent'",
                    params![replacement, serde_json::to_string(&config("wrong")).unwrap()],
                )
                .unwrap();
        })
        .await,
        1
    );
    let actual: (String, Option<String>) = raw(&w)
        .query_row(
            "SELECT state,reason FROM delegation_notices WHERE delivery_id=?1",
            [replacement],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        actual,
        ("parent_queued".into(), None),
        "a probe for the replaced head cannot refuse its successor"
    );
    assert_eq!(w.store.find_session("parent").unwrap().unwrap().lifecycle, "parked");
}
#[tokio::test]
async fn worker_notice_dropped_model_requires_operator_after_one_probe() {
    let (w, _p) = pair();
    ready_parent(&w);
    raw(&w)
        .execute("UPDATE sessions SET model='wanted' WHERE id='parent'", [])
        .unwrap();
    pending(&w, "child", 2, "question", PendingKind::Elicitation);
    raw(&w)
        .execute("UPDATE sessions SET lifecycle='parked' WHERE id='parent'", [])
        .unwrap();
    let (state, mut rx) = worker_probe_fixture(&w);
    let mut removed = catalog();
    removed.config_options.retain(|o| o["id"] != "model");
    assert_eq!(drain_count_catalogs(&state, &mut rx, removed.clone()).await, 1);
    let actual: (String, Option<String>) = raw(&w)
        .query_row(
            "SELECT state,reason FROM delegation_notices WHERE parent_id='parent'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        actual,
        ("operator_required".into(), Some("configuration_unavailable".into()))
    );
    assert_eq!(drain_count_catalogs(&state, &mut rx, removed).await, 0);
    assert_eq!(w.store.find_session("parent").unwrap().unwrap().lifecycle, "parked");
}
