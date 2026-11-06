//! Direct-child inspection and ordinary elicitation, using canonical extracts.
use crate::{
    hub::Hub,
    store::{AnswerSubmission, Store},
};
use anyhow::{Result, bail, ensure};
use hennery_gateway::identity::SessionIdentity;
use hennery_proto::{
    delegation::OperationReceipt,
    delegation_status::{ChildList, ChildQuestion, ChildStatus, ChildSummary},
    frames::{CollectorFrame, PendingKind},
    plugin::InputOrigin,
    rest::AnswerRequest,
};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

/// One ordinary answer, admitted through the same validator as operator input.
pub struct DelegatedAnswer<'a> {
    pub key: &'a str,
    pub child: &'a str,
    pub pending: &'a str,
    pub answer: &'a AnswerRequest,
}
struct FormSnapshot {
    payload: String,
    schema_digest: Vec<u8>,
}
impl Store {
    /// The expensive bounded evaluator runs on a blocking worker, without the
    /// shared DB mutex. Admission below rechecks all authority and form facts.
    pub async fn queue_ordinary_answer_async(
        self: &std::sync::Arc<Self>,
        identity: &SessionIdentity,
        revision: i64,
        digest: &str,
        input: &DelegatedAnswer<'_>,
        hub: &std::sync::Arc<Hub>,
    ) -> Result<OperationReceipt> {
        self.queue_answer_async(identity, revision, digest, input, hub, || {})
            .await
    }
    #[cfg(feature = "test-hooks")]
    pub async fn queue_ordinary_answer_async_during_validation(
        self: &std::sync::Arc<Self>,
        identity: &SessionIdentity,
        revision: i64,
        digest: &str,
        input: &DelegatedAnswer<'_>,
        hub: &std::sync::Arc<Hub>,
        validation_hook: impl FnOnce() + Send + 'static,
    ) -> Result<OperationReceipt> {
        self.queue_answer_async(identity, revision, digest, input, hub, validation_hook)
            .await
    }
    async fn queue_answer_async(
        self: &std::sync::Arc<Self>,
        identity: &SessionIdentity,
        revision: i64,
        digest: &str,
        input: &DelegatedAnswer<'_>,
        hub: &std::sync::Arc<Hub>,
        validation_hook: impl FnOnce() + Send + 'static,
    ) -> Result<OperationReceipt> {
        let store = self.clone();
        let identity = identity.clone();
        let hub = hub.clone();
        let digest = digest.to_owned();
        let key = input.key.to_owned();
        let child = input.child.to_owned();
        let pending = input.pending.to_owned();
        let answer = input.answer.clone();
        tokio::task::spawn_blocking(move || {
            store.queue_answer_checked(
                &identity,
                revision,
                &digest,
                &DelegatedAnswer {
                    key: &key,
                    child: &child,
                    pending: &pending,
                    answer: &answer,
                },
                &hub,
                validation_hook,
            )
        })
        .await?
    }
    fn ordinary_form_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        caller: &crate::delegation::SessionToolContext,
        child: &str,
        pending: &str,
    ) -> Result<FormSnapshot> {
        ensure!(self.owned_child_in(tx, caller, child, false)?, "not_found");
        let row:Option<(String,String,String,Option<String>,bool)>=tx.query_row("SELECT kind,state,payload,payload -> '$.requestedSchema',EXISTS(SELECT 1 FROM answer_queue a WHERE a.owner_id=p.owner_id AND a.pending_id=p.pending_id) FROM pending p WHERE owner_id = ?1 AND session_id = ?2 AND pending_id = ?3",params![self.owner_id(),child,pending],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let (kind, state, payload, schema, answered) = row.ok_or_else(|| anyhow::anyhow!("not_found"))?;
        ensure!(kind != "permission", "operator_required");
        ensure!(kind == "elicitation", "not_found");
        ensure!(state == "open", "not_open");
        ensure!(!answered, "already_answered");
        Ok(FormSnapshot {
            payload,
            schema_digest: Sha256::digest(schema.unwrap_or_default().as_bytes()).to_vec(),
        })
    }
    pub fn queue_ordinary_answer(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        digest: &str,
        input: &DelegatedAnswer<'_>,
        hub: &Hub,
    ) -> Result<OperationReceipt> {
        self.queue_answer_checked(identity, revision, digest, input, hub, || {})
    }
    #[cfg(feature = "test-hooks")]
    pub fn queue_ordinary_answer_during_validation(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        digest: &str,
        input: &DelegatedAnswer<'_>,
        hub: &Hub,
        after_validation: impl FnOnce(),
    ) -> Result<OperationReceipt> {
        self.queue_answer_checked(identity, revision, digest, input, hub, after_validation)
    }
    fn queue_answer_checked(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        digest: &str,
        input: &DelegatedAnswer<'_>,
        hub: &Hub,
        after_validation: impl FnOnce(),
    ) -> Result<OperationReceipt> {
        let DelegatedAnswer {
            key,
            child,
            pending,
            answer,
        } = *input;
        let snapshot = {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            let caller = self
                .session_tool_context_in(&tx, identity, revision, true)?
                .ok_or_else(|| anyhow::anyhow!("not_found"))?;
            if let Some(receipt) = self.retry_in(&tx, &caller, key, digest)? {
                return Ok(receipt);
            }
            self.ordinary_form_in(&tx, &caller, child, pending)?
        };
        crate::delegation_form::validate_with_hook(
            &serde_json::from_str(&snapshot.payload)?,
            answer,
            after_validation,
        )?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let caller = self
            .session_tool_context_in(&tx, identity, revision, true)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        if let Some(receipt) = self.retry_in(&tx, &caller, key, digest)? {
            return Ok(receipt);
        }
        let current = self.ordinary_form_in(&tx, &caller, child, pending)?;
        ensure!(current.schema_digest == snapshot.schema_digest, "schema_changed");
        let operation = uuid::Uuid::now_v7().to_string();
        let origin = InputOrigin::Parent {
            session_id: caller.session_id().into(),
            operation_id: operation.clone(),
        };
        let queued = match self.submit_answer_in(&tx, child, pending, answer, &origin)? {
            AnswerSubmission::Queued(q) => q,
            AnswerSubmission::NotFound => bail!("not_found"),
            AnswerSubmission::NotOpen => bail!("not_open"),
            AnswerSubmission::AlreadyAnswered => bail!("already_answered"),
            AnswerSubmission::Invalid(_) => bail!("invalid_arguments"),
        };
        tx.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,pending_id,request_id,state,accepted_revision)
            VALUES (?1,?2,?3,?4,?5,'session_answer',?6,?7,?8,'queued',?9)",params![self.owner_id(),caller.session_id(),key,operation,digest,child,pending,queued.request_id,revision])?;
        tx.execute(
            "UPDATE answer_queue SET delegation_handoff = 0 WHERE owner_id = ?1 AND pending_id = ?2",
            params![self.owner_id(), pending],
        )?;
        let receipt = self.receipt_in(&tx, caller.session_id(), key)?.expect("inserted");
        tx.commit()?;
        drop(conn);
        hub.publish(queued.event);
        Ok(receipt)
    }
    pub fn queue_child_close(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        key: &str,
        digest: &str,
        child: &str,
        hub: &Hub,
    ) -> Result<OperationReceipt> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let caller = self
            .session_tool_context_in(&tx, identity, revision, true)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        if let Some(receipt) = self.retry_in(&tx, &caller, key, digest)? {
            return Ok(receipt);
        }
        ensure!(self.owned_child_in(&tx, &caller, child, true)?, "not_found");
        let routing:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM session_families WHERE owner_id = ?1 AND parent_id = ?2 AND child_id = ?3 AND detached = 0)",params![self.owner_id(),caller.session_id(),child],|r|r.get(0))?;
        ensure!(routing, "not_found");
        let (host, lifecycle, presumed): (String, String, bool) = tx.query_row(
            "SELECT host_id,lifecycle,presumed_parked FROM sessions WHERE owner_id = ?1 AND id = ?2",
            params![self.owner_id(), child],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let operation = uuid::Uuid::now_v7().to_string();
        tx.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,request_id,state)
            VALUES (?1,?2,?3,?4,?5,'session_close',?6,?4,?7)",params![self.owner_id(),caller.session_id(),key,operation,digest,child,if lifecycle=="closed" {"completed"}else{"awaiting_reconciliation"}])?;
        tx.execute("UPDATE sessions SET close_requested = 1 WHERE owner_id = ?1 AND id = ?2 AND lifecycle NOT IN ('closed','deleted')",params![self.owner_id(),child])?;
        let event = if lifecycle != "closed" {
            Some(crate::store::collector_event(
                &tx,
                self.owner_id(),
                child,
                "operator_closed",
                serde_json::json!({"origin": InputOrigin::Parent {
                    session_id: caller.session_id().into(),
                    operation_id: operation.clone(),
                }}),
                &crate::store::now(),
            )?)
        } else {
            None
        };
        let receipt = self.receipt_in(&tx, caller.session_id(), key)?.expect("inserted");
        tx.commit()?;
        let handed = hub.notify(
            &host,
            CollectorFrame::CloseSession {
                request_id: operation,
                session_id: child.into(),
            },
        );
        drop(conn);
        let closed = if !handed || !matches!(lifecycle.as_str(), "active" | "starting") {
            self.close_delegated_unattached(
                child,
                &crate::store::Unattached {
                    lifecycle: lifecycle.clone(),
                    presumed_parked: presumed,
                },
            )
        } else {
            Ok(Vec::new())
        };
        // Publish after the fallback commits, so a dashboard wake-up observes
        // its final lifecycle even when closing an idle parked child emits
        // no additional event. The durable intent is published on error too.
        if let Some(event) = event {
            hub.publish(event);
        }
        for event in closed? {
            hub.publish(event);
        }
        Ok(receipt)
    }
    pub fn child_list(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        cursor: Option<&str>,
        limit: u16,
    ) -> Result<ChildList> {
        let page = self.child_list_snapshot(identity, revision, cursor, limit, false)?;
        Ok(ChildList {
            children: page.children.into_iter().map(|row| row.summary).collect(),
            next_cursor: page.next_cursor,
        })
    }
    pub fn child_work_list(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        cursor: Option<&str>,
        limit: u16,
    ) -> Result<hennery_view::work::WorkChildList> {
        self.child_list_snapshot(identity, revision, cursor, limit, true)
    }
    fn child_list_snapshot(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        cursor: Option<&str>,
        limit: u16,
        with_work: bool,
    ) -> Result<hennery_view::work::WorkChildList> {
        ensure!((1..=100).contains(&limit), "invalid_arguments");
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let caller = self
            .session_tool_context_in(&tx, identity, revision, true)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        let scope = hex::encode(Sha256::digest(serde_json::to_vec(&(
            self.owner_id(),
            caller.session_id(),
            caller.hat_id(),
        ))?));
        let after = if let Some(cursor) = cursor {
            let (bound, after) = cursor
                .split_once('.')
                .ok_or_else(|| anyhow::anyhow!("invalid_cursor"))?;
            ensure!(bound == scope && after.len() <= 128, "invalid_cursor");
            after.to_owned()
        } else {
            String::new()
        };
        let mut stmt=tx.prepare("SELECT c.id,c.lifecycle,c.activity,c.close_requested,
            (SELECT COUNT(*) FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id = c.id AND q.state IN ('queued','waiting_config'))
            FROM session_families f JOIN sessions c ON c.owner_id = ?1 AND c.id = f.child_id
            WHERE f.owner_id = ?1 AND f.parent_id = ?2 AND f.hat_id = ?3 AND c.hat_id = ?3 AND c.host_id = ?4
            AND c.lifecycle <> 'deleted' AND (f.detached = 0 OR f.reason = 'closed') AND c.id > ?5 ORDER BY c.id LIMIT ?6")?;
        let mut children: Vec<ChildSummary> = stmt
            .query_map(
                params![
                    self.owner_id(),
                    caller.session_id(),
                    caller.hat_id(),
                    caller.host_id(),
                    after,
                    limit as i64 + 1
                ],
                |r| {
                    Ok(ChildSummary {
                        child_id: r.get(0)?,
                        lifecycle: r.get(1)?,
                        activity: r.get(2)?,
                        close_requested: r.get(3)?,
                        queued: r.get(4)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<_>>()?;
        let mut next_cursor = if children.len() > limit as usize {
            children.pop();
            Some(format!("{scope}.{}", children.last().unwrap().child_id))
        } else {
            None
        };
        drop(stmt);
        let mut observations: Vec<hennery_view::work::WorkChildSummary> = Vec::new();
        // Reserve the envelope and cursor as well as array separators.
        let mut bytes = 256;
        for summary in children {
            let work = if with_work {
                crate::work::snapshot_in(&tx, self.owner_id(), &summary.child_id, true, None)?
            } else {
                None
            };
            let row = hennery_view::work::WorkChildSummary { summary, work };
            let size = serde_json::to_vec(&row)?.len() + 1;
            if !observations.is_empty() && bytes + size > 192 * 1024 {
                next_cursor = observations.last().map(|row| format!("{scope}.{}", row.child_id));
                break;
            }
            bytes += size;
            observations.push(row);
        }
        Ok(hennery_view::work::WorkChildList {
            children: observations,
            next_cursor,
        })
    }
    pub fn child_status(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        child: &str,
        key: Option<&str>,
    ) -> Result<ChildStatus> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let caller = self
            .session_tool_context_in(&tx, identity, revision, true)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        ensure!(self.owned_child_in(&tx, &caller, child, true)?, "not_found");
        self.settle_delegation_in(&tx)?;
        let summary=tx.query_row("SELECT id,lifecycle,activity,close_requested,(SELECT COUNT(*) FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id = sessions.id AND q.state IN ('queued','waiting_config')) FROM sessions WHERE owner_id = ?1 AND id = ?2",params![self.owner_id(),child],|r|Ok(ChildSummary {child_id:r.get(0)?,lifecycle:r.get(1)?,activity:r.get(2)?,close_requested:r.get(3)?,queued:r.get(4)?}))?;
        let config: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT requested_config,evidence FROM session_attach_evidence WHERE owner_id = ?1 AND session_id = ?2",
                params![self.owner_id(), child],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (requested, actual) = match config {
            Some((r, e)) => (
                serde_json::from_str(&r)?,
                e.map(|e| serde_json::from_str::<hennery_proto::delegation_host::ConfigEvidence>(&e).map(|e| e.actual))
                    .transpose()?,
            ),
            None => (Default::default(), None),
        };
        let mut operations = Vec::new();
        let mut stmt=tx.prepare("SELECT operation_key FROM session_operations WHERE owner_id = ?1 AND caller_id = ?2 AND child_id = ?3 AND (?4 IS NULL OR operation_key = ?4) ORDER BY rowid DESC LIMIT 100")?;
        let keys = stmt
            .query_map(params![self.owner_id(), caller.session_id(), child, key], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for key in keys {
            if let Some(receipt) = self.receipt_in(&tx, caller.session_id(), &key)? {
                operations.push(receipt)
            }
        }
        let last_outcome:Option<String>=tx.query_row("SELECT outcome FROM turns WHERE owner_id = ?1 AND session_id = ?2 AND state = 'ended' ORDER BY rowid DESC LIMIT 1",params![self.owner_id(),child],|r|r.get(0)).optional()?.flatten();
        let mut questions = Vec::new();
        let mut question_bytes = 0usize;
        let mut truncated = false;
        for pending in self.open_pending_in(&tx, child, 100)? {
            let permission = pending.kind == PendingKind::Permission;
            let mut question = if permission {
                None
            } else {
                let mut question = serde_json::to_value(hennery_view::item::Question::from_pending(&pending))?;
                question["requestedSchema"] =
                    crate::delegation_form::advertised(&pending.payload).unwrap_or(serde_json::Value::Null);
                Some(question)
            };
            if let Some(value) = &question {
                let bytes = serde_json::to_vec(value)?.len();
                if bytes > 8192 || question_bytes + bytes > 32768 {
                    question = None;
                    truncated = true;
                } else {
                    question_bytes += bytes;
                }
            }
            questions.push(ChildQuestion {
                pending_id: pending.pending_id.clone(),
                kind: pending.kind,
                operator_required: permission,
                answered: pending.answered,
                question,
            });
        }
        let mut fold = hennery_view::Fold::new();
        let mut events = Vec::new();
        let mut stmt=tx.prepare("SELECT event_id,host_seq,kind,body,ts FROM events WHERE owner_id = ?1 AND session_id = ?2 AND applied = 1 ORDER BY event_id DESC LIMIT 1000")?;
        let mut rows = stmt.query(params![self.owner_id(), child])?;
        let mut bytes = 0usize;
        while let Some(row) = rows.next()? {
            let raw: String = row.get(3)?;
            if bytes + raw.len() > 128 * 1024 {
                truncated = true;
                break;
            }
            bytes += raw.len();
            let kind: String = row.get(2)?;
            let boundary = kind == "user_turn";
            events.push(hennery_proto::rest::EventDto {
                event_id: row.get(0)?,
                session_id: child.into(),
                host_seq: row.get::<_, Option<i64>>(1)?.map(|n| n as u64),
                kind,
                body: serde_json::from_str(&raw)?,
                ts: row.get(4)?,
            });
            if boundary {
                break;
            }
        }
        drop(rows);
        drop(stmt);
        events.reverse();
        for event in &events {
            fold.apply(event, &|_: &str| false);
        }
        let mut report = String::new();
        truncated |= events.len() == 1000;
        for item in fold.take() {
            if let hennery_view::Body::Message(text) = item.body {
                report = text.text;
                truncated |= text.truncated;
            }
        }
        if report.len() > 8192 {
            let mut start = report.len() - 8192;
            while !report.is_char_boundary(start) {
                start += 1;
            }
            report = report[start..].into();
            truncated = true;
        }
        tx.commit()?;
        Ok(ChildStatus {
            summary,
            requested,
            actual,
            questions,
            operations,
            last_outcome,
            last_report: (!report.is_empty()).then_some(report),
            truncated,
            reason: None,
            url: format!("/sessions/{child}"),
        })
    }
}

impl Store {
    pub fn dispatch_delegation_answers(&self, hub: &Hub) -> Result<()> {
        self.dispatch_delegation_answers_after(hub, "").map(|_| ())
    }
    /// Dispatch one bounded page, including held rows in cursor progress. An
    /// empty page returns `None`, so a persistent caller can wrap to the start.
    pub fn dispatch_delegation_answers_after(&self, hub: &Hub, after: &str) -> Result<Option<String>> {
        let mut conn = self.conn();
        let rows = {
            let mut stmt=conn.prepare("SELECT o.operation_id,o.caller_id,o.child_id,o.pending_id,o.request_id,o.accepted_revision,a.answer,s.host_id
            FROM session_operations o JOIN answer_queue a ON a.owner_id = ?1 AND a.pending_id = o.pending_id
            JOIN sessions s ON s.owner_id = ?1 AND s.id = o.child_id
            WHERE o.owner_id = ?1 AND o.tool = 'session_answer' AND o.state = 'queued' AND a.delegation_handoff = 0 AND a.delivered IS NULL AND o.operation_id > ?2 ORDER BY o.operation_id LIMIT 128")?;
            stmt.query_map(params![self.owner_id(), after], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let next = rows.last().map(|row| row.0.clone());
        for (operation, source, child, pending, request, revision, answer, host) in rows {
            let tx = conn.transaction()?;
            if !self.transfer_in(&tx, &source, &child, revision)? || !hub.is_ready(&host) {
                continue;
            }
            tx.execute(
                "UPDATE answer_queue SET delegation_handoff = 1 WHERE owner_id = ?1 AND pending_id = ?2",
                params![self.owner_id(), pending],
            )?;
            tx.execute("UPDATE session_operations SET state = 'awaiting_reconciliation' WHERE owner_id = ?1 AND operation_id = ?2",params![self.owner_id(),operation])?;
            tx.commit()?;
            let frame = crate::store::answer_frame(request, &child, &pending, serde_json::from_str(&answer)?);
            if !self.handoff_delegated(&conn, &source, &host, frame, hub)? {
                let tx = conn.transaction()?;
                tx.execute(
                    "UPDATE answer_queue SET delegation_handoff = 0 WHERE owner_id = ?1 AND pending_id = ?2",
                    params![self.owner_id(), pending],
                )?;
                tx.execute(
                    "UPDATE session_operations SET state = 'queued' WHERE owner_id = ?1 AND operation_id = ?2",
                    params![self.owner_id(), operation],
                )?;
                tx.commit()?;
            }
        }
        Ok(next)
    }
}
