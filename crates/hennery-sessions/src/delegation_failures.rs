//! Pre-attach catalog refusals settle accepted work without failing an
//! operator's existing session or changing the established offline semantics.
use crate::{delegation_runtime::same_hat_in, store::Store};
use anyhow::Result;
use rusqlite::{Transaction, params};
use std::collections::{BTreeMap, BTreeSet};

fn terminal(code: &str, operation: &str, now: i64, failures: &mut BTreeMap<String, i64>) -> bool {
    match code {
        "auth_required"
        | crate::mcp_source::UNSUPPORTED
        | "runtime_unavailable"
        | "configuration_unavailable"
        | "cwd_unavailable"
        | "agent_has_no_record" => true,
        "busy" | "timeout" | "adapter_failed" | "catalog_unavailable" => {
            let first_failure = failures.entry(operation.into()).or_insert(now);
            now.saturating_sub(*first_failure) >= 30
        }
        _ => false,
    }
}

fn prune_failures(tx: &Transaction<'_>, owner: &str, failures: &mut BTreeMap<String, i64>) -> Result<()> {
    if failures.is_empty() {
        return Ok(());
    }
    let mut statement = tx.prepare(
        "SELECT q.operation_id FROM session_dispatch_queue q JOIN session_operations o
         ON o.owner_id = ?1 AND o.operation_id=q.operation_id
         WHERE q.owner_id = ?1 AND q.state IN ('queued','waiting_config')
         AND o.state IN ('queued','waiting_config','awaiting_reconciliation')",
    )?;
    let pending = statement
        .query_map([owner], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    // Retain every live operation's original clock. The map is bounded by
    // pending queues, and settled/deleted operations cannot accumulate in it.
    failures.retain(|operation, _| pending.contains(operation));
    Ok(())
}
impl Store {
    /// An accepted fresh catalog ends the failed-probe streak for every
    /// pending operation on that target, including work behind the head.
    pub(crate) fn clear_delegation_catalog_failures(&self, target: &str) -> Result<()> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT operation_id FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id = ?2
             AND state IN ('queued','waiting_config')",
        )?;
        let pending = statement
            .query_map(params![self.owner_id(), target], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        self.delegation_catalog_failures
            .lock()
            .expect("delegation catalog failures lock")
            .retain(|operation, _| !pending.contains(operation));
        Ok(())
    }

    pub(crate) fn fail_delegation_catalog(&self, target: &str, code: &str, now: i64) -> Result<()> {
        self.fail_delegation_catalog_for(target, code, now, None)
    }

    /// Reject only the configuration whose operation was checked before I/O;
    /// a later queue head must receive its own configuration check.
    pub fn fail_delegation_configuration(&self, target: &str, expected_operation: &str, now: i64) -> Result<()> {
        self.fail_delegation_catalog_for(target, "configuration_unavailable", now, Some(expected_operation))
    }

    pub fn fail_delegation_source_policy(&self, target: &str, expected_operation: &str, now: i64) -> Result<()> {
        self.fail_delegation_catalog_for(target, crate::mcp_source::UNSUPPORTED, now, Some(expected_operation))
    }

    fn fail_delegation_catalog_for(
        &self,
        target: &str,
        code: &str,
        now: i64,
        expected_operation: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut failures = self
            .delegation_catalog_failures
            .lock()
            .expect("delegation catalog failures lock");
        prune_failures(&tx, self.owner_id(), &mut failures)?;
        let operations = {
            let mut statement = tx.prepare(
                "SELECT q.operation_id,q.source_id,q.accepted_revision,q.origin='operator' FROM session_dispatch_queue q
                 JOIN sessions s ON s.owner_id = ?1 AND s.id=q.target_id
                 JOIN session_operations o ON o.owner_id = ?1 AND o.operation_id=q.operation_id
                 WHERE q.owner_id = ?1 AND q.target_id=?2 AND q.state IN ('queued','waiting_config')
                 AND o.state IN ('queued','waiting_config','awaiting_reconciliation')
                 AND (s.lifecycle='parked' OR (s.lifecycle='starting' AND s.agent_session_id IS NULL
                   AND EXISTS(SELECT 1 FROM session_attach_evidence e WHERE e.owner_id = ?1
                     AND e.session_id=s.id AND e.launch_sent=0))) ORDER BY q.position",
            )?;
            statement
                .query_map(params![self.owner_id(), target], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, bool>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if expected_operation.is_some_and(|expected| {
            operations
                .first()
                .is_none_or(|(operation, _, _, _)| operation != expected)
        }) {
            tx.commit()?;
            return Ok(());
        }
        let mut failed = false;
        for (position, (operation, source, revision, operator)) in operations.into_iter().enumerate() {
            // A missing requested value applies to the selected queue head,
            // while a runtime failure affects every unsent operation.
            if code == "configuration_unavailable" && position > 0 {
                break;
            }
            if !self.queue_transfer_in(&tx, &source, target, revision, operator)? {
                continue;
            }
            let (host, hat, cwd): (String, String, String) = tx.query_row(
                "SELECT host_id,hat_id,cwd FROM sessions WHERE owner_id = ?1 AND id=?2",
                params![self.owner_id(), target],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            if !same_hat_in(&tx, self.owner_id(), &host, &hat, &cwd)? {
                continue;
            }
            if !terminal(code, &operation, now, &mut failures) {
                continue;
            }
            failed |= tx.execute(
                "UPDATE session_operations SET state='failed',code=?3 WHERE owner_id = ?1 AND operation_id=?2
                 AND state IN ('queued','waiting_config','awaiting_reconciliation') AND turn_id IS NULL",
                params![self.owner_id(), operation, code],
            )? > 0;
            if operator {
                // An operator resume refusal stops its whole queue. Keep the
                // text behind the head instead of attempting more prompts.
                self.fail_operator_queue_in(&tx, target, code)?;
            }
        }
        if failed {
            // Set operation codes before the lifecycle trigger: the session UI
            // retains its established login help, while tool receipts use ACP's
            // auth_required classification.
            let session_code = if code == "auth_required" {
                "agent_not_logged_in"
            } else {
                code
            };
            let changed = tx.execute(
                "UPDATE sessions SET lifecycle='failed',failure_reason=?3
                 WHERE owner_id = ?1 AND id=?2 AND lifecycle='starting' AND agent_session_id IS NULL
                 AND EXISTS(SELECT 1 FROM session_attach_evidence e WHERE e.owner_id = ?1
                   AND e.session_id=sessions.id AND e.launch_sent=0)",
                params![self.owner_id(), target, session_code],
            )?;
            tx.execute(
                "UPDATE sessions SET family_dirty=1 WHERE owner_id = ?1 AND id=?2",
                params![self.owner_id(), target],
            )?;
            self.settle_delegation_in(&tx)?;
            prune_failures(&tx, self.owner_id(), &mut failures)?;
            if changed > 0 {
                self.record_delegation_failure_in(&tx, target, session_code)?;
                let cut = self.mcp().revoke_in(&tx, target)?;
                return crate::store::commit_then_cut(tx, self.mcp().as_ref(), cut);
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::terminal;
    #[test]
    fn catalog_failures_retry_only_transient_errors_from_their_first_failure() {
        let mut failures = Default::default();
        for code in [
            "auth_required",
            "runtime_unavailable",
            "configuration_unavailable",
            "cwd_unavailable",
            "agent_has_no_record",
        ] {
            assert!(terminal(code, code, 600, &mut failures));
        }
        for code in ["busy", "timeout", "adapter_failed", "catalog_unavailable"] {
            assert!(!terminal(code, code, 600, &mut failures));
            assert!(!terminal(code, code, 629, &mut failures));
            assert!(terminal(code, code, 630, &mut failures));
        }
        assert!(!terminal("host_offline", "offline", 3600, &mut failures));
    }

    fn queued() -> (tempfile::TempDir, crate::store::Store, String) {
        use hennery_kernel::hosts::{Enrollment, Hosts};
        use hennery_proto::frames::{AgentIsolation, Capabilities, Capability, McpIsolation};
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
        hosts
            .record_hello("host", "fixture", &Capabilities(vec![Capability::McpServers]), 0)
            .unwrap();
        hosts
            .record_mcp_isolation(
                "host",
                &AgentIsolation(
                    [(String::from("claude"), McpIsolation::ClaudeStrict)]
                        .into_iter()
                        .collect(),
                ),
            )
            .unwrap();
        let hat = hosts.host("host").unwrap().unwrap().default_hat_id;
        let store = crate::store::Store::open(&db).unwrap();
        for id in ["parent", "child"] {
            assert!(
                store
                    .create_session(id, "host", "claude", "/project", &hat, None)
                    .unwrap()
            );
        }
        let operation = "00000000-0000-7000-8000-000000000001".to_string();
        {
            let conn = store.conn();
            let owner = store.owner_id();
            conn.execute(
                "INSERT INTO hat_session_tools(owner_id,hat_id,delegation_enabled,revision) VALUES(?1,?2,1,1)",
                rusqlite::params![owner, hat],
            )
            .unwrap();
            conn.execute(
                "UPDATE sessions SET lifecycle='active',mcp_delivery_mode='isolated' WHERE owner_id = ?1 AND id='parent'",
                [owner],
            )
            .unwrap();
            conn.execute("INSERT INTO session_families(owner_id,child_id,parent_id,root_id,hat_id,created_event_id) VALUES(?1,'child','parent','parent',?2,0)", rusqlite::params![owner,hat]).unwrap();
            conn.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,accepted_revision) VALUES(?1,'parent','operation',?2,'digest','session_spawn','child','queued',1)", rusqlite::params![owner,operation]).unwrap();
            conn.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES(?1,?2,'parent','child',1,'brief','{}')", rusqlite::params![owner,operation]).unwrap();
            conn.execute("INSERT INTO session_attach_evidence(owner_id,session_id,attach_generation,requested_config,launch_sent) VALUES(?1,'child',1,'{}',0)", [owner]).unwrap();
        }
        (dir, store, operation)
    }
    fn operation_state(store: &crate::store::Store) -> (String, Option<String>) {
        store
            .conn()
            .query_row(
                "SELECT state,code FROM session_operations WHERE owner_id = ?1 AND child_id='child'",
                [store.owner_id()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }
    #[test]
    fn old_queued_operation_receives_full_retry_window_after_first_probe_failure() {
        for code in ["busy", "timeout", "adapter_failed", "catalog_unavailable"] {
            let (_dir, store, _operation) = queued();
            store.fail_delegation_catalog("child", code, 600).unwrap();
            assert_eq!(
                operation_state(&store),
                ("queued".into(), None),
                "first {code} after ten minutes offline"
            );
            store.fail_delegation_catalog("child", code, 629).unwrap();
            assert_eq!(
                operation_state(&store),
                ("queued".into(), None),
                "{code} must not expire early"
            );
            store.fail_delegation_catalog("child", code, 630).unwrap();
            assert_eq!(
                operation_state(&store),
                ("failed".into(), Some(code.into())),
                "{code} must expire without resetting its first failure"
            );
        }
    }
    #[test]
    fn impossible_configuration_fails_without_waiting_for_retry() {
        use std::sync::Arc;
        let (dir, store, operation) = queued();
        let db = dir.path().join("hennery.db");
        let keys = hennery_gateway::key::KeySource::from_vars(dir.path(), Some("09".repeat(32).into()), None).unwrap();
        let gateway = hennery_gateway::open(
            &db,
            &keys,
            Arc::new(hennery_kernel::operator::Operator::open(&db).unwrap()),
            hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap(),
            Arc::new(hennery_gateway::notify::Silent),
        )
        .unwrap();
        store.set_session_mcp(Arc::new(hennery_gateway::session::GatewayMcp::for_collector(&gateway)));
        let hat = store.find_session("child").unwrap().unwrap().hat_id;
        {
            let mut conn = store.conn();
            let tx = conn.transaction().unwrap();
            hennery_gateway::tokens::mint_in(&tx, store.owner_id(), "child", "host", &hat, 600).unwrap();
            tx.commit().unwrap();
        }
        store.fail_delegation_configuration("child", &operation, 600).unwrap();
        assert_eq!(
            operation_state(&store),
            ("failed".into(), Some("configuration_unavailable".into()))
        );
        let child = store.find_session("child").unwrap().unwrap();
        assert_eq!(child.lifecycle, "failed");
        assert_eq!(child.failure_reason.as_deref(), Some("configuration_unavailable"));
        assert!(
            store
                .conn()
                .query_row(
                    "SELECT revoked_at IS NOT NULL FROM gw_session_tokens WHERE owner_id = ?1 AND session_id='child'",
                    [store.owner_id()],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
    }
    #[test]
    fn configuration_failure_only_rejects_the_head_of_a_parked_queue() {
        let (_dir, store, head) = queued();
        let tail = "00000000-0000-7000-8000-000000000002";
        {
            let conn = store.conn();
            let owner = store.owner_id();
            conn.execute("UPDATE sessions SET lifecycle='parked',agent_session_id='attached-before' WHERE owner_id = ?1 AND id='child'", [owner]).unwrap();
            conn.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,accepted_revision) VALUES(?1,'parent','tail',?2,'digest','session_send','child','queued',1)", rusqlite::params![owner,tail]).unwrap();
            conn.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES(?1,?2,'parent','child',1,'later brief','{}')", rusqlite::params![owner,tail]).unwrap();
        }
        store.fail_delegation_configuration("child", &head, 600).unwrap();
        let conn = store.conn();
        for (operation, wanted) in [(head.as_str(), "failed"), (tail, "queued")] {
            let state: String = conn
                .query_row(
                    "SELECT state FROM session_operations WHERE owner_id = ?1 AND operation_id=?2",
                    rusqlite::params![store.owner_id(), operation],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(state, wanted, "operation {operation}");
        }
        drop(conn);
        assert_eq!(store.find_session("child").unwrap().unwrap().lifecycle, "parked");
    }
    #[test]
    fn settled_operation_clocks_do_not_accumulate() {
        let (_dir, store, operation) = queued();
        store.fail_delegation_catalog("child", "busy", 600).unwrap();
        assert_eq!(
            store.delegation_catalog_failures.lock().unwrap().get(&operation),
            Some(&600)
        );
        store
            .conn()
            .execute(
                "UPDATE session_operations SET state='completed' WHERE owner_id = ?1 AND operation_id=?2",
                rusqlite::params![store.owner_id(), operation],
            )
            .unwrap();
        store.fail_delegation_catalog("child", "busy", 900).unwrap();
        assert!(store.delegation_catalog_failures.lock().unwrap().is_empty());
    }
    #[test]
    fn offline_does_not_start_clock_and_changed_transient_codes_do_not_reset_it() {
        let (_dir, store, _operation) = queued();
        store.fail_delegation_catalog("child", "host_offline", 600).unwrap();
        assert!(store.delegation_catalog_failures.lock().unwrap().is_empty());
        store.fail_delegation_catalog("child", "busy", 1200).unwrap();
        store.fail_delegation_catalog("child", "adapter_failed", 1229).unwrap();
        assert_eq!(operation_state(&store), ("queued".into(), None));
        store
            .fail_delegation_catalog("child", "catalog_unavailable", 1230)
            .unwrap();
        assert_eq!(
            operation_state(&store),
            ("failed".into(), Some("catalog_unavailable".into()))
        );
        assert!(store.delegation_catalog_failures.lock().unwrap().is_empty());
    }
    #[test]
    fn stale_configuration_head_does_not_reject_a_later_operation() {
        let (_dir, store, captured_head) = queued();
        let tail = "00000000-0000-7000-8000-000000000002";
        {
            let conn = store.conn();
            let owner = store.owner_id();
            conn.execute("UPDATE sessions SET lifecycle='parked',agent_session_id='attached-before' WHERE owner_id = ?1 AND id='child'", [owner]).unwrap();
            conn.execute(
                "UPDATE session_operations SET state='completed' WHERE owner_id = ?1 AND operation_id=?2",
                rusqlite::params![owner, captured_head],
            )
            .unwrap();
            conn.execute(
                "UPDATE session_dispatch_queue SET state='settled' WHERE owner_id = ?1 AND operation_id=?2",
                rusqlite::params![owner, captured_head],
            )
            .unwrap();
            conn.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,accepted_revision) VALUES(?1,'parent','tail',?2,'digest','session_send','child','queued',1)", rusqlite::params![owner,tail]).unwrap();
            conn.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES(?1,?2,'parent','child',1,'later brief','{}')", rusqlite::params![owner,tail]).unwrap();
        }
        store
            .fail_delegation_configuration("child", &captured_head, 600)
            .unwrap();
        let state: String = store
            .conn()
            .query_row(
                "SELECT state FROM session_operations WHERE owner_id = ?1 AND operation_id=?2",
                rusqlite::params![store.owner_id(), tail],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            state, "queued",
            "a refusal for captured A must not reject replacement B"
        );
    }

    #[test]
    fn accepted_fresh_catalog_resets_all_pending_clocks_for_its_target() {
        let (_dir, store, head) = queued();
        let tail = "00000000-0000-7000-8000-000000000002";
        let other = "00000000-0000-7000-8000-000000000003";
        let hat = store.find_session("child").unwrap().unwrap().hat_id;
        store
            .create_session("other-child", "host", "claude", "/project", &hat, None)
            .unwrap();
        {
            let conn = store.conn();
            let owner = store.owner_id();
            conn.execute("UPDATE sessions SET lifecycle='parked',agent_session_id='attached-before' WHERE owner_id = ?1 AND id='child'", [owner]).unwrap();
            conn.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,accepted_revision) VALUES(?1,'parent','tail',?2,'digest','session_send','child','queued',1)", rusqlite::params![owner,tail]).unwrap();
            conn.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES(?1,?2,'parent','child',1,'later brief','{}')", rusqlite::params![owner,tail]).unwrap();
            conn.execute("INSERT INTO session_families(owner_id,child_id,parent_id,root_id,hat_id,created_event_id) VALUES(?1,'other-child','parent','parent',?2,0)", rusqlite::params![owner,hat]).unwrap();
            conn.execute("UPDATE sessions SET lifecycle='parked',agent_session_id='other-attached' WHERE owner_id = ?1 AND id='other-child'", [owner]).unwrap();
            conn.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,accepted_revision) VALUES(?1,'parent','other-operation',?2,'digest','session_send','other-child','queued',1)", rusqlite::params![owner,other]).unwrap();
            conn.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES(?1,?2,'parent','other-child',1,'other brief','{}')", rusqlite::params![owner,other]).unwrap();
        }
        store.fail_delegation_catalog("other-child", "busy", 600).unwrap();
        store
            .conn()
            .execute(
                "UPDATE session_dispatch_queue SET state='waiting_config' WHERE owner_id = ?1 AND operation_id=?2",
                rusqlite::params![store.owner_id(), tail],
            )
            .unwrap();
        store.fail_delegation_catalog("child", "busy", 600).unwrap();
        assert_eq!(store.delegation_catalog_failures.lock().unwrap().len(), 3);
        store.clear_delegation_catalog_failures("child").unwrap();
        assert_eq!(store.delegation_catalog_failures.lock().unwrap().len(), 1);
        assert_eq!(store.delegation_catalog_failures.lock().unwrap().get(other), Some(&600));
        {
            let conn = store.conn();
            conn.execute(
                "UPDATE session_operations SET state='completed' WHERE owner_id = ?1 AND operation_id=?2",
                rusqlite::params![store.owner_id(), head],
            )
            .unwrap();
            conn.execute(
                "UPDATE session_dispatch_queue SET state='settled' WHERE owner_id = ?1 AND operation_id=?2",
                rusqlite::params![store.owner_id(), head],
            )
            .unwrap();
        }
        store.fail_delegation_catalog("child", "busy", 4200).unwrap();
        let tail_state = || {
            store
                .conn()
                .query_row(
                    "SELECT state FROM session_operations WHERE owner_id = ?1 AND operation_id=?2",
                    rusqlite::params![store.owner_id(), tail],
                    |r| r.get::<_, String>(0),
                )
                .unwrap()
        };
        assert_eq!(
            tail_state(),
            "queued",
            "B receives a new retry window after A accepted a fresh catalog"
        );
        store.fail_delegation_catalog("child", "busy", 4229).unwrap();
        assert_eq!(tail_state(), "queued");
        store.fail_delegation_catalog("child", "busy", 4230).unwrap();
        assert_eq!(tail_state(), "failed");
    }

    #[test]
    fn missing_parked_agent_record_fails_immediately() {
        let (_dir, store, _operation) = queued();
        store
            .conn()
            .execute(
                "UPDATE sessions SET lifecycle='parked' WHERE owner_id = ?1 AND id='child'",
                [store.owner_id()],
            )
            .unwrap();
        store
            .fail_delegation_catalog("child", "agent_has_no_record", 600)
            .unwrap();
        assert_eq!(
            operation_state(&store),
            ("failed".into(), Some("agent_has_no_record".into()))
        );
    }
    #[tokio::test]
    async fn worker_accepted_fresh_catalog_clears_failed_probe_clock() {
        use hennery_proto::{
            delegation_host::{CatalogFreshness, HostCatalog, SecurityClassification},
            frames::{
                AgentIsolation, Capabilities, Capability, CollectorFrame, HostFrame, McpIsolation, SessionConfig,
            },
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let (dir, store, _operation) = queued();
        store.fail_delegation_catalog("child", "busy", 600).unwrap();
        let db = dir.path().join("hennery.db");
        let state = crate::AppState::new(
            store,
            hennery_kernel::hosts::Hosts::open(&db).unwrap(),
            hennery_kernel::operator::Operator::open(&db).unwrap(),
        );
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let registration = state
            .hub
            .register(
                "host",
                sender,
                Capabilities(vec![
                    Capability::ResolvePath,
                    Capability::CatalogProbe,
                    Capability::DelegationConfig,
                    Capability::McpServers,
                ]),
                AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect()),
            )
            .unwrap();
        state.hub.mark_ready("host", registration.conn_id);
        let probes = Arc::new(AtomicUsize::new(0));
        let replies = probes.clone();
        let hub = state.hub.clone();
        let host = tokio::spawn(async move {
            while let Some(frame) = receiver.recv().await {
                let reply = match frame {
                    CollectorFrame::ResolvePath { request_id, path } => HostFrame::ResolvedPath {
                        request_id,
                        canonical: path,
                        exists: true,
                        is_dir: true,
                    },
                    CollectorFrame::ProbeCatalog { request_id, agent, .. } => {
                        replies.fetch_add(1, Ordering::SeqCst);
                        HostFrame::Catalog {
                            request_id,
                            catalog: HostCatalog {
                                saved_at: None,
                                agent,
                                adapter_revision: "fixture".into(),
                                freshness: CatalogFreshness::Fresh,
                                config_options: vec![],
                                defaults: SessionConfig::default(),
                                security: SecurityClassification {
                                    known: true,
                                    axes: Default::default(),
                                },
                            },
                        }
                    }
                    _ => continue,
                };
                hub.probe_reply("host", registration.conn_id, reply);
            }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            crate::delegation_worker::drain(&state),
        )
        .await
        .unwrap()
        .unwrap();
        host.abort();
        let _ = host.await;
        assert_eq!(probes.load(Ordering::SeqCst), 1);
        assert!(
            state.store.delegation_catalog_failures.lock().unwrap().is_empty(),
            "accepted fresh catalog ends the failed-probe streak"
        );
    }

    #[test]
    fn disconnected_host_has_no_delegated_attach_candidate() {
        use hennery_proto::frames::{AgentIsolation, Capabilities, Capability};
        let (_dir, store, _operation) = queued();
        let hub = crate::hub::Hub::new();
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let registration = hub
            .register(
                "host",
                sender,
                Capabilities(vec![Capability::DelegationConfig]),
                AgentIsolation::default(),
            )
            .unwrap();
        hub.mark_ready("host", registration.conn_id);
        assert!(hub.is_ready("host"));
        assert!(store.delegated_attach_candidate("child", &hub).unwrap().is_some());
        hub.unregister("host", registration.conn_id);
        assert!(!hub.is_ready("host"));
        assert!(store.delegated_attach_candidate("child", &hub).unwrap().is_none());
    }
}
