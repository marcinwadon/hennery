//! Durable delivery of scoped input into the current turn.
//! The operation owns a distinct request id. It never owns the running turn.
use crate::{hub::Hub, store::Store};
use anyhow::{Context, Result, ensure};
use hennery_proto::{
    frames::{Capabilities, Capability, CollectorFrame, SteeringOutcome},
    rest::{EventDto, StoredBlock},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

pub(super) enum SteeringMode {
    WhenAvailable,
    Required,
}

impl SteeringMode {
    fn unavailable(&self, reason: &'static str) -> Result<Option<SteeringHandoff>> {
        match self {
            Self::WhenAvailable => Ok(None),
            Self::Required => anyhow::bail!(reason),
        }
    }
}

pub(super) struct SteeringTarget {
    pub host: String,
    pub turn: String,
    pub capabilities: Capabilities,
}

pub(super) struct SteeringHandoff {
    attempt: EventDto,
    host: String,
    turn: String,
    pub request_id: String,
    content: Vec<serde_json::Value>,
}

impl SteeringHandoff {
    /// The attempt is committed before handoff. The caller still holds the
    /// store guard, ordering delivery with session closure and host revocation.
    pub(super) fn deliver(self, store: &Store, conn: &mut Connection, session: &str, hub: &Hub) -> Result<()> {
        hub.publish(self.attempt);
        let frame = CollectorFrame::Steer {
            request_id: self.request_id.clone(),
            session_id: session.into(),
            turn_id: self.turn,
            content: self.content,
        };
        if !hub.notify(&self.host, frame) {
            let tx = conn.transaction()?;
            // The frame never entered a host mailbox, so an ordinary send can
            // safely stay in the FIFO. Once notify succeeds only host facts settle it.
            let outcome = SteeringOutcome::Rejected {
                reason: "The host could not receive this steering message.".into(),
                steering_unavailable: true,
            };
            store.settle_steering_in(&tx, session, &self.request_id, &outcome)?;
            tx.commit()?;
        }
        Ok(())
    }
}

impl Store {
    pub(crate) fn steer_operator_prompt(&self, session: &str, id: &str, hub: &Hub) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let handoff = self.prepare_steering_in(&tx, session, id, hub, SteeringMode::Required)?;
        tx.commit()?;
        if let Some(handoff) = handoff {
            handoff.deliver(self, &mut conn, session, hub)?;
        }
        Ok(())
    }

    pub(super) fn prepare_steering_in(
        &self,
        tx: &Transaction<'_>,
        session: &str,
        id: &str,
        hub: &Hub,
        mode: SteeringMode,
    ) -> Result<Option<SteeringHandoff>> {
        let target = match self.steering_target_in(tx, session, hub)? {
            Ok(target) => target,
            Err(reason) => return mode.unavailable(reason),
        };
        let payload: String = tx
            .query_row(
                "SELECT payload FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id=?2 AND operation_id=?3
             AND origin='operator' AND state='queued'",
                params![self.owner_id(), session, id],
                |r| r.get(0),
            )
            .optional()?
            .context("queue_item_unavailable")?;
        let Some(input_origin) = self.independent_origin_in(tx, session, id)? else {
            return Ok(None);
        };
        let stored: Vec<StoredBlock> = serde_json::from_str(&payload)?;
        let Some(content) = self.queued_content(&stored, true, true)? else {
            self.fail_operator_item_in(tx, id, "image_unavailable")?;
            return Ok(None);
        };
        if !crate::hub::supports_steering_content(&target.capabilities, &content) {
            if matches!(mode, SteeringMode::Required) {
                self.fail_operator_item_in(tx, id, "image_unavailable")?;
            }
            return Ok(None);
        }
        let handoff = self.steering_attempt_in(
            tx,
            session,
            target,
            &stored,
            content,
            input_origin,
            matches!(mode, SteeringMode::WhenAvailable),
        )?;
        tx.execute(
            "UPDATE session_operations SET state='steering',request_id=?3,turn_id=NULL,code=NULL
             WHERE owner_id = ?1 AND operation_id=?2",
            params![self.owner_id(), id, handoff.request_id],
        )?;
        tx.execute(
            "UPDATE session_dispatch_queue SET state='steering',steering_deadline=unixepoch()+30,steering_failure=NULL
             WHERE owner_id = ?1 AND operation_id=?2",
            params![self.owner_id(), id],
        )?;
        Ok(Some(handoff))
    }

    pub(super) fn steering_target_in(
        &self,
        tx: &Connection,
        session: &str,
        hub: &Hub,
    ) -> Result<Result<SteeringTarget, &'static str>> {
        let target: Option<(String, Option<String>, bool, bool)> = tx
            .query_row(
                "SELECT s.host_id,s.open_turn_id,s.steering_supported,h.revoked_at IS NOT NULL
             FROM sessions s JOIN hosts h ON h.owner_id=s.owner_id AND h.id=s.host_id
             WHERE s.owner_id = ?1 AND s.id=?2 AND s.lifecycle='active' AND s.activity IN ('running','blocked') AND s.presumed_parked=0
               AND s.close_requested=0 AND s.configuration_error IS NULL AND s.prompt_queue_held=0
               AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id=s.owner_id AND p.hat_id=s.hat_id)",
                params![self.owner_id(), session],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((host, turn, supported, revoked)) = target else {
            return Ok(Err("steering_unavailable"));
        };
        ensure!(!revoked, "host_revoked");
        let Some(turn) = turn else {
            return Ok(Err("steering_unavailable"));
        };
        let Some(capabilities) = hub.ready_capabilities(&host) else {
            return Ok(Err("host_offline"));
        };
        if !supported || !capabilities.has(Capability::Steering) {
            return Ok(Err("steering_unsupported"));
        }
        Ok(Ok(SteeringTarget {
            host,
            turn,
            capabilities,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn steering_attempt_in(
        &self,
        tx: &Transaction<'_>,
        session: &str,
        target: SteeringTarget,
        stored: &[StoredBlock],
        content: Vec<serde_json::Value>,
        origin: hennery_proto::plugin::InputOrigin,
        automatic: bool,
    ) -> Result<SteeringHandoff> {
        let request_id = uuid::Uuid::now_v7().to_string();
        // A dispatched attempt belongs to the session history even if its
        // recovery row is dismissed before the host can report admission.
        let mut body = serde_json::json!({"request_id":request_id,"turn_id":target.turn,"content":stored});
        body["origin"] = serde_json::to_value(origin)?;
        if automatic {
            body["automatic"] = true.into();
        }
        let attempt = crate::store::collector_event(
            tx,
            self.owner_id(),
            session,
            "steering_attempt",
            body,
            &crate::store::now(),
        )?;
        crate::store::link_attachments(tx, self.owner_id(), attempt.event_id, &attempt.body["content"])?;
        Ok(SteeringHandoff {
            attempt,
            host: target.host,
            turn: target.turn,
            request_id,
            content,
        })
    }

    pub(crate) fn settle_steering_in(
        &self,
        tx: &Transaction<'_>,
        session: &str,
        request: &str,
        outcome: &SteeringOutcome,
    ) -> Result<Option<EventDto>> {
        let row: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT q.operation_id,q.steering_failure FROM session_dispatch_queue q
             JOIN session_operations o ON o.owner_id=q.owner_id AND o.operation_id=q.operation_id
             WHERE q.owner_id = ?1 AND q.target_id=?2 AND q.origin='operator' AND q.state IN ('steering','uncertain')
             AND o.tool='operator_prompt' AND o.request_id=?3 AND o.state='steering'",
                params![self.owner_id(), session, request],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let admitted = if *outcome == SteeringOutcome::Injected {
            self.record_steered_input(tx, session, request)?
        } else {
            None
        };
        let Some((id, prior_failure)) = row else {
            self.settle_external_steering_in(tx, session, request, outcome)?;
            // Dismissal released the queue slot. A late admission belongs in
            // history, while an idle reply must leave the item dismissed.
            return Ok(admitted);
        };
        // Older attempts were explicit, and older rejections carry no proof
        // that ordinary delivery is safe. Only retry a positively identified miss.
        let outcome = if matches!(
            outcome,
            SteeringOutcome::Rejected {
                steering_unavailable: true,
                ..
            }
        ) && tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE owner_id = ?1 AND session_id=?2 AND kind='steering_attempt'
                 AND json_extract(body,'$.request_id')=?3 AND json_extract(body,'$.automatic')=1)",
            params![self.owner_id(), session, request],
            |r| r.get::<_, bool>(0),
        )? {
            &SteeringOutcome::PromptRequired
        } else {
            outcome
        };
        let (queue_state, op_state, code) = match outcome {
            SteeringOutcome::Injected => ("settled", "delivered", None),
            SteeringOutcome::PromptRequired if prior_failure.is_some() => ("failed", "failed", prior_failure),
            SteeringOutcome::PromptRequired => ("queued", "queued", None),
            SteeringOutcome::Rejected { reason, .. } => {
                let reason = crate::store::one_line(reason, 512, 2048)
                    .unwrap_or_else(|| "The adapter refused this message.".into());
                ("failed", "failed", Some(format!("steering_rejected:{reason}")))
            }
            SteeringOutcome::Uncertain => ("uncertain", "steering", None),
        };
        tx.execute("UPDATE session_operations SET state=?3,code=?4,request_id=CASE WHEN ?3='queued' THEN NULL ELSE request_id END
             WHERE owner_id = ?1 AND operation_id=?2",params![self.owner_id(),id,op_state,code])?;
        tx.execute("UPDATE session_dispatch_queue SET state=?3,steering_deadline=NULL,
             payload=CASE WHEN ?3='settled' THEN '' ELSE payload END,image_bytes=CASE WHEN ?3='settled' THEN 0 ELSE image_bytes END
             WHERE owner_id = ?1 AND operation_id=?2",params![self.owner_id(),id,queue_state])?;
        if *outcome == SteeringOutcome::Injected {
            tx.execute(
                "DELETE FROM prompt_queue_attachments WHERE owner_id = ?1 AND operation_id=?2",
                params![self.owner_id(), id],
            )?;
        }
        Ok(admitted)
    }

    fn record_steered_input(&self, tx: &Transaction<'_>, session: &str, request: &str) -> Result<Option<EventDto>> {
        let attempt: Option<(i64, String)> = tx.query_row(
            "SELECT a.event_id,a.body FROM events a WHERE a.owner_id = ?1 AND a.session_id=?2 AND a.kind='steering_attempt'
             AND json_extract(a.body,'$.request_id')=?3 AND NOT EXISTS (
                SELECT 1 FROM events e WHERE e.owner_id = ?1 AND e.session_id=?2 AND e.kind='user_steering'
                AND json_extract(e.body,'$.request_id')=?3)",
            params![self.owner_id(),session,request], |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        let Some((anchor, payload)) = attempt else {
            return Ok(None);
        };
        let late: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE owner_id = ?1 AND session_id=?2 AND
                kind='user_turn' AND applied=1 AND event_id>?3)",
            params![self.owner_id(), session, anchor],
            |r| r.get(0),
        )?;
        let mut body: serde_json::Value = serde_json::from_str(&payload)?;
        body.as_object_mut()
            .context("invalid steering attempt")?
            .remove("automatic");
        if body.get("origin").is_none() {
            body["origin"] = serde_json::json!({"kind":"operator"});
        }
        if late {
            body["display_at"] = anchor.into();
        }
        // Raw cursor readers always receive this single, monotonically appended
        // admission. Only the folded view needs a different position.
        let event = crate::store::collector_event(
            tx,
            self.owner_id(),
            session,
            "user_steering",
            body,
            &crate::store::now(),
        )?;
        crate::store::link_attachments(tx, self.owner_id(), event.event_id, &event.body["content"])?;
        Ok(Some(event))
    }

    /// An actor that ended before consuming its mailbox rejects the frame as
    /// not_attached. Scope transport errors to the authenticated host.
    pub(crate) fn reject_steering(&self, host: &str, request: &str, code: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let session: Option<String> = tx.query_row(
            "SELECT s.id FROM session_operations o JOIN sessions s ON s.owner_id=o.owner_id AND s.id=o.child_id
             WHERE o.owner_id = ?1 AND s.host_id=?2 AND o.request_id=?3 AND o.tool='operator_prompt' AND o.state='steering'
             UNION ALL SELECT s.id FROM external_operations o JOIN sessions s ON s.owner_id=o.owner_id AND s.id=o.session_id
             WHERE o.owner_id = ?1 AND s.host_id=?2 AND o.request_id=?3 AND o.steering_turn IS NOT NULL AND o.state IN ('running','uncertain')",
            params![self.owner_id(),host,request], |r| r.get(0)
        ).optional()?;
        let Some(session) = session else {
            return Ok(false);
        };
        let outcome = if code == "not_attached" {
            SteeringOutcome::Rejected {
                reason: "The session is no longer attached to the host.".into(),
                steering_unavailable: true,
            }
        } else {
            SteeringOutcome::Uncertain
        };
        self.settle_steering_in(&tx, &session, request, &outcome)?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn uncertain_steering_for_host(&self, host: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("UPDATE external_operations SET state='uncertain',code='delivery_unknown',steering_deadline=NULL WHERE owner_id = ?1 AND steering_deadline IS NOT NULL AND session_id IN (SELECT id FROM sessions WHERE owner_id = ?1 AND host_id=?2)",params![self.owner_id(),host])?;
        tx.execute(
            "UPDATE session_dispatch_queue SET state='uncertain',steering_deadline=NULL WHERE owner_id = ?1 AND state='steering'
             AND target_id IN (SELECT id FROM sessions WHERE owner_id = ?1 AND host_id=?2)",params![self.owner_id(),host])?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn expire_steering(&self, now: i64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("UPDATE external_operations SET state='uncertain',code='delivery_unknown',steering_deadline=NULL WHERE owner_id = ?1 AND steering_deadline<=?2",params![self.owner_id(),now])?;
        tx.execute(
            "UPDATE session_dispatch_queue SET state='uncertain',steering_deadline=NULL
             WHERE owner_id = ?1 AND state='steering' AND steering_deadline<=?2",
            params![self.owner_id(), now],
        )?;
        tx.commit()?;
        Ok(())
    }
}
