//! Durable operator fallback. Push is a best-effort projection, never a receipt.
use crate::{
    hub::Hub,
    store::{Edge, EdgeSession, PushEdge, Store},
};
use anyhow::Result;
use hennery_kernel::push::{Notice, Push, Urgency};
use hennery_proto::{frames::Capability, plugin::InputOrigin, rest::EventDto};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct OperatorAttention {
    pub id: String,
    pub child_id: String,
    pub hat_id: String,
    pub reason: String,
    pub pending_id: Option<String>,
}

pub(crate) fn child_needs_human(conn: &Connection, owner: &str, child: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions c WHERE c.owner_id = ?1 AND c.id=?2
        AND c.close_requested=0 AND c.lifecycle IN ('starting','active','parked')
        AND (c.configuration_error IS NOT NULL OR EXISTS(SELECT 1 FROM pending p WHERE p.owner_id = ?1
          AND p.session_id=c.id AND p.state='open' AND NOT EXISTS(SELECT 1 FROM answer_queue a
            WHERE a.owner_id = ?1 AND a.pending_id=p.pending_id))))",
        params![owner, child],
        |r| r.get(0),
    )?)
}

/// Also repairs historical parent-closed status rows on their first read.
pub(crate) fn reconcile_closed_attention(conn: &Connection, owner: &str) -> Result<()> {
    let rows = {
        let mut stmt=conn.prepare("SELECT DISTINCT a.child_id,
            c.close_requested=1 OR c.lifecycle IN ('closed','deleted'),
            c.delegated_child=1 AND (NOT EXISTS(SELECT 1 FROM session_families f WHERE f.owner_id = ?1 AND f.child_id=c.id)
              OR EXISTS(SELECT 1 FROM session_families f JOIN sessions p ON p.owner_id = ?1 AND p.id=f.parent_id
                WHERE f.owner_id = ?1 AND f.child_id=c.id AND (p.close_requested=1 OR p.lifecycle IN ('closed','deleted'))))
            FROM delegation_attention a JOIN sessions c ON c.owner_id = ?1 AND c.id=a.child_id
            WHERE a.owner_id = ?1 AND a.state IN ('open','standby')")?;
        stmt.query_map([owner], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, bool>(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (child, closed, parent_closed) in rows {
        if closed || (parent_closed && !child_needs_human(conn, owner, &child)?) {
            conn.execute("UPDATE delegation_attention SET state='resolved' WHERE owner_id = ?1 AND child_id=?2 AND state IN ('open','standby')",params![owner,child])?;
        } else if parent_closed {
            conn.execute(
                "UPDATE delegation_attention SET reason='parent_closed_child_needs_human'
                WHERE owner_id = ?1 AND child_id=?2 AND state='open' AND reason<>'parent_closed_child_needs_human'",
                params![owner, child],
            )?;
        }
    }
    Ok(())
}
impl Store {
    pub fn refresh_delegation_attention(&self, hub: &Hub, now: i64) -> Result<()> {
        self.refresh_attention_after(hub, now, "")?;
        self.flush_family_changes(hub)
    }
    pub(crate) fn refresh_attention_after(&self, hub: &Hub, now: i64, after: &str) -> Result<String> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        self.expire_configuration_waits_in(&tx, now)?;
        self.settle_notices_in(&tx)?;
        let rows = {
            let mut statement = tx.prepare(
                "SELECT n.delivery_id,n.deadline,p.host_id,
                f.detached = 0 AND p.hat_id = f.hat_id AND c.hat_id = f.hat_id
                AND p.close_requested = 0 AND p.lifecycle IN ('starting','active','parked')
                AND c.close_requested = 0 AND c.lifecycle IN ('starting','active','parked','failed')
                AND h.revoked_at IS NULL
                AND (p.mcp_delivery_mode IN ('isolated','managed_plus_native') OR (p.mcp_delivery_mode = 'unisolated' AND p.hat_id = h.default_hat_id))
                AND NOT EXISTS(SELECT 1 FROM purged_hats x WHERE x.owner_id = ?1 AND x.hat_id = n.hat_id)
                AND g.delegation_enabled = 1 AND g.revision = n.accepted_revision,
                n.kind = 'turn_ended' AND n.target_turn IS NULL AND EXISTS(
                    SELECT 1 FROM events e WHERE e.owner_id = ?1 AND e.session_id = n.child_id
                    AND e.event_id = n.source_event_id AND e.applied = 1 AND e.kind = 'turn_ended'
                    AND json_extract(e.body,'$.outcome') = 'completed')
                FROM delegation_notices n JOIN session_families f ON f.owner_id = ?1 AND f.child_id = n.child_id
                JOIN sessions p ON p.owner_id = ?1 AND p.id = n.parent_id
                JOIN sessions c ON c.owner_id = ?1 AND c.id = n.child_id
                JOIN hosts h ON h.owner_id = ?1 AND h.id = p.host_id
                JOIN hat_session_tools g ON g.owner_id = ?1 AND g.hat_id = n.hat_id
                WHERE n.owner_id = ?1 AND n.delivery_id > ?2
                AND (n.state IN ('parent_queued','waiting_config','awaiting_reconciliation')
                    OR (n.state = 'parent_delivered' AND n.kind = 'question')
                    OR (n.state = 'operator_required' AND n.reason = 'handling_deadline'
                        AND n.kind = 'turn_ended' AND n.target_turn IS NULL))
                ORDER BY n.delivery_id LIMIT 128",
            )?;
            statement
                .query_map(params![self.owner_id(), after], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, bool>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let cursor = rows.last().map(|r| r.0.clone()).unwrap_or_default();
        for (id, deadline, host, eligible, completed_report) in rows {
            let reason = if !eligible {
                Some("parent_unavailable")
            } else if hub.is_ready(&host) && !hub.has_capability(&host, Capability::DelegationConfig) {
                Some("unsupported_delivery")
            } else if now >= deadline && !completed_report {
                Some("handling_deadline")
            } else {
                None
            };
            if let Some(reason) = reason {
                tx.execute(
                    "UPDATE delegation_notices SET state = 'operator_required',reason = ?3
                    WHERE owner_id = ?1 AND delivery_id = ?2",
                    params![self.owner_id(), id, reason],
                )?;
            } else if completed_report {
                // A successful result needs no operator answer. Keep it queued
                // while the parent is busy/offline, including old deadline
                // fallbacks. Never retry a reserved, possibly executed turn.
                let repaired = tx.execute(
                    "UPDATE delegation_notices SET state = 'parent_queued',reason = NULL
                    WHERE owner_id = ?1 AND delivery_id = ?2 AND state = 'operator_required'
                    AND reason = 'handling_deadline' AND target_turn IS NULL",
                    params![self.owner_id(), id],
                )?;
                if repaired > 0 {
                    tx.execute(
                        "UPDATE delegation_attention SET state = 'standby',push_run = '',push_at = 0
                        WHERE owner_id = ?1 AND delivery_id = ?2 AND state = 'open'",
                        params![self.owner_id(), id],
                    )?;
                }
            }
        }
        reconcile_closed_attention(&tx, self.owner_id())?;
        tx.commit()?;
        Ok(cursor)
    }
    /// The factual pending counts remain separate; D4 consumes this projection.
    pub fn operator_attention(&self, hat: Option<&str>, after: &str) -> Result<Vec<OperatorAttention>> {
        let conn = self.conn();
        reconcile_closed_attention(&conn, self.owner_id())?;
        let mut stmt = conn.prepare(
            "SELECT a.attention_id,a.child_id,a.hat_id,a.reason,a.pending_id
            FROM delegation_attention a JOIN sessions c ON c.owner_id = ?1 AND c.id = a.child_id AND c.hat_id = a.hat_id
            WHERE a.owner_id = ?1 AND a.state = 'open' AND c.lifecycle <> 'deleted'
            AND (?2 IS NULL OR a.hat_id = ?2) AND a.attention_id > ?3
            AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id = a.hat_id)
            ORDER BY a.attention_id LIMIT 100",
        )?;
        Ok(stmt
            .query_map(params![self.owner_id(), hat, after], |r| {
                Ok(OperatorAttention {
                    id: r.get(0)?,
                    child_id: r.get(1)?,
                    hat_id: r.get(2)?,
                    reason: r.get(3)?,
                    pending_id: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// Try push once per attention episode, including across collector restarts.
    /// A kernel queue drop or failed network request never changes the durable
    /// attention disposition; without a delivery receipt, retrying would spam.
    pub fn enqueue_delegation_attention(&self, push: &Push, run: &str, after: &str, now: i64) -> Result<String> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        reconcile_closed_attention(&tx, self.owner_id())?;
        let rows = {
            let mut stmt=tx.prepare("SELECT a.attention_id,c.id,c.hat_id,CASE WHEN c.title_source='prompt' THEN NULL ELSE c.title END,c.cwd,a.pending_id,
                json_extract(e.body,'$.indexed.pending.title')
                FROM delegation_attention a JOIN sessions c ON c.owner_id = ?1 AND c.id = a.child_id AND c.hat_id = a.hat_id
                LEFT JOIN pending p ON p.owner_id = ?1 AND p.session_id = c.id AND p.pending_id = a.pending_id
                LEFT JOIN events e ON e.owner_id = ?1 AND e.session_id = c.id AND e.event_id = p.opened_event_id
                WHERE a.owner_id = ?1 AND a.state = 'open' AND c.lifecycle <> 'deleted' AND a.attention_id > ?2
                AND a.push_at = 0
                AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id = a.hat_id)
                ORDER BY a.attention_id LIMIT 128")?;
            stmt.query_map(params![self.owner_id(), after], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    EdgeSession {
                        id: r.get(1)?,
                        hat_id: r.get(2)?,
                        title: r.get(3)?,
                        cwd: r.get(4)?,
                    },
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let cursor = rows.last().map(|r| r.0.clone()).unwrap_or_default();
        let mut notices = Vec::new();
        for (id, session, pending, title) in rows {
            let mut notice = if let Some(pending) = pending {
                crate::notify::notice_for(
                    &PushEdge::QuestionOutsideTurn {
                        pending_id: pending,
                        title: title.and_then(|text| {
                            crate::store::one_line(
                                &text,
                                hennery_proto::rest::TITLE_MAX_CHARS,
                                hennery_proto::rest::TITLE_MAX_JSON_BYTES,
                            )
                        }),
                    },
                    &session,
                )
                .expect("question notifies")
            } else {
                Notice {
                    hat_id: session.hat_id.clone(),
                    urgency: Urgency::High,
                    title: "Session needs attention".into(),
                    generic_title: "Session needs attention".into(),
                    body: "needs your attention".into(),
                    detail: None,
                    url: format!("/sessions/{}", crate::notify::encoded_id(&session.id)),
                    tag: String::new(),
                }
            };
            notice.tag = format!("delegation:{id}");
            tx.execute(
                "UPDATE delegation_attention SET push_run = ?3,push_at = ?4
                WHERE owner_id = ?1 AND attention_id = ?2 AND state = 'open'",
                params![self.owner_id(), id, run, now],
            )?;
            notices.push(notice);
        }
        tx.commit()?;
        // Still hold the shared owner gate: delete/reassign cannot interleave
        // before enqueue. Kernel revalidates again at transport admission.
        for notice in notices {
            push.notify(notice);
        }
        Ok(cursor)
    }
    /// A plain operator epoch changes budgets, never routing. Re-arm is an
    /// explicit operator action naming a NEW actually acknowledged root prompt.
    pub fn rearm_delegation_attention(&self, attention: &str, operator_turn: &str, now: i64) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let found: Option<(String, i64, Option<String>)> = tx
            .query_row(
                "SELECT n.delivery_id,e.event_id,n.target_turn
            FROM delegation_attention a JOIN delegation_notices n ON n.owner_id = ?1 AND n.delivery_id = a.delivery_id
            JOIN session_families f ON f.owner_id = ?1 AND f.child_id = n.child_id AND f.parent_id = n.parent_id
            JOIN sessions p ON p.owner_id = ?1 AND p.id = n.parent_id
            JOIN sessions c ON c.owner_id = ?1 AND c.id = n.child_id
            JOIN hat_session_tools g ON g.owner_id = ?1 AND g.hat_id = n.hat_id
            JOIN events e ON e.owner_id = ?1 AND e.session_id = n.root_id
            WHERE a.owner_id = ?1 AND a.attention_id = ?2 AND a.state = 'open'
            AND e.kind = 'user_turn' AND e.applied = 1 AND json_extract(e.body,'$.turn_id') = ?3
            AND COALESCE(json_extract(e.body,'$.origin.kind'),'operator') = 'operator' AND e.event_id > a.operator_epoch
            AND f.detached = 0 AND p.hat_id = f.hat_id AND c.hat_id = f.hat_id
            AND p.close_requested = 0 AND c.close_requested = 0 AND p.presumed_parked = 0
            AND p.lifecycle IN ('active','parked') AND c.lifecycle IN ('starting','active','parked','failed')
            AND g.delegation_enabled = 1 AND g.revision = n.accepted_revision
            AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id = n.hat_id)
            AND (n.target_turn IS NULL OR (n.kind = 'question' AND EXISTS(SELECT 1 FROM turns t
                WHERE t.owner_id = ?1 AND t.turn_id = n.target_turn AND t.state IN ('started','ended'))))
            AND (n.kind <> 'question' OR EXISTS(SELECT 1 FROM pending q WHERE q.owner_id = ?1
                AND q.session_id = n.child_id AND q.pending_id = n.pending_id AND q.kind = 'elicitation' AND q.state = 'open'
                AND NOT EXISTS(SELECT 1 FROM answer_queue b WHERE b.owner_id = ?1 AND b.pending_id = q.pending_id)))",
                params![self.owner_id(), attention, operator_turn],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((delivery, epoch, target)) = found else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE delegation_notices SET state = ?3,reason = NULL,deadline = ?4 + 300
            WHERE owner_id = ?1 AND delivery_id = ?2",
            params![
                self.owner_id(),
                delivery,
                if target.is_some() {
                    "parent_delivered"
                } else {
                    "parent_queued"
                },
                now
            ],
        )?;
        tx.execute(
            "UPDATE delegation_attention SET state = 'resolved',operator_epoch = ?3,push_run = '',push_at = 0
            WHERE owner_id = ?1 AND attention_id = ?2",
            params![self.owner_id(), attention, epoch],
        )?;
        tx.commit()?;
        Ok(true)
    }
    /// Kernel-owned guard: only reserved delegation tags are interpreted here.
    /// Success admits a push attempt; an already admitted external copy cannot
    /// be recalled. No database gate is retained across asynchronous transport.
    pub fn delegation_push_current(&self, notice: &Notice) -> Result<bool> {
        let Some(id) = notice.tag.strip_prefix("delegation:") else {
            return Ok(true);
        };
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM delegation_attention a
            JOIN sessions c ON c.owner_id = ?1 AND c.id = a.child_id AND c.hat_id = a.hat_id
            WHERE a.owner_id = ?1 AND a.attention_id = ?2 AND a.hat_id = ?3 AND a.state = 'open'
            AND c.lifecycle <> 'deleted'
            AND (a.pending_id IS NULL OR EXISTS(SELECT 1 FROM pending p WHERE p.owner_id = ?1
                AND p.session_id = a.child_id AND p.pending_id = a.pending_id AND p.state = 'open'
                AND NOT EXISTS(SELECT 1 FROM answer_queue q WHERE q.owner_id = ?1 AND q.pending_id = p.pending_id)))
            AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id = a.hat_id))",
            params![self.owner_id(), id, notice.hat_id],
            |r| r.get(0),
        )?)
    }
    /// Acknowledge a status report without impersonating an answer or retrying
    /// an uncertain prompt. Questions resolve only through canonical answers.
    pub fn acknowledge_delegation_attention(&self, id: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let delivery: Option<Option<String>> = tx
            .query_row(
                "SELECT a.delivery_id FROM delegation_attention a
            JOIN sessions c ON c.owner_id = ?1 AND c.id = a.child_id AND c.hat_id = a.hat_id
            WHERE a.owner_id = ?1 AND a.attention_id = ?2 AND a.state = 'open' AND a.pending_id IS NULL
            AND c.lifecycle <> 'deleted'
            AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id = a.hat_id)",
                params![self.owner_id(), id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(delivery) = delivery else { return Ok(false) };
        tx.execute(
            "UPDATE delegation_notices SET state = 'resolved' WHERE owner_id = ?1 AND delivery_id = ?2",
            params![self.owner_id(), delivery],
        )?;
        tx.execute(
            "UPDATE delegation_attention SET state = 'resolved' WHERE owner_id = ?1 AND attention_id = ?2",
            params![self.owner_id(), id],
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub fn route_push_edge(&self, edge: &Edge, push: &Push) -> Result<()> {
        let conn = self.conn();
        let delegated: Option<bool> = conn
            .query_row(
                "SELECT delegated_child FROM sessions WHERE owner_id = ?1 AND id = ?2 AND lifecycle <> 'deleted'",
                params![self.owner_id(), edge.session.id],
                |r| r.get(0),
            )
            .optional()?;
        if delegated != Some(false) {
            return Ok(());
        }
        if matches!(edge.kind, PushEdge::TurnEnded(_)) {
            let origin:Option<String>=conn.query_row("SELECT t.input_origin FROM events e JOIN turns t
                ON t.owner_id = ?1 AND t.session_id = e.session_id AND t.turn_id = json_extract(e.body,'$.turn_id')
                WHERE e.owner_id = ?1 AND e.session_id = ?2 AND e.event_id = ?3 AND e.kind = 'turn_ended' AND e.applied = 1",
                params![self.owner_id(),edge.session.id,edge.source_event_id],|r|r.get(0)).optional()?;
            if !origin
                .and_then(|raw| serde_json::from_str::<InputOrigin>(&raw).ok())
                .is_some_and(|origin| matches!(origin, InputOrigin::Operator {}))
            {
                return Ok(());
            }
        }
        if let Some(notice) = crate::notify::notice_for(&edge.kind, &edge.session) {
            push.notify(notice);
        }
        Ok(())
    }
}

/// Parent deletion removes routing edges, but the immutable delegated identity
/// survives. Live questions and configuration failures still create operator
/// work; ordinary reports remain status only. No deleted parent id or payload
/// is retained.
pub(crate) fn record_orphan_in(
    tx: &Transaction<'_>,
    owner: &str,
    event: &EventDto,
    kind: &str,
    incident: &str,
    pending: Option<&str>,
) -> Result<()> {
    if !child_needs_human(tx, owner, &event.session_id)? {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO delegation_attention(owner_id,attention_id,child_id,hat_id,reason,pending_id,state,incident)
        SELECT ?1,?2,s.id,s.hat_id,'parent_closed_child_needs_human',CASE WHEN ?4 = 'question' THEN ?5 END,'open',?6
        FROM sessions s WHERE s.owner_id = ?1 AND s.id = ?3 AND s.delegated_child = 1 AND s.lifecycle <> 'deleted'
        AND (?4 <> 'question' OR EXISTS(SELECT 1 FROM pending p WHERE p.owner_id = ?1 AND p.session_id = s.id
            AND p.pending_id = ?5 AND p.state = 'open' AND NOT EXISTS(SELECT 1 FROM answer_queue a
                WHERE a.owner_id = ?1 AND a.pending_id = p.pending_id)))
        ON CONFLICT(owner_id,child_id,incident) DO NOTHING",
        params![
            owner,
            uuid::Uuid::now_v7().to_string(),
            event.session_id,
            kind,
            pending,
            incident
        ],
    )?;
    Ok(())
}
