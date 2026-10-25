use base64::{Engine, engine::general_purpose::STANDARD};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_proto::{
    delegation_host::{CatalogFreshness, ConfigCompleteness, ConfigEvidence, HostCatalog, SecurityClassification},
    frames::{
        AgentIsolation, Capabilities, Capability, CollectorFrame, Indexed, ParkReason, PendingExtract, PendingKind,
        PendingResolution, SessionBody, SessionConfig, TurnOutcome,
    },
};
use hennery_sessions::{
    content,
    hub::Hub,
    store::{McpContext, Store},
};
use rusqlite::{Connection, params};
use serde_json::json;
use std::{cell::Cell, sync::Arc};

struct World {
    dir: tempfile::TempDir,
    store: Arc<Store>,
    seq: Cell<u64>,
}
impl World {
    fn complete_mailbox(&self) {
        self.conn()
            .execute(
                "UPDATE hosts SET capabilities='[\"prompt_mailbox_snapshot\"]' WHERE id='host' AND owner_id=?1",
                [self.store.owner_id()],
            )
            .unwrap();
    }
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        hosts
            .register(
                "host",
                &Enrollment {
                    public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                    name: "fixture".into(),
                    host_version: "fixture".into(),
                    platform: "fixture".into(),
                },
                0,
            )
            .unwrap();
        let hat = hosts.host("host").unwrap().unwrap().default_hat_id;
        let store = Arc::new(Store::open(&db).unwrap());
        store
            .create_session("session", "host", "fake", "/project", &hat, None)
            .unwrap();
        let world = Self {
            dir,
            store,
            seq: Cell::new(0),
        };
        world.emit(SessionBody::session_started("start", "agent-session"));
        world
    }
    fn conn(&self) -> Connection {
        Connection::open(self.dir.path().join("hennery.db")).unwrap()
    }
    fn emit(&self, body: SessionBody) {
        self.seq.set(self.seq.get() + 1);
        self.store.ingest("session", self.seq.get(), &body).unwrap();
    }
    fn queue(&self, text: &str) -> String {
        self.store.enqueue_operator_prompt("session", &checked(text)).unwrap()
    }
    fn operation(&self, request: &str) -> String {
        self.conn()
            .query_row(
                "SELECT operation_id FROM session_operations WHERE owner_id=?1 AND request_id=?2",
                params![self.store.owner_id(), request],
                |r| r.get(0),
            )
            .unwrap()
    }
    fn start(&self, request: &str, turn: &str) {
        self.emit(SessionBody::TurnStarted {
            request_id: request.into(),
            turn_id: turn.into(),
        });
    }
    fn end(&self, turn: &str, outcome: TurnOutcome) {
        self.emit(SessionBody::TurnEnded {
            turn_id: turn.into(),
            outcome,
            stop_reason: None,
            error: None,
        });
    }
    fn dispatch(&self, hub: &Hub) -> bool {
        self.store
            .dispatch_operator_prompt("session", hub, &mut None, None)
            .unwrap()
    }
}
fn checked(text: &str) -> content::Checked {
    content::check(vec![json!({"type":"text","text":text})]).unwrap()
}
fn operator_catalog() -> HostCatalog {
    HostCatalog {
        saved_at: None,
        agent: "fake".into(),
        adapter_revision: "fixture".into(),
        freshness: CatalogFreshness::Fresh,
        config_options: vec![],
        defaults: SessionConfig::default(),
        security: SecurityClassification {
            known: true,
            axes: Default::default(),
        },
    }
}
fn host(caps: Vec<Capability>) -> (Arc<Hub>, tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>) {
    let hub = Arc::new(Hub::new());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let reg = hub
        .register("host", tx, Capabilities(caps), AgentIsolation::default())
        .unwrap();
    hub.mark_ready("host", reg.conn_id);
    (hub, rx)
}
fn prompt(rx: &mut tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>) -> (String, String, Vec<serde_json::Value>) {
    let CollectorFrame::Prompt {
        request_id,
        turn_id,
        content,
        ..
    } = rx.try_recv().unwrap()
    else {
        panic!("expected prompt")
    };
    (request_id, turn_id, content)
}

fn config_snapshot(
    actual: &SessionConfig,
    generation: u64,
    revision: u64,
    completeness: ConfigCompleteness,
) -> Indexed {
    Indexed {
        config_options: Some(vec![json!({"id":"model","currentValue":actual.model})]),
        current_model: actual.model.clone(),
        current_mode: actual.mode.clone(),
        current_axes: Some(actual.axes.clone()),
        config_evidence: Some(Box::new(ConfigEvidence {
            attach_generation: generation,
            config_revision: revision,
            completeness,
            actual: actual.clone(),
        })),
        ..Default::default()
    }
}

#[test]
fn operator_model_switch_replaces_launch_expectations_and_repairs_a_stale_mismatch() {
    for expire_first in [false, true] {
        let w = World::new();
        let (hub, mut rx) = host(vec![]);
        let wanted = SessionConfig {
            model: Some("model-a".into()),
            mode: Some("default".into()),
            axes: [("fast".into(), hennery_proto::frames::ConfigValue::Bool(false))].into(),
        };
        w.store.expect_initial_configuration("session", &wanted).unwrap();
        w.emit(SessionBody::AcpUpdate {
            indexed: config_snapshot(&wanted, 0, 1, ConfigCompleteness::Complete),
            payload: json!({}),
        });
        assert!(w.store.open_prompt("session", "first", &checked("first")).unwrap());
        w.start("first", "first");
        let mut queued = w.queue("follow up after switching models");
        let actual = SessionConfig {
            model: Some("model-b".into()),
            mode: wanted.mode.clone(),
            ..Default::default()
        };
        // A model change can also remove an axis from the adapter's catalogue.
        w.emit(SessionBody::AcpUpdate {
            indexed: config_snapshot(&actual, 0, 2, ConfigCompleteness::Complete),
            payload: json!({}),
        });
        if expire_first {
            w.store
                .refresh_delegation_attention(&hub, hennery_kernel::secret::unix_now() + 31)
                .unwrap();
            assert!(
                w.store
                    .find_session("session")
                    .unwrap()
                    .unwrap()
                    .configuration_error
                    .is_some()
            );
        }
        w.emit(SessionBody::ConfigApplied {
            request_id: "model-switch".into(),
            indexed: config_snapshot(&actual, 0, 3, ConfigCompleteness::Complete),
        });
        w.store
            .refresh_delegation_attention(&hub, hennery_kernel::secret::unix_now() + 31)
            .unwrap();
        let session = w.store.find_session("session").unwrap().unwrap();
        assert_eq!(session.config.model.as_deref(), Some("model-b"));
        assert!(session.configuration_error.is_none(), "{session:?}");
        let expected: String = w
            .conn()
            .query_row(
                "SELECT requested_config FROM session_attach_evidence WHERE session_id='session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<SessionConfig>(&expected).unwrap(), actual);
        if expire_first {
            // A failed queued message stays visible; repair must not resend it.
            let failed = w.store.operator_prompt_queue("session").unwrap();
            assert_eq!(failed[0].state, "failed");
            queued = w.queue("resubmitted after configuration repair");
        }
        w.end("first", TurnOutcome::Completed);
        assert!(w.dispatch(&hub));
        assert_eq!(w.operation(&prompt(&mut rx).0), queued);
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn unconfirmed_or_stale_operator_switch_does_not_replace_launch_expectations() {
    for (generation, revision, completeness) in [
        (0, 2, ConfigCompleteness::Unknown),
        (0, 2, ConfigCompleteness::Partial),
        (1, 2, ConfigCompleteness::Complete),
        (0, 1, ConfigCompleteness::Complete),
    ] {
        let w = World::new();
        let wanted = SessionConfig {
            model: Some("model-a".into()),
            ..Default::default()
        };
        w.store.expect_initial_configuration("session", &wanted).unwrap();
        w.emit(SessionBody::AcpUpdate {
            indexed: config_snapshot(&wanted, 0, 1, ConfigCompleteness::Complete),
            payload: json!({}),
        });
        let actual = SessionConfig {
            model: Some("model-b".into()),
            ..Default::default()
        };
        w.emit(SessionBody::ConfigApplied {
            request_id: "unconfirmed-switch".into(),
            indexed: config_snapshot(&actual, generation, revision, completeness),
        });
        let expected: String = w
            .conn()
            .query_row(
                "SELECT requested_config FROM session_attach_evidence WHERE session_id='session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<SessionConfig>(&expected).unwrap(), wanted);
    }
}

#[test]
fn queue_waits_for_turn_end_and_delivers_fifo_as_operator() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    assert!(w.store.open_prompt("session", "running", &checked("first")).unwrap());
    w.start("first", "running");
    let head = w.queue("second");
    let tail = w.queue("third");
    assert!(!w.dispatch(&hub));
    assert!(rx.try_recv().is_err());
    w.end("running", TurnOutcome::Completed);
    assert!(w.dispatch(&hub));
    let (request, turn, content) = prompt(&mut rx);
    assert_eq!(w.operation(&request), head);
    assert_eq!(content[0]["text"], "second");
    assert!(!w.dispatch(&hub));
    w.start(&request, &turn);
    let events = w.store.events("session", 0, 100).unwrap();
    let user = events.iter().rev().find(|e| e.kind == "user_turn").unwrap();
    assert_eq!(user.body["origin"]["kind"], "operator");
    w.end(&turn, TurnOutcome::Completed);
    assert!(w.dispatch(&hub));
    assert_eq!(w.operation(&prompt(&mut rx).0), tail);
}

#[test]
fn pending_permission_never_consumes_queue_even_without_an_open_turn() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    w.emit(SessionBody::PendingOpened {
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
        payload: json!({"options":[{"optionId":"allow","kind":"allow_once","name":"Allow"}]}),
    });
    let id = w.queue("later");
    assert!(!w.dispatch(&hub));
    assert!(rx.try_recv().is_err());
    assert_eq!(w.store.operator_prompt_queue("session").unwrap()[0].id, id);
    w.emit(SessionBody::PendingResolved {
        pending_id: "permission".into(),
        resolution: PendingResolution::Delivered,
        reason: None,
    });
    assert!(w.dispatch(&hub));
    assert_eq!(w.operation(&prompt(&mut rx).0), id);
}

#[test]
fn edit_and_remove_preserve_fifo_and_do_not_touch_the_open_turn() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    let first = w.queue("one");
    let removed = w.queue("remove");
    let third = w.queue("three");
    w.store.edit_operator_prompt("session", &first, "edited").unwrap();
    assert!(w.store.remove_operator_prompt("session", &removed).unwrap());
    assert!(w.dispatch(&hub));
    assert_eq!(prompt(&mut rx).2[0]["text"], "edited");
    assert!(w.store.edit_operator_prompt("session", &first, "too late").is_err());
    assert!(!w.store.remove_operator_prompt("session", &first).unwrap());
    assert_eq!(w.store.operator_prompt_queue("session").unwrap()[1].id, third);
}

#[test]
fn cancelled_turn_keeps_and_delivers_the_next_message() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    w.store.open_prompt("session", "running", &checked("first")).unwrap();
    w.start("first", "running");
    let queued = w.queue("next");
    w.end("running", TurnOutcome::Cancelled);
    assert!(w.dispatch(&hub));
    assert_eq!(w.operation(&prompt(&mut rx).0), queued);
}

