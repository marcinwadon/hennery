//! Durable parent commands. D3 can reuse the dispatch/evidence gate but owns
//! child notices and attention; no child-to-parent delivery is produced here.
use crate::{
    delegation::SessionToolContext,
    store::{McpContext, Store, now},
};
use anyhow::{Result, ensure};
use hennery_gateway::identity::SessionIdentity;
use hennery_proto::{
    delegation::{OperationReceipt, OperationState},
    frames::SessionConfig,
};
use rusqlite::{OptionalExtension, Transaction, params};

pub const MAX_QUEUE: i64 = 32;
pub const MAX_QUEUE_BYTES: i64 = 1024 * 1024;

/// Validated launch values retained together through atomic allocation.
pub struct DelegatedLaunch<'a> {
    pub key: &'a str,
    pub agent: &'a str,
    pub cwd: &'a str,
    pub brief: &'a str,
    pub name: Option<&'a str>,
    pub config: &'a SessionConfig,
    pub mcp: McpContext,
}

fn uuid() -> String {
    uuid::Uuid::now_v7().to_string()
}
fn state(value: &str, code: Option<String>) -> OperationState {
    match value {
        "accepted" => OperationState::Accepted,
        "waiting_config" => OperationState::WaitingConfig,
        "queued" => OperationState::Queued,
        "awaiting_reconciliation" => OperationState::AwaitingReconciliation,
        "delivered" => OperationState::Delivered,
        "completed" => OperationState::Completed,
        "not_delivered" => OperationState::NotDelivered,
        "cancelled" => OperationState::Cancelled,
        _ => OperationState::Failed {
            code: code
                .filter(|code| {
                    code.len() <= 128
                        && code
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                })
                .unwrap_or_else(|| "failed".into()),
        },
    }
}
fn receipt_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<OperationReceipt> {
    Ok(OperationReceipt {
        operation_id: r.get(0)?,
        child_id: r.get(1)?,
        turn_id: r.get(2)?,
        pending_id: r.get(3)?,
        state: state(&r.get::<_, String>(4)?, r.get(5)?),
    })
}
struct QueuedInput<'a> {
    operation: Option<&'a str>,
    key: &'a str,
    digest: &'a str,
    child: &'a str,
    text: &'a str,
    config: &'a SessionConfig,
    tool: &'a str,
}
const RECEIPT: &str = "SELECT operation_id, child_id, turn_id, pending_id, state, code FROM session_operations
    WHERE owner_id = ?1 AND caller_id = ?2 AND operation_key = ?3";

