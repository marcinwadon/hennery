//! Operator family projection. Reading history never grants an agent capability.
use crate::{
    hub::Hub,
    store::{Store, read_item},
};
use anyhow::Result;
use hennery_proto::rest::SessionItem;
use hennery_view::api::*;
use rusqlite::{Connection, OptionalExtension, params};

fn item(conn: &Connection, owner: &str, id: &str) -> Result<Option<SessionItem>> {
    Ok(conn
        .query_row(
            "SELECT id,host_id,agent,cwd,title,lifecycle,activity,failure_reason,presumed_parked,
                git_branch,git_dirty,model,mode,created_at,last_event_at,hat_id,title_source,project_root,sidebar_pinned
             FROM sessions WHERE owner_id = ?1 AND id = ?2 AND lifecycle<>'deleted'
        AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=sessions.hat_id)",
            params![owner, id],
            read_item,
        )
        .optional()?
        .map(SessionItem::bounded))
}
fn link(s: SessionItem) -> FamilyLink {
    FamilyLink {
        session_id: s.session_id,
        title: s.title,
        title_source: s.title_source,
        agent: s.agent,
        model: s.model,
    }
}
fn children(
    conn: &Connection,
    owner: &str,
    id: &str,
    hat: &str,
    after: &str,
    limit: u32,
) -> Result<Vec<(String, bool, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT f.child_id,f.detached,f.reason FROM session_families f
        JOIN sessions c ON c.owner_id = ?1 AND c.id=f.child_id AND c.hat_id = ?3 AND c.lifecycle<>'deleted'
        JOIN sessions p ON p.owner_id = ?1 AND p.id=f.parent_id AND p.hat_id = ?3 AND p.lifecycle<>'deleted'
        WHERE f.owner_id = ?1 AND f.parent_id = ?2 AND f.hat_id = ?3 AND f.child_id>?4
        ORDER BY f.child_id LIMIT ?5",
    )?;
    Ok(stmt
        .query_map(params![owner, id, hat, after, limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?)
}
fn questions(conn: &Connection, owner: &str, id: &str) -> Result<Vec<FamilyQuestion>> {
    let mut stmt = conn.prepare(
        "SELECT pending_id,kind,handling,reason FROM delegation_question_routing
        WHERE owner_id = ?1 AND session_id = ?2 ORDER BY pending_id LIMIT 256",
    )?;
    Ok(stmt
        .query_map(params![owner, id], |r| {
            Ok(FamilyQuestion {
                pending_id: r.get(0)?,
                kind: r.get(1)?,
                handling: r.get(2)?,
                reason: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}
fn counts(conn: &Connection, owner: &str, id: &str) -> Result<(u32, u32, u32)> {
    Ok(conn.query_row(
        "SELECT count(*),COALESCE(sum(handling='operator_required'),0),
        COALESCE(sum(handling IN ('parent_queued','parent_delivered','waiting_config','awaiting_reconciliation')),0)
        FROM delegation_question_routing WHERE owner_id = ?1 AND session_id = ?2",
        params![owner, id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}
fn attention(conn: &Connection, owner: &str, s: &SessionItem) -> Result<(bool, u32, u32)> {
    let (pending, mut operator, parent) = counts(conn, owner, &s.session_id)?;
    operator += conn.query_row(
        "SELECT count(*) FROM delegation_attention WHERE owner_id = ?1 AND child_id = ?2
        AND hat_id = ?3 AND state='open' AND pending_id IS NULL",
        params![owner, s.session_id, s.hat_id],
        |r| r.get::<_, u32>(0),
    )?;
    if pending == 0 && s.activity.as_deref() == Some("blocked") {
        operator = operator.max(1);
    }
    Ok((pending > 0, operator, parent))
}
fn summary_in(conn: &Connection, owner: &str, s: SessionItem) -> Result<SessionSummary> {
    crate::delegation_attention::reconcile_closed_attention(conn, owner)?;
    let (question_waits, operator_attention, parent_handled_questions) = attention(conn, owner, &s)?;
    let parent: Option<String> = conn
        .query_row(
            "SELECT f.parent_id FROM session_families f
        JOIN sessions p ON p.owner_id = ?1 AND p.id=f.parent_id AND p.hat_id = ?3 AND p.lifecycle<>'deleted'
        WHERE f.owner_id = ?1 AND f.child_id = ?2 AND f.hat_id = ?3",
            params![owner, s.session_id, s.hat_id],
            |r| r.get(0),
        )
        .optional()?;
    let parent = parent.map(|id| item(conn, owner, &id)).transpose()?.flatten().map(link);
    let (total, active): (u32, u32) = conn.query_row(
        "SELECT count(*),COALESCE(sum(c.lifecycle='starting'
        OR c.activity IN ('running','blocked') OR c.open_turn_id IS NOT NULL
        OR EXISTS(SELECT 1 FROM pending q WHERE q.owner_id = ?1 AND q.session_id=c.id AND q.state='open')
        OR EXISTS(SELECT 1 FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id=c.id
            AND q.state IN ('queued','waiting_config'))),0)
        FROM session_families f JOIN sessions c ON c.owner_id = ?1 AND c.id=f.child_id AND c.hat_id = ?3
        WHERE f.owner_id = ?1 AND f.parent_id = ?2 AND f.hat_id = ?3 AND c.lifecycle<>'deleted'",
        params![owner, s.session_id, s.hat_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut snippets = Vec::new();
    for (id, _, _) in children(conn, owner, &s.session_id, &s.hat_id, "", 3)? {
        if let Some(s) = item(conn, owner, &id)? {
            snippets.push(link(s));
        }
    }
    let (waiting, family_attention): (u32, u32) = conn.query_row(
        "SELECT COALESCE(sum(c.activity='blocked' OR EXISTS(SELECT 1 FROM pending p
            WHERE p.owner_id = ?1 AND p.session_id=c.id AND p.state='open')),0),
          COALESCE(sum((SELECT count(*) FROM delegation_question_routing q WHERE q.owner_id = ?1
            AND q.session_id=c.id AND q.handling='operator_required') +
            (SELECT count(*) FROM delegation_attention a WHERE a.owner_id = ?1 AND a.child_id=c.id
                AND a.hat_id = ?3 AND a.state='open' AND a.pending_id IS NULL)),0)
        FROM session_families f
        JOIN sessions c ON c.owner_id = ?1 AND c.id=f.child_id AND c.hat_id = ?3 AND c.lifecycle<>'deleted'
        WHERE f.owner_id = ?1 AND f.parent_id = ?2 AND f.hat_id = ?3",
        params![owner, s.session_id, s.hat_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let revision = conn.query_row(
        "SELECT COALESCE(MAX(e.event_id),0) FROM events e WHERE e.owner_id = ?1
        AND e.kind='family_changed' AND e.applied=1 AND e.session_id IN (SELECT ?2 UNION ALL
            SELECT f.child_id FROM session_families f JOIN sessions c ON c.owner_id = ?1 AND c.id=f.child_id AND c.hat_id = ?3
            WHERE f.owner_id = ?1 AND f.parent_id = ?2 AND f.hat_id = ?3)",
        params![owner, s.session_id, s.hat_id],
        |r| r.get(0),
    )?;
    let family = (parent.is_some() || total > 0).then_some(FamilySummary {
        revision,
        parent,
        child_counts: ChildCounts { total, active, waiting },
        children: snippets,
        attention: family_attention,
    });
    let review = crate::review::unread(conn, owner, &s.session_id)?;
    let unseen = crate::unseen::unseen(conn, owner, &s.session_id)?;
    Ok(SessionSummary {
        work: crate::work::snapshot_in(conn, owner, &s.session_id, false, None)?,
        configuration_error: conn.query_row(
            "SELECT configuration_error FROM sessions WHERE owner_id = ?1 AND id=?2",
            params![owner, s.session_id],
            |r| r.get(0),
        )?,
        session: s,
        question_waits,
        operator_attention,
        review,
        unseen,
        parent_handled_questions,
        family,
    })
}
impl Store {
    /// Drain durable invalidations through ordinary owner-parameterized event
    /// writes. Triggers only mark dirtiness; they do not invent caller authority.
    pub fn flush_family_changes(&self, hub: &Hub) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let ids = {
            let mut stmt = tx.prepare(
                "SELECT id FROM sessions WHERE owner_id = ?1 AND family_dirty=1
            AND lifecycle<>'deleted' AND NOT EXISTS(SELECT 1 FROM purged_hats h
                WHERE h.owner_id = ?1 AND h.hat_id=sessions.hat_id)
            ORDER BY id LIMIT 128",
            )?;
            stmt.query_map([self.owner_id()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut events = Vec::new();
        for id in ids {
            let ts = crate::store::stamp_ago(std::time::Duration::ZERO);
            tx.execute(
                "INSERT INTO events(owner_id,session_id,kind,body,ts,applied)
                VALUES (?1,?2,'family_changed',?3,?4,1)",
                params![self.owner_id(), id, serde_json::json!({}).to_string(), ts],
            )?;
            events.push(hennery_proto::rest::EventDto {
                event_id: tx.last_insert_rowid(),
                session_id: id.clone(),
                host_seq: None,
                kind: "family_changed".into(),
                body: serde_json::json!({}),
                ts,
            });
            tx.execute(
                "UPDATE sessions SET family_dirty=0 WHERE owner_id = ?1 AND id = ?2",
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
    pub fn family_summary(&self, id: &str) -> Result<Option<SessionSummary>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
        item(&tx, self.owner_id(), id)?
            .map(|s| summary_in(&tx, self.owner_id(), s))
            .transpose()
    }
    pub fn family_page(&self, id: &str, after: &str, limit: u32) -> Result<Option<FamilyPage>> {
        let limit = limit.clamp(1, 100);
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
        let Some(s) = item(&tx, self.owner_id(), id)? else {
            return Ok(None);
        };
        let ids = children(&tx, self.owner_id(), id, &s.hat_id, after, limit + 1)?;
        let next_cursor = (ids.len() > limit as usize).then(|| ids[limit as usize - 1].0.clone());
        let mut rows = Vec::new();
        for (child, detached, reason) in ids.into_iter().take(limit as usize) {
            let Some(child_item) = item(&tx, self.owner_id(), &child)? else {
                continue;
            };
            let mut stmt = tx.prepare(
                "SELECT operation_id,tool,state,code,turn_id,pending_id FROM session_operations
                WHERE owner_id = ?1 AND caller_id = ?2 AND child_id = ?3 ORDER BY operation_id DESC LIMIT 16",
            )?;
            let operations = stmt
                .query_map(params![self.owner_id(), id, child], |r| {
                    Ok(FamilyOperation {
                        operation_id: r.get(0)?,
                        tool: r.get(1)?,
                        state: r.get(2)?,
                        code: r.get(3)?,
                        turn_id: r.get(4)?,
                        pending_id: r.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            rows.push(FamilyChild {
                summary: summary_in(&tx, self.owner_id(), child_item)?,
                routing: if detached { "detached" } else { "active" }.into(),
                reason,
                operations,
                questions: questions(&tx, self.owner_id(), &child)?,
            });
        }
        let routing: Option<(bool, Option<String>)> = tx
            .query_row(
                "SELECT f.detached,f.reason FROM session_families f
            JOIN sessions p ON p.owner_id = ?1 AND p.id=f.parent_id AND p.hat_id = ?3 AND p.lifecycle<>'deleted'
            WHERE f.owner_id = ?1 AND f.child_id = ?2 AND f.hat_id = ?3",
                params![self.owner_id(), id, s.hat_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(Some(FamilyPage {
            summary: summary_in(&tx, self.owner_id(), s)?,
            children: rows,
            next_cursor,
            questions: questions(&tx, self.owner_id(), id)?,
            routing: routing
                .as_ref()
                .map(|(d, _)| if *d { "detached" } else { "active" }.into()),
            reason: routing.and_then(|(_, r)| r),
        }))
    }
    /// Serializes with every acceptance and final handoff via the shared owner
    /// connection gate. Already handed-off answers remain the first accepted.
    pub fn take_over_child(&self, id: &str, hub: &Hub) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM session_families f
            JOIN sessions c ON c.owner_id = ?1 AND c.id=f.child_id AND c.hat_id=f.hat_id AND c.lifecycle<>'deleted'
            JOIN sessions p ON p.owner_id = ?1 AND p.id=f.parent_id AND p.hat_id=f.hat_id AND p.lifecycle<>'deleted'
            WHERE f.owner_id = ?1 AND f.child_id = ?2 AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=f.hat_id))",
            params![self.owner_id(),id],|r|r.get(0))?;
        if !valid {
            return Ok(false);
        }
        tx.execute("UPDATE session_families SET detached=1,reason='operator_takeover' WHERE owner_id = ?1 AND child_id = ?2 AND detached=0",params![self.owner_id(),id])?;
        tx.execute(
            "UPDATE delegation_notices SET state='operator_required',reason='operator_takeover'
            WHERE owner_id = ?1 AND child_id = ?2 AND (state IN ('parent_queued','waiting_config','awaiting_reconciliation')
                OR (state='operator_required' AND reason='family_detached')
                OR (kind='question' AND state='parent_delivered'))",
            params![self.owner_id(), id],
        )?;
        let cancelled = {
            let mut stmt = tx.prepare("DELETE FROM answer_queue WHERE owner_id = ?1 AND session_id = ?2 AND delegation_handoff=0 AND delivered IS NULL
                AND json_extract(input_origin,'$.kind')='parent' RETURNING pending_id")?;
            stmt.query_map(params![self.owner_id(), id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut reopened = Vec::new();
        for pending_id in cancelled {
            reopened.push(crate::store::collector_event(
                &tx,
                self.owner_id(),
                id,
                "answer_cancelled",
                serde_json::json!({"pending_id":pending_id,"reason":"operator_takeover"}),
                &crate::store::stamp_ago(std::time::Duration::ZERO),
            )?);
        }
        // The question event and cancellation commit together. Publish only
        // afterward; dirty family projections drain separately.
        tx.commit()?;
        drop(conn);
        for event in reopened {
            hub.publish(event);
        }
        self.flush_family_changes(hub)?;
        Ok(true)
    }
    /// Durable owner-scope sequence, independent of frozen transcript cursors.
    pub fn view_scope_revision(&self) -> Result<i64> {
        Ok(self.conn().query_row(
            "SELECT COALESCE(MAX(revision),0) FROM view_scope_invalidations WHERE owner_id = ?1",
            [self.owner_id()],
            |row| row.get(0),
        )?)
    }
    pub fn session_scope_frozen(&self, id: &str) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions s JOIN purged_hats p
            ON p.owner_id = ?1 AND p.hat_id=s.hat_id WHERE s.owner_id = ?1 AND s.id = ?2)",
            params![self.owner_id(), id],
            |row| row.get(0),
        )?)
    }
    pub fn operator_waiting_count(&self, hat: Option<&str>) -> Result<u32> {
        Ok(self.conn().query_row("SELECT count(*) FROM sessions s WHERE s.owner_id = ?1 AND s.lifecycle<>'deleted'
            AND (?2 IS NULL OR s.hat_id = ?2)
            AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=s.hat_id)
            AND (EXISTS(SELECT 1 FROM delegation_question_routing q WHERE q.owner_id = ?1 AND q.session_id=s.id AND q.handling='operator_required')
              OR EXISTS(SELECT 1 FROM delegation_attention a WHERE a.owner_id = ?1 AND a.child_id=s.id AND a.hat_id=s.hat_id AND a.state='open' AND a.pending_id IS NULL)
              OR (s.activity='blocked' AND NOT EXISTS(SELECT 1 FROM pending p WHERE p.owner_id = ?1 AND p.session_id=s.id AND p.state='open')))",params![self.owner_id(),hat],|r|r.get(0))?)
    }
    /// Both source and parent invalidations are derived from owner-authorized
    /// current edges. On resume, durable owner-scope removals replay even
    /// when no transcript cursor advanced; ordinary event polls omit them.
    pub fn family_changed_after(&self, after: i64) -> Result<Vec<String>> {
        self.family_changes_after(after, true)
    }
    pub(crate) fn family_changes_after(&self, after: i64, include_scope: bool) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT session_id FROM (
            SELECT session_id,event_id FROM events WHERE owner_id = ?1 AND event_id>?2 AND applied=1
            UNION ALL SELECT f.parent_id,e.event_id FROM events e JOIN session_families f ON f.owner_id = ?1 AND f.child_id=e.session_id
            JOIN sessions p ON p.owner_id = ?1 AND p.id=f.parent_id AND p.hat_id=f.hat_id AND p.lifecycle<>'deleted'
            JOIN sessions c ON c.owner_id = ?1 AND c.id=f.child_id AND c.hat_id=f.hat_id AND c.lifecycle<>'deleted'
            WHERE e.owner_id = ?1 AND e.event_id>?2 AND e.applied=1
            UNION ALL SELECT s.id,?2 FROM sessions s JOIN purged_hats v
                ON v.owner_id = ?1 AND v.hat_id=s.hat_id WHERE s.owner_id = ?1 AND s.lifecycle<>'deleted' AND ?3)
            GROUP BY +session_id ORDER BY MAX(event_id)",
        )?;
        Ok(stmt
            .query_map(params![self.owner_id(), after, include_scope], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
}