#[test]
fn failed_and_closed_sessions_keep_unsent_text_and_readable_reasons() {
    for close in [false, true] {
        let w = World::new();
        w.queue("keep these words");
        if close {
            w.store.close_now("session").unwrap();
        } else {
            w.store.mark_failed("session", "agent_not_logged_in").unwrap();
        }
        let queue = w.store.operator_prompt_queue("session").unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].state, "failed");
        assert!(
            queue[0]
                .reason
                .as_deref()
                .unwrap()
                .contains(if close { "closed" } else { "signed out" })
        );
        assert_eq!(
            serde_json::to_value(&queue[0].content).unwrap()[0]["text"],
            "keep these words"
        );
        assert!(w.store.enqueue_operator_prompt("session", &checked("refuse")).is_err());
    }
}

#[test]
fn queue_survives_restart_and_inflight_association_is_not_replayed() {
    let w = World::new();
    let id = w.queue("restart me");
    let (hub, mut rx) = host(vec![]);
    let reopened = Store::open(&w.dir.path().join("hennery.db")).unwrap();
    assert_eq!(reopened.operator_prompt_queue("session").unwrap()[0].id, id);
    assert!(
        reopened
            .dispatch_operator_prompt("session", &hub, &mut None, None)
            .unwrap()
    );
    let (request, turn, _) = prompt(&mut rx);
    let restarted = Store::open(&w.dir.path().join("hennery.db")).unwrap();
    assert!(
        !restarted
            .dispatch_operator_prompt("session", &hub, &mut None, None)
            .unwrap()
    );
    assert!(rx.try_recv().is_err());
    w.start(&request, &turn);
    assert!(restarted.operator_prompt_queue("session").unwrap().is_empty());
}

#[test]
fn queue_changes_are_durable_events_seen_by_every_window() {
    let w = World::new();
    let (hub, _rx) = host(vec![]);
    let mut first = hub.subscribe();
    let mut second = hub.subscribe();
    let id = w.queue("one");
    w.store.flush_prompt_queue_changes(&hub).unwrap();
    assert_eq!(first.try_recv().unwrap().kind, "queue_changed");
    assert_eq!(second.try_recv().unwrap().kind, "queue_changed");
    w.store.edit_operator_prompt("session", &id, "two").unwrap();
    w.store.flush_prompt_queue_changes(&hub).unwrap();
    assert_eq!(first.try_recv().unwrap().kind, "queue_changed");
    assert_eq!(second.try_recv().unwrap().kind, "queue_changed");
    w.store.remove_operator_prompt("session", &id).unwrap();
    w.store.flush_prompt_queue_changes(&hub).unwrap();
    assert_eq!(first.try_recv().unwrap().kind, "queue_changed");
    assert_eq!(second.try_recv().unwrap().kind, "queue_changed");
}

#[test]
fn queue_bounds_include_images_edits_and_the_shared_parent_fifo() {
    let w = World::new();
    for _ in 0..32 {
        w.queue("small");
    }
    assert_eq!(
        w.store
            .enqueue_operator_prompt("session", &checked("overflow"))
            .unwrap_err()
            .to_string(),
        "queue_full"
    );
    w.store.mark_failed("session", "agent_not_logged_in").unwrap();
    let w = World::new();
    let id = w.queue("small");
    assert_eq!(
        w.store
            .edit_operator_prompt("session", &id, &"x".repeat(1024 * 1024))
            .unwrap_err()
            .to_string(),
        "queue_full"
    );
    let mut image = b"\x89PNG\r\n\x1a\n".to_vec();
    image.resize(1024 * 1024, 0);
    let image = content::check(vec![
        json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(image)}),
    ])
    .unwrap();
    assert_eq!(
        w.store
            .enqueue_operator_prompt("session", &image)
            .unwrap_err()
            .to_string(),
        "queue_full"
    );
}

#[test]
fn queued_images_survive_sweep_edit_and_delivery_and_are_cleaned_on_removal() {
    let w = World::new();
    let (hub, mut rx) = host(vec![Capability::Images]);
    let bytes = b"\x89PNG\r\n\x1a\nqueued";
    let image = content::check(vec![
        json!({"type":"text","text":"image"}),
        json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(bytes)}),
    ])
    .unwrap();
    let sha = image.images[0].sha256.clone();
    let id = w.store.enqueue_operator_prompt("session", &image).unwrap();
    w.store
        .sweep_attachments(
            std::time::SystemTime::now() + std::time::Duration::from_secs(7200),
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();
    assert!(w.store.attachment(&sha).unwrap().is_some());
    w.store.edit_operator_prompt("session", &id, "edited").unwrap();
    assert!(w.dispatch(&hub));
    let (request, turn, sent) = prompt(&mut rx);
    assert_eq!(sent[1]["data"], STANDARD.encode(bytes));
    w.start(&request, &turn);
    w.end(&turn, TurnOutcome::Completed);
    assert!(w.store.attachment(&sha).unwrap().is_some());
    let other = content::check(vec![
        json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(b"\x89PNG\r\n\x1a\nother")}),
    ])
    .unwrap();
    let sha = other.images[0].sha256.clone();
    let id = w.store.enqueue_operator_prompt("session", &other).unwrap();
    assert!(w.store.remove_operator_prompt("session", &id).unwrap());
    assert!(w.store.attachment(&sha).unwrap().is_none());
}