impl Store {
    /// Refuse ineligible callers before any disposable runtime probe. Allocation
    /// repeats these checks under its writer transaction after the probe.
    pub fn precheck_spawn(&self, identity: &SessionIdentity, revision: i64, cwd: Option<&str>) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let caller = self
            .session_tool_context_in(&tx, identity, revision, true)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        self.spawn_precheck_in(&tx, &caller, cwd)?;
        Ok(())
    }
    fn spawn_precheck_in(&self, tx: &Transaction<'_>, caller: &SessionToolContext, cwd: Option<&str>) -> Result<i64> {
        let nested: bool = tx.query_row(
            "SELECT delegated_child = 1 FROM sessions WHERE owner_id = ?1 AND id = ?2",
            params![self.owner_id(), caller.session_id()],
            |r| r.get(0),
        )?;
        ensure!(!nested, "generation_limit");
        if let Some(cwd) = cwd {
            ensure!(hennery_kernel::hats::is_canonical(cwd), "invalid_cwd");
            ensure!(
                crate::delegation_runtime::same_hat_in(tx, self.owner_id(), caller.host_id(), caller.hat_id(), cwd)?,
                "not_found"
            );
        }
        let live: i64 = tx.query_row(
            "SELECT COUNT(*) FROM session_families f JOIN sessions c ON c.owner_id = ?1 AND c.id = f.child_id
            WHERE f.owner_id = ?1 AND f.parent_id = ?2 AND c.lifecycle NOT IN ('closed','archived','deleted')",
            params![self.owner_id(), caller.session_id()],
            |r| r.get(0),
        )?;
        let limits = self.delegation_limits_in(tx)?;
        ensure!(live < i64::from(limits.children), "child_limit");
        let epoch:i64=tx.query_row("SELECT COALESCE(MAX(event_id),0) FROM events WHERE owner_id = ?1 AND session_id = ?2
            AND kind = 'user_turn' AND applied = 1 AND COALESCE(json_extract(body,'$.origin.kind'),'operator') = 'operator'",params![self.owner_id(),caller.session_id()],|r|r.get(0))?;
        let starts: i64 = tx.query_row(
            "SELECT COALESCE((SELECT starts FROM delegation_start_budgets WHERE owner_id = ?1 AND caller_id = ?2 AND epoch = ?3),0)",
            params![self.owner_id(), caller.session_id(), epoch], |r| r.get(0),
        )?;
        ensure!(starts < i64::from(limits.starts_per_operator_epoch), "start_budget");
        Ok(epoch)
    }
    pub fn delegation_retry(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        key: &str,
        digest: &str,
    ) -> Result<Option<OperationReceipt>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let caller = self
            .session_tool_context_in(&tx, identity, revision, true)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        self.retry_in(&tx, &caller, key, digest)
    }
    pub fn delegation_receipt(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        key: &str,
    ) -> Result<Option<OperationReceipt>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let caller = self
            .session_tool_context_in(&tx, identity, revision, true)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        self.receipt_in(&tx, caller.session_id(), key)
    }
    pub(crate) fn receipt_in(&self, tx: &Transaction<'_>, caller: &str, key: &str) -> Result<Option<OperationReceipt>> {
        Ok(tx
            .query_row(RECEIPT, params![self.owner_id(), caller, key], receipt_row)
            .optional()?)
    }
    pub(crate) fn retry_in(
        &self,
        tx: &Transaction<'_>,
        caller: &SessionToolContext,
        key: &str,
        digest: &str,
    ) -> Result<Option<OperationReceipt>> {
        let stored: Option<String> = tx
            .query_row(
                "SELECT digest FROM session_operations WHERE owner_id = ?1 AND caller_id = ?2 AND operation_key = ?3",
                params![self.owner_id(), caller.session_id(), key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(stored) = stored {
            ensure!(stored == digest, "operation_conflict");
            let receipt = self.receipt_in(tx, caller.session_id(), key)?;
            if let Some(receipt) = &receipt {
                if let Some(child) = receipt.child_id.as_deref() {
                    ensure!(self.owned_child_in(tx, caller, child, true)?, "not_found");
                } else {
                    ensure!(
                        matches!(&receipt.state, OperationState::Failed { code } if code == "gone"),
                        "not_found"
                    );
                }
            }
            Ok(receipt)
        } else {
            Ok(None)
        }
    }
    pub(crate) fn owned_child_in(
        &self,
        tx: &Transaction<'_>,
        caller: &SessionToolContext,
        child: &str,
        terminal: bool,
    ) -> Result<bool> {
        Ok(tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_families f
            JOIN sessions c ON c.owner_id = ?1 AND c.id = f.child_id
            WHERE f.owner_id = ?1 AND f.parent_id = ?2 AND f.child_id = ?3 AND f.hat_id = ?4
            AND c.hat_id = ?4 AND c.host_id = ?5 AND c.lifecycle <> 'deleted'
            AND (f.detached = 0 OR (f.reason = 'closed' AND ?6 = 1))
            AND (?6 = 1 OR (f.detached = 0 AND c.close_requested = 0 AND c.lifecycle NOT IN ('closed','archived','failed'))))",
            params![
                self.owner_id(),
                caller.session_id(),
                child,
                caller.hat_id(),
                caller.host_id(),
                terminal
            ],
            |r| r.get(0),
        )?)
    }
    pub fn delegation_queue_depth(&self, child: &str) -> Result<(i64, i64)> {
        let conn = self.conn();
        Ok(conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(payload AS BLOB)) + image_bytes),0) FROM session_dispatch_queue
            WHERE owner_id = ?1 AND target_id = ?2 AND state IN ('queued','waiting_config','awaiting_reconciliation','steering','uncertain')",
            params![self.owner_id(), child],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }
    fn enqueue_in(
        &self,
        tx: &Transaction<'_>,
        caller: &SessionToolContext,
        input: QueuedInput<'_>,
    ) -> Result<OperationReceipt> {
        let QueuedInput {
            operation,
            key,
            digest,
            child,
            text,
            config,
            tool,
        } = input;
        ensure!(text.len() <= 65536, "invalid_arguments");
        let text = hennery_kernel::redact::text(text).unwrap_or_else(|| text.to_owned());
        let (count,bytes):(i64,i64)=tx.query_row("SELECT COUNT(*), COALESCE(SUM(length(CAST(payload AS BLOB)) + image_bytes),0)
            FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id = ?2 AND state IN ('queued','waiting_config','awaiting_reconciliation','steering','uncertain')",
            params![self.owner_id(),child],|r|Ok((r.get(0)?,r.get(1)?)))?;
        ensure!(
            count < MAX_QUEUE && bytes + text.len() as i64 <= MAX_QUEUE_BYTES,
            "queue_full"
        );
        let operation = operation.map(str::to_owned).unwrap_or_else(uuid);
        tx.execute(
            "INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state)
            VALUES (?1,?2,?3,?4,?5,?6,?7,'queued')",
            params![
                self.owner_id(),
                caller.session_id(),
                key,
                operation,
                digest,
                tool,
                child
            ],
        )?;
        tx.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config)
            VALUES (?1,?2,?3,?4,?5,?6,?7)",params![self.owner_id(),operation,caller.session_id(),child,caller.grant_revision(),text,serde_json::to_string(config)?])?;
        Ok(self
            .receipt_in(tx, caller.session_id(), key)?
            .expect("inserted receipt"))
    }
    pub fn queue_followup(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        key: &str,
        digest: &str,
        child: &str,
        text: &str,
    ) -> Result<OperationReceipt> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let caller = self
            .session_tool_context_in(&tx, identity, revision, true)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        if let Some(receipt) = self.retry_in(&tx, &caller, key, digest)? {
            return Ok(receipt);
        }
        ensure!(self.owned_child_in(&tx, &caller, child, false)?, "not_found");
        let raw: Option<String> = tx
            .query_row(
                "SELECT requested_config FROM session_attach_evidence WHERE owner_id = ?1 AND session_id = ?2",
                params![self.owner_id(), child],
                |r| r.get(0),
            )
            .optional()?;
        let config = raw.map(|v| serde_json::from_str(&v)).transpose()?.unwrap_or_default();
        let receipt = self.enqueue_in(
            &tx,
            &caller,
            QueuedInput {
                operation: None,
                key,
                digest,
                child,
                text,
                config: &config,
                tool: "session_send",
            },
        )?;
        tx.commit()?;
        Ok(receipt)
    }
    pub fn queue_spawn(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        digest: &str,
        launch: &DelegatedLaunch<'_>,
    ) -> Result<OperationReceipt> {
        self.queue_spawn_with_operation(identity, revision, digest, launch, None)
    }
    pub(crate) fn queue_spawn_admitted(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        digest: &str,
        launch: &DelegatedLaunch<'_>,
        operation: &str,
    ) -> Result<OperationReceipt> {
        self.queue_spawn_with_operation(identity, revision, digest, launch, Some(operation))
    }
    fn queue_spawn_with_operation(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        digest: &str,
        launch: &DelegatedLaunch<'_>,
        operation: Option<&str>,
    ) -> Result<OperationReceipt> {
        let DelegatedLaunch {
            key,
            agent,
            cwd,
            brief,
            name,
            config,
            mcp,
        } = *launch;
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let caller = self
            .session_tool_context_in(&tx, identity, revision, true)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        if let Some(receipt) = self.retry_in(&tx, &caller, key, digest)? {
            return Ok(receipt);
        }
        let epoch = self.spawn_precheck_in(&tx, &caller, Some(cwd))?;
        tx.execute(
            "INSERT INTO delegation_start_budgets(owner_id,caller_id,epoch,starts) VALUES (?1,?2,?3,1)
            ON CONFLICT(owner_id,caller_id,epoch) DO UPDATE SET starts = starts + 1
            WHERE delegation_start_budgets.owner_id = ?1",
            params![self.owner_id(), caller.session_id(), epoch],
        )?;
        let child = uuid();
        let ts = now();
        tx.execute(
            "INSERT INTO sessions(id,owner_id,host_id,hat_id,agent,cwd,lifecycle,created_at,last_event_at,title,agent_title,title_source)
            VALUES (?1,?2,?3,?4,?5,?6,'starting',?7,?7,?8,?8,CASE WHEN ?8 IS NOT NULL THEN 'agent' END)",
            params![
                child,
                self.owner_id(),
                caller.host_id(),
                caller.hat_id(),
                agent,
                cwd,
                ts,
                name
            ],
        )?;
        ensure!(
            self.bind_family_in(&tx, identity, revision, &child, epoch)?,
            "not_found"
        );
        // Delivery commits at allocation, but launch is performed by the worker
        // with a fresh descriptor on attach (never put a bearer in the queue).
        let (_, cut) = self.deliver_in(&tx, &child, caller.host_id(), caller.hat_id(), mcp, &ts)?;
        let receipt = self.enqueue_in(
            &tx,
            &caller,
            QueuedInput {
                operation,
                key,
                digest,
                child: &child,
                text: brief,
                config,
                tool: "session_spawn",
            },
        )?;
        tx.execute(
            "UPDATE session_operations SET epoch = ?3 WHERE owner_id = ?1 AND operation_id = ?2",
            params![self.owner_id(), receipt.operation_id, epoch],
        )?;
        tx.execute("INSERT INTO session_attach_evidence(owner_id,session_id,attach_generation,requested_config) VALUES (?1,?2,1,?3)",params![self.owner_id(),child,serde_json::to_string(config)?])?;
        crate::store::commit_then_cut(tx, self.mcp().as_ref(), cut)?;
        Ok(receipt)
    }
}
