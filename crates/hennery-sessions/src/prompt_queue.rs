//! Operator prompts share the delegated dispatch FIFO and its durable receipts.
use crate::{
    content::{Checked, MAX_TEXT_TOTAL},
    delegation_queue::{MAX_QUEUE, MAX_QUEUE_BYTES},
    hub::Hub,
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use hennery_proto::{
    frames::{Capability, CollectorFrame},
    rest::{QueuedPrompt, StoredBlock},
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::BTreeSet;

fn bounds(tx: &Transaction<'_>, owner: &str, target: &str, exclude: &str, bytes: i64) -> Result<()> {
    let (count, used): (i64, i64) = tx.query_row(
        "SELECT COUNT(*),COALESCE(SUM(length(CAST(payload AS BLOB))+image_bytes),0)
         FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id=?2 AND operation_id<>?3
         AND state IN ('queued','waiting_config','awaiting_reconciliation','steering','uncertain')",
        params![owner, target, exclude],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(count < MAX_QUEUE && used + bytes <= MAX_QUEUE_BYTES, "queue_full");
    Ok(())
}

fn hashes(tx: &Transaction<'_>, owner: &str, operation: &str) -> Result<BTreeSet<String>> {
    let mut stmt = tx.prepare("SELECT sha256 FROM prompt_queue_attachments WHERE owner_id = ?1 AND operation_id=?2")?;
    Ok(stmt
        .query_map(params![owner, operation], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn failure_reason(code: &str) -> String {
    if let Some(reason) = code.strip_prefix("steering_rejected:") {
        return format!("Steering was rejected. {reason}");
    }
    match code {
        "session_closed" | "endpoint_unavailable" => {
            "The session was closed. Copy this message to send it in another session."
        }
        "hat_changed" => "The session moved to another hat. Review it before sending again.",
        "auth_required" | "agent_not_logged_in" => "The agent is signed out. Sign in before sending again.",
        "configuration_mismatch" | "configuration_unavailable" => {
            "The agent could not confirm the session settings. Review them before sending again."
        }
        "not_delivered" => "The host did not confirm delivery. Check the transcript before sending again.",
        "runtime_unavailable" => "The agent runtime is unavailable. Restore it before sending again.",
        "agent_has_no_record" => "The agent no longer has this session. Copy this message to a new session.",
        "previous_turn_failed" => "The previous turn failed. Review the transcript before sending again.",
        "image_unavailable" => "An attached image is unavailable. Attach it again before sending.",
        "host_revoked" => "The host was revoked. Copy this message to a session on a paired host.",
        "host_offline" => "The host is offline. Copy or resend this message after it reconnects.",
        "operator_resume_unsupported" => {
            "This host needs hennery 0.3.0 to resume and send. Update the host before sending again."
        }
        _ => "The session could not accept this message. Review its status before sending again.",
    }
    .into()
}

impl Store {
    /// Rehydrate the stored queue payload for either ordinary or steering admission.
    pub(crate) fn queued_content(
        &self,
        blocks: &[StoredBlock],
        images: bool,
        files: bool,
    ) -> Result<Option<Vec<serde_json::Value>>> {
        let mut sent = Vec::with_capacity(blocks.len());
        for block in blocks {
            match block {
                StoredBlock::Text { text } => sent.push(serde_json::json!({"type":"text","text":text})),
                StoredBlock::Image { mime_type, sha256, .. } => {
                    let Some(bytes) = self
                        .attachments
                        .as_deref()
                        .filter(|_| images)
                        .and_then(|dir| crate::attachments::read(dir, sha256).ok().flatten())
                    else {
                        return Ok(None);
                    };
                    sent.push(serde_json::json!({"type":"image","mimeType":mime_type,"data":STANDARD.encode(bytes)}));
                }
                StoredBlock::File {
                    name,
                    mime_type,
                    sha256,
                    ..
                } => {
                    let Some(bytes) = self
                        .attachments
                        .as_deref()
                        .filter(|_| files)
                        .and_then(|dir| crate::attachments::read(dir, sha256).ok().flatten())
                    else {
                        return Ok(None);
                    };
                    sent.push(serde_json::json!({"type":"file","name":name,"mimeType":mime_type,"data":STANDARD.encode(bytes)}));
                }
                StoredBlock::PluginContext { .. } => anyhow::bail!("invalid queued content"),
            }
        }
        Ok(Some(sent))
    }

    /// Also used to keep immediate prompts behind an accepted FIFO head.
    pub fn prompt_must_queue(&self, session: &str) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT open_turn_id IS NOT NULL OR lifecycle='parked' OR activity='blocked' OR prompt_queue_held<>0
             OR EXISTS(SELECT 1 FROM pending p WHERE p.owner_id = ?1 AND p.session_id=?2 AND p.state='open')
             OR EXISTS(SELECT 1 FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id=?2 AND q.state IN ('queued','waiting_config','awaiting_reconciliation','steering'))
             FROM sessions WHERE owner_id = ?1 AND id=?2", params![self.owner_id(),session], |r| r.get(0),
        )?)
    }

    /// Materialize a validated attachment under the same lock and transaction as its queue or event link.
    pub(super) fn store_queued_attachment_in(&self, tx: &Transaction<'_>, image: &crate::content::Image) -> Result<()> {
        tx.execute(
            "INSERT INTO attachments(owner_id,sha256,mime,size,created_at) VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(owner_id,sha256) DO NOTHING",
            params![
                self.owner_id(),
                image.sha256,
                image.mime,
                image.bytes.len() as i64,
                now()
            ],
        )?;
        let dir = self.attachments.as_deref().context("attachment storage unavailable")?;
        crate::attachments::write(dir, &image.sha256, &image.bytes)?;
        Ok(())
    }

    pub fn enqueue_operator_prompt(&self, session: &str, prompt: &Checked) -> Result<String> {
        match self.enqueue_operator_prompt_with_key(session, prompt, None)? {
            crate::prompt_receipts::PromptReceipt::Queued { queue_id } => Ok(queue_id),
            crate::prompt_receipts::PromptReceipt::Turn { .. } => anyhow::bail!("unexpected turn receipt"),
        }
    }

    pub fn enqueue_operator_prompt_with_key(
        &self,
        session: &str,
        prompt: &Checked,
        key: Option<&crate::prompt_receipts::PromptRequestKey>,
    ) -> Result<crate::prompt_receipts::PromptReceipt> {
        self.admit_operator_followup(session, prompt, key, None)
    }

    pub(crate) fn admit_operator_followup(
        &self,
        session: &str,
        prompt: &Checked,
        key: Option<&crate::prompt_receipts::PromptRequestKey>,
        steering: Option<&Hub>,
    ) -> Result<crate::prompt_receipts::PromptReceipt> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(receipt) = crate::prompt_receipts::lookup_in(&tx, self.owner_id(), session, key)? {
            return Ok(receipt);
        }
        let revoked: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions s JOIN hosts h ON h.owner_id=s.owner_id AND h.id=s.host_id WHERE s.owner_id = ?1 AND s.id=?2 AND h.revoked_at IS NOT NULL)",params![self.owner_id(),session],|r|r.get(0))?;
        ensure!(!revoked, "host_revoked");
        let eligible: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions s WHERE s.owner_id = ?1 AND s.id=?2
             AND s.lifecycle IN ('active','parked') AND s.close_requested=0 AND s.configuration_error IS NULL
             AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id=s.hat_id))",
            params![self.owner_id(), session],
            |r| r.get(0),
        )?;
        ensure!(eligible, "queue_unavailable");
        let payload = serde_json::to_string(&prompt.stored)?;
        let image_bytes = prompt.images.iter().map(|i| i.bytes.len() as i64).sum::<i64>();
        bounds(&tx, self.owner_id(), session, "", payload.len() as i64 + image_bytes)?;
        let id = uuid::Uuid::now_v7().to_string();
        tx.execute(
            "INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state)
             VALUES (?1,?2,?3,?3,'','operator_prompt',?2,'queued')",
            params![self.owner_id(), session, id],
        )?;
        // Operator input has no delegation grant. The current configuration is
        // retained only for the existing automatic-resume confirmation gate.
        let mut config = crate::store::stored_config(tx.query_row(
            "SELECT model,mode,config_axes FROM sessions WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), session],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)?;
        config.protocol = crate::background::requested_protocol(&tx, self.owner_id(), session)?;
        let config = serde_json::to_string(&config)?;
        tx.execute(
            "INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config,origin,image_bytes)
             VALUES (?1,?2,?3,?3,0,?4,?5,'operator',?6)", params![self.owner_id(),id,session,payload,config,image_bytes],
        )?;
        for (position, image) in prompt.images.iter().enumerate() {
            self.store_queued_attachment_in(&tx, image)?;
            tx.execute(
                "INSERT INTO prompt_queue_attachments(owner_id,operation_id,sha256,position) VALUES (?1,?2,?3,?4)",
                params![self.owner_id(), id, image.sha256, position as i64],
            )?;
        }
        tx.execute(
            "UPDATE sessions SET prompt_queue_held=CASE WHEN lifecycle='active' AND prompt_queue_held<>0 THEN 2 ELSE 0 END WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), session],
        )?;
        // Only the initial admission can steer. A replay after promptRequired
        // keeps its original queue receipt even if another turn has since begun.
        let handoff = if let Some(hub) = steering {
            self.prepare_steering_in(&tx, session, &id, hub, crate::steering::SteeringMode::WhenAvailable)?
                .map(|attempt| (attempt, hub))
        } else {
            None
        };
        let receipt = crate::prompt_receipts::PromptReceipt::Queued { queue_id: id };
        crate::prompt_receipts::record_in(&tx, self.owner_id(), session, key, &receipt)?;
        tx.commit()?;
        if let Some((attempt, hub)) = handoff
            && let Err(err) = attempt.deliver(self, &mut conn, session, hub)
        {
            // Acceptance is durable. The steering deadline retains an unresolved
            // attempt for operator recovery if recording the unsent result failed.
            tracing::warn!(session_id = session, error = %err, "steering settlement deferred");
        }
        Ok(receipt)
    }

    pub fn operator_prompt_queue(&self, session: &str) -> Result<Vec<QueuedPrompt>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT q.operation_id,q.payload,q.state,o.code FROM session_dispatch_queue q
             JOIN session_operations o ON o.owner_id = ?1 AND o.operation_id=q.operation_id
             WHERE q.owner_id = ?1 AND q.target_id=?2 AND q.origin='operator'
             AND q.state IN ('queued','waiting_config','awaiting_reconciliation','steering','uncertain','failed') ORDER BY q.position",
        )?;
        let raw = stmt
            .query_map(params![self.owner_id(), session], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.into_iter()
            .map(|(id, payload, state, code)| {
                Ok(QueuedPrompt {
                    id,
                    content: serde_json::from_str(&payload)?,
                    reason: match state.as_str() {
                        "uncertain" => Some(
                            "The host could not confirm steering delivery. Check the transcript before resending."
                                .into(),
                        ),
                        "failed" => Some(failure_reason(code.as_deref().unwrap_or("failed"))),
                        _ => None,
                    },
                    state: match state.as_str() {
                        "awaiting_reconciliation" | "steering" => "delivering",
                        "uncertain" => "uncertain",
                        "failed" => "failed",
                        _ => "queued",
                    }
                    .into(),
                })
            })
            .collect()
    }

    pub fn edit_operator_prompt(&self, session: &str, id: &str, text: &str) -> Result<()> {
        ensure!(text.len() <= MAX_TEXT_TOTAL, "content_too_large");
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let old: Option<(String,i64)> = tx.query_row(
            "SELECT payload,image_bytes FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id=?2 AND operation_id=?3
             AND origin='operator' AND state IN ('queued','waiting_config')",
            params![self.owner_id(),session,id], |r|Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        let (old, image_bytes) = old.context("queue_item_unavailable")?;
        let blocks: Vec<StoredBlock> = serde_json::from_str(&old)?;
        let mut content = vec![StoredBlock::Text { text: text.into() }];
        content.extend(
            blocks
                .into_iter()
                .filter(|b| matches!(b, StoredBlock::Image { .. } | StoredBlock::File { .. })),
        );
        ensure!(!text.trim().is_empty() || content.len() > 1, "empty_prompt");
        let payload = serde_json::to_string(&content)?;
        bounds(&tx, self.owner_id(), session, id, payload.len() as i64 + image_bytes)?;
        tx.execute(
            "UPDATE session_dispatch_queue SET payload=?4 WHERE owner_id = ?1 AND target_id=?2 AND operation_id=?3",
            params![self.owner_id(), session, id, payload],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_operator_prompt(&self, session: &str, id: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let images = hashes(&tx, self.owner_id(), id)?;
        let changed=tx.execute(
            "DELETE FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id=?2 AND operation_id=?3 AND origin='operator'
             AND state IN ('queued','waiting_config','failed','uncertain')",params![self.owner_id(),session,id],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        tx.execute("DELETE FROM session_operations WHERE owner_id = ?1 AND caller_id=?2 AND operation_id=?3 AND tool='operator_prompt'",params![self.owner_id(),session,id])?;
        let dropped = crate::store::drop_unreferenced(&tx, self.owner_id(), &images)?;
        tx.commit()?;
        self.remove_files(&conn, &dropped);
        Ok(true)
    }

    /// A self-source operator row uses owner/session authority; family rows
    /// retain the existing family/grant checks. Callers select a stored head.
    pub(crate) fn queue_transfer_in(
        &self,
        tx: &Transaction<'_>,
        source: &str,
        target: &str,
        revision: i64,
        operator: bool,
    ) -> Result<bool> {
        if self.operator_queue_fenced_in(tx, target)? {
            return Ok(false);
        }
        let held: bool = tx.query_row(
            "SELECT prompt_queue_held FROM sessions WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), target],
            |r| r.get(0),
        )?;
        if held {
            return Ok(false);
        }
        if operator {
            return Ok(tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions s WHERE s.owner_id = ?1 AND s.id=?2
                 AND s.close_requested=0 AND s.presumed_parked=0 AND s.lifecycle IN ('active','parked') AND s.prompt_queue_held=0
                 AND s.configuration_error IS NULL
                 AND NOT EXISTS(SELECT 1 FROM hosts h WHERE h.owner_id = ?1 AND h.id=s.host_id AND h.revoked_at IS NOT NULL)
                 AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=s.hat_id)
                 AND EXISTS(SELECT 1 FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id=?2
                     AND q.origin='operator' AND q.state IN ('queued','waiting_config')))",
                params![self.owner_id(), target],
                |r| r.get(0),
            )?);
        }
        self.transfer_in(tx, source, target, revision)
    }

    pub(crate) fn operator_dispatch_id(&self, session: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT operation_id FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id=?2
            AND state IN ('queued','waiting_config') ORDER BY position LIMIT 1",
                params![self.owner_id(), session],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Only an operator FIFO head with no open request can be prepared.
    pub fn operator_dispatch_ready(&self, session: &str) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions s WHERE s.owner_id = ?1 AND s.id=?2 AND s.lifecycle='active'
             AND s.open_turn_id IS NULL AND COALESCE(s.activity,'idle') NOT IN ('blocked','running') AND s.close_requested=0
             AND s.configuration_error IS NULL AND s.presumed_parked=0 AND s.prompt_queue_held=0
             AND NOT EXISTS(SELECT 1 FROM hosts h WHERE h.owner_id = ?1 AND h.id=s.host_id AND h.revoked_at IS NOT NULL)
             AND NOT EXISTS(SELECT 1 FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id=?2 AND q.origin='operator' AND q.state IN ('awaiting_reconciliation','steering'))
             AND NOT EXISTS(SELECT 1 FROM pending p WHERE p.owner_id = ?1 AND p.session_id=?2 AND p.state='open')
             AND (SELECT origin FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id=?2
                 AND q.state IN ('queued','waiting_config') ORDER BY position LIMIT 1)='operator'
             AND (SELECT retry_after FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id=?2
                 AND q.state IN ('queued','waiting_config') ORDER BY position LIMIT 1)<=CAST(round(unixepoch('subsec')*1000) AS INTEGER))",
            params![self.owner_id(), session],
            |r| r.get(0),
        )?)
    }

    /// Allocation and final handoff share the writer gate. No retry follows a
    /// successful handoff: host facts settle the immutable request/turn mapping.
    pub fn dispatch_operator_prompt(
        &self,
        session: &str,
        hub: &Hub,
        startup: &mut Option<hennery_proto::plugin::StartupRecord>,
        catalog: Option<&hennery_proto::delegation_host::HostCatalog>,
    ) -> Result<bool> {
        let request = startup
            .as_ref()
            .map(|record| record.request_id.clone())
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        self.dispatch_operator_prompt_with_request(session, hub, startup, catalog, &request)
    }

    /// The worker allocates this attempt's request before fetching startup
    /// context, so the context and host correlation name the same attempt.
    pub fn dispatch_operator_prompt_with_request(
        &self,
        session: &str,
        hub: &Hub,
        startup: &mut Option<hennery_proto::plugin::StartupRecord>,
        catalog: Option<&hennery_proto::delegation_host::HostCatalog>,
        request: &str,
    ) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if self.operator_queue_fenced_in(&tx, session)? {
            return Ok(false);
        }
        let head: Option<(String, String, String, String, bool)> = tx
            .query_row(
                "SELECT q.operation_id,q.payload,s.host_id,q.state,
             s.lifecycle='active' AND s.open_turn_id IS NULL AND COALESCE(s.activity,'idle') NOT IN ('blocked','running')
             AND s.close_requested=0 AND s.presumed_parked=0 AND s.configuration_error IS NULL AND s.prompt_queue_held=0 AND q.retry_after<=CAST(round(unixepoch('subsec')*1000) AS INTEGER)
             AND NOT EXISTS(SELECT 1 FROM hosts h WHERE h.owner_id = ?1 AND h.id=s.host_id AND h.revoked_at IS NOT NULL)
             AND NOT EXISTS(SELECT 1 FROM pending p WHERE p.owner_id = ?1 AND p.session_id=s.id AND p.state='open')
             AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=s.hat_id)
             FROM session_dispatch_queue q JOIN sessions s ON s.owner_id = ?1 AND s.id=q.target_id
             WHERE q.owner_id = ?1 AND q.target_id=?2 AND q.state IN ('queued','waiting_config')
             ORDER BY q.position LIMIT 1",
                params![self.owner_id(), session],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((id, payload, host, queue_state, eligible)) = head else {
            return Ok(false);
        };
        let operator: bool = tx.query_row(
            "SELECT origin='operator' FROM session_dispatch_queue WHERE owner_id = ?1 AND operation_id=?2",
            params![self.owner_id(), id],
            |r| r.get(0),
        )?;
        if !operator || !eligible || !hub.is_ready(&host) {
            return Ok(false);
        }
        // A resumed adapter confirms the requested configuration before the
        // first queued prompt. Already attached ordinary prompts need no new
        // adapter capability or model catalog probe.
        if queue_state == "waiting_config" {
            let Some(catalog) = catalog else { return Ok(false) };
            let evidence:Option<(i64,String,String)>=tx.query_row(
                "SELECT e.attach_generation,e.evidence,q.required_config FROM session_attach_evidence e JOIN session_dispatch_queue q
                 ON q.owner_id = ?1 AND q.operation_id=?2 WHERE e.owner_id = ?1 AND e.session_id=?3 AND e.evidence IS NOT NULL",
                params![self.owner_id(),id,session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            ).optional()?;
            let Some((generation, evidence, required)) = evidence else {
                return Ok(false);
            };
            let evidence: hennery_proto::delegation_host::ConfigEvidence = serde_json::from_str(&evidence)?;
            let required: hennery_proto::frames::SessionConfig = serde_json::from_str(&required)?;
            let confirmed = if required == hennery_proto::frames::SessionConfig::default() {
                // With no requested settings, attachment evidence is enough,
                // as on an ordinary operator resume; delegated checks stay strict.
                evidence.attach_generation == u64::try_from(generation)? && evidence.config_revision > 0
            } else {
                crate::delegation_runtime::evidence_satisfies(&evidence, u64::try_from(generation)?, &required, catalog)
            };
            if !confirmed {
                return Ok(false);
            };
        }
        let stored: Vec<StoredBlock> = serde_json::from_str(&payload)?;
        let Some(sent) = self.queued_content(
            &stored,
            hub.has_capability(&host, Capability::Images),
            hub.has_capability(&host, Capability::Files),
        )?
        else {
            self.fail_operator_item_in(&tx, &id, "image_unavailable")?;
            tx.commit()?;
            return Ok(false);
        };
        let turn = uuid::Uuid::now_v7().to_string();
        tx.execute(
            "UPDATE sessions SET open_turn_id=?3 WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), session, turn],
        )?;
        crate::startup::prepare_in(self, &tx, session, &turn, startup)?;
        let mut content = stored
            .iter()
            .map(serde_json::to_value)
            .collect::<serde_json::Result<Vec<_>>>()?;
        if let Some(record) = startup.as_ref() {
            content.insert(
                0,
                serde_json::to_value(StoredBlock::PluginContext {
                    startup: record.clone(),
                })?,
            );
        }
        tx.execute(
            "INSERT INTO turns(owner_id,session_id,turn_id,content,created_at,operator_queued) VALUES (?1,?2,?3,?4,?5,1)",
            params![self.owner_id(), session, turn, serde_json::to_string(&content)?, now()],
        )?;
        crate::store::link_turn_attachments(&tx, self.owner_id(), &turn, &content)?;
        tx.execute("UPDATE session_operations SET state='awaiting_reconciliation',turn_id=?3,request_id=?4 WHERE owner_id = ?1 AND operation_id=?2",params![self.owner_id(),id,turn,request])?;
        tx.execute(
            "UPDATE session_dispatch_queue SET state='awaiting_reconciliation' WHERE owner_id = ?1 AND operation_id=?2",
            params![self.owner_id(), id],
        )?;
        tx.commit()?;
        let frame = CollectorFrame::Prompt {
            request_id: request.into(),
            session_id: session.into(),
            turn_id: turn.clone(),
            content: sent,
            plugin_context: startup.as_ref().and_then(|s| s.block.clone()),
        };
        if !hub.notify(&host, frame) {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE sessions SET open_turn_id=NULL WHERE owner_id = ?1 AND id=?2 AND open_turn_id=?3",
                params![self.owner_id(), session, turn],
            )?;
            tx.execute(
                "DELETE FROM turns WHERE owner_id = ?1 AND turn_id=?2 AND state='sent'",
                params![self.owner_id(), turn],
            )?;
            tx.execute("UPDATE session_operations SET state='queued',turn_id=NULL,request_id=NULL WHERE owner_id = ?1 AND operation_id=?2",params![self.owner_id(),id])?;
            tx.execute(
                "UPDATE session_dispatch_queue SET state='queued' WHERE owner_id = ?1 AND operation_id=?2",
                params![self.owner_id(), id],
            )?;
            tx.commit()?;
            return Ok(false);
        }
        Ok(true)
    }

    pub(crate) fn fail_operator_queue_in(&self, tx: &Transaction<'_>, session: &str, code: &str) -> Result<()> {
        tx.execute(
            "UPDATE session_dispatch_queue SET steering_failure=COALESCE(steering_failure,?3)
             WHERE owner_id = ?1 AND target_id=?2 AND state IN ('steering','uncertain')",
            params![self.owner_id(), session, code],
        )?;
        tx.execute("UPDATE session_operations SET state='failed',code=?3 WHERE owner_id = ?1 AND child_id=?2 AND tool='operator_prompt' AND state IN ('queued','waiting_config')",params![self.owner_id(),session,code])?;
        tx.execute("UPDATE session_dispatch_queue SET state='failed' WHERE owner_id = ?1 AND target_id=?2 AND origin='operator' AND state IN ('queued','waiting_config')",params![self.owner_id(),session])?;
        Ok(())
    }

    pub(crate) fn operator_queue_fenced_in(&self, tx: &Transaction<'_>, session: &str) -> Result<bool> {
        Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id=?2 AND origin='operator' AND state IN ('awaiting_reconciliation','steering'))",params![self.owner_id(),session],|r|r.get(0))?)
    }

    pub(crate) fn fail_operator_item_in(&self, tx: &Transaction<'_>, id: &str, code: &str) -> Result<()> {
        tx.execute("UPDATE session_operations SET state='failed',code=?3 WHERE owner_id = ?1 AND operation_id=?2 AND tool='operator_prompt' AND state IN ('queued','waiting_config','awaiting_reconciliation')",params![self.owner_id(),id,code])?;
        tx.execute("UPDATE session_dispatch_queue SET state='failed' WHERE owner_id = ?1 AND operation_id=?2 AND origin='operator' AND state IN ('queued','waiting_config','awaiting_reconciliation')",params![self.owner_id(),id])?;
        Ok(())
    }

    pub(crate) fn fail_unavailable_operator_queue(&self, session: &str, code: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        self.fail_operator_queue_in(&tx, session, code)?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn expire_operator_prompts(&self, now: i64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ids = {
            let mut stmt=tx.prepare("SELECT operation_id,target_id FROM session_dispatch_queue WHERE owner_id = ?1 AND origin='operator' AND state='failed' AND failed_at<=?2 ORDER BY position LIMIT 128")?;
            stmt.query_map(params![self.owner_id(), now - 7 * 24 * 60 * 60], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut images = BTreeSet::new();
        for (id, session) in ids {
            images.extend(hashes(&tx, self.owner_id(), &id)?);
            tx.execute("DELETE FROM session_dispatch_queue WHERE owner_id = ?1 AND operation_id=?2 AND origin='operator' AND state='failed'",params![self.owner_id(),id])?;
            tx.execute("DELETE FROM session_operations WHERE owner_id = ?1 AND caller_id=?2 AND operation_id=?3 AND tool='operator_prompt'",params![self.owner_id(),session,id])?;
        }
        let dropped = crate::store::drop_unreferenced(&tx, self.owner_id(), &images)?;
        tx.commit()?;
        self.remove_files(&conn, &dropped);
        Ok(())
    }

    pub fn reject_operator_prompt(&self, host: &str, request: &str, code: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row:Option<(String,String,String)>=tx.query_row(
            "SELECT o.operation_id,o.child_id,o.turn_id FROM session_operations o JOIN sessions s ON s.owner_id = ?1 AND s.id=o.child_id
             WHERE o.owner_id = ?1 AND o.tool='operator_prompt' AND o.request_id=?2 AND s.host_id=?3
             AND o.state='awaiting_reconciliation' AND o.turn_id IS NOT NULL",params![self.owner_id(),request,host],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional()?;
        let Some((operation, session, turn)) = row else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE turns SET state='not_delivered' WHERE owner_id = ?1 AND turn_id=?2 AND state='sent'",
            params![self.owner_id(), turn],
        )?;
        tx.execute(
            "UPDATE sessions SET open_turn_id=NULL WHERE owner_id = ?1 AND id=?2 AND open_turn_id=?3",
            params![self.owner_id(), session, turn],
        )?;
        if code == "turn_in_progress" {
            tx.execute(
                "UPDATE session_dispatch_queue SET state='queued',busy_refusals=MIN(busy_refusals+1,5),
                 retry_after=CAST(round(unixepoch('subsec')*1000) AS INTEGER)+1000*MIN(1 << MIN(busy_refusals,4),10)
                 WHERE owner_id = ?1 AND operation_id=?2 AND origin='operator'",
                params![self.owner_id(), operation],
            )?;
            tx.execute(
                "UPDATE session_operations SET state='queued',turn_id=NULL,request_id=NULL,code=NULL WHERE owner_id = ?1 AND operation_id=?2",
                params![self.owner_id(), operation],
            )?;
            // This explicit refusal proves no adapter admission. Unlike a
            // disconnected delivery, the unsent turn and image links can go.
            tx.execute(
                "DELETE FROM turns WHERE owner_id = ?1 AND turn_id=?2 AND state='not_delivered'",
                params![self.owner_id(), turn],
            )?;
        } else {
            self.fail_operator_item_in(&tx, &operation, code)?;
            self.fail_operator_queue_in(&tx, &session, code)?;
        }
        self.settle_delegation_in(&tx)?;
        tx.commit()?;
        Ok(true)
    }

    /// Durable dirtiness includes lifecycle triggers and reconciliation, so a
    /// crash between commit and publish is repaired by the next worker tick.
    pub fn flush_prompt_queue_changes(&self, hub: &Hub) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let ids = {
            let mut stmt=tx.prepare("SELECT id FROM sessions WHERE owner_id = ?1 AND prompt_queue_dirty=1 AND lifecycle<>'deleted' ORDER BY id LIMIT 128")?;
            stmt.query_map([self.owner_id()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut events = Vec::new();
        for id in ids {
            events.push(crate::store::collector_event(
                &tx,
                self.owner_id(),
                &id,
                "queue_changed",
                serde_json::json!({}),
                &now(),
            )?);
            tx.execute(
                "UPDATE sessions SET prompt_queue_dirty=0 WHERE owner_id = ?1 AND id=?2",
                params![self.owner_id(), id],
            )?;
        }
        tx.commit()?;
        drop(conn);
        for event in events {
            hub.publish(event);
        }
        Ok(())
    }
}
