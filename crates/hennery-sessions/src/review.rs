//! Durable review of a particular result, independent of formal pending requests.
use crate::{hub::Hub, store::Store};
use anyhow::Result;
use hennery_view::api::ReviewResult;
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) fn unread(conn: &Connection, owner: &str, session: &str) -> Result<Option<ReviewResult>> {
    let row: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT r.event_id,json_extract(e.body,'$.turn_id'),json_extract(e.body,'$.outcome')
         FROM session_reviews r JOIN sessions s ON s.owner_id = ?1 AND s.id=r.session_id
         JOIN events e ON e.owner_id = ?1 AND e.session_id=r.session_id AND e.event_id=r.event_id
         WHERE r.owner_id = ?1 AND r.session_id = ?2 AND r.event_id>r.read_event_id
         AND s.lifecycle<>'deleted'
         AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=s.hat_id)",
            params![owner, session],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    row.map(|(event_id, turn_id, outcome)| {
        Ok(ReviewResult {
            event_id,
            turn_id,
            outcome: serde_json::from_value(serde_json::Value::String(outcome))?,
        })
    })
    .transpose()
}

pub(crate) fn record(conn: &Connection, owner: &str, session: &str, event_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO session_reviews(owner_id,session_id,event_id)
         SELECT ?1,id,?3 FROM sessions WHERE owner_id = ?1 AND id = ?2
         AND lifecycle<>'deleted'
         ON CONFLICT(owner_id,session_id) DO UPDATE SET event_id=excluded.event_id",
        params![owner, session, event_id],
    )?;
    Ok(())
}

impl Store {
    /// Only the supplied, actually completed result is acknowledged. An old tab
    /// cannot consume a newer completion, and acknowledgements never go backwards.
    pub fn mark_result_read(&self, session: &str, event_id: i64, hub: &Hub) -> Result<Option<bool>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions s WHERE s.owner_id = ?1 AND s.id = ?2
             AND s.lifecycle<>'deleted' AND NOT EXISTS(SELECT 1 FROM purged_hats h
                WHERE h.owner_id = ?1 AND h.hat_id=s.hat_id))",
            params![self.owner_id(), session],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        let valid: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_reviews r JOIN events e ON e.owner_id = ?1
             AND e.session_id=r.session_id AND e.event_id = ?3
             WHERE r.owner_id = ?1 AND r.session_id = ?2 AND r.event_id>=?3
             AND e.applied=1 AND e.kind='turn_ended'
             AND json_extract(e.body,'$.outcome') IN ('completed','failed'))",
            params![self.owner_id(), session, event_id],
            |r| r.get(0),
        )?;
        if !valid {
            return Ok(Some(false));
        }
        crate::unseen::acknowledge_in(&tx, self.owner_id(), session, event_id)?;
        let changed = tx.execute(
            "UPDATE session_reviews SET read_event_id = ?3
             WHERE owner_id = ?1 AND session_id = ?2 AND read_event_id<?3",
            params![self.owner_id(), session, event_id],
        )?;
        let event = if changed > 0 {
            Some(crate::unseen::receipt_in(
                &tx,
                self.owner_id(),
                session,
                "result_read",
                event_id,
            )?)
        } else {
            None
        };
        tx.commit()?;
        drop(conn);
        if let Some(event) = event {
            hub.publish(event);
        }
        Ok(Some(true))
    }
}
