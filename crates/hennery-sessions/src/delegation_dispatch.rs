//! Dispatch is serialized with every endpoint/policy mutation by the shared
//! database gate, retained across COMMIT and final synchronous host handoff.
use crate::{
    delegation_runtime::{evidence_satisfies, same_hat_in},
    hub::Hub,
    store::{McpContext, Store},
};
use anyhow::Result;
use hennery_proto::{
    delegation_host::{ConfigEvidence, HostCatalog, PromptFence},
    frames::{Capability, CollectorFrame, SessionBody, SessionConfig},
    plugin::InputOrigin,
};
use rusqlite::{OptionalExtension, Transaction, params};

pub const CONFIG_CONFIRMATION_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

fn configuration_mismatch(wanted: &SessionConfig, actual: &SessionConfig) -> Option<String> {
    let mut differences = Vec::new();
    for (axis, requested, reported) in [
        ("model", wanted.model.as_deref(), actual.model.as_deref()),
        ("mode", wanted.mode.as_deref(), actual.mode.as_deref()),
    ] {
        if let Some(requested) = requested
            && Some(requested) != reported
        {
            differences.push(format!(
                "{axis}: requested {requested}, agent reports {}",
                reported.unwrap_or("unknown")
            ));
        }
    }
    for (axis, requested) in &wanted.axes {
        if actual.axes.get(axis) != Some(requested) {
            differences.push(format!(
                "{axis}: requested {}, agent reports {}",
                serde_json::to_string(requested).ok()?,
                actual
                    .axes
                    .get(axis)
                    .and_then(|v| serde_json::to_string(v).ok())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
    }
    if differences.is_empty() {
        return None;
    }
    let text = format!(
        "Configuration not confirmed: {} (requested mode {}; actual mode {})",
        differences.join("; "),
        wanted.mode.as_deref().unwrap_or("default"),
        actual.mode.as_deref().unwrap_or("unknown")
    );
    crate::store::one_line(&crate::redact::shown(&text), 240, 512)
}

impl Store {
    pub fn expect_initial_configuration(&self, session: &str, config: &SessionConfig) -> Result<()> {
        self.conn().execute(
            "INSERT INTO session_attach_evidence(owner_id,session_id,attach_generation,requested_config,launch_sent,config_updated_at)
             SELECT ?1,?2,0,?3,1,?4 WHERE EXISTS(SELECT 1 FROM sessions WHERE owner_id = ?1 AND id=?2 AND lifecycle<>'deleted')
             ON CONFLICT(owner_id,session_id) DO UPDATE SET requested_config=excluded.requested_config,
               evidence=NULL,config_revision=0,config_checked=0,config_updated_at=excluded.config_updated_at WHERE session_attach_evidence.owner_id = ?1",
            params![self.owner_id(),session,serde_json::to_string(config)?,hennery_kernel::secret::unix_now()],
        )?;
        Ok(())
    }

    pub(crate) fn expire_configuration_waits_in(&self, tx: &Transaction<'_>, now: i64) -> Result<()> {
        let candidates = {
            let mut statement = tx.prepare(
                "SELECT s.id,s.hat_id,e.requested_config,e.evidence,e.config_revision FROM sessions s
                 JOIN session_attach_evidence e ON e.owner_id = ?1 AND e.session_id=s.id
                 WHERE s.owner_id = ?1 AND s.lifecycle='active' AND s.close_requested=0
                 AND s.configuration_error IS NULL AND e.evidence IS NOT NULL AND e.config_updated_at<=?2 AND e.config_checked=0
                 AND (EXISTS(SELECT 1 FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id=s.id
                     AND q.state IN ('queued','waiting_config'))
                   OR (s.delegated_child=0 AND NOT EXISTS(SELECT 1 FROM events v WHERE v.owner_id = ?1
                     AND v.session_id=s.id AND v.kind='user_turn' AND v.applied=1)))
                 AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=s.hat_id)
                 ORDER BY s.id LIMIT 128",
            )?;
            statement
                .query_map(
                    params![self.owner_id(), now - CONFIG_CONFIRMATION_WAIT.as_secs() as i64],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, i64>(4)?,
                        ))
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut failed = false;
        for (session, hat, requested, evidence, revision) in candidates {
            tx.execute(
                "UPDATE session_attach_evidence SET config_checked=1 WHERE owner_id = ?1 AND session_id=?2",
                params![self.owner_id(), session],
            )?;
            let requested: SessionConfig = serde_json::from_str(&requested)?;
            let evidence: ConfigEvidence = serde_json::from_str(&evidence)?;
            if evidence.completeness != hennery_proto::delegation_host::ConfigCompleteness::Complete {
                continue;
            }
            let Some(message) = configuration_mismatch(&requested, &evidence.actual) else {
                continue;
            };
            failed = true;
            tx.execute(
                "UPDATE sessions SET configuration_error=?3,failure_reason='configuration_mismatch',family_dirty=1
                WHERE owner_id = ?1 AND id=?2",
                params![self.owner_id(), session, message],
            )?;
            tx.execute(
                "UPDATE session_operations SET state='failed',code='configuration_mismatch'
                WHERE owner_id = ?1 AND child_id=?2 AND state IN ('queued','waiting_config')",
                params![self.owner_id(), session],
            )?;
            crate::store::collector_event(
                tx,
                self.owner_id(),
                &session,
                "start_failed",
                serde_json::json!({"code":"configuration_mismatch","message":message,"error":message}),
                &crate::store::now(),
            )?;
            let incident = format!("configuration:{session}:{revision}");
            tx.execute(
                "INSERT INTO delegation_attention(owner_id,attention_id,child_id,hat_id,reason,state,incident)
                VALUES (?1,?2,?3,?4,'configuration_mismatch','open',?2) ON CONFLICT DO NOTHING",
                params![self.owner_id(), incident, session, hat],
            )?;
        }
        if failed {
            self.settle_delegation_in(tx)?;
        }
        Ok(())
    }
}

const TRANSFER: &str = "SELECT EXISTS(SELECT 1 FROM session_families f
    JOIN sessions p ON p.owner_id = ?1 AND p.id = f.parent_id
    JOIN sessions c ON c.owner_id = ?1 AND c.id = f.child_id
    JOIN hosts h ON h.owner_id = ?1 AND h.id = p.host_id
    JOIN hat_session_tools g ON g.owner_id = ?1 AND g.hat_id = f.hat_id
    WHERE f.owner_id = ?1 AND f.parent_id = ?2 AND f.child_id = ?3 AND f.detached = 0
    AND p.hat_id = f.hat_id AND c.hat_id = f.hat_id AND p.host_id = c.host_id
    AND p.close_requested = 0 AND c.close_requested = 0 AND h.revoked_at IS NULL
    AND p.lifecycle IN ('active','parked') AND p.presumed_parked = 0
    AND c.lifecycle IN ('starting','active','parked') AND c.presumed_parked = 0
    AND g.delegation_enabled = 1 AND g.revision = ?4
    AND (p.mcp_delivery_mode = 'isolated' OR (p.mcp_delivery_mode = 'unisolated' AND p.hat_id = h.default_hat_id))
    AND NOT EXISTS (SELECT 1 FROM purged_hats x WHERE x.owner_id = ?1 AND x.hat_id = f.hat_id))";
impl Store {
    pub(crate) fn transfer_in(&self, tx: &Transaction<'_>, source: &str, target: &str, revision: i64) -> Result<bool> {
        let eligible: bool = tx.query_row(TRANSFER, params![self.owner_id(), source, target, revision], |r| {
            r.get(0)
        })?;
        if !eligible {
            return Ok(false);
        }
        let (capabilities,isolation,agent,mode):(String,Option<String>,String,String)=tx.query_row(
            "SELECT h.capabilities,h.mcp_isolation,p.agent,p.mcp_delivery_mode FROM sessions p JOIN hosts h ON h.owner_id = ?1 AND h.id = p.host_id WHERE p.owner_id = ?1 AND p.id = ?2",
            params![self.owner_id(),source],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let capable = serde_json::from_str::<hennery_proto::frames::Capabilities>(&capabilities)
            .is_ok_and(|c| c.has(Capability::McpServers));
        let isolated = isolation
            .and_then(|s| serde_json::from_str::<hennery_proto::frames::AgentIsolation>(&s).ok())
            .is_some_and(|i| i.isolates(&agent));
        Ok(capable && (mode != "isolated" || isolated))
    }
    pub fn delegation_targets(&self) -> Result<Vec<String>> {
        self.delegation_targets_after("")
    }
    pub fn delegation_targets_after(&self, after: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT DISTINCT target_id FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id > ?2 AND state IN ('queued','waiting_config') ORDER BY target_id LIMIT 128")?;
        Ok(stmt
            .query_map(params![self.owner_id(), after], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn attach_delegated(&self, target: &str, mcp: McpContext, catalog: &HostCatalog, hub: &Hub) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let queue: Option<(String, i64, String, bool)> = tx
            .query_row(
                "SELECT source_id,accepted_revision,required_config,origin='operator' FROM session_dispatch_queue
            WHERE owner_id = ?1 AND target_id = ?2 AND state IN ('queued','waiting_config') ORDER BY position LIMIT 1",
                params![self.owner_id(), target],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((source, revision, required, operator)) = queue else {
            return Ok(false);
        };
        if !self.queue_transfer_in(&tx, &source, target, revision, operator)? {
            return Ok(false);
        }
        let (lifecycle, host, hat, agent, cwd, agent_session): (
            String,
            String,
            String,
            String,
            String,
            Option<String>,
        ) = tx.query_row(
            "SELECT lifecycle,host_id,hat_id,agent,cwd,agent_session_id FROM sessions WHERE owner_id = ?1 AND id = ?2",
            params![self.owner_id(), target],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )?;
        if operator
            && hub.is_ready(&host)
            && (!hub.has_capability(&host, Capability::OperatorResume)
                || !hub.has_capability(&host, Capability::DelegationConfig))
        {
            self.fail_operator_queue_in(&tx, target, "operator_resume_unsupported")?;
            tx.commit()?;
            return Ok(false);
        }
        if !hub.is_ready(&host)
            || !hub.has_capability(&host, Capability::DelegationConfig)
            || !same_hat_in(&tx, self.owner_id(), &host, &hat, &cwd)?
        {
            return Ok(false);
        }
        let previous:Option<(i64,bool)>=tx.query_row("SELECT attach_generation,launch_sent FROM session_attach_evidence WHERE owner_id = ?1 AND session_id = ?2",params![self.owner_id(),target],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        // Separate automatic-resume CAS: never the operator path which accepts
        // closed/failed rows and clears close intent. Starting is only a freshly
        // allocated child whose start has provably not been handed off.
        let new_start = lifecycle == "starting" && previous.is_some_and(|(_, sent)| !sent) && agent_session.is_none();
        if !new_start && lifecycle != "parked" {
            return Ok(false);
        }
        if !new_start && agent_session.is_none() {
            return Ok(false);
        }
        let generation = if new_start {
            previous.unwrap().0
        } else {
            previous.map_or(1, |(g, _)| g + 1)
        };
        let config: SessionConfig = serde_json::from_str(&required)?;
        if catalog.agent != agent
            || !(operator && config == SessionConfig::default()
                || crate::delegation_runtime::config_offered(catalog, &config))
        {
            return Ok(false);
        }
        let changed = tx.execute(
            "UPDATE sessions SET lifecycle = 'starting',activity = NULL
            WHERE owner_id = ?1 AND id = ?2 AND close_requested = 0 AND presumed_parked = 0
            AND (lifecycle = 'parked' OR (?3 = 1 AND lifecycle = 'starting'))",
            params![self.owner_id(), target, new_start],
        )?;
        if changed != 1 {
            return Ok(false);
        }
        tx.execute("INSERT INTO session_attach_evidence(owner_id,session_id,attach_generation,requested_config,launch_sent)
            VALUES (?1,?2,?3,?4,1) ON CONFLICT(owner_id,session_id) DO UPDATE SET attach_generation = excluded.attach_generation,
            config_revision = 0,evidence = NULL,launch_sent = 1",params![self.owner_id(),target,generation,required])?;
        tx.execute("UPDATE session_dispatch_queue SET state = 'waiting_config' WHERE owner_id = ?1 AND target_id = ?2 AND state IN ('queued','waiting_config')",params![self.owner_id(),target])?;
        tx.execute("UPDATE session_operations SET state = 'awaiting_reconciliation' WHERE owner_id = ?1 AND child_id = ?2 AND tool<>'operator_prompt' AND state IN ('queued','waiting_config')",params![self.owner_id(),target])?;
        let committed: i64 = tx.query_row(
            "SELECT COALESCE(MAX(host_seq),0) FROM events WHERE owner_id = ?1 AND session_id = ?2",
            params![self.owner_id(), target],
            |r| r.get(0),
        )?;
        let (given, cut) = self.deliver_in(&tx, target, &host, &hat, mcp, &crate::store::now())?;
        let request_id = uuid::Uuid::now_v7().to_string();
        tx.execute(
            "UPDATE session_attach_evidence SET request_id = ?3 WHERE owner_id = ?1 AND session_id = ?2",
            params![self.owner_id(), target, request_id],
        )?;
        let frame = CollectorFrame::DelegatedAttach {
            operator_initiated: operator,
            request_id,
            session_id: target.into(),
            committed_seq: u64::try_from(committed)?,
            agent,
            cwd,
            config,
            hat_id: hat,
            mcp: given.frame(),
            attach_generation: u64::try_from(generation)?,
            agent_session_id: if new_start { None } else { agent_session },
        };
        crate::store::commit_then_cut(tx, self.mcp().as_ref(), cut)?;
        // conn owns the shared gate here: close cannot commit in this gap.
        if !(if operator {
            hub.notify(&host, frame)
        } else {
            self.handoff_delegated(&conn, &source, &host, frame, hub)?
        }) {
            conn.execute("UPDATE session_operations SET state = 'waiting_config' WHERE owner_id = ?1 AND child_id = ?2 AND turn_id IS NULL AND tool IN ('session_spawn','session_send') AND state = 'awaiting_reconciliation'",params![self.owner_id(),target])?;
            // Proven unsent, so allowing a later attempt cannot duplicate work.
            conn.execute(
                "UPDATE session_attach_evidence SET launch_sent = 0 WHERE owner_id = ?1 AND session_id = ?2",
                params![self.owner_id(), target],
            )?;
            conn.execute(
                "UPDATE sessions SET lifecycle = ?3 WHERE owner_id = ?1 AND id = ?2 AND close_requested = 0",
                params![self.owner_id(), target, if new_start { "starting" } else { "parked" }],
            )?;
            return Ok(false);
        }
        Ok(true)
    }
    pub fn dispatch_delegated(&self, target: &str, catalog: &HostCatalog, hub: &Hub) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let queue: Option<(String, String, i64, String, String,bool)> = tx
            .query_row(
                "SELECT operation_id,source_id,accepted_revision,payload,required_config,origin='operator' FROM session_dispatch_queue
            WHERE owner_id = ?1 AND target_id = ?2 AND state IN ('queued','waiting_config') ORDER BY position LIMIT 1",
                params![self.owner_id(), target],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,r.get(5)?)),
            )
            .optional()?;
        let Some((operation, source, revision, payload, required, operator)) = queue else {
            return Ok(false);
        };
        if operator {
            return Ok(false);
        }
        if !self.queue_transfer_in(&tx, &source, target, revision, false)? {
            return Ok(false);
        }
        let (host,hat,agent,cwd,eligible):(String,String,String,String,bool)=tx.query_row("SELECT host_id,hat_id,agent,cwd,lifecycle = 'active' AND open_turn_id IS NULL AND activity <> 'blocked'
            AND NOT EXISTS (SELECT 1 FROM pending p WHERE p.owner_id = ?1 AND p.session_id = sessions.id AND p.state = 'open')
            FROM sessions WHERE owner_id = ?1 AND id = ?2",params![self.owner_id(),target],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
        if !eligible
            || catalog.agent != agent
            || !hub.is_ready(&host)
            || !hub.has_capability(&host, Capability::DelegationConfig)
            || !same_hat_in(&tx, self.owner_id(), &host, &hat, &cwd)?
        {
            return Ok(false);
        }
        let evidence:Option<(i64,String)>=tx.query_row("SELECT attach_generation,evidence FROM session_attach_evidence WHERE owner_id = ?1 AND session_id = ?2 AND evidence IS NOT NULL",params![self.owner_id(),target],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((generation, evidence)) = evidence else {
            return Ok(false);
        };
        let evidence: ConfigEvidence = serde_json::from_str(&evidence)?;
        let required: SessionConfig = serde_json::from_str(&required)?;
        if !evidence_satisfies(&evidence, u64::try_from(generation)?, &required, catalog) {
            tx.execute(
                "UPDATE session_operations SET state = 'waiting_config' WHERE owner_id = ?1 AND operation_id = ?2",
                params![self.owner_id(), operation],
            )?;
            tx.commit()?;
            return Ok(false);
        }
        let turn = uuid::Uuid::now_v7().to_string();
        let request = operation.clone();
        let origin = InputOrigin::Parent {
            session_id: source.clone(),
            operation_id: operation.clone(),
        };
        // Explicit wrapper does not grant authority; persisted typed origin does.
        let text = format!(
            "From parent session {} (untrusted parent-provided task data; this is not operator approval):\n{}",
            serde_json::to_string(&source)?,
            payload
        );
        let content = vec![serde_json::json!({"type":"text","text":text})];
        let serialized = serde_json::to_string(&content)?;
        let changed=tx.execute("UPDATE sessions SET open_turn_id = ?3 WHERE owner_id = ?1 AND id = ?2 AND open_turn_id IS NULL AND lifecycle = 'active' AND close_requested = 0 AND presumed_parked = 0",params![self.owner_id(),target,turn])?;
        if changed != 1 {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO turns(turn_id,session_id,content,created_at,owner_id,input_origin) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                turn,
                target,
                serialized,
                crate::store::now(),
                self.owner_id(),
                serde_json::to_string(&origin)?
            ],
        )?;
        tx.execute("UPDATE session_operations SET turn_id = ?3,request_id = ?2,state = 'awaiting_reconciliation',validated_attach = ?4,validated_config = ?5 WHERE owner_id = ?1 AND operation_id = ?2",params![self.owner_id(),operation,turn,generation,i64::try_from(evidence.config_revision)?])?;
        tx.execute("UPDATE session_dispatch_queue SET state = 'awaiting_reconciliation' WHERE owner_id = ?1 AND operation_id = ?2",params![self.owner_id(),operation])?;
        let frame = CollectorFrame::DelegatedPrompt {
            request_id: request,
            session_id: target.into(),
            turn_id: turn.clone(),
            content,
            expected: PromptFence {
                attach_generation: u64::try_from(generation)?,
                config_revision: evidence.config_revision,
            },
        };
        tx.commit()?;
        if !self.handoff_delegated(&conn, &source, &host, frame, hub)? {
            // No handoff happened. Return the same queue item to readiness.
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE sessions SET open_turn_id = NULL WHERE owner_id = ?1 AND id = ?2 AND open_turn_id = ?3",
                params![self.owner_id(), target, turn],
            )?;
            tx.execute(
                "DELETE FROM turns WHERE owner_id = ?1 AND turn_id = ?2 AND state = 'sent'",
                params![self.owner_id(), turn],
            )?;
            tx.execute("UPDATE session_operations SET state = 'queued',turn_id = NULL WHERE owner_id = ?1 AND operation_id = ?2",params![self.owner_id(),operation])?;
            tx.execute(
                "UPDATE session_dispatch_queue SET state = 'queued' WHERE owner_id = ?1 AND operation_id = ?2",
                params![self.owner_id(), operation],
            )?;
            tx.commit()?;
            return Ok(false);
        }
        Ok(true)
    }
    pub(crate) fn ingest_delegation_in(&self, tx: &Transaction<'_>, session: &str, body: &SessionBody) -> Result<()> {
        if matches!(
            body,
            SessionBody::TurnEnded {
                outcome: hennery_proto::frames::TurnOutcome::Failed,
                ..
            }
        ) {
            self.fail_operator_queue_in(tx, session, "previous_turn_failed")?;
        }
        if matches!(body, SessionBody::AdapterExited { .. }) {
            self.fail_operator_queue_in(tx, session, "runtime_unavailable")?;
        }
        if matches!(body, SessionBody::SessionStarted { .. }) {
            tx.execute("UPDATE session_operations SET state = 'waiting_config' WHERE owner_id = ?1 AND child_id = ?2 AND turn_id IS NULL AND tool IN ('session_spawn','session_send') AND state = 'awaiting_reconciliation'",params![self.owner_id(),session])?;
        }
        let indexed = match body {
            SessionBody::SessionStarted { indexed, .. }
            | SessionBody::ConfigApplied { indexed, .. }
            | SessionBody::AcpUpdate { indexed, .. } => Some(indexed),
            _ => None,
        };
        if let Some(evidence) = indexed.and_then(|i| i.config_evidence.as_ref()) {
            if matches!(body, SessionBody::SessionStarted { .. }) && evidence.attach_generation == 0 {
                tx.execute("INSERT INTO session_attach_evidence(owner_id,session_id,attach_generation,requested_config,launch_sent)
                    VALUES (?1,?2,0,?3,1) ON CONFLICT(owner_id,session_id) DO UPDATE SET attach_generation = 0,
                    config_revision = 0,evidence = NULL,launch_sent = 1
                    WHERE session_attach_evidence.owner_id = ?1",params![self.owner_id(),session,serde_json::to_string(&evidence.actual)?])?;
            }
            let changed=tx.execute("UPDATE session_attach_evidence SET config_revision = ?4,evidence = ?5,config_updated_at=?6,config_checked=0 WHERE owner_id = ?1 AND session_id = ?2 AND attach_generation = ?3 AND config_revision < ?4
                AND EXISTS (SELECT 1 FROM sessions WHERE owner_id = ?1 AND id = ?2 AND lifecycle IN ('active', 'parked') AND close_requested = 0)",params![self.owner_id(),session,i64::try_from(evidence.attach_generation)?,i64::try_from(evidence.config_revision)?,serde_json::to_string(evidence)?,hennery_kernel::secret::unix_now()])?;
            if changed > 0 {
                let wanted: String = tx.query_row(
                    "SELECT requested_config FROM session_attach_evidence WHERE owner_id = ?1 AND session_id=?2",
                    params![self.owner_id(), session],
                    |r| r.get(0),
                )?;
                if evidence.completeness == hennery_proto::delegation_host::ConfigCompleteness::Complete
                    && configuration_mismatch(&serde_json::from_str(&wanted)?, &evidence.actual).is_none()
                {
                    tx.execute("UPDATE sessions SET configuration_error=NULL,failure_reason=CASE WHEN failure_reason='configuration_mismatch' THEN NULL ELSE failure_reason END WHERE owner_id = ?1 AND id=?2",params![self.owner_id(),session])?;
                    tx.execute("UPDATE delegation_attention SET state='resolved' WHERE owner_id = ?1 AND child_id=?2 AND reason='configuration_mismatch'",params![self.owner_id(),session])?;
                }
            }
        }
        self.settle_delegation_in(tx)
    }
    pub(crate) fn settle_delegation_in(&self, tx: &Transaction<'_>) -> Result<()> {
        self.settle_notices_in(tx)?;
        crate::delegation_attention::reconcile_closed_attention(tx, self.owner_id())?;
        // A terminal lifecycle can race the acknowledgement of a prompt that
        // the adapter actually accepted. Delivery truth wins over the guess.
        tx.execute("UPDATE session_operations SET state=CASE WHEN EXISTS(SELECT 1 FROM turns t WHERE t.owner_id = ?1 AND t.turn_id=session_operations.turn_id AND t.state='ended') THEN 'completed' ELSE 'delivered' END,code=NULL
            WHERE owner_id = ?1 AND tool='operator_prompt' AND state='failed'
            AND EXISTS(SELECT 1 FROM turns t WHERE t.owner_id = ?1 AND t.turn_id=session_operations.turn_id AND t.state IN ('started','ended'))",[self.owner_id()])?;
        tx.execute("UPDATE session_operations SET state = 'completed' WHERE owner_id = ?1 AND tool = 'session_close' AND state = 'awaiting_reconciliation' AND EXISTS (SELECT 1 FROM sessions WHERE owner_id = ?1 AND id = session_operations.child_id AND lifecycle = 'closed')",[self.owner_id()])?;
        tx.execute("UPDATE session_operations SET state = CASE
            WHEN EXISTS (SELECT 1 FROM turns t WHERE t.owner_id = ?1 AND t.turn_id = session_operations.turn_id AND t.state = 'ended') THEN 'completed'
            WHEN EXISTS (SELECT 1 FROM turns t WHERE t.owner_id = ?1 AND t.turn_id = session_operations.turn_id AND t.state = 'started') THEN 'delivered'
            WHEN EXISTS (SELECT 1 FROM turns t WHERE t.owner_id = ?1 AND t.turn_id = session_operations.turn_id AND t.state = 'not_delivered') THEN 'not_delivered'
            WHEN pending_id IS NOT NULL AND EXISTS (SELECT 1 FROM answer_queue a WHERE a.owner_id = ?1 AND a.pending_id = session_operations.pending_id AND a.delivered = 1) THEN 'delivered'
            WHEN pending_id IS NOT NULL AND EXISTS (SELECT 1 FROM answer_queue a WHERE a.owner_id = ?1 AND a.pending_id = session_operations.pending_id AND a.delivered = 0) THEN 'not_delivered'
            ELSE state END WHERE owner_id = ?1 AND state IN ('awaiting_reconciliation','delivered','not_delivered')",[self.owner_id()])?;
        tx.execute("UPDATE session_dispatch_queue SET payload = '',state = 'settled' WHERE origin <> 'operator' AND owner_id = ?1 AND operation_id IN
            (SELECT operation_id FROM session_operations WHERE owner_id = ?1 AND state IN ('completed','delivered','not_delivered','failed','cancelled'))",[self.owner_id()])?;
        // Only hosts promising a complete actor-mailbox snapshot can prove
        // non-admission by absence. Older hosts may still hold this prompt.
        let candidates = {
            let mut statement = tx.prepare("SELECT o.operation_id,h.capabilities FROM session_operations o
                JOIN sessions s ON s.owner_id = ?1 AND s.id=o.child_id
                JOIN hosts h ON h.owner_id = ?1 AND h.id=s.host_id
                WHERE o.owner_id = ?1 AND o.tool='operator_prompt' AND o.state='not_delivered'
                AND EXISTS(SELECT 1 FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.operation_id=o.operation_id
                    AND q.origin='operator' AND q.state='awaiting_reconciliation')")?;
            statement
                .query_map([self.owner_id()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (operation, capabilities) in candidates {
            if serde_json::from_str::<hennery_proto::frames::Capabilities>(&capabilities)
                .is_ok_and(|capabilities| capabilities.has(Capability::PromptMailboxSnapshot))
            {
                tx.execute("UPDATE session_dispatch_queue SET state='queued' WHERE owner_id = ?1 AND operation_id=?2 AND origin='operator' AND state='awaiting_reconciliation'",params![self.owner_id(),operation])?;
            }
        }
        // Preserve the fence before releasing the current attempt association.
        tx.execute("UPDATE turns SET queue_retry_released=1 WHERE owner_id = ?1 AND operator_queued=1 AND state='not_delivered'
            AND turn_id IN (SELECT o.turn_id FROM session_operations o JOIN session_dispatch_queue q
                ON q.owner_id = ?1 AND q.operation_id=o.operation_id
                WHERE o.owner_id = ?1 AND o.tool='operator_prompt' AND o.state='not_delivered' AND q.state='queued')",[self.owner_id()])?;
        tx.execute("UPDATE session_operations SET state='failed',code='not_delivered' WHERE owner_id = ?1 AND tool='operator_prompt' AND state='not_delivered'
            AND operation_id IN (SELECT operation_id FROM session_dispatch_queue WHERE owner_id = ?1 AND origin='operator' AND state='awaiting_reconciliation')",[self.owner_id()])?;
        tx.execute("UPDATE session_operations SET state='queued',turn_id=NULL,request_id=NULL,code=NULL WHERE owner_id = ?1 AND tool='operator_prompt' AND state='not_delivered'
            AND operation_id IN (SELECT operation_id FROM session_dispatch_queue WHERE owner_id = ?1 AND origin='operator' AND state='queued')",[self.owner_id()])?;
        // Permanent failures retain their text/images until dismissal or expiry.
        tx.execute("UPDATE session_operations SET state='failed',code='not_delivered' WHERE owner_id = ?1 AND tool='operator_prompt'
            AND state IN ('queued','waiting_config') AND child_id IN
            (SELECT q.target_id FROM session_dispatch_queue q JOIN session_operations o ON o.owner_id = ?1 AND o.operation_id=q.operation_id
             WHERE q.owner_id = ?1 AND q.origin='operator' AND q.state='awaiting_reconciliation' AND o.state='failed')",[self.owner_id()])?;
        tx.execute(
            "UPDATE session_dispatch_queue SET state='failed' WHERE owner_id = ?1 AND origin='operator'
            AND state IN ('queued','waiting_config','awaiting_reconciliation') AND operation_id IN
            (SELECT operation_id FROM session_operations WHERE owner_id = ?1 AND state IN ('failed','cancelled'))",
            [self.owner_id()],
        )?;
        tx.execute("UPDATE session_dispatch_queue SET state='settled',payload='',image_bytes=0 WHERE owner_id = ?1 AND origin='operator'
            AND state<>'settled' AND operation_id IN
            (SELECT operation_id FROM session_operations WHERE owner_id = ?1 AND state IN ('delivered','completed'))",[self.owner_id()])?;
        tx.execute(
            "DELETE FROM prompt_queue_attachments WHERE owner_id = ?1 AND operation_id IN
            (SELECT operation_id FROM session_dispatch_queue WHERE owner_id = ?1 AND state='settled')",
            [self.owner_id()],
        )?;
        Ok(())
    }
}

impl Store {
    /// A host rejection is proof of non-admission. Match the current host and
    /// request association; a late or foreign rejection cannot undo a new turn.
    pub fn reject_delegation(&self, host: &str, request: &str) -> Result<bool> {
        if self.reject_child_notice(host, request)? {
            return Ok(true);
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let target:Option<String>=tx.query_row("SELECT e.session_id FROM session_attach_evidence e JOIN sessions s ON s.owner_id = ?1 AND s.id = e.session_id
            WHERE e.owner_id = ?1 AND e.request_id = ?2 AND s.host_id = ?3 AND s.lifecycle = 'starting'",params![self.owner_id(),request,host],|r|r.get(0)).optional()?;
        if let Some(target) = target {
            tx.commit()?;
            drop(conn);
            self.mark_failed_if_starting(&target, "delegated_attach_rejected")?;
            return Ok(true);
        }
        let turn:Option<(String,String)>=tx.query_row("SELECT o.child_id,o.turn_id FROM session_operations o JOIN sessions s ON s.owner_id = ?1 AND s.id = o.child_id
            WHERE o.owner_id = ?1 AND o.request_id = ?2 AND s.host_id = ?3 AND o.state = 'awaiting_reconciliation' AND o.turn_id IS NOT NULL",params![self.owner_id(),request,host],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((target, turn)) = turn else { return Ok(false) };
        tx.execute(
            "UPDATE turns SET state = 'not_delivered' WHERE owner_id = ?1 AND turn_id = ?2 AND state = 'sent'",
            params![self.owner_id(), turn],
        )?;
        tx.execute("UPDATE sessions SET open_turn_id = NULL WHERE owner_id = ?1 AND id = ?2 AND open_turn_id = ?3 AND EXISTS (SELECT 1 FROM turns WHERE owner_id = ?1 AND turn_id = ?3 AND state = 'not_delivered')",params![self.owner_id(),target,turn])?;
        self.settle_delegation_in(&tx)?;
        tx.commit()?;
        Ok(true)
    }
}

impl Store {
    pub(crate) fn handoff_delegated(
        &self,
        conn: &rusqlite::Connection,
        source: &str,
        host: &str,
        frame: CollectorFrame,
        hub: &Hub,
    ) -> Result<bool> {
        // A parked child became starting in the committed attach transaction.
        // Its parent's hold must precede child handoff, while this gate is held.
        // Refuse through the caller's existing compensation on a failed flush.
        if let Err(error) = self.flush_idle_holds_in(conn, hub) {
            tracing::warn!(%error, "delegated handoff deferred by idle hold flush");
            return Ok(false);
        }
        let (agent, isolated): (String, bool) = conn.query_row(
            "SELECT agent,mcp_delivery_mode = 'isolated' FROM sessions WHERE owner_id = ?1 AND id = ?2",
            params![self.owner_id(), source],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(hub.notify_parent_transfer(host, &agent, isolated, frame))
    }
}

/// A read-only head snapshot. Attach repeats its transaction-bound predicates
/// after I/O; a handed-off start is never a new catalog-discovery candidate.
pub(crate) struct AttachCandidate {
    pub operation: String,
    pub operator: bool,
    pub required: SessionConfig,
    pub refusal: Option<&'static str>,
}
// The outer None defers discovery for an offline/unreconciled host. A ready
// host can either attach (inner None) or establish a permanent refusal.
pub(crate) fn attach_availability(hub: &Hub, host: &str, record: bool) -> Option<Option<&'static str>> {
    let capable = hub.ready_capability(host, Capability::DelegationConfig)?;
    Some(if !record {
        Some("agent_has_no_record")
    } else if !capable {
        Some("configuration_unavailable")
    } else {
        None
    })
}

impl Store {
    pub(crate) fn delegated_attach_candidate(&self, target: &str, hub: &Hub) -> Result<Option<AttachCandidate>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let head: Option<(String, String, i64, String,bool)> = tx
            .query_row(
                "SELECT operation_id,source_id,accepted_revision,required_config,origin='operator' FROM session_dispatch_queue
             WHERE owner_id = ?1 AND target_id=?2 AND state IN ('queued','waiting_config') ORDER BY position LIMIT 1",
                params![self.owner_id(), target],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?,r.get(4)?)),
            )
            .optional()?;
        let Some((operation, source, revision, required, operator)) = head else {
            return Ok(None);
        };
        if !self.queue_transfer_in(&tx, &source, target, revision, operator)? {
            return Ok(None);
        }
        let (lifecycle, host, hat, cwd, record): (String, String, String, String, Option<String>) = tx.query_row(
            "SELECT lifecycle,host_id,hat_id,cwd,agent_session_id FROM sessions WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), target],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        if !same_hat_in(&tx, self.owner_id(), &host, &hat, &cwd)? {
            return Ok(None);
        }
        let possible = match lifecycle.as_str() {
            "parked" => record.is_some(),
            "starting" => {
                let sent: Option<bool> = tx
                    .query_row(
                        "SELECT launch_sent FROM session_attach_evidence WHERE owner_id = ?1 AND session_id=?2",
                        params![self.owner_id(), target],
                        |r| r.get(0),
                    )
                    .optional()?;
                if sent != Some(false) || record.is_some() {
                    return Ok(None);
                }
                true
            }
            _ => return Ok(None),
        };
        let Some(mut refusal) = attach_availability(hub, &host, possible) else {
            return Ok(None);
        };
        if operator
            && (!hub.has_capability(&host, Capability::OperatorResume)
                || !hub.has_capability(&host, Capability::DelegationConfig))
        {
            refusal = Some("operator_resume_unsupported");
        }
        Ok(Some(AttachCandidate {
            operator,
            operation,
            required: serde_json::from_str(&required)?,
            refusal,
        }))
    }
}

#[cfg(test)]
mod availability_tests {
    use super::*;
    use hennery_proto::frames::{AgentIsolation, Capabilities};

    #[test]
    fn disconnect_after_ready_check_defers_attach_instead_of_refusing() {
        let hub = Hub::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = hub
            .register(
                "host",
                tx,
                Capabilities(vec![Capability::DelegationConfig]),
                AgentIsolation::default(),
            )
            .unwrap();
        hub.mark_ready("host", reg.conn_id);
        assert!(hub.is_ready("host"));
        hub.unregister("host", reg.conn_id);
        assert_eq!(attach_availability(&hub, "host", true), None);
        assert_eq!(attach_availability(&hub, "host", false), None);
    }

    #[test]
    fn unreconciled_host_defers_attach_even_when_capability_is_missing() {
        let hub = Hub::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        hub.register("host", tx, Capabilities::default(), AgentIsolation::default())
            .unwrap();
        assert_eq!(attach_availability(&hub, "host", true), None);
    }

    #[test]
    fn ready_host_distinguishes_missing_record_from_unsupported_attach() {
        for (capable, record, expected) in [
            (true, true, None),
            (true, false, Some("agent_has_no_record")),
            (false, true, Some("configuration_unavailable")),
        ] {
            let hub = Hub::new();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let caps = if capable {
                vec![Capability::DelegationConfig]
            } else {
                vec![]
            };
            let reg = hub
                .register("host", tx, Capabilities(caps), AgentIsolation::default())
                .unwrap();
            hub.mark_ready("host", reg.conn_id);
            assert_eq!(attach_availability(&hub, "host", record), Some(expected));
        }
    }
}