#[test]
fn runtime_rejection_stops_the_queue_and_keeps_all_messages() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    let id = w.queue("first");
    w.queue("second");
    assert!(w.dispatch(&hub));
    let (request, _turn, _) = prompt(&mut rx);
    assert!(
        !w.store
            .reject_operator_prompt("foreign", &request, "auth_required")
            .unwrap()
    );
    assert!(
        w.store
            .reject_operator_prompt("host", &request, "auth_required")
            .unwrap()
    );
    let queue = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(queue[0].id, id);
    assert_eq!(queue.len(), 2);
    assert!(
        queue
            .iter()
            .all(|q| q.state == "failed" && q.reason.as_deref().unwrap().contains("signed out"))
    );
    assert!(!w.dispatch(&hub));
}

#[test]
fn operator_prompts_cannot_overtake_a_shared_parent_fifo_head() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    let owner = w.store.owner_id();
    let conn = w.conn();
    let hat = w.store.find_session("session").unwrap().unwrap().hat_id;
    w.store
        .create_session("parent", "host", "fake", "/project", &hat, None)
        .unwrap();
    conn.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state) VALUES (?1,'parent','parent-head','parent-head','','session_send','session','queued')",[owner]).unwrap();
    conn.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES (?1,'parent-head','parent','session',1,'parent first','{}')",[owner]).unwrap();
    w.queue("operator second");
    assert!(!w.dispatch(&hub));
    assert!(!w.store.open_prompt("session", "overtake", &checked("direct")).unwrap());
    assert!(rx.try_recv().is_err());
}

#[test]
fn a_parent_origin_row_cannot_gain_operator_authority_from_equal_source_and_target() {
    let w = World::new();
    let (hub, mut rx) = host(vec![Capability::DelegationConfig, Capability::OperatorResume]);
    let owner = w.store.owner_id();
    let conn = w.conn();
    conn.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state) VALUES (?1,'session','forged-parent','forged-parent','','session_send','session','queued')",[owner]).unwrap();
    conn.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config,origin) VALUES (?1,'forged-parent','session','session',0,'parent without grant','{}','parent')",[owner]).unwrap();
    w.queue("operator behind the parent");
    w.emit(SessionBody::SessionParked {
        reason: ParkReason::Idle,
    });
    let catalog = HostCatalog {
        saved_at: None,
        agent: "fake".into(),
        adapter_revision: "fixture".into(),
        freshness: CatalogFreshness::Fresh,
        config_options: vec![],
        defaults: SessionConfig::default(),
        security: SecurityClassification {
            known: true,
            axes: Default::default(),
        },
    };
    assert!(
        !w.store
            .attach_delegated("session", McpContext::NONE, &catalog, &hub)
            .unwrap()
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(w.store.find_session("session").unwrap().unwrap().lifecycle, "parked");
    assert_eq!(w.store.operator_prompt_queue("session").unwrap()[0].state, "queued");
}

#[test]
fn grant_revision_changes_do_not_cancel_operator_input() {
    let w = World::new();
    w.queue("operator");
    let conn = w.conn();
    let owner = w.store.owner_id();
    let hat = w.store.find_session("session").unwrap().unwrap().hat_id;
    conn.execute(
        "INSERT INTO hat_session_tools(owner_id,hat_id,delegation_enabled,revision) VALUES (?1,?2,1,1)",
        params![owner, hat],
    )
    .unwrap();
    conn.execute(
        "UPDATE hat_session_tools SET revision=2 WHERE owner_id=?1 AND hat_id=?2",
        params![owner, hat],
    )
    .unwrap();
    assert_eq!(w.store.operator_prompt_queue("session").unwrap()[0].state, "queued");
}

#[test]
fn two_workers_dispatch_each_queue_head_exactly_once() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    let id = w.queue("only once");
    let mut threads = Vec::new();
    for _ in 0..8 {
        let store = w.store.clone();
        let hub = hub.clone();
        threads.push(std::thread::spawn(move || {
            store
                .dispatch_operator_prompt("session", &hub, &mut None, None)
                .unwrap()
        }));
    }
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|sent| *sent)
            .count(),
        1
    );
    let (request, turn, _) = prompt(&mut rx);
    assert_eq!(w.operation(&request), id);
    assert!(rx.try_recv().is_err());
    w.start(&request, &turn);
    w.end(&turn, TurnOutcome::Completed);
    assert!(w.store.operator_prompt_queue("session").unwrap().is_empty());
    assert!(!w.dispatch(&hub));
}

#[test]
fn parked_operator_queue_uses_the_existing_attach_generation_and_confirmed_configuration() {
    let w = World::new();
    let (hub, mut rx) = host(vec![Capability::DelegationConfig, Capability::OperatorResume]);
    w.emit(SessionBody::SessionParked {
        reason: ParkReason::Idle,
    });
    let id = w.queue("resume then deliver");
    let catalog = HostCatalog {
        saved_at: None,
        agent: "fake".into(),
        adapter_revision: "fixture".into(),
        freshness: CatalogFreshness::Fresh,
        config_options: vec![],
        defaults: SessionConfig::default(),
        security: SecurityClassification {
            known: true,
            axes: Default::default(),
        },
    };
    assert!(
        w.store
            .attach_delegated("session", McpContext::NONE, &catalog, &hub)
            .unwrap()
    );
    let CollectorFrame::DelegatedAttach {
        attach_generation,
        agent_session_id,
        ..
    } = rx.try_recv().unwrap()
    else {
        panic!("attach")
    };
    assert_eq!(agent_session_id.as_deref(), Some("agent-session"));
    assert!(!w.dispatch(&hub));
    let mut started = SessionBody::session_started("resumed", "agent-session");
    if let SessionBody::SessionStarted { indexed, .. } = &mut started {
        indexed.config_evidence = Some(Box::new(ConfigEvidence {
            attach_generation,
            config_revision: 1,
            completeness: ConfigCompleteness::Complete,
            actual: SessionConfig::default(),
        }));
    }
    w.emit(started);
    assert!(!w.dispatch(&hub));
    assert!(
        w.store
            .dispatch_operator_prompt("session", &hub, &mut None, Some(&catalog))
            .unwrap()
    );
    assert_eq!(w.operation(&prompt(&mut rx).0), id);
}

#[tokio::test]
async fn competing_workers_drain_after_turn_end_without_adapter_queueing_or_catalog_capabilities() {
    for agent in ["claude", "codex"] {
        let w = World::new();
        w.conn()
            .execute(
                "UPDATE sessions SET agent=?2 WHERE owner_id=?1 AND id='session'",
                params![w.store.owner_id(), agent],
            )
            .unwrap();
        w.store.open_prompt("session", "running", &checked("first")).unwrap();
        w.start("first", "running");
        let id = w.queue("next");
        let db = w.dir.path().join("hennery.db");
        let state = hennery_sessions::AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            hennery_kernel::operator::Operator::open(&db).unwrap(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = state
            .hub
            .register("host", tx, Capabilities::default(), AgentIsolation::default())
            .unwrap();
        state.hub.mark_ready("host", reg.conn_id);
        hennery_sessions::delegation_worker::drain(&state).await.unwrap();
        assert!(rx.try_recv().is_err());
        w.end("running", TurnOutcome::Completed);
        let (one, two) = tokio::join!(
            hennery_sessions::delegation_worker::drain(&state),
            hennery_sessions::delegation_worker::drain(&state)
        );
        one.unwrap();
        two.unwrap();
        let (request, turn, _) = prompt(&mut rx);
        assert_eq!(w.operation(&request), id);
        assert!(rx.try_recv().is_err());
        w.start(&request, &turn);
        w.end(&turn, TurnOutcome::Completed);
        assert!(state.store.operator_prompt_queue("session").unwrap().is_empty());
        state.shutdown.cancel();
    }
}

#[test]
fn a_failed_turn_stops_pending_operator_prompts_and_dismissal_cleans_their_images() {
    let w = World::new();
    w.store.open_prompt("session", "running", &checked("first")).unwrap();
    w.start("first", "running");
    let image = content::check(vec![
        json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(b"\x89PNG\r\n\x1a\nfailed")}),
    ])
    .unwrap();
    let sha = image.images[0].sha256.clone();
    let id = w.store.enqueue_operator_prompt("session", &image).unwrap();
    w.end("running", TurnOutcome::Failed);
    let queue = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(queue[0].state, "failed");
    assert!(queue[0].reason.as_deref().unwrap().contains("previous turn failed"));
    assert!(w.store.remove_operator_prompt("session", &id).unwrap());
    assert!(w.store.attachment(&sha).unwrap().is_none());
    assert!(!w.dir.path().join("attachments").join(&sha).exists());
}

