//! Explicit delivery of one operator queue item into the current turn.
//! The operation owns a distinct request id. It never owns the running turn.
use crate::{hub::Hub, store::Store};
use anyhow::{Context, Result, ensure};
use hennery_proto::{
    frames::{Capability, CollectorFrame, SteeringOutcome},
    rest::{EventDto, StoredBlock},
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

impl Store {
    pub(crate) fn steer_operator_prompt(&self, session: &str, id: &str, hub: &Hub) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (host, turn, supported, revoked): (String, Option<String>, bool, bool) = tx
            .query_row(
                "SELECT s.host_id,s.open_turn_id,s.steering_supported,h.revoked_at IS NOT NULL
             FROM sessions s JOIN hosts h ON h.owner_id=s.owner_id AND h.id=s.host_id
             WHERE s.owner_id = ?1 AND s.id=?2 AND s.lifecycle='active' AND s.activity IN ('running','blocked') AND s.presumed_parked=0
               AND s.close_requested=0 AND s.configuration_error IS NULL AND s.prompt_queue_held=0
               AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id=s.owner_id AND p.hat_id=s.hat_id)",
                params![self.owner_id(), session],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
            .context("steering_unavailable")?;
        ensure!(!revoked, "host_revoked");
        let turn = turn.context("steering_unavailable")?;
        let host_supports = hub
            .ready_capability(&host, Capability::Steering)
            .context("host_offline")?;
        ensure!(supported && host_supports, "steering_unsupported");
        let payload: String = tx
            .query_row(
                "SELECT payload FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id=?2 AND operation_id=?3
             AND origin='operator' AND state='queued'",
                params![self.owner_id(), session, id],
                |r| r.get(0),
            )
            .optional()?
            .context("queue_item_unavailable")?;
        let stored: Vec<StoredBlock> = serde_json::from_str(&payload)?;
        let Some(content) = self.queued_content(
            &stored,
            hub.has_capability(&host, Capability::Images),
            hub.has_capability(&host, Capability::Files),
        )?
        else {
            self.fail_operator_item_in(&tx, id, "image_unavailable")?;
            tx.commit()?;
            return Ok(());
        };
        let request_id = uuid::Uuid::now_v7().to_string();
        // A dispatched attempt belongs to the session history even if its
        // recovery row is dismissed before the host can report admission.
        let attempt = crate::store::collector_event(
            &tx,
            self.owner_id(),
            session,
            "steering_attempt",
            serde_json::json!({"request_id":request_id,"turn_id":turn,"content":stored}),
            &crate::store::now(),
        )?;
        crate::store::link_attachments(&tx, self.owner_id(), attempt.event_id, &attempt.body["content"])?;
        tx.execute(
            "UPDATE session_operations SET state='steering',request_id=?3,turn_id=NULL,code=NULL
             WHERE owner_id = ?1 AND operation_id=?2",
            params![self.owner_id(), id, request_id],
        )?;
        tx.execute(
            "UPDATE session_dispatch_queue SET state='steering',steering_deadline=unixepoch()+30,steering_failure=NULL
             WHERE owner_id = ?1 AND operation_id=?2",
            params![self.owner_id(), id],
        )?;
        // Commit before handoff so a collector crash can only leave an uncertain
        // delivery. Holding the store guard through notify orders it with close/revoke.
        tx.commit()?;
        hub.publish(attempt);
        let frame = CollectorFrame::Steer {
            request_id: request_id.clone(),
            session_id: session.into(),
            turn_id: turn,
            content,
        };
        if !hub.notify(&host, frame) {
            let tx = conn.transaction()?;
            self.settle_steering_in(
                &tx,
                session,
                &request_id,
                &SteeringOutcome::Rejected {
                    reason: "The host disconnected before the message was sent.".into(),
                },
            )?;
            tx.commit()?;
        }
        Ok(())
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
            // Dismissal released the queue slot. A late admission belongs in
            // history, while an idle reply must leave the item dismissed.
            return Ok(admitted);
        };
        let (queue_state, op_state, code) = match outcome {
            SteeringOutcome::Injected => ("settled", "delivered", None),
            SteeringOutcome::PromptRequired if prior_failure.is_some() => ("failed", "failed", prior_failure),
            SteeringOutcome::PromptRequired => ("queued", "queued", None),
            SteeringOutcome::Rejected { reason } => {
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
        body["origin"] = serde_json::json!({"kind":"operator"});
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
             WHERE o.owner_id = ?1 AND s.host_id=?2 AND o.request_id=?3 AND o.tool='operator_prompt' AND o.state='steering'",
            params![self.owner_id(),host,request], |r| r.get(0)
        ).optional()?;
        let Some(session) = session else {
            return Ok(false);
        };
        let outcome = if code == "not_attached" {
            SteeringOutcome::Rejected {
                reason: "The session is no longer attached to the host.".into(),
            }
        } else {
            SteeringOutcome::Uncertain
        };
        self.settle_steering_in(&tx, &session, request, &outcome)?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn uncertain_steering_for_host(&self, host: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE session_dispatch_queue SET state='uncertain',steering_deadline=NULL WHERE owner_id = ?1 AND state='steering'
             AND target_id IN (SELECT id FROM sessions WHERE owner_id = ?1 AND host_id=?2)",params![self.owner_id(),host])?;
        Ok(())
    }

    pub(crate) fn expire_steering(&self, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE session_dispatch_queue SET state='uncertain',steering_deadline=NULL
             WHERE owner_id = ?1 AND state='steering' AND steering_deadline<=?2",
            params![self.owner_id(), now],
        )?;
        Ok(())
    }
}