#[tokio::test]
async fn parked_resume_refusals_keep_every_message_with_a_readable_reason() {
    for missing_record in [true, false] {
        let w = World::new();
        w.emit(SessionBody::SessionParked {
            reason: ParkReason::Idle,
        });
        if missing_record {
            w.conn()
                .execute(
                    "UPDATE sessions SET agent_session_id=NULL WHERE owner_id=?1 AND id='session'",
                    [w.store.owner_id()],
                )
                .unwrap();
        }
        w.queue("keep the first message");
        w.queue("keep the second message");
        let db = w.dir.path().join("hennery.db");
        let state = hennery_sessions::AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            hennery_kernel::operator::Operator::open(&db).unwrap(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let capabilities = if missing_record {
            vec![Capability::DelegationConfig, Capability::OperatorResume]
        } else {
            vec![]
        };
        let reg = state
            .hub
            .register("host", tx, Capabilities(capabilities), AgentIsolation::default())
            .unwrap();
        state.hub.mark_ready("host", reg.conn_id);
        hennery_sessions::delegation_worker::drain(&state).await.unwrap();
        let queue = state.store.operator_prompt_queue("session").unwrap();
        assert_eq!(queue.len(), 2);
        for (item, expected) in queue.iter().zip(["keep the first message", "keep the second message"]) {
            assert_eq!(item.state, "failed");
            assert_eq!(item.content, checked(expected).stored);
            assert!(item.reason.as_deref().unwrap().contains(if missing_record {
                "no longer has this session"
            } else {
                "0.3.0"
            }));
        }
        assert!(rx.try_recv().is_err());
        state.shutdown.cancel();
    }
}

#[test]
fn session_delete_scrubs_queued_text_and_removes_queue_only_images() {
    let w = World::new();
    let image = content::check(vec![
        json!({"type":"text","text":"erase queued text"}),
        json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(b"\x89PNG\r\n\x1a\ndelete")}),
    ])
    .unwrap();
    let sha = image.images[0].sha256.clone();
    w.store.enqueue_operator_prompt("session", &image).unwrap();
    let file = w.dir.path().join("attachments").join(&sha);
    assert!(file.exists());
    w.store.close_now("session").unwrap();
    let result = w.store.delete_session("session", None).unwrap();
    assert!(matches!(result, hennery_sessions::store::Deletion::Done { .. }));
    assert!(w.store.operator_prompt_queue("session").unwrap().is_empty());
    assert!(w.store.attachment(&sha).unwrap().is_none());
    assert!(!file.exists());
    let conn = w.conn();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_operations WHERE owner_id=?1 AND caller_id='session'",
            [w.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    let retained: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_dispatch_queue WHERE owner_id=?1 AND (source_id='session' OR target_id='session' OR payload LIKE '%erase queued text%')",
            [w.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(retained, 0);
}

// Review B1: these two paths reproduce the independent reviewer's lost-head probes.
#[test]
fn reconnect_requeues_a_proven_unsent_head_before_any_later_prompt() {
    lost_head_is_retried_in_order(false);
}

#[test]
fn late_slot_takeover_does_not_retry_a_displaced_queue_attempt_still_in_the_mailbox() {
    let w = World::new();
    w.complete_mailbox();
    let (hub, mut rx) = host(vec![]);
    w.store
        .open_prompt("session", "original", &checked("original immediate prompt"))
        .unwrap();
    w.store
        .reconcile_host(
            "host",
            &[serde_json::from_value(json!({
                "session_id":"session", "last_seq":w.seq.get()
            }))
            .unwrap()],
        )
        .unwrap();
    let id = w.queue("displaced queue attempt");
    assert!(w.dispatch(&hub));
    let (request, turn, _) = prompt(&mut rx);
    w.start("original-request", "original");
    w.end("original", TurnOutcome::Completed);
    assert!(!w.dispatch(&hub), "slot takeover is not an absent-mailbox proof");
    assert!(
        rx.try_recv().is_err(),
        "the displaced attempt may still be read by the actor"
    );
    let q = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(q[0].id, id);
    assert_eq!(q[0].state, "failed");
    assert!(q[0].reason.as_deref().is_some_and(|reason| !reason.is_empty()));
    w.start(&request, &turn);
    let state: String = w
        .conn()
        .query_row(
            "SELECT state FROM session_operations WHERE owner_id=?1 AND operation_id=?2",
            params![w.store.owner_id(), id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        state, "delivered",
        "a later real admission still corrects the retained failure"
    );
}

#[test]
fn legacy_mailbox_snapshots_never_auto_retry_an_unreported_prompt() {
    randomized_legacy_mailbox_interleaving(1);
}

fn randomized_legacy_mailbox_interleaving(seed: u64) {
    let w = World::new();
    // v0.2.2 can advertise operator resume without reporting its mailbox.
    w.conn()
        .execute(
            "UPDATE hosts SET capabilities='[\"operator_resume\",\"delegation_config\"]' WHERE owner_id=?1",
            [w.store.owner_id()],
        )
        .unwrap();
    let (hub, mut rx) = host(vec![]);
    let id = w.queue("original still in the old host mailbox");
    assert!(w.dispatch(&hub));
    let (request, turn, _) = prompt(&mut rx);
    let mut rng = seed;
    for step in 0..12 {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let tail = w.queue(&format!("seed {seed} step {step}"));
        if rng & 1 == 0 {
            w.store.edit_operator_prompt("session", &tail, "edited tail").unwrap();
        }
        if rng & 2 != 0 {
            w.store.remove_operator_prompt("session", &tail).unwrap();
        }
    }
    w.store
        .reconcile_host(
            "host",
            &[hennery_proto::frames::AttachedSession {
                ending: None,
                session_id: "session".into(),
                last_seq: w.seq.get(),
                open_turn_id: None,
            }],
        )
        .unwrap();
    let q = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(q[0].id, id);
    assert!(
        q.iter().all(|q| q.state == "failed"),
        "legacy snapshot must not authorize retries: {q:?}"
    );
    assert!(q[0].reason.as_deref().unwrap().contains("did not confirm delivery"));
    assert!(!w.dispatch(&hub));
    assert!(rx.try_recv().is_err(), "old mailbox plus retry would deliver twice");
    // Its actor can still admit the original after reconnect.
    w.start(&request, &turn);
    let disposition: String = w
        .conn()
        .query_row(
            "SELECT state FROM session_operations WHERE owner_id=?1 AND operation_id=?2",
            params![w.store.owner_id(), id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        disposition, "delivered",
        "late original start repairs the legacy head; seed {seed}"
    );
    w.start(&request, &turn);
    w.end(&turn, TurnOutcome::Completed);
    let users: Vec<_> = w
        .store
        .events("session", 0, 1000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "user_turn")
        .collect();
    assert_eq!(users.len(), 1, "seed {seed}");
    assert_eq!(users[0].body["turn_id"], turn);
    assert!(!w.dispatch(&hub));
}

#[test]
fn a_superseded_queue_attempt_cannot_be_revived_by_a_late_turn_started() {
    let w = World::new();
    w.complete_mailbox();
    let (hub, mut rx) = host(vec![]);
    w.queue("retry once");
    assert!(w.dispatch(&hub));
    let (old_request, old_turn, _) = prompt(&mut rx);
    w.store
        .reconcile_host(
            "host",
            &[hennery_proto::frames::AttachedSession {
                ending: None,
                session_id: "session".into(),
                last_seq: w.seq.get(),
                open_turn_id: None,
            }],
        )
        .unwrap();
    w.start(&old_request, &old_turn);
    let ignored: i64 = w.conn().query_row(
        "SELECT COUNT(*) FROM events WHERE owner_id=?1 AND session_id='session' AND kind='turn_started' AND applied=0 AND json_extract(body,'$.turn_id')=?2",
        params![w.store.owner_id(),old_turn], |r|r.get(0),
    ).unwrap();
    assert_eq!(ignored, 1, "a superseded fact is retained as unapplied");
    assert!(
        w.store
            .events("session", 0, 100)
            .unwrap()
            .iter()
            .all(|e| e.kind != "user_turn")
    );
    let slot: Option<String> = w
        .conn()
        .query_row(
            "SELECT open_turn_id FROM sessions WHERE owner_id=?1 AND id='session'",
            [w.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(slot.is_none());
    assert!(w.dispatch(&hub));
    let (request, turn, _) = prompt(&mut rx);
    w.start(&old_request, &old_turn);
    w.start(&request, &turn);
    let users: Vec<_> = w
        .store
        .events("session", 0, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "user_turn")
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].body["turn_id"], turn);
}

#[test]
fn queue_owned_not_delivered_turns_never_offer_a_parallel_send_again_path() {
    for complete in [false, true] {
        let w = World::new();
        if complete {
            w.complete_mailbox();
        }
        let (hub, mut rx) = host(vec![]);
        w.queue("owned by FIFO");
        assert!(w.dispatch(&hub));
        let (_, turn, _) = prompt(&mut rx);
        let reconciliation = w
            .store
            .reconcile_host(
                "host",
                &[hennery_proto::frames::AttachedSession {
                    ending: None,
                    session_id: "session".into(),
                    last_seq: w.seq.get(),
                    open_turn_id: None,
                }],
            )
            .unwrap();
        let marker = reconciliation
            .events
            .iter()
            .find(|e| e.kind == "turn_not_delivered")
            .unwrap();
        assert_eq!(marker.body["operator_queued"], true);
        assert!(w.store.undelivered_turn("session", &turn).unwrap().is_none());
    }
}

#[test]
fn reconciliation_preserves_a_park_hold_while_the_attached_actor_is_ending() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    w.queue("wait for explicit resume");
    w.store.record_park_request("session").unwrap();
    // Decode the actual optional wire flag: pre-fix readers ignore it.
    let ending = serde_json::from_value(json!({
        "session_id":"session", "last_seq":w.seq.get(), "ending":true
    }))
    .unwrap();
    w.store.reconcile_host("host", &[ending]).unwrap();
    assert!(
        !w.dispatch(&hub),
        "an attached but ending actor must retain the Park hold"
    );
    assert!(rx.try_recv().is_err());
    w.emit(SessionBody::SessionParked {
        reason: hennery_proto::frames::ParkReason::Operator,
    });
    assert!(!w.dispatch(&hub), "the completed explicit Park still holds the queue");
    let hat = w.store.find_session("session").unwrap().unwrap().hat_id;
    w.store.request_resume("session", &hat).unwrap();
    w.emit(SessionBody::session_started("resume", "agent-session"));
    assert!(w.dispatch(&hub));
    assert_eq!(prompt(&mut rx).2[0]["text"], "wait for explicit resume");
}

#[test]
fn reconciliation_releases_an_unapplied_park_hold_when_the_host_remains_attached() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    w.queue("waiting on ambiguous park");
    w.store.record_park_request("session").unwrap();
    assert!(!w.dispatch(&hub));
    w.store
        .reconcile_host(
            "host",
            &[hennery_proto::frames::AttachedSession {
                ending: None,
                session_id: "session".into(),
                last_seq: w.seq.get(),
                open_turn_id: None,
            }],
        )
        .unwrap();
    assert!(w.dispatch(&hub));
    assert_eq!(prompt(&mut rx).2[0]["text"], "waiting on ambiguous park");
}

#[test]
fn host_restart_preserves_an_explicit_park_hold_until_operator_resume() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    w.queue("held across host restart");
    w.store.record_park_request("session").unwrap();
    w.store.reconcile_host("host", &[]).unwrap();
    let held: bool = w
        .conn()
        .query_row(
            "SELECT prompt_queue_held=1 FROM sessions WHERE owner_id=?1 AND id='session'",
            [w.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(held);
    assert!(!w.dispatch(&hub));
    assert!(rx.try_recv().is_err());
    let hat = w.store.find_session("session").unwrap().unwrap().hat_id;
    w.store.request_resume("session", &hat).unwrap();
    w.emit(SessionBody::session_started("resume", "agent-session"));
    assert!(w.dispatch(&hub));
    assert_eq!(prompt(&mut rx).2[0]["text"], "held across host restart");
}

#[test]
fn an_unresolved_operator_receipt_fences_typed_input_even_without_a_turn_slot() {
    let w = World::new();
    let (hub, mut rx) = host(vec![Capability::DelegationConfig, Capability::OperatorResume]);
    w.queue("uncertain head");
    assert!(w.dispatch(&hub));
    prompt(&mut rx);
    let tail = w.queue("later queued input");
    w.conn()
        .execute(
            "UPDATE sessions SET open_turn_id=NULL WHERE owner_id = ?1 AND id='session'",
            [w.store.owner_id()],
        )
        .unwrap();
    assert!(w.store.prompt_must_queue("session").unwrap());
    assert!(!w.store.operator_dispatch_ready("session").unwrap());
    assert!(!w.dispatch(&hub));
    w.conn()
        .execute(
            "UPDATE sessions SET lifecycle='parked' WHERE owner_id = ?1 AND id='session'",
            [w.store.owner_id()],
        )
        .unwrap();
    assert!(
        !w.store
            .attach_delegated("session", McpContext::NONE, &operator_catalog(), &hub)
            .unwrap()
    );
    w.conn()
        .execute(
            "UPDATE sessions SET lifecycle='active' WHERE owner_id = ?1 AND id='session'",
            [w.store.owner_id()],
        )
        .unwrap();
    assert!(w.store.remove_operator_prompt("session", &tail).unwrap());
    assert!(
        !w.store
            .open_prompt("session", "overtake-uncertain", &checked("later input"))
            .unwrap()
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn an_image_io_error_fails_only_that_item_and_keeps_the_next_item_dispatchable() {
    let w = World::new();
    let (hub, mut rx) = host(vec![Capability::Images]);
    let image = content::check(vec![
        json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(b"\x89PNG\r\n\x1a\nread-error")}),
    ])
    .unwrap();
    let path = w.dir.path().join("attachments").join(&image.images[0].sha256);
    let broken = w.store.enqueue_operator_prompt("session", &image).unwrap();
    let next = w.queue("healthy next");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(!w.dispatch(&hub));
    let q = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(q[0].id, broken);
    assert_eq!(q[0].state, "failed");
    assert!(q[0].reason.as_deref().unwrap().contains("image is unavailable"));
    assert_eq!(q[1].state, "queued");
    assert!(w.dispatch(&hub));
    assert_eq!(w.operation(&prompt(&mut rx).0), next);
}

#[test]
fn host_restart_requeues_a_proven_unsent_head_before_resuming_later_work() {
    lost_head_is_retried_in_order(true);
}

fn lost_head_is_retried_in_order(restarted: bool) {
    let w = World::new();
    w.complete_mailbox();
    let (hub, mut rx) = host(vec![]);
    let ids = [w.queue("one"), w.queue("two"), w.queue("three")];
    assert!(w.dispatch(&hub));
    assert_eq!(w.operation(&prompt(&mut rx).0), ids[0]);
    let listed = if restarted {
        vec![]
    } else {
        vec![hennery_proto::frames::AttachedSession {
            ending: None,
            session_id: "session".into(),
            last_seq: w.seq.get(),
            open_turn_id: None,
        }]
    };
    w.store.reconcile_host("host", &listed).unwrap();
    let queue = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(
        queue.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(),
        ids.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(queue.iter().all(|q| q.state == "queued"), "{queue:?}");
    if restarted {
        let hat = w.store.find_session("session").unwrap().unwrap().hat_id;
        w.store.request_resume("session", &hat).unwrap();
        w.emit(SessionBody::session_started("resume", "agent-session"));
    }
    for expected in ids {
        assert!(w.dispatch(&hub));
        let (request, turn, _) = prompt(&mut rx);
        assert_eq!(w.operation(&request), expected);
        w.start(&request, &turn);
        w.end(&turn, TurnOutcome::Completed);
    }
    assert!(w.store.operator_prompt_queue("session").unwrap().is_empty());
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn a_bad_session_does_not_abort_the_dispatch_page_before_later_sessions() {
    let w = World::new();
    let hat = w.store.find_session("session").unwrap().unwrap().hat_id;
    for id in ["a", "b", "c"] {
        w.store
            .create_session(id, "host", "fake", "/project", &hat, None)
            .unwrap();
        w.store
            .ingest(id, 1, &SessionBody::session_started("start", "agent"))
            .unwrap();
        w.store.enqueue_operator_prompt(id, &checked(id)).unwrap();
    }
    w.conn()
        .execute(
            "UPDATE session_dispatch_queue SET payload=?2 WHERE owner_id=?1 AND target_id='b'",
            params![w.store.owner_id(), "invalid stored payload"],
        )
        .unwrap();
    let db = w.dir.path().join("hennery.db");
    let state = hennery_sessions::AppState::new(
        Store::open(&db).unwrap(),
        Hosts::open(&db).unwrap(),
        hennery_kernel::operator::Operator::open(&db).unwrap(),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let reg = state
        .hub
        .register("host", tx, Capabilities::default(), AgentIsolation::default())
        .unwrap();
    state.hub.mark_ready("host", reg.conn_id);
    hennery_sessions::delegation_worker::drain(&state).await.unwrap();
    let mut sent = vec![];
    while let Ok(frame) = rx.try_recv() {
        if let CollectorFrame::Prompt { session_id, .. } = frame {
            sent.push(session_id)
        }
    }
    assert_eq!(sent, ["a", "c"]);
    state.shutdown.cancel();
}

#[test]
fn explicit_park_holds_existing_queue_while_idle_park_resumes_it() {
    for reason in [ParkReason::Operator, ParkReason::Idle] {
        let w = World::new();
        let (hub, mut rx) = host(vec![Capability::DelegationConfig, Capability::OperatorResume]);
        w.queue("already queued");
        if reason == ParkReason::Operator {
            w.store.record_park_request("session").unwrap();
        }
        w.emit(SessionBody::SessionParked { reason });
        let catalog = HostCatalog {
            saved_at: None,
            agent: "fake".into(),
            adapter_revision: "fixture".into(),
            freshness: CatalogFreshness::Fresh,
            config_options: vec![],
            defaults: SessionConfig::default(),
            security: SecurityClassification {
                known: true,
                axes: Default::default(),
            },
        };
        let attached = w
            .store
            .attach_delegated("session", McpContext::NONE, &catalog, &hub)
            .unwrap();
        if reason == ParkReason::Operator {
            assert!(!attached, "explicit Park must hold the queue");
            assert!(rx.try_recv().is_err());
            assert_eq!(w.store.operator_prompt_queue("session").unwrap()[0].state, "queued");
            w.queue("new operator send releases the hold");
            assert!(
                w.store
                    .attach_delegated("session", McpContext::NONE, &catalog, &hub)
                    .unwrap()
            );
        } else {
            assert!(attached)
        }
    }
}

#[test]
fn a_send_during_the_park_handshake_releases_the_hold_and_resume_clears_it() {
    for (send_before_park, reason) in [
        (false, ParkReason::Operator),
        (true, ParkReason::Operator),
        (false, ParkReason::Idle),
        (true, ParkReason::Idle),
    ] {
        let w = World::new();
        let (hub, _rx) = host(vec![]);
        w.queue("existing");
        w.store.record_park_request("session").unwrap();
        assert!(!w.store.operator_dispatch_ready("session").unwrap());
        assert!(!w.dispatch(&hub));
        if send_before_park {
            w.queue("send while parking");
            assert!(!w.store.operator_dispatch_ready("session").unwrap());
            assert!(!w.dispatch(&hub));
        }
        w.emit(SessionBody::SessionParked { reason });
        let held: bool = w
            .conn()
            .query_row(
                "SELECT prompt_queue_held FROM sessions WHERE owner_id=?1 AND id='session'",
                [w.store.owner_id()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(held, !send_before_park);
        let hat = w.store.find_session("session").unwrap().unwrap().hat_id;
        assert!(matches!(
            w.store.request_resume("session", &hat).unwrap(),
            hennery_sessions::store::ResumeRequest::Starting { .. }
        ));
        let held: bool = w
            .conn()
            .query_row(
                "SELECT prompt_queue_held FROM sessions WHERE owner_id=?1 AND id='session'",
                [w.store.owner_id()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!held);
    }
}

#[test]
fn host_busy_refusal_requeues_the_head_without_failing_its_tail() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    let one = w.queue("one");
    let two = w.queue("two");
    assert!(w.dispatch(&hub));
    let (request, _, _) = prompt(&mut rx);
    assert!(
        w.store
            .reject_operator_prompt("host", &request, "turn_in_progress")
            .unwrap()
    );
    let q = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(q.iter().map(|q| q.id.clone()).collect::<Vec<_>>(), [one.clone(), two]);
    assert!(q.iter().all(|q| q.state == "queued"));
    assert!(
        !w.store.operator_dispatch_ready("session").unwrap(),
        "busy retry must back off"
    );
    assert!(!w.dispatch(&hub), "the delayed head must also fence its tail");
    w.conn()
        .execute(
            "UPDATE session_dispatch_queue SET retry_after=0 WHERE owner_id=?1 AND operation_id=?2",
            params![w.store.owner_id(), one],
        )
        .unwrap();
    assert!(w.dispatch(&hub));
    assert_eq!(w.operation(&prompt(&mut rx).0), one);
}

#[test]
fn busy_retries_back_off_and_delete_unsent_turns_and_image_links() {
    let millis = || {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    };
    let w = World::new();
    let (hub, mut rx) = host(vec![Capability::Images]);
    let image = content::check(vec![
        json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(b"\x89PNG\r\n\x1a\nretry")}),
    ])
    .unwrap();
    let id = w.store.enqueue_operator_prompt("session", &image).unwrap();
    for (attempt, delay) in [1, 2, 4, 8, 10, 10].into_iter().enumerate() {
        assert!(w.dispatch(&hub));
        let (request, turn, _) = prompt(&mut rx);
        let before = millis();
        assert!(
            w.store
                .reject_operator_prompt("host", &request, "turn_in_progress")
                .unwrap()
        );
        assert!(
            w.store.turn_state(&turn).unwrap().is_none(),
            "refused turn must be deleted"
        );
        let links: i64 = w
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM turn_attachments WHERE owner_id=?1 AND turn_id=?2",
                params![w.store.owner_id(), turn],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(links, 0);
        assert!(!w.store.operator_dispatch_ready("session").unwrap());
        assert!(!w.dispatch(&hub));
        let reopened = Store::open(&w.dir.path().join("hennery.db")).unwrap();
        assert!(
            !reopened.operator_dispatch_ready("session").unwrap(),
            "the backoff must survive collector restart"
        );
        assert!(
            !reopened
                .dispatch_operator_prompt("session", &hub, &mut None, None)
                .unwrap()
        );
        let (count, deadline): (i64, i64) = w
            .conn()
            .query_row(
                "SELECT busy_refusals,retry_after FROM session_dispatch_queue WHERE owner_id=?1 AND operation_id=?2",
                params![w.store.owner_id(), id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, ((attempt + 1) as i64).min(5));
        assert!(
            deadline >= before + delay * 1000 && deadline <= millis() + delay * 1000,
            "backoff must cover the full interval, including a fractional second"
        );
        assert!(w.store.attachment(&image.images[0].sha256).unwrap().is_some());
        w.conn()
            .execute(
                "UPDATE session_dispatch_queue SET retry_after=?3 WHERE owner_id=?1 AND operation_id=?2",
                params![w.store.owner_id(), id, millis() - 1000],
            )
            .unwrap();
        assert!(
            w.store.operator_dispatch_ready("session").unwrap(),
            "an expired millisecond deadline must release the head"
        );
    }
}

#[test]
fn a_stale_busy_error_cannot_reject_a_new_delivery_attempt() {
    stale_error_does_not_reject_retry(false);
}

#[test]
fn a_reconnected_error_for_a_lost_attempt_cannot_reject_its_retry() {
    stale_error_does_not_reject_retry(true);
}

#[tokio::test]
async fn the_worker_uses_a_fresh_attempt_request_instead_of_the_queue_receipt() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    let operation = w.queue("one submit");
    let db = w.dir.path().join("hennery.db");
    let mut state = hennery_sessions::AppState::new(
        Store::open(&db).unwrap(),
        Hosts::open(&db).unwrap(),
        hennery_kernel::operator::Operator::open(&db).unwrap(),
    );
    state.hub = hub;
    hennery_sessions::delegation_worker::drain(&state).await.unwrap();
    let (request, _, _) = prompt(&mut rx);
    assert_ne!(request, operation);
    assert_eq!(w.operation(&request), operation);
}

#[test]
fn a_dispatch_attempt_preserves_the_request_id_used_to_prepare_its_startup_record() {
    let w = World::new();
    let (hub, mut rx) = host(vec![]);
    let operation = w.queue("first prompt");
    let request = uuid::Uuid::now_v7().to_string();
    let mut startup = Some(hennery_proto::plugin::StartupRecord {
        request_id: request.clone(),
        status: hennery_proto::plugin::StartupStatus::Unavailable,
        block: None,
    });
    assert!(
        w.store
            .dispatch_operator_prompt("session", &hub, &mut startup, None)
            .unwrap()
    );
    let (sent_request, _, _) = prompt(&mut rx);
    assert_eq!(sent_request, request);
    assert_eq!(w.operation(&sent_request), operation);
}

fn stale_error_does_not_reject_retry(reconnected: bool) {
    let w = World::new();
    w.complete_mailbox();
    let (hub, mut rx) = host(vec![]);
    let head = w.queue("head");
    let tail = w.queue("tail");
    assert!(w.dispatch(&hub));
    let (old_request, _, _) = prompt(&mut rx);
    assert_eq!(w.operation(&old_request), head);
    if reconnected {
        w.store
            .reconcile_host(
                "host",
                &[hennery_proto::frames::AttachedSession {
                    ending: None,
                    session_id: "session".into(),
                    last_seq: w.seq.get(),
                    open_turn_id: None,
                }],
            )
            .unwrap();
    } else {
        assert!(
            w.store
                .reject_operator_prompt("host", &old_request, "turn_in_progress")
                .unwrap()
        );
    }
    w.conn()
        .execute(
            "UPDATE session_dispatch_queue SET retry_after=0 WHERE owner_id=?1",
            [w.store.owner_id()],
        )
        .unwrap();
    assert!(w.dispatch(&hub));
    let (request, turn, _) = prompt(&mut rx);
    assert_eq!(w.operation(&request), head);
    assert!(
        !w.store
            .reject_operator_prompt("host", &old_request, "turn_in_progress")
            .unwrap(),
        "an error for the old attempt must not reject the handed-off retry"
    );
    assert_ne!(request, old_request);
    assert_eq!(
        w.store
            .find_session("session")
            .unwrap()
            .unwrap()
            .open_turn_id
            .as_deref(),
        Some(turn.as_str())
    );
    let queue = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(
        queue.iter().map(|item| item.id.clone()).collect::<Vec<_>>(),
        [head, tail.clone()]
    );
    assert_eq!(queue[0].state, "delivering");
    assert_eq!(queue[1].state, "queued");
    w.start(&request, &turn);
    w.end(&turn, TurnOutcome::Completed);
    assert!(w.dispatch(&hub));
    let (request, turn, _) = prompt(&mut rx);
    assert_eq!(w.operation(&request), tail);
    w.start(&request, &turn);
    w.end(&turn, TurnOutcome::Completed);
    assert!(w.store.operator_prompt_queue("session").unwrap().is_empty());
}

#[test]
fn retained_failures_do_not_consume_the_active_queue_capacity() {
    let w = World::new();
    w.store.open_prompt("session", "running", &checked("first")).unwrap();
    w.start("first", "running");
    for _ in 0..32 {
        w.queue("retained failure");
    }
    w.end("running", TurnOutcome::Failed);
    for _ in 0..32 {
        w.queue("new work");
    }
    assert_eq!(w.store.operator_prompt_queue("session").unwrap().len(), 64);
    assert_eq!(
        w.store
            .enqueue_operator_prompt("session", &checked("overflow"))
            .unwrap_err()
            .to_string(),
        "queue_full"
    );
}

#[tokio::test]
async fn parked_offline_and_revoked_hosts_fail_waiting_input_readably() {
    for revoked in [false, true] {
        let w = World::new();
        w.emit(SessionBody::SessionParked {
            reason: ParkReason::Idle,
        });
        w.queue("keep this text");
        let db = w.dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        if revoked {
            hosts.revoke("host", 1).unwrap();
        }
        let state = hennery_sessions::AppState::new(
            Store::open(&db).unwrap(),
            hosts,
            hennery_kernel::operator::Operator::open(&db).unwrap(),
        );
        hennery_sessions::delegation_worker::drain(&state).await.unwrap();
        let q = state.store.operator_prompt_queue("session").unwrap();
        assert_eq!(q[0].state, "failed");
        assert!(
            q[0].reason
                .as_deref()
                .unwrap()
                .contains(if revoked { "revoked" } else { "offline" })
        );
        if revoked {
            assert!(
                state
                    .store
                    .enqueue_operator_prompt("session", &checked("refuse revoked"))
                    .is_err()
            );
        }
        state.shutdown.cancel();
    }
}

#[tokio::test]
async fn a_reconnecting_parked_host_defers_the_queue_until_resend_completes() {
    let w = World::new();
    w.emit(SessionBody::SessionParked {
        reason: ParkReason::Idle,
    });
    w.queue("wait for reconnect proof");
    let db = w.dir.path().join("hennery.db");
    let state = hennery_sessions::AppState::new(
        Store::open(&db).unwrap(),
        Hosts::open(&db).unwrap(),
        hennery_kernel::operator::Operator::open(&db).unwrap(),
    );
    let (tx, mut frames) = tokio::sync::mpsc::unbounded_channel();
    let connection = state
        .hub
        .register(
            "host",
            tx,
            Capabilities(vec![Capability::DelegationConfig, Capability::OperatorResume]),
            AgentIsolation::default(),
        )
        .unwrap();
    assert!(!state.hub.is_ready("host"));
    hennery_sessions::delegation_worker::drain(&state).await.unwrap();
    assert_eq!(state.store.operator_prompt_queue("session").unwrap()[0].state, "queued");
    assert!(frames.try_recv().is_err());
    state.hub.unregister("host", connection.conn_id);
    hennery_sessions::delegation_worker::drain(&state).await.unwrap();
    let queue = state.store.operator_prompt_queue("session").unwrap();
    assert_eq!(queue[0].state, "failed");
    assert!(queue[0].reason.as_deref().unwrap().contains("offline"));
    state.shutdown.cancel();
}

#[test]
fn revocation_after_worker_preparation_fences_every_operator_handoff() {
    let w = World::new();
    let (hub, mut frames) = host(vec![Capability::DelegationConfig, Capability::OperatorResume]);
    w.queue("prepared before revocation");
    assert!(w.store.operator_dispatch_ready("session").unwrap());
    Hosts::open(&w.dir.path().join("hennery.db"))
        .unwrap()
        .revoke("host", 1)
        .unwrap();
    assert!(!w.store.operator_dispatch_ready("session").unwrap());
    assert!(!w.dispatch(&hub));
    w.emit(SessionBody::SessionParked {
        reason: ParkReason::Idle,
    });
    assert!(
        !w.store
            .attach_delegated("session", McpContext::NONE, &operator_catalog(), &hub)
            .unwrap()
    );
    assert!(frames.try_recv().is_err());
    let idle = World::new();
    Hosts::open(&idle.dir.path().join("hennery.db"))
        .unwrap()
        .revoke("host", 1)
        .unwrap();
    assert!(
        !idle
            .store
            .open_prompt("session", "revoked-immediate", &checked("new immediate input"))
            .unwrap()
    );
}

#[test]
fn operator_resume_requires_a_host_capability_instead_of_opaque_failure() {
    let w = World::new();
    let (hub, mut rx) = host(vec![Capability::DelegationConfig]);
    w.emit(SessionBody::SessionParked {
        reason: ParkReason::Idle,
    });
    w.queue("resume");
    let catalog = HostCatalog {
        saved_at: None,
        agent: "fake".into(),
        adapter_revision: "fixture".into(),
        freshness: CatalogFreshness::Fresh,
        config_options: vec![],
        defaults: SessionConfig::default(),
        security: SecurityClassification {
            known: true,
            axes: Default::default(),
        },
    };
    assert!(
        !w.store
            .attach_delegated("session", McpContext::NONE, &catalog, &hub)
            .unwrap()
    );
    assert!(rx.try_recv().is_err());
    let q = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(q[0].state, "failed");
    assert!(q[0].reason.as_deref().unwrap().contains("hennery 0.3.0"));
}

#[tokio::test]
async fn failed_items_expire_after_seven_days_and_remove_only_their_images() {
    let w = World::new();
    w.store.open_prompt("session", "running", &checked("first")).unwrap();
    w.start("first", "running");
    let image = content::check(vec![
        json!({"type":"text","text":"expire"}),
        json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(b"\x89PNG\r\n\x1a\nexpire")}),
    ])
    .unwrap();
    let sha = image.images[0].sha256.clone();
    let key = hennery_sessions::prompt_receipts::PromptRequestKey::new("expired-image", &image).unwrap();
    let hennery_sessions::prompt_receipts::PromptReceipt::Queued { queue_id: old } = w
        .store
        .enqueue_operator_prompt_with_key("session", &image, Some(&key))
        .unwrap()
    else {
        panic!("queued receipt")
    };
    let fresh = w.queue("recent failure");
    let failed_after = hennery_kernel::secret::unix_now();
    w.end("running", TurnOutcome::Failed);
    let now = hennery_kernel::secret::unix_now();
    for id in [&old, &fresh] {
        let at: Option<i64> = w
            .conn()
            .query_row(
                "SELECT failed_at FROM session_dispatch_queue WHERE owner_id = ?1 AND operation_id=?2",
                params![w.store.owner_id(), id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            at.is_some_and(|at| at >= failed_after && at <= now),
            "failure age must be durably recorded"
        );
    }
    w.conn()
        .execute(
            "UPDATE session_dispatch_queue SET failed_at=?3 WHERE owner_id=?1 AND operation_id=?2",
            params![w.store.owner_id(), old, now - 7 * 24 * 60 * 60],
        )
        .unwrap();
    let db = w.dir.path().join("hennery.db");
    let state = hennery_sessions::AppState::new(
        Store::open(&db).unwrap(),
        Hosts::open(&db).unwrap(),
        hennery_kernel::operator::Operator::open(&db).unwrap(),
    );
    hennery_sessions::delegation_worker::drain(&state).await.unwrap();
    let q = w.store.operator_prompt_queue("session").unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].id, fresh);
    assert!(w.store.attachment(&sha).unwrap().is_none());
    assert!(!w.dir.path().join("attachments").join(sha).exists());
    assert!(w.store.prompt_receipt("session", &key).unwrap().is_none());
    state.shutdown.cancel();
}

#[test]
fn randomized_dispatch_drop_reconcile_edit_remove_preserves_a_unique_delivery_prefix() {
    for seed in 1_u64..=32 {
        randomized_legacy_mailbox_interleaving(seed);
        let w = World::new();
        w.complete_mailbox();
        let (hub, mut rx) = host(vec![]);
        let mut rng = seed;
        let mut order = Vec::new();
        let mut current: Option<(String, String, String, bool)> = None;
        let mut stale_requests = Vec::new();
        let mut delivered = Vec::new();
        for step in 0..96 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            match rng % 8 {
                0 => {
                    if let Ok(id) = w
                        .store
                        .enqueue_operator_prompt("session", &checked(&format!("seed {seed} step {step}")))
                    {
                        order.push(id);
                    }
                }
                1 => {
                    if w.dispatch(&hub) {
                        assert!(current.is_none());
                        let (request, turn, _) = prompt(&mut rx);
                        let operation = w.operation(&request);
                        current = Some((operation, request, turn, false));
                    }
                }
                2 => {
                    if let Some((operation, request, turn, started)) = &mut current
                        && !*started
                    {
                        w.start(request, turn);
                        delivered.push(operation.clone());
                        *started = true;
                    }
                }
                3 => {
                    if current.as_ref().is_some_and(|(_, _, _, started)| *started) {
                        let (_, _, turn, _) = current.take().unwrap();
                        w.end(&turn, TurnOutcome::Completed);
                    }
                }
                4 => {
                    if current.as_ref().is_some_and(|(_, _, _, started)| !*started) {
                        let restarted = rng & 8 != 0;
                        let listed = if restarted {
                            vec![]
                        } else {
                            vec![hennery_proto::frames::AttachedSession {
                                ending: None,
                                session_id: "session".into(),
                                last_seq: w.seq.get(),
                                open_turn_id: None,
                            }]
                        };
                        w.store.reconcile_host("host", &listed).unwrap();
                        let (_, request, turn, _) = current.as_ref().unwrap();
                        stale_requests.push((request.clone(), turn.clone()));
                        current = None;
                        if restarted {
                            let hat = w.store.find_session("session").unwrap().unwrap().hat_id;
                            w.store.request_resume("session", &hat).unwrap();
                            w.emit(SessionBody::session_started("resume", "agent-session"));
                        }
                    }
                }
                5 | 6 => {
                    if order.len() > delivered.len() {
                        let index = delivered.len() + ((rng >> 8) as usize % (order.len() - delivered.len()));
                        let id = &order[index];
                        if rng % 8 == 5 {
                            let _ = w.store.edit_operator_prompt("session", id, "edited text");
                        } else if w.store.remove_operator_prompt("session", id).unwrap() {
                            order.remove(index);
                        }
                    }
                }
                _ => {
                    if let Some((_, request, turn, true)) = &current {
                        w.start(request, turn);
                    }
                    if !stale_requests.is_empty() {
                        let (request, turn) = &stale_requests[(rng >> 8) as usize % stale_requests.len()];
                        w.start(request, turn);
                        assert!(
                            !w.store
                                .reject_operator_prompt("host", request, "turn_in_progress")
                                .unwrap()
                        );
                    }
                }
            }
            assert_eq!(delivered, order[..delivered.len()], "seed={seed}, step={step}");
            assert_eq!(
                delivered.iter().collect::<std::collections::BTreeSet<_>>().len(),
                delivered.len()
            );
            let actual = w
                .store
                .events("session", 0, 10000)
                .unwrap()
                .into_iter()
                .filter(|e| e.kind == "user_turn")
                .map(|e| {
                    w.conn()
                        .query_row(
                            "SELECT operation_id FROM session_operations WHERE owner_id=?1 AND turn_id=?2",
                            params![w.store.owner_id(), e.body["turn_id"].as_str().unwrap()],
                            |r| r.get::<_, String>(0),
                        )
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, delivered, "applied transcript prefix, seed={seed}, step={step}");
        }
    }
}
